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

#[cfg(any(feature = "record", test))]
use std::collections::VecDeque;
#[cfg(any(feature = "record", test))]
use std::sync::Arc;
#[cfg(any(feature = "record", test))]
use std::time::Duration;

#[cfg(any(feature = "record", test))]
use tokio::sync::mpsc::UnboundedReceiver;
#[cfg(any(feature = "record", test))]
use tokio::sync::watch;

#[cfg(any(feature = "record", test))]
use crate::Endpoint;
#[cfg(any(feature = "record", test))]
use crate::kws::{AsrTextMatch, WakeWordDetector};
#[cfg(any(feature = "record", test))]
use crate::stream_asr::StreamingAsrSession;
#[cfg(any(feature = "record", test))]
use crate::stt::StreamingSpeechToText;
#[cfg(any(feature = "record", test))]
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
    /// Heavier, but language-agnostic.
    #[default]
    AsrTextMatch,
    /// Use a tiny sherpa-onnx transducer keyword spotter.
    /// Requires a tokenized keyword string in `kws.keywords_buf` and the
    /// `sherpa` feature. `KwsConfig::for_wenetspeech` leaves `keywords_buf`
    /// empty by default, so callers must populate it before selecting this
    /// backend.
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

/// State machine for one ambient capture loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(feature = "record", test))]
enum AmbientState {
    /// Waiting for any speech; pre-speech chunks roll through a short buffer.
    WaitingForSpeech,
    /// An utterance has started; feeding the wake word detector.
    ListeningForWake,
    /// The wake word fired; capturing the full command in the ASR stream.
    CapturingCommand,
}

/// The shared ambient gate pipeline.
#[cfg(any(feature = "record", test))]
pub struct AmbientPipeline<'a> {
    config: AmbientConfig,
    endpoint: &'a mut (dyn Endpoint + 'a),
    wake: Box<dyn WakeWordDetector>,
    asr: Arc<dyn StreamingSpeechToText>,
    stop: watch::Receiver<bool>,
}

#[cfg(any(feature = "record", test))]
impl<'a> AmbientPipeline<'a> {
    /// Assemble a pipeline with the given endpoint and ASR backend.
    ///
    /// The endpoint should not normalize audio (the VAD must see raw levels).
    /// The ASR feed is normalized inside the pipeline.
    pub async fn new(
        config: AmbientConfig,
        endpoint: &'a mut (dyn Endpoint + 'a),
        asr: Arc<dyn StreamingSpeechToText>,
        stop: watch::Receiver<bool>,
    ) -> Result<Self, AmbientError> {
        let wake: Box<dyn WakeWordDetector> = match config.wake_word_backend {
            WakeWordBackend::AsrTextMatch => {
                let detector =
                    AsrTextMatch::new(asr.clone(), &config.wake_word, &config.asr_language).await?;
                Box::new(detector)
            }
            #[cfg(feature = "sherpa")]
            WakeWordBackend::SherpaKws => {
                let kws_config = config.kws.clone();
                let detector =
                    tokio::task::spawn_blocking(move || crate::kws::SherpaKws::new(&kws_config))
                        .await
                        .map_err(|e| KwsError::Other(e.to_string().into()))??;
                Box::new(detector)
            }
            #[cfg(not(feature = "sherpa"))]
            WakeWordBackend::SherpaKws => {
                return Err(
                    KwsError::Other("sherpa KWS requires the `sherpa` feature".into()).into(),
                );
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

    /// Run the pipeline on an existing chunk receiver. Used by tests and by
    /// `run()` to enable testing without a real microphone.
    pub async fn run_with_chunks(
        &mut self,
        mut chunk_rx: UnboundedReceiver<Vec<f32>>,
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
        let pre_speech_ms = self
            .config
            .pre_speech_buffer_ms
            .max(crate::record_streaming::CHUNK_MS);
        let max_pre_samples = self.config.sample_rate as u64 * pre_speech_ms / 1000;

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
                            post_speech.extend(pre_speech.drain(..));
                            post_speech.push(chunk);
                            self.wake.reset().await?;
                        }
                        Some(VadEvent::SpeechEnd) => match state {
                            AmbientState::WaitingForSpeech | AmbientState::ListeningForWake => {
                                pre_speech.clear();
                                post_speech.clear();
                                wake_pushed = 0;
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
                                push_with_cap(&mut pre_speech, chunk, max_pre_samples);
                            }
                            AmbientState::ListeningForWake => {
                                post_speech.push(chunk);
                            }
                            AmbientState::CapturingCommand => {}
                        },
                    }

                    if state == AmbientState::ListeningForWake {
                        while wake_pushed < post_speech.len() {
                            let chunk_ref = &post_speech[wake_pushed];
                            if self.wake.push(chunk_ref).await? {
                                let mut session = StreamingAsrSession::new(
                                    self.asr.clone(),
                                    &self.config.asr_language,
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

#[cfg(any(feature = "record", test))]
fn push_with_cap(buffer: &mut VecDeque<Vec<f32>>, chunk: Vec<f32>, max_samples: u64) {
    buffer.push_back(chunk);
    let mut total: u64 = buffer.iter().map(|c| c.len() as u64).sum();
    while total > max_samples {
        let Some(front) = buffer.pop_front() else {
            break;
        };
        total -= front.len() as u64;
    }
}

/// Remove the leading wake word (and any trailing punctuation/whitespace) from
/// an ASR transcript. The match is done on normalized text, but the split is
/// applied to the original string so the rest of the command keeps its
/// capitalization and punctuation.
#[cfg(any(feature = "record", test))]
fn strip_wake_word(text: &str, wake: &str) -> String {
    if text.is_empty() || wake.is_empty() {
        return text.to_string();
    }

    let normalized_wake = AsrTextMatch::normalize(wake);
    if normalized_wake.is_empty() {
        return text.to_string();
    }

    let mut wake_chars = normalized_wake.chars().peekable();
    let mut split_at = None;
    let mut byte_pos = 0;

    for c in text.chars() {
        if c.is_whitespace() || !c.is_alphanumeric() {
            // normalize drops these, so skip them but keep the byte offset
            // so a split can happen right after the wake word.
            byte_pos += c.len_utf8();
            continue;
        }

        for lc in c.to_lowercase() {
            if wake_chars.peek() == Some(&lc) {
                wake_chars.next();
            }
            if wake_chars.peek().is_none() {
                split_at = Some(byte_pos + c.len_utf8());
                break;
            }
        }

        if split_at.is_some() {
            break;
        }
        byte_pos += c.len_utf8();
    }

    if wake_chars.peek().is_some() {
        // The wake word was not found at the start of the transcript.
        return text.to_string();
    }

    let rest = &text[split_at.unwrap_or(text.len())..];
    rest.trim_start().to_string()
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
}
