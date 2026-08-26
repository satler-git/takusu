//! Ambient gate pipeline: continuous VAD → wake word → streaming ASR.
//!
//! This is the shared, memory-only gate described in WI-20. Audio is never
//! persisted or logged before the wake word fires; only the utterance that
//! passes the gate is fed to the ASR and passed back as raw samples for
//! downstream speaker verification.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::kws::{KwsConfig, KwsError};
use crate::stream_asr::StreamAsrError;
use crate::stt::SttError;
use crate::wav::SHERPA_SAMPLE_RATE;

#[cfg(any(feature = "record", feature = "sherpa", test))]
use unicode_segmentation::UnicodeSegmentation;

#[cfg(any(feature = "record", feature = "sherpa", test))]
use std::collections::VecDeque;
#[cfg(any(feature = "record", feature = "sherpa", test))]
use std::future::Future;
#[cfg(any(feature = "record", feature = "sherpa", test))]
use std::sync::Arc;
#[cfg(any(feature = "record", test))]
use std::time::Duration;

#[cfg(any(feature = "record", feature = "sherpa", test))]
use tokio::sync::mpsc::{Receiver as BoundedPcmReceiver, UnboundedReceiver};
#[cfg(any(feature = "record", feature = "sherpa", test))]
use tokio::sync::watch;

#[cfg(any(feature = "record", feature = "sherpa", test))]
use crate::CHUNK_MS;
#[cfg(any(feature = "record", feature = "sherpa", test))]
use crate::Endpoint;
#[cfg(any(feature = "record", feature = "sherpa", test))]
use crate::kws::{AsrTextMatch, WakeWordDetector};
#[cfg(any(feature = "record", feature = "sherpa", test))]
use crate::stream_asr::StreamingAsrSession;
#[cfg(any(feature = "record", feature = "sherpa", test))]
use crate::stt::StreamingSpeechToText;
#[cfg(any(feature = "record", feature = "sherpa", test))]
use crate::vad::VadEvent;
#[cfg(feature = "record")]
use crate::{RecordConfig, RecorderError, StreamingRecorder};

/// Errors from the ambient pipeline.
#[derive(Debug, Error)]
pub enum AmbientError {
    #[error("wake word detector failed: {0}")]
    Wake(#[from] KwsError),
    #[error("ASR failed: {0}")]
    Asr(#[from] StreamAsrError),
    #[error("recording failed: {0}")]
    Record(String),
    #[error("pipeline cancelled")]
    Cancelled,
    #[error("streaming ASR backend error: {0}")]
    Stt(#[from] SttError),
}

#[cfg(feature = "record")]
impl From<RecorderError> for AmbientError {
    fn from(e: RecorderError) -> Self {
        Self::Record(e.to_string())
    }
}

/// Result of one ambient utterance capture.
#[derive(Debug, Clone, PartialEq)]
pub struct AmbientResult {
    pub text: String,
    pub samples: Vec<f32>,
}

/// Which wake word detector the ambient gate should use.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WakeWordBackend {
    /// Match the wake phrase in a streaming ASR partial transcript.
    /// Heavier, but language-agnostic. The default: sherpa-onnx has no
    /// Japanese-capable keyword-spotting model, so for a Japanese wake word
    /// the ASR text match is the only backend that actually detects it.
    #[default]
    AsrTextMatch,
    /// Use a tiny sherpa-onnx transducer keyword spotter.
    /// `KwsConfig::keyword` is romanized and tokenized automatically; set
    /// `keywords_buf` explicitly to override the tokenization.
    /// The pretrained KWS models are Chinese/English and are unreliable for
    /// Japanese wake words.
    SherpaKws,
}

/// Configuration for the ambient listening gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AmbientConfig {
    /// Whether ambient listening is enabled at all.
    pub enabled: bool,
    /// The wake word or phrase to listen for.
    pub wake_word: String,
    /// Which backend to use for wake word detection.
    pub wake_word_backend: WakeWordBackend,
    /// Configuration for the sherpa-onnx KWS backend (when selected).
    pub kws: KwsConfig,
    /// Language passed to the streaming ASR after the wake word.
    pub asr_language: String,
    /// Maximum duration of one utterance in seconds.
    pub max_utterance_seconds: u64,
    /// How many milliseconds of pre-speech audio to keep in memory so the
    /// ASR does not miss the start of the wake word.
    pub pre_speech_buffer_ms: u64,
    /// Sample rate of the audio stream.
    pub sample_rate: u32,
    /// Whether speaker verification is required by the ambient-immediate
    /// approval layer. The pipeline itself does not enforce this; the caller
    /// uses the returned `samples` for verification.
    pub verify_speaker: bool,
}

impl Default for AmbientConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            wake_word: "たくす".into(),
            wake_word_backend: WakeWordBackend::AsrTextMatch,
            kws: KwsConfig::for_wenetspeech(),
            asr_language: "ja".into(),
            max_utterance_seconds: 60,
            pre_speech_buffer_ms: 800,
            sample_rate: SHERPA_SAMPLE_RATE,
            verify_speaker: true,
        }
    }
}

/// Try to load the sherpa-onnx keyword spotter off the async runtime thread.
#[cfg(feature = "sherpa")]
async fn try_load_sherpa_kws(
    config: &crate::kws::KwsConfig,
) -> Result<crate::kws::SherpaKws, AmbientError> {
    let config = config.clone();
    tokio::task::spawn_blocking(move || crate::kws::SherpaKws::new(&config))
        .await
        .map_err(|e| crate::kws::KwsError::Other(e.to_string().into()))?
        .map_err(Into::into)
}

/// State machine for one ambient capture loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(feature = "record", feature = "sherpa", test))]
enum AmbientState {
    /// Waiting for any speech; pre-speech chunks roll through a short buffer.
    WaitingForSpeech,
    /// An utterance has started; feeding the wake word detector.
    ListeningForWake,
    /// The wake word fired; capturing the full command in the ASR stream.
    CapturingCommand,
}

/// The shared ambient gate pipeline.
#[cfg(any(feature = "record", feature = "sherpa", test))]
pub struct AmbientPipeline<'a> {
    config: AmbientConfig,
    endpoint: &'a mut (dyn Endpoint + 'a),
    wake: Box<dyn WakeWordDetector>,
    asr: Arc<dyn StreamingSpeechToText>,
    stop: watch::Receiver<bool>,
}

/// A stream of PCM chunks that the gate can drive. Implemented by both the
/// unbounded and bounded tokio receivers so `run_with_chunks` stays generic.
#[cfg(any(feature = "record", feature = "sherpa", test))]
pub trait AmbientChunkSource {
    fn recv(&mut self) -> impl Future<Output = Option<Vec<f32>>> + '_;
}

#[cfg(any(feature = "record", feature = "sherpa", test))]
impl AmbientChunkSource for UnboundedReceiver<Vec<f32>> {
    fn recv(&mut self) -> impl Future<Output = Option<Vec<f32>>> + '_ {
        UnboundedReceiver::recv(self)
    }
}

#[cfg(any(feature = "record", feature = "sherpa", test))]
impl AmbientChunkSource for BoundedPcmReceiver<Vec<f32>> {
    fn recv(&mut self) -> impl Future<Output = Option<Vec<f32>>> + '_ {
        BoundedPcmReceiver::recv(self)
    }
}

#[cfg(any(feature = "record", feature = "sherpa", test))]
impl<'a> AmbientPipeline<'a> {
    /// Assemble a pipeline with the given endpoint and ASR backend.
    ///
    /// The endpoint should not normalize audio (the VAD must see raw levels).
    /// The ASR feed is normalized inside the pipeline.
    pub async fn new(
        mut config: AmbientConfig,
        endpoint: &'a mut (dyn Endpoint + 'a),
        asr: Arc<dyn StreamingSpeechToText>,
        stop: watch::Receiver<bool>,
    ) -> Result<Self, AmbientError> {
        // Pass the human-readable wake word to the KWS config so the model can
        // tokenize it when no explicit keywords_buf is provided.
        if config.kws.keyword.is_empty() && !config.wake_word.is_empty() {
            config.kws.keyword = config.wake_word.clone();
        }

        let wake: Box<dyn WakeWordDetector> = match config.wake_word_backend {
            WakeWordBackend::AsrTextMatch => {
                let detector =
                    AsrTextMatch::new(asr.clone(), &config.wake_word, &config.asr_language).await?;
                Box::new(detector)
            }
            #[cfg(feature = "sherpa")]
            WakeWordBackend::SherpaKws => match try_load_sherpa_kws(&config.kws).await {
                Ok(detector) => Box::new(detector),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        wake_word = %config.wake_word,
                        "sherpa-kws load failed, falling back to streaming asr wake word"
                    );
                    let detector =
                        AsrTextMatch::new(asr.clone(), &config.wake_word, &config.asr_language)
                            .await?;
                    Box::new(detector)
                }
            },
            #[cfg(not(feature = "sherpa"))]
            WakeWordBackend::SherpaKws => {
                tracing::warn!(
                    wake_word = %config.wake_word,
                    "sherpa feature disabled, falling back to streaming asr wake word"
                );
                let detector =
                    AsrTextMatch::new(asr.clone(), &config.wake_word, &config.asr_language).await?;
                Box::new(detector)
            }
        };

        Ok(Self {
            config,
            endpoint,
            wake,
            asr,
            stop,
        })
    }

    /// Run the pipeline from the microphone until a wake word + command is
    /// captured, the stop signal fires, or a recording error occurs.
    #[cfg(feature = "record")]
    pub async fn run(&mut self) -> Result<Option<AmbientResult>, AmbientError> {
        let (recorder, chunk_rx) = StreamingRecorder::start(RecordConfig {
            max_duration: Duration::from_secs(self.config.max_utterance_seconds),
            target_sample_rate: Some(self.config.sample_rate),
            normalize_audio: false,
        })?;

        let result = self.run_with_chunks(chunk_rx).await;

        recorder.stop();
        // Wait for the recorder thread to finish before returning so the next
        // capture does not race for OS audio resources.
        tokio::task::spawn_blocking(move || recorder.join())
            .await
            .map_err(|e| AmbientError::Record(format!("recorder join task failed: {e}")))?
            .map_err(|e| AmbientError::Record(format!("recorder thread failed: {e}")))?;

        result
    }

    /// Run the pipeline on an existing chunk receiver. Used by tests, by
    /// `run()`, and by the Android foreground service. Any receiver with a
    /// `recv` future works, so both unbounded (desktop recorder, tests) and
    /// bounded (Android) chunk sources can feed the same gate logic.
    pub async fn run_with_chunks<R: AmbientChunkSource>(
        &mut self,
        mut chunk_rx: R,
    ) -> Result<Option<AmbientResult>, AmbientError> {
        self.endpoint.reset();
        self.wake.reset().await?;

        // If the stop signal is already set, mark it seen and abort before the
        // select loop; otherwise changed() will not fire until a *new* value is
        // sent.
        if *self.stop.borrow_and_update() {
            return Err(AmbientError::Cancelled);
        }

        let mut state = AmbientState::WaitingForSpeech;
        // Short rolling buffer of raw chunks before SpeechStart.
        let mut pre_speech: VecDeque<Vec<f32>> = VecDeque::new();
        // Running total of samples in `pre_speech` so we can cap the buffer
        // without re-summing on every chunk.
        let mut pre_speech_samples: u64 = 0;
        // Raw chunks accumulated after SpeechStart and before the wake word.
        let mut post_speech: Vec<Vec<f32>> = Vec::new();
        // Index into `post_speech` of the first chunk not yet fed to the wake
        // word detector. Resets whenever `post_speech` is rebuilt.
        let mut wake_pushed: usize = 0;
        // Post-wake ASR session. Kept in an `Option` so it can be `take`n on
        // SpeechEnd.
        let mut asr_session: Option<StreamingAsrSession> = None;

        // Clamp the pre-speech buffer to at least one recorder chunk so a
        // misconfigured or very small `pre_speech_buffer_ms` does not leave
        // the buffer permanently empty (`CHUNK_MS` is the recorder's fixed
        // chunk duration).
        let pre_speech_ms = self.config.pre_speech_buffer_ms.max(CHUNK_MS);
        let max_pre_samples = self.config.sample_rate as u64 * pre_speech_ms / 1000;

        // Total samples in the current speech segment, used to enforce
        // `max_utterance_seconds` and cap memory growth.
        let max_utterance_samples =
            self.config.max_utterance_seconds * self.config.sample_rate as u64;
        let mut utterance_samples: u64 = 0;

        loop {
            tokio::select! {
                chunk = chunk_rx.recv() => {
                    let Some(chunk) = chunk else {
                        break;
                    };

                    // Feed the VAD from the raw chunk before it is moved into
                    // any buffer.
                    let event = self.endpoint.push(&chunk);

                    if state == AmbientState::CapturingCommand
                        && let Some(session) = asr_session.as_mut()
                    {
                        session.feed(&chunk);
                    }

                    match event {
                        Some(VadEvent::SpeechStart) => {
                            state = AmbientState::ListeningForWake;
                            post_speech.clear();
                            wake_pushed = 0;
                            let chunk_len = chunk.len() as u64;
                            post_speech.extend(pre_speech.drain(..));
                            post_speech.push(chunk);
                            // The pre-speech buffer was drained into
                            // `post_speech`; account for it once and reset the
                            // running counter.
                            utterance_samples = pre_speech_samples + chunk_len;
                            pre_speech_samples = 0;
                            self.wake.reset().await?;
                        }
                        Some(VadEvent::SpeechEnd) => match state {
                            AmbientState::WaitingForSpeech | AmbientState::ListeningForWake => {
                                pre_speech.clear();
                                pre_speech_samples = 0;
                                post_speech.clear();
                                wake_pushed = 0;
                                utterance_samples = 0;
                                self.wake.reset().await?;
                                state = AmbientState::WaitingForSpeech;
                            }
                            AmbientState::CapturingCommand => {
                                if let Some(session) = asr_session.take() {
                                    let (text, samples) = session.finish().await?;
                                    let keyword = self
                                        .wake
                                        .last_keyword()
                                        .unwrap_or(&self.config.wake_word);
                                    let text = strip_wake_word(&text, keyword);
                                    return Ok(Some(AmbientResult { text, samples }));
                                }
                            }
                        },
                        None => match state {
                            AmbientState::WaitingForSpeech => {
                                push_with_cap(
                                    &mut pre_speech,
                                    &mut pre_speech_samples,
                                    chunk,
                                    max_pre_samples,
                                );
                            }
                            AmbientState::ListeningForWake => {
                                utterance_samples += chunk.len() as u64;
                                post_speech.push(chunk);
                            }
                            AmbientState::CapturingCommand => {
                                utterance_samples += chunk.len() as u64;
                                // `session.feed` borrows; `chunk` is not moved.
                                // (fed above in the pre-match `if` block)
                            }
                        },
                    }

                    // Enforce the per-utterance duration limit. Discard long
                    // false positives that never triggered the wake word; for
                    // an active command, finalize the ASR with what we have.
                    if utterance_samples >= max_utterance_samples {
                        match asr_session.take() {
                            Some(session) => {
                                let (text, samples) = session.finish().await?;
                                let keyword = self
                                    .wake
                                    .last_keyword()
                                    .unwrap_or(&self.config.wake_word);
                                let text = strip_wake_word(&text, keyword);
                                return Ok(Some(AmbientResult { text, samples }));
                            }
                            None => {
                                pre_speech.clear();
                                pre_speech_samples = 0;
                                post_speech.clear();
                                wake_pushed = 0;
                                utterance_samples = 0;
                                self.wake.reset().await?;
                                state = AmbientState::WaitingForSpeech;
                            }
                        }
                    }

                    if state == AmbientState::ListeningForWake {
                        while wake_pushed < post_speech.len() {
                            let chunk_ref = &post_speech[wake_pushed];
                            if self.wake.push(chunk_ref).await? {
                                let mut session = StreamingAsrSession::new(
                                    self.asr.clone(),
                                    &self.config.asr_language,
                                    self.config.verify_speaker,
                                )
                                .await?;
                                for c in &post_speech {
                                    session.feed(c);
                                }
                                post_speech.clear();
                                wake_pushed = 0;
                                asr_session = Some(session);
                                state = AmbientState::CapturingCommand;
                                break;
                            }
                            wake_pushed += 1;
                        }
                    }
                }
                _ = self.stop.changed() => {
                    if *self.stop.borrow() {
                        return Err(AmbientError::Cancelled);
                    }
                }
            }
        }

        // The recording stream closed without an endpoint. Finalize any open
        // ASR session and return if a command was in flight.
        if let Some(session) = asr_session.take() {
            let (text, samples) = session.finish().await?;
            let keyword = self.wake.last_keyword().unwrap_or(&self.config.wake_word);
            let text = strip_wake_word(&text, keyword);
            return Ok(Some(AmbientResult { text, samples }));
        }

        Ok(None)
    }
}

#[cfg(any(feature = "record", feature = "sherpa", test))]
fn push_with_cap(
    buffer: &mut VecDeque<Vec<f32>>,
    total: &mut u64,
    chunk: Vec<f32>,
    max_samples: u64,
) {
    *total += chunk.len() as u64;
    buffer.push_back(chunk);
    while *total > max_samples {
        let Some(front) = buffer.pop_front() else {
            break;
        };
        *total -= front.len() as u64;
    }
}

/// Remove the leading wake word (and any trailing punctuation/whitespace) from
/// an ASR transcript. The match is done on normalized text, but the split is
/// applied to the original string so the rest of the command keeps its
/// capitalization and punctuation.
///
/// The match is anchored to the start of the transcript (after any leading
/// punctuation) and stops at the first word boundary, so "abc" does not match
/// a subsequence like "aXbXc".
#[cfg(any(feature = "record", feature = "sherpa", test))]
fn strip_wake_word(text: &str, wake: &str) -> String {
    if text.is_empty() || wake.is_empty() {
        return text.to_string();
    }

    let normalized_wake = AsrTextMatch::normalize(wake);
    if normalized_wake.is_empty() {
        return text.to_string();
    }

    // Build a normalized copy of `text` one grapheme at a time, keeping a map
    // from each normalized byte position to the byte position in the original
    // string. This lets us find the split point in the original text even when
    // normalization changes the number of characters (e.g. ß -> ss).
    let mut normalized_text = String::with_capacity(text.len());
    let mut byte_positions: Vec<usize> = Vec::with_capacity(text.len());
    let mut byte_start = 0;
    let mut pending_space = false;

    for grapheme in text.graphemes(true) {
        let byte_end = byte_start + grapheme.len();
        let g_normalized = AsrTextMatch::normalize(grapheme);

        if g_normalized.is_empty() {
            if !normalized_text.is_empty() {
                pending_space = true;
            }
            byte_start = byte_end;
            continue;
        }

        if pending_space {
            normalized_text.push(' ');
            byte_positions.push(byte_start);
            pending_space = false;
        }

        // Using `byte_end` for every char means multi-char expansions (e.g.
        // ligatures) remove the whole grapheme in the original string.
        normalized_text.push_str(&g_normalized);
        for _ in g_normalized.chars() {
            byte_positions.push(byte_end);
        }

        byte_start = byte_end;
    }

    // The wake word must be a prefix of the normalized text and must be
    // followed by a word boundary (end of string or a space).
    if !normalized_text.starts_with(&normalized_wake) {
        return text.to_string();
    }
    let wake_bytes = normalized_wake.len();
    if wake_bytes < normalized_text.len() && !normalized_text[wake_bytes..].starts_with(' ') {
        return text.to_string();
    }

    let wake_char_count = normalized_wake.chars().count();
    let mut split_at = *byte_positions
        .get(wake_char_count.saturating_sub(1))
        .unwrap_or(&0);

    // Skip any trailing separators (punctuation, whitespace) after the wake word.
    for grapheme in text[split_at..].graphemes(true) {
        if AsrTextMatch::normalize(grapheme).is_empty() {
            split_at += grapheme.len();
        } else {
            break;
        }
    }

    text[split_at..].trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kws::WakeWordDetector;
    use crate::stt::{AsrStream, SpeechToText};
    use async_trait::async_trait;

    #[derive(Default)]
    struct ScriptedWake {
        fire_at: usize,
        seen: usize,
    }

    #[async_trait]
    impl WakeWordDetector for ScriptedWake {
        async fn push(&mut self, _samples: &[f32]) -> Result<bool, KwsError> {
            self.seen += 1;
            Ok(self.seen >= self.fire_at)
        }
        async fn reset(&mut self) -> Result<(), KwsError> {
            self.seen = 0;
            Ok(())
        }
    }

    #[derive(Default)]
    struct ScriptedEndpoint {
        next_start: Option<usize>,
        next_end: Option<usize>,
        pushed: usize,
    }

    impl Endpoint for ScriptedEndpoint {
        fn push(&mut self, _samples: &[f32]) -> Option<VadEvent> {
            self.pushed += 1;
            if self.next_start == Some(self.pushed) {
                self.next_start = None;
                Some(VadEvent::SpeechStart)
            } else if self.next_end == Some(self.pushed) {
                self.next_end = None;
                Some(VadEvent::SpeechEnd)
            } else {
                None
            }
        }
        fn has_speech(&self) -> bool {
            false
        }
        fn reset(&mut self) {
            self.pushed = 0;
        }
    }

    struct MockAsrStream {
        text: String,
    }

    #[async_trait]
    impl AsrStream for MockAsrStream {
        fn accept_waveform(&mut self, _samples: &[f32]) {}
        fn text(&mut self) -> String {
            self.text.clone()
        }
        async fn finish(&mut self) -> Result<String, SttError> {
            Ok(self.text.clone())
        }
    }

    struct MockStt {
        text: String,
    }

    #[async_trait]
    impl StreamingSpeechToText for MockStt {
        async fn start_stream(&self, _language: &str) -> Result<Box<dyn AsrStream>, SttError> {
            Ok(Box::new(MockAsrStream {
                text: self.text.clone(),
            }))
        }
    }

    #[async_trait]
    impl SpeechToText for MockStt {
        async fn transcribe(&self, _audio: &[f32]) -> Result<String, SttError> {
            Ok(self.text.clone())
        }
    }

    // The pipeline is generic over `Box<dyn WakeWordDetector>` and a borrowed
    // VAD endpoint, so we can inject a scripted detector and endpoint to test
    // the gate logic without any model.
    #[tokio::test]
    async fn wake_word_then_speech_end_returns_result() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(vec![0.5; 2560]).unwrap();
        tx.send(vec![0.5; 2560]).unwrap();
        tx.send(vec![0.0; 2560]).unwrap();
        drop(tx);

        let mut endpoint = ScriptedEndpoint {
            next_start: Some(1),
            next_end: Some(3),
            ..Default::default()
        };
        let mut pipeline = AmbientPipeline {
            config: AmbientConfig::default(),
            endpoint: &mut endpoint,
            wake: Box::new(ScriptedWake {
                fire_at: 2,
                seen: 0,
            }),
            asr: Arc::new(MockStt {
                text: "start report".into(),
            }),
            stop: watch::channel(false).1,
        };

        let result = pipeline.run_with_chunks(rx).await.unwrap().unwrap();
        assert_eq!(result.text, "start report");
        // Samples include the full utterance: voiced wake + command and the
        // trailing silence chunk that produced SpeechEnd.
        assert_eq!(result.samples.len(), 3 * 2560);
    }

    #[tokio::test]
    async fn no_wake_word_discards_buffer() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(vec![0.5; 2560]).unwrap();
        tx.send(vec![0.0; 2560]).unwrap();
        drop(tx);

        let mut endpoint = ScriptedEndpoint {
            next_start: Some(1),
            next_end: Some(2),
            ..Default::default()
        };
        let mut pipeline = AmbientPipeline {
            config: AmbientConfig::default(),
            endpoint: &mut endpoint,
            wake: Box::new(ScriptedWake {
                fire_at: 10,
                seen: 0,
            }),
            asr: Arc::new(MockStt {
                text: "nope".into(),
            }),
            stop: watch::channel(false).1,
        };

        let result = pipeline.run_with_chunks(rx).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn max_utterance_seconds_discards_long_false_positive() {
        // One second of audio should be enough to trigger the per-utterance
        // limit and discard the buffer before the wake word fires.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(vec![0.5; 16000]).unwrap();
        tx.send(vec![0.0; 16000]).unwrap();
        drop(tx);

        let config = AmbientConfig {
            max_utterance_seconds: 1,
            pre_speech_buffer_ms: 0,
            ..Default::default()
        };

        let mut endpoint = ScriptedEndpoint {
            next_start: Some(1),
            next_end: None,
            ..Default::default()
        };
        let mut pipeline = AmbientPipeline {
            config,
            endpoint: &mut endpoint,
            wake: Box::new(ScriptedWake {
                fire_at: 10,
                seen: 0,
            }),
            asr: Arc::new(MockStt {
                text: "nope".into(),
            }),
            stop: watch::channel(false).1,
        };

        let result = pipeline.run_with_chunks(rx).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn max_utterance_seconds_finalizes_active_command() {
        // A wake word on the first chunk, then a second chunk, should trigger
        // the limit while capturing and finalize the ASR result.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tx.send(vec![0.5; 8000]).unwrap();
        tx.send(vec![0.5; 8000]).unwrap();
        drop(tx);

        let config = AmbientConfig {
            max_utterance_seconds: 1,
            pre_speech_buffer_ms: 0,
            ..Default::default()
        };

        let mut endpoint = ScriptedEndpoint {
            next_start: Some(1),
            next_end: None,
            ..Default::default()
        };
        let mut pipeline = AmbientPipeline {
            config,
            endpoint: &mut endpoint,
            wake: Box::new(ScriptedWake {
                fire_at: 1,
                seen: 0,
            }),
            asr: Arc::new(MockStt {
                text: "hello world".into(),
            }),
            stop: watch::channel(false).1,
        };

        let result = pipeline.run_with_chunks(rx).await.unwrap().unwrap();
        assert_eq!(result.text, "hello world");
        assert_eq!(result.samples.len(), 16000);
    }

    #[test]
    fn strip_wake_word_handles_combining_marks() {
        // "e" + COMBINING ACUTE ACCENT should be treated as a single grapheme
        // and removed in full when it matches the wake word.
        let text = "\u{00e9} turn on the lights";
        let wake = "\u{00e9}";
        assert_eq!(strip_wake_word(text, wake), "turn on the lights");
    }

    #[test]
    fn strip_wake_word_handles_esszet() {
        // U+00DF normalizes to itself (Rust lowercasing does not expand it),
        // so it must still match the wake word "ß".
        let text = "\u{00df} please";
        let wake = "\u{00df}";
        assert_eq!(strip_wake_word(text, wake), "please");
    }

    #[test]
    fn strip_wake_word_does_not_match_subsequence() {
        // The wake word must be a contiguous prefix, not scattered characters.
        assert_eq!(strip_wake_word("aXbXc", "abc"), "aXbXc");
        assert_eq!(strip_wake_word("たXくXす", "たくす"), "たXくXす");
    }
}
