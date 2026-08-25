//! Application-level audio adapter for the takusu agent.
//!
//! This module is responsible for the audio loop:
//! record → transcribe → agent turn → synthesize → play. It backs both the
//! push-to-talk CLI loop ([`AudioAdapter::run`]) and the continuous voice
//! session ([`AudioAdapter`] implements [`VoiceSessionIo`]), where recording
//! stops itself via VAD endpointing. It is not exposed as an LLM tool.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use takusu_audio::play::{
    PcmFormat, PlayError, StreamedAudioFormat, decode_pcm_chunk, play_stream,
};
use takusu_audio::{
    Aec, AmbientError, AmbientPipeline, AmbientResult, BargeInDetector, CartesiaSonic,
    CartesiaSonicConfig, DEFAULT_SPEAKER_MODEL_ID, FishAudio, FishAudioConfig, LatencyBudget,
    LatencyCheckpoint, MIN_SPEAKER_AUDIO_SECONDS, ModelCache, NlmsAec, NoOpAec, RecordConfig,
    SHERPA_SAMPLE_RATE, SpeakerConfig, SpeakerEmbeddingMatch, SpeakerVerifier, StreamingRecorder,
    StreamingSpeechToText, TextToSpeech, TtsBackend, TtsError, TtsOptions, TtsRequest, TtsStream,
    VadEvent, VerificationResult, mix_to_mono, normalize, normalize_for_tts, resample,
};
use thiserror::Error;

use crate::capability::InputPath;
use crate::surface::AudioCallback;
use crate::voice_session::{InputOrigin, ProcessedTurn, VoiceSessionError, VoiceSessionIo};
use crate::{AgentError, AgentSession, TurnEvent, TurnResult};

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("recording failed: {0}")]
    Record(String),
    #[error("transcription failed: {0}")]
    Transcribe(String),
    #[error("tts failed: {0}")]
    Tts(String),
    #[error("playback failed: {0}")]
    Play(String),
    #[error("audio backend {0} is not supported")]
    UnsupportedBackend(String),
    #[error("audio operation timed out")]
    Timeout,
    /// The caller requested an in-progress capture or playback to stop.
    #[error("audio session stopped")]
    UserCancelled,
    /// A `Mutex` / `RwLock` guard was poisoned by a panic while held.
    #[error("lock poisoned: {0}")]
    Lock(String),
    #[error("speaker verification failed: {0}")]
    Speaker(#[from] takusu_audio::SpeakerError),
    #[error("{0}")]
    Other(String),
}

impl AudioError {
    /// Whether the error is likely transient and the session can keep listening.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Timeout
                | Self::Record(_)
                | Self::Transcribe(_)
                | Self::Tts(_)
                | Self::Play(_)
                | Self::Other(_)
        )
    }
}

// Generic `From` so audio call sites can write `.read()?` / `.lock()?`
// directly against `Result<_, AudioError>`. See `AgentError` for rationale.
impl<G> From<std::sync::PoisonError<G>> for AudioError {
    fn from(e: std::sync::PoisonError<G>) -> Self {
        AudioError::Lock(e.to_string())
    }
}

impl From<takusu_audio::tts::TtsError> for AudioError {
    fn from(e: takusu_audio::tts::TtsError) -> Self {
        AudioError::Tts(e.to_string())
    }
}

impl From<PlayError> for AudioError {
    fn from(e: PlayError) -> Self {
        AudioError::Play(e.to_string())
    }
}

pub use crate::audio_config::{AudioConfig, SttConfig, TtsConfig};

/// Shared sink for streaming assistant turn events.
type EventSink = Arc<Mutex<dyn FnMut(TurnEvent) + Send>>;

/// Shared sink for audio lifecycle callbacks (listening / speaking / finished).
type AudioCallbackSink = Arc<Mutex<dyn FnMut(AudioCallback) + Send>>;

/// Temporary guard that removes the cached VAD endpoint from `AudioAdapter` and
/// restores it when dropped, so `capture_utterance` can mutably borrow the
/// endpoint while still calling `&self` methods like `emit`.
struct EndpointGuard<'a> {
    slot: &'a mut Option<Box<dyn takusu_audio::Endpoint>>,
    endpoint: Option<Box<dyn takusu_audio::Endpoint>>,
}

impl<'a> EndpointGuard<'a> {
    fn new(slot: &'a mut Option<Box<dyn takusu_audio::Endpoint>>) -> Option<Self> {
        let endpoint = slot.take();
        if endpoint.is_none() {
            *slot = endpoint;
            return None;
        }
        Some(Self { slot, endpoint })
    }

    fn get_mut(&mut self) -> &mut dyn takusu_audio::Endpoint {
        // The option is always Some while the guard lives.
        &mut **self.endpoint.as_mut().expect("endpoint present")
    }
}

impl Drop for EndpointGuard<'_> {
    fn drop(&mut self) {
        *self.slot = self.endpoint.take();
    }
}

/// Application-level audio adapter. Owns the agent session and the audio clients.
pub struct AudioAdapter {
    session: Arc<AgentSession>,
    last_audio: AudioConfig,
    stt: Arc<dyn StreamingSpeechToText>,
    tts: Arc<dyn TextToSpeech>,
    tts_voice_id: String,
    tts_speed: Option<f32>,
    tts_format: StreamedAudioFormat,
    /// Flipped by [`stop_tts_signal`](Self::stop_tts_signal) to cut an
    /// in-progress TTS playback.
    stop_tts: Arc<AtomicBool>,
    /// Receiver for an out-of-band stop signal. The matching sender lives in
    /// [`VoiceSessionHandle`] (desktop) or is dropped for the push-to-talk loop.
    stop: tokio::sync::watch::Receiver<bool>,
    /// Sender paired with `stop`. `None` when the stop channel is owned by a
    /// caller outside the adapter (e.g. the desktop voice session handle).
    stop_tx: Option<tokio::sync::watch::Sender<bool>>,
    /// Sink for assistant turn events, used by the CLI runner and the voice
    /// session so surfaces can render streaming state.
    on_event: Option<EventSink>,
    /// Sink for audio lifecycle callbacks, used by the desktop daemon to keep
    /// the shared `SurfaceStateMachine` in sync with capture and playback.
    on_audio_callback: Option<AudioCallbackSink>,
    /// Cached VAD endpoint, loaded once when the adapter is created and reused
    /// across every utterance instead of rebuilding the Silero detector each
    /// turn.
    endpoint: Option<Box<dyn takusu_audio::Endpoint>>,
    /// Cached VAD endpoint for ambient listening, built with
    /// `audio.ambient.max_utterance_seconds` as its speech cap. Lazily created
    /// on the first ambient capture.
    ambient_endpoint: Option<Box<dyn takusu_audio::Endpoint>>,
    /// Optional speaker embedding verifier for voiceprint enrollment and
    /// verification. Shared so `&self` methods can run verification.
    speaker_verifier: Option<Arc<SpeakerVerifier>>,
    /// The most recently captured 16 kHz mono f32 samples. This is updated by
    /// `capture_utterance` and is used to verify the speaker for voice
    /// confirmations and ambient-immediate commands.
    last_captured_samples: Vec<f32>,
    /// Latency budget for the current/resident voice turn. Collected per turn
    /// and reported when the turn finishes. `Arc` so spawned TTS tasks can
    /// record checkpoints without borrowing `self`.
    latency: Arc<tokio::sync::Mutex<LatencyBudget>>,
}

impl AudioAdapter {
    /// Create an audio adapter from an existing agent session.
    pub async fn new(session: Arc<AgentSession>) -> Result<Self, AudioError> {
        let audio = {
            let config = session.config.read()?;
            config.audio.clone()
        };
        let (stt, tts, voice_id, speed, tts_format) = Self::build_audio(&audio).await?;
        let vad_config = takusu_audio::VadEndpointConfig {
            energy_threshold: audio.vad.energy_threshold,
            ..Default::default()
        };
        let endpoint = takusu_audio::default_endpoint_async_with_config(vad_config).await;
        let speaker_verifier = Self::build_speaker_verifier(audio.speaker.as_ref()).await?;
        let (stop_tx, stop) = tokio::sync::watch::channel(false);
        Ok(Self {
            session,
            last_audio: audio,
            stt,
            tts,
            tts_voice_id: voice_id,
            tts_speed: speed,
            tts_format,
            stop_tts: Arc::new(AtomicBool::new(false)),
            stop,
            stop_tx: Some(stop_tx),
            on_event: None,
            on_audio_callback: None,
            endpoint: Some(endpoint),
            ambient_endpoint: None,
            speaker_verifier,
            last_captured_samples: Vec::new(),
            latency: Arc::new(tokio::sync::Mutex::new(LatencyBudget::new())),
        })
    }

    /// Route assistant turn events to `f`. Used by the CLI runner and any
    /// surface that wants to render streaming transcriptions.
    pub fn with_events(mut self, f: impl FnMut(TurnEvent) + Send + 'static) -> Self {
        self.on_event = Some(Arc::new(Mutex::new(f)));
        self
    }

    /// Route audio lifecycle callbacks to `f`. Used by the desktop daemon to
    /// keep the shared `SurfaceStateMachine` in sync with capture and playback.
    pub fn with_audio_callback(mut self, f: impl FnMut(AudioCallback) + Send + 'static) -> Self {
        self.on_audio_callback = Some(Arc::new(Mutex::new(f)));
        self
    }

    /// Replace the stop signal receiver with one owned by the caller.
    /// The matching sender is typically held by a [`VoiceSessionHandle`].
    pub fn with_stop_signal(mut self, stop: tokio::sync::watch::Receiver<bool>) -> Self {
        self.stop = stop;
        self.stop_tx = None;
        self
    }

    /// Request that the current capture or playback stop. This flips the TTS
    /// stop flag and, if the adapter owns the stop sender, sends a stop signal.
    pub fn request_stop(&self) {
        self.stop_tts_signal();
        if let Some(tx) = self.stop_tx.as_ref() {
            let _ = tx.send(true);
        }
    }

    /// Emit a turn event to the configured sink, if any.
    fn emit(&self, event: TurnEvent) {
        Self::emit_with(&self.on_event, event);
    }

    /// Emit a turn event to `sink` without borrowing `self`.
    fn emit_with(sink: &Option<EventSink>, event: TurnEvent) {
        if let Some(sink) = sink
            && let Ok(mut guard) = sink.lock()
        {
            guard(event);
        }
    }

    /// Emit an audio lifecycle callback to the configured sink, if any.
    fn audio_callback(&self, callback: AudioCallback) {
        Self::audio_callback_with(&self.on_audio_callback, callback);
    }

    /// Emit an audio callback to `sink` without borrowing `self`.
    fn audio_callback_with(sink: &Option<AudioCallbackSink>, callback: AudioCallback) {
        if let Some(sink) = sink
            && let Ok(mut guard) = sink.lock()
        {
            guard(callback);
        }
    }

    /// Request that any in-progress TTS playback stop at the next block.
    ///
    /// Surfaces register this as a callback on [`crate::surface::SurfaceStateMachine`]
    /// via [`Self::stop_tts_callback`] so tray/mobile surfaces can stop the
    /// assistant's speech mid-turn.
    pub fn stop_tts_signal(&self) {
        self.stop_tts.store(true, Ordering::Relaxed);
    }

    /// Return a `'static'` closure that calls [`Self::stop_tts_signal`].
    ///
    /// The closure captures only the shared `stop_tts` flag, so it can be stored
    /// in the surface state machine without borrowing the adapter.
    pub fn stop_tts_callback(&self) -> Box<dyn Fn() + Send + 'static> {
        let stop_tts = Arc::clone(&self.stop_tts);
        Box::new(move || {
            stop_tts.store(true, Ordering::Relaxed);
        })
    }

    /// Run the push-to-talk loop until interrupted or an unrecoverable error.
    ///
    /// Recording stops itself on VAD endpointing; pressing Enter on stdin also
    /// cuts the active recording. `no_tts` mutes speech and `yes` auto-approves
    /// any approval request.
    pub async fn run(&mut self, no_tts: bool, yes: bool) -> Result<(), AgentError> {
        // Single background thread that reads "Enter" from stdin and routes it
        // to the currently active recording's stop channel. This avoids spawning
        // a new thread per turn and prevents multiple readers from competing for
        // the same stdin bytes.
        let current_stop: Arc<Mutex<Option<tokio::sync::mpsc::Sender<()>>>> =
            Arc::new(Mutex::new(None));
        let current_stop_c = Arc::clone(&current_stop);
        let _stop_thread = tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            loop {
                line.clear();
                let _ = std::io::stdin().read_line(&mut line);
                if let Some(tx) = current_stop_c
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                {
                    let _ = tx.blocking_send(());
                }
            }
        });

        loop {
            self.reconfigure_if_needed().await?;

            let (stop_tx, stop_rx) = tokio::sync::mpsc::channel(1);
            *current_stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop_tx);

            let captured = self
                .capture_utterance(stop_rx)
                .await
                .map_err(AgentError::Audio)?;
            *current_stop.lock().unwrap_or_else(|e| e.into_inner()) = None;

            let Some(text) = captured else {
                continue;
            };

            self.emit(TurnEvent::AsrText(text.clone()));

            // Push-to-talk is a single-shot turn; any barge-in during the
            // reply is intentionally ignored because there is no session to
            // route it into.
            let (mut result, _barge_in) = self
                .run_agent_turn(&text, !no_tts, InputPath::ExplicitVoiceSession)
                .await?;

            if let Some(approval) = result.approval_request.as_ref() {
                // The `--yes` flag is a development convenience. It bypasses the
                // voice / screen approval layers and should not be enabled in
                // release builds.
                if yes {
                    let res = self
                        .session
                        .resolve_approval(&approval.id, true, None)
                        .await
                        .map_err(|e| AudioError::Other(e.to_string()))?;
                    if res.approved {
                        eprintln!("approved {} change(s)", res.changes.len());
                        result.changes = res.changes;
                        result.schedule_dirty |= res.schedule_dirty;
                    } else {
                        eprintln!("denied");
                    }
                } else {
                    eprintln!("approval required; re-run with --yes to auto-approve");
                }
                result.approval_request = None;
            }

            if !result.changes.is_empty() {
                match serde_json::to_string_pretty(&result.changes) {
                    Ok(changes) => eprintln!("{changes}"),
                    Err(e) => eprintln!("changes: {e}"),
                }
            }

            if result.schedule_dirty {
                eprintln!("schedule dirty: true");
            }
        }
    }

    /// Capture one utterance: stream the microphone into the ASR session and
    /// stop when VAD endpointing detects the end of speech, when `stop_rx`
    /// signals a manual cut, or when the out-of-band stop signal fires.
    /// Returns `None` when no speech was detected.
    async fn capture_utterance(
        &mut self,
        mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    ) -> Result<Option<String>, AudioError> {
        if *self.stop.borrow() {
            return Err(AudioError::UserCancelled);
        }

        self.reconfigure_if_needed().await?;
        self.latency.lock().await.reset();

        let language = self.last_audio.stt.language.clone();
        let mut asr_stream = self
            .stt
            .start_stream(&language)
            .await
            .map_err(|e| AudioError::Transcribe(e.to_string()))?;

        let (recorder, mut chunk_rx) = StreamingRecorder::start(RecordConfig {
            max_duration: Duration::from_secs(60),
            // Feed the VAD gate raw levels; normalization would amplify
            // silence into "speech" and the endpoint would never fire.
            normalize_audio: false,
            ..Default::default()
        })
        .map_err(|e| AudioError::Record(e.to_string()))?;

        // Tell any surface that we are now listening.
        self.audio_callback(AudioCallback::Listening);

        // Snapshot the event sink so we can emit events while the endpoint
        // guard holds a mutable borrow on `self.endpoint`.
        let on_event = self.on_event.clone();
        let emit = |event: TurnEvent| Self::emit_with(&on_event, event);

        // Latency recording needs a cloned Arc so it can be called while the
        // endpoint guard holds a mutable borrow on `self`.
        let latency = Arc::clone(&self.latency);
        let record_latency = self.last_audio.barge_in.record_latency;

        // Reuse the cached VAD endpoint (loaded once when the adapter was
        // created) and reset it for a fresh utterance.
        let mut endpoint = EndpointGuard::new(&mut self.endpoint)
            .ok_or_else(|| AudioError::Other("VAD endpoint not initialized".into()))?;
        endpoint.get_mut().reset();

        // Accumulate raw 16 kHz mono f32 samples for speaker verification.
        self.last_captured_samples.clear();

        let mut last_text = String::new();
        let mut stopped = false;
        loop {
            tokio::select! {
                chunk = chunk_rx.recv() => match chunk {
                    Some(chunk) => {
                        // VAD must see raw microphone levels so near-silence is not
                        // amplified into "speech", but ASR is generally happier with
                        // a normalized RMS. Keep the raw chunk for endpointing and
                        // feed a normalized copy to the transcription stream.
                        self.last_captured_samples.extend_from_slice(&chunk);
                        let reached_end =
                            endpoint.get_mut().push(&chunk) == Some(VadEvent::SpeechEnd);
                        asr_stream.accept_waveform(&normalize(&chunk, 0.1));
                        let text = asr_stream.text();
                        if text != last_text {
                            last_text = text.clone();
                            emit(TurnEvent::AsrText(text));
                        }
                        if reached_end {
                            Self::record_latency_shared(
                                &latency,
                                record_latency,
                                LatencyCheckpoint::VadEndpoint,
                            )
                            .await;
                            recorder.stop();
                            break;
                        }
                    }
                    None => break,
                },
                _ = stop_rx.recv() => {
                    recorder.stop();
                    break;
                },
                _ = self.stop.changed() => {
                    recorder.stop();
                    stopped = true;
                    break;
                }
            }
        }

        // Join on a blocking thread so the std thread::join does not trip
        // tokio's blocking-in-async guard.
        tokio::task::spawn_blocking(move || recorder.join())
            .await
            .map_err(|e| AudioError::Record(format!("recording thread join task failed: {e}")))?
            .map_err(|e| AudioError::Record(format!("recording thread panicked: {e}")))?;

        if stopped {
            return Err(AudioError::UserCancelled);
        }

        let text = asr_stream
            .finish()
            .await
            .map_err(|e| AudioError::Transcribe(e.to_string()))?;
        Self::record_latency_shared(&latency, record_latency, LatencyCheckpoint::AsrFinal).await;
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }

    /// Map an ambient-pipeline error to the agent's audio error space.
    fn ambient_to_audio_error(e: AmbientError) -> AudioError {
        tracing::error!(error = %e, "ambient pipeline failed");
        match e {
            AmbientError::Cancelled => AudioError::UserCancelled,
            AmbientError::Record(_) => AudioError::Record(format!("{e}")),
            AmbientError::Asr(_) | AmbientError::Stt(_) => AudioError::Transcribe(format!("{e}")),
            AmbientError::Wake(_) => AudioError::Other(format!("ambient pipeline failed: {e}")),
        }
    }

    /// Capture one ambient utterance: wait for the configured wake word, then
    /// stream the full command to ASR. Returns `None` when the wake word does
    /// not fire or the pipeline is cancelled.
    async fn capture_ambient(&mut self) -> Result<Option<String>, AudioError> {
        if *self.stop.borrow() {
            return Err(AudioError::UserCancelled);
        }

        self.reconfigure_if_needed().await?;

        if !self.last_audio.ambient.enabled {
            return Err(AudioError::Other("ambient listening is not enabled".into()));
        }

        self.latency.lock().await.reset();
        self.last_captured_samples.clear();
        self.audio_callback(AudioCallback::Listening);

        // Snapshot the callback sink so we can emit the Transcribing state
        // while `AmbientPipeline` holds a mutable borrow on `self`.
        let on_audio_callback = self.on_audio_callback.clone();

        let audio = self.last_audio.clone();
        if self.ambient_endpoint.is_none() {
            let vad_config = takusu_audio::VadEndpointConfig {
                energy_threshold: audio.vad.energy_threshold,
                max_speech: Duration::from_secs(audio.ambient.max_utterance_seconds),
                ..Default::default()
            };
            self.ambient_endpoint =
                Some(takusu_audio::default_endpoint_async_with_config(vad_config).await);
        }

        let stop = self.stop.clone();
        let endpoint = self
            .ambient_endpoint
            .as_mut()
            .ok_or_else(|| AudioError::Other("ambient VAD endpoint not initialized".into()))?;
        let mut pipeline = AmbientPipeline::new(
            audio.ambient.clone(),
            &mut **endpoint,
            self.stt.clone(),
            stop,
        )
        .await
        .map_err(Self::ambient_to_audio_error)?;

        Self::audio_callback_with(&on_audio_callback, AudioCallback::Transcribing);

        let result = pipeline.run().await.map_err(Self::ambient_to_audio_error)?;

        let Some(AmbientResult { text, samples }) = result else {
            return Ok(None);
        };

        self.last_captured_samples = samples;
        if text.trim().is_empty() {
            return Ok(None);
        }
        self.emit(TurnEvent::AsrText(text.clone()));
        Ok(Some(text))
    }

    /// Run one agent turn from `text` on the given `input_path`, streaming TTS
    /// when `speak` is `true`, and return the turn result plus any text the
    /// user barged in with while the assistant was speaking.
    async fn run_agent_turn(
        &mut self,
        text: &str,
        speak: bool,
        input_path: InputPath,
    ) -> Result<(TurnResult, Option<String>), AgentError> {
        // Reset the tap-to-stop flag for this turn.
        self.stop_tts.store(false, Ordering::Relaxed);

        let no_tts_this_turn = !speak || self.last_audio.tts.mute;
        let barge_in_enabled = !no_tts_this_turn && self.last_audio.barge_in.enabled;
        let record_latency = self.last_audio.barge_in.record_latency;

        Self::record_latency_shared(&self.latency, record_latency, LatencyCheckpoint::LlmStart)
            .await;

        let (tts_tx, tts_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel::<TtsStream>(3);
        let (reference_tx, reference_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
        let tts = Arc::clone(&self.tts);
        let tts_format = self.tts_format;
        let voice_id = Arc::new(self.tts_voice_id.clone());
        let speed = self.tts_speed;
        let tts_language = Arc::new(self.last_audio.tts.language.clone());
        let stop_tts = Arc::clone(&self.stop_tts);
        let stop_tts_play = Arc::clone(&stop_tts);
        let stop_tts_barge = Arc::clone(&stop_tts);
        let latency = Arc::clone(&self.latency);
        let latency_play = Arc::clone(&latency);
        let first_tts_text = Arc::new(AtomicBool::new(true));
        let first_tts_audio = Arc::new(AtomicBool::new(true));
        let reference_tx_play = reference_tx.clone();

        enum TurnTaskResult {
            TtsDone,
            BargeIn(Option<String>, Vec<f32>),
        }

        let tts_synth = async move {
            if no_tts_this_turn {
                return Ok(TurnTaskResult::TtsDone);
            }

            use futures_util::StreamExt;

            let stream = futures_util::stream::unfold(tts_rx, |mut rx| async move {
                rx.recv().await.map(|block| (block, rx))
            })
            .filter(|block| std::future::ready(!block.trim().is_empty()))
            .map(move |block| {
                let tts = Arc::clone(&tts);
                let voice_id = Arc::clone(&voice_id);
                let tts_language = Arc::clone(&tts_language);
                async move {
                    synthesize_stream_with_timeout(
                        tts.as_ref(),
                        &block,
                        voice_id.as_str(),
                        tts_language.as_str(),
                        speed,
                        Duration::from_secs(120),
                    )
                    .await
                }
            })
            .buffered(3);

            tokio::pin!(stream);

            while let Some(stream) = stream.next().await {
                if stop_tts.load(Ordering::Relaxed) {
                    break;
                }
                let stream = stream?;
                if audio_tx.send(stream).await.is_err() {
                    break;
                }
            }
            Ok(TurnTaskResult::TtsDone)
        };

        let tts_play = async move {
            use futures_util::StreamExt;

            let mut first_stream = true;
            while let Some(stream) = audio_rx.recv().await {
                if stop_tts_play.load(Ordering::Relaxed) {
                    break;
                }
                if first_stream {
                    first_stream = false;
                    if record_latency {
                        Self::record_latency_shared(
                            &latency_play,
                            record_latency,
                            LatencyCheckpoint::PlaybackStart,
                        )
                        .await;
                    }
                }

                // Record FirstTtsAudio when the first audio chunk actually
                // arrives from the TTS stream.
                let latency_for_first_audio = Arc::clone(&latency_play);
                let first_audio = Arc::clone(&first_tts_audio);
                let stream = stream.inspect(move |res| {
                    if record_latency && first_audio.load(Ordering::Relaxed) && res.is_ok() {
                        first_audio.store(false, Ordering::Relaxed);
                        let latency = Arc::clone(&latency_for_first_audio);
                        tokio::spawn(async move {
                            Self::record_latency_shared(
                                &latency,
                                true,
                                LatencyCheckpoint::FirstTtsAudio,
                            )
                            .await;
                        });
                    }
                });
                let stream: TtsStream = Box::pin(stream);

                if barge_in_enabled {
                    play_stream_with_reference(
                        stream,
                        tts_format,
                        reference_tx_play.clone(),
                        Arc::clone(&stop_tts_play),
                        Duration::from_secs(120),
                    )
                    .await?;
                } else {
                    play_stream_with_timeout(
                        stream,
                        tts_format,
                        Arc::clone(&stop_tts_play),
                        Duration::from_secs(120),
                    )
                    .await?;
                }
            }
            Ok(TurnTaskResult::TtsDone)
        };

        let mut set = tokio::task::JoinSet::new();
        set.spawn(tts_synth);
        set.spawn(tts_play);

        // If barge-in is enabled, start the listener before the agent turn
        // begins so the microphone is open and the AEC reference buffer is
        // already being filled by the time TTS playback starts.
        if barge_in_enabled {
            let stop = self.stop.clone();
            let stt = Arc::clone(&self.stt);
            let audio = self.last_audio.clone();
            set.spawn(async move {
                let (text, samples) =
                    Self::barge_in_loop(reference_rx, stop_tts_barge, stop, stt, audio).await?;
                Ok(TurnTaskResult::BargeIn(text, samples))
            });
        }

        // The turn event callback may be called from a spawned task, so move
        // the cloned event sink into the closure instead of borrowing `self`.
        if !no_tts_this_turn {
            self.audio_callback(AudioCallback::Speaking);
        }
        let on_event = self.on_event.clone();
        let llm_timeout = Duration::from_secs(50);
        let latency_for_first_text = Arc::clone(&latency);
        let record_latency_for_first_text = record_latency;
        let tts_tx_for_callback = tts_tx.clone();

        // Stop requests (from VoiceSessionHandle / surface) cancel the LLM/TTS
        // wait instead of letting the turn run to completion (WI-12).
        let mut stop = self.stop.clone();
        stop.mark_changed();

        let result = tokio::select! {
            result = tokio::time::timeout(
                llm_timeout,
                self.session.run_turn_stream_with_input(
                    text,
                    input_path,
                    move |event| Self::emit_with(&on_event, event),
                    move |block| {
                        if !no_tts_this_turn {
                            if first_tts_text.load(Ordering::Relaxed) {
                                first_tts_text.store(false, Ordering::Relaxed);
                                if record_latency_for_first_text {
                                    let latency = Arc::clone(&latency_for_first_text);
                                    tokio::spawn(async move {
                                        Self::record_latency_shared(
                                            &latency,
                                            true,
                                            LatencyCheckpoint::FirstTtsText,
                                        )
                                        .await;
                                    });
                                }
                            }
                            let _ = tts_tx_for_callback.send(block);
                        }
                    },
                    !no_tts_this_turn,
                ),
            ) => match result {
                Ok(Ok(result)) => result,
                Ok(Err(e)) => {
                    drop(tts_tx);
                    drop(set);
                    return Err(e);
                }
                Err(_) => {
                    drop(tts_tx);
                    drop(set);
                    return Err(AgentError::Audio(AudioError::Timeout));
                }
            },
            _ = stop.changed() => {
                drop(tts_tx);
                self.stop_tts.store(true, Ordering::Relaxed);
                drop(set);
                return Err(AgentError::Audio(AudioError::UserCancelled));
            }
        };

        // Drop the text sender so the synthesizer exits after the final block.
        drop(tts_tx);

        let mut barge_in: Option<String> = None;
        let mut barge_done = !barge_in_enabled;
        let mut tts_done = 0;
        let mut barge_in_samples = Vec::new();
        let mut turn_err = None;

        // Re-arm the stop watch for the TTS/barge-in tail of the turn.
        let mut stop = self.stop.clone();
        stop.mark_changed();

        loop {
            let res = tokio::select! {
                res = set.join_next() => res,
                _ = stop.changed() => {
                    self.stop_tts.store(true, Ordering::Relaxed);
                    continue;
                }
            };
            let Some(res) = res else {
                break;
            };
            match res {
                Ok(Ok(TurnTaskResult::TtsDone)) => {
                    tts_done += 1;
                    if tts_done == 2 && barge_done {
                        break;
                    }
                }
                Ok(Ok(TurnTaskResult::BargeIn(Some(text), samples))) => {
                    barge_in = Some(text);
                    barge_in_samples = samples;
                    break;
                }
                Ok(Ok(TurnTaskResult::BargeIn(None, samples))) => {
                    barge_done = true;
                    barge_in_samples = samples;
                    if tts_done == 2 {
                        break;
                    }
                }
                Ok(Err(e)) => {
                    turn_err = Some(AgentError::Audio(e));
                    break;
                }
                Err(e) => {
                    turn_err = Some(AgentError::Audio(AudioError::Play(format!(
                        "turn task panicked: {e}"
                    ))));
                    break;
                }
            }
        }

        // Abort any remaining tasks. The barge-in task may have already set
        // stop_tts to stop playback; set it again defensively.
        self.stop_tts.store(true, Ordering::Relaxed);
        drop(set);

        if !barge_in_samples.is_empty() {
            self.last_captured_samples = barge_in_samples;
        }

        if !no_tts_this_turn {
            self.audio_callback(AudioCallback::PlaybackFinished);
            Self::record_latency_shared(
                &self.latency,
                record_latency,
                LatencyCheckpoint::PlaybackFinished,
            )
            .await;
            if record_latency {
                let budget = self.latency.lock().await;
                let report = budget.report();
                tracing::info!(latency = %report, "turn latency");
            }
        }

        if let Some(e) = turn_err {
            return Err(e);
        }

        Ok((result, barge_in))
    }

    /// If `result` carries a voice-confirmable approval request, resolve it
    /// according to its approval layer. `VoiceConfirmed` requests are read back
    /// and require a closed-vocabulary yes/no that also passes speaker
    /// verification. `AmbientImmediate` requests are verified against the
    /// already-captured wake-word and then executed or downgraded to a screen
    /// approval. Screen-required or unverified requests fall back to the
    /// surface/transport layer so the compact panel can render them.
    async fn resolve_voice_approval(
        &mut self,
        result: &mut TurnResult,
        input_path: InputPath,
    ) -> Result<(), AudioError> {
        if self.is_stopped() {
            return Err(AudioError::UserCancelled);
        }

        let Some(request) = result.approval_request.as_ref() else {
            return Ok(());
        };

        let layer = crate::approval_layers::classify_set(&request.changes, input_path);
        if !layer.requires_voice_confirmation() && !layer.is_ambient_immediate() {
            // ScreenRequired falls through to the surface; Immediate needs no
            // further approval; VoiceConfirmed and AmbientImmediate are handled
            // below.
            return Ok(());
        }

        if layer.is_ambient_immediate() {
            // The wake-word was already captured before this turn. Verify the
            // speaker (if configured) and either execute immediately or leave
            // the approval on the surface for manual confirmation.
            let verified = if self.last_audio.ambient.verify_speaker {
                self.verify_last_captured_speaker()
            } else {
                Some(true)
            };
            match verified {
                Some(true) => {
                    let approved = self
                        .session
                        .resolve_approval(&request.id, true, None)
                        .await
                        .map_err(|e| AudioError::Other(e.to_string()))?;
                    result.changes = approved.changes;
                    result.schedule_dirty |= approved.schedule_dirty;
                    result.approval_request = None;
                    result.presentation = approved.presentation.clone().or_else(|| {
                        Some(crate::presentation::Presentation::Text {
                            text: "承認しました。".into(),
                        })
                    });
                    result.text = match &result.presentation {
                        Some(p) => format!("承認しました。{}", p.voice_template()),
                        None => "承認しました。".to_string(),
                    };
                    self.speak_text(&result.text).await?;
                }
                Some(false) | None => {
                    result.text =
                        "話者確認が取れなかったため、画面で承認をお待ちします。".to_string();
                    result.presentation = Some(crate::presentation::Presentation::ChangeProposal(
                        result.approval_request.clone().unwrap(),
                    ));
                    self.speak_text(&result.text).await?;
                }
            }
            return Ok(());
        }

        // VoiceConfirmed: read back and capture yes/no.
        let readback =
            crate::presentation::Presentation::ChangeProposal(request.clone()).voice_template();
        match self.voice_confirm_loop(&readback).await? {
            Some(confirmed) => {
                let approved = self
                    .session
                    .resolve_approval(&request.id, confirmed, None)
                    .await
                    .map_err(|e| AudioError::Other(e.to_string()))?;
                if approved.approved {
                    result.changes = approved.changes;
                    result.schedule_dirty |= approved.schedule_dirty;
                    result.approval_request = None;
                    result.presentation = approved.presentation.clone().or_else(|| {
                        Some(crate::presentation::Presentation::Text {
                            text: "承認しました。".into(),
                        })
                    });
                    result.text = match &result.presentation {
                        Some(p) => format!("承認しました。{}", p.voice_template()),
                        None => "承認しました。".to_string(),
                    };
                    self.speak_text(&result.text).await?;
                } else {
                    result.text = "キャンセルしました。".to_string();
                    result.changes = Vec::new();
                    result.schedule_dirty |= approved.schedule_dirty;
                    result.approval_request = None;
                    result.presentation = Some(crate::presentation::Presentation::Text {
                        text: result.text.clone(),
                    });
                    self.speak_text(&result.text).await?;
                }
            }
            None => {
                // Fall back to screen; keep the approval request and surface it.
                result.text = format!("{}。画面で承認をお待ちします。", readback);
                result.presentation = Some(crate::presentation::Presentation::ChangeProposal(
                    result.approval_request.clone().unwrap(),
                ));
            }
        }

        Ok(())
    }

    /// Verify the last captured utterance against any enrolled speaker.
    /// Returns `Some(true)` when the best match passes the configured
    /// threshold and the sample is long enough, `Some(false)` when it does not,
    /// and `None` when no verifier or no enrolled speakers are configured.
    fn verify_last_captured_speaker(&self) -> Option<bool> {
        let verifier = self.speaker_verifier.as_ref()?;
        let speaker = self.last_audio.speaker.as_ref()?;
        let samples = &self.last_captured_samples;
        if samples.is_empty() {
            return Some(false);
        }
        let seconds = samples.len() as f32 / SHERPA_SAMPLE_RATE as f32;
        if seconds < MIN_SPEAKER_AUDIO_SECONDS {
            return Some(false);
        }
        match verifier.search(samples) {
            Ok(Some(m)) => Some(m.score >= speaker.verify_threshold),
            Ok(None) => Some(false),
            Err(_) => Some(false),
        }
    }

    /// Speak a prompt, then capture and classify yes/no answers up to
    /// `MAX_VOICE_CONFIRM_ATTEMPTS`. Returns `Some(true)` for an affirmative
    /// answer, `Some(false)` for a negative one, and `None` when the answer is
    /// ambiguous, silent, or fails speaker verification. In the `None` case the
    /// caller must fall back to the screen and keep the approval request pending.
    async fn voice_confirm_loop(&mut self, prompt: &str) -> Result<Option<bool>, AudioError> {
        const AFFIRMATIVE: &[&str] = &["はい", "うん", "yes", "ok", "おう"];
        const NEGATIVE: &[&str] = &["いいえ", "いや", "no", "やだ", "キャンセル"];
        const MAX_ATTEMPTS: usize = 3;

        for attempt in 0..MAX_ATTEMPTS {
            if self.is_stopped() {
                return Ok(None);
            }

            self.speak_text(prompt).await?;

            if self.is_stopped() {
                return Ok(None);
            }

            let (_stop_tx, stop_rx) = tokio::sync::mpsc::channel(1);
            let Some(text) = self.capture_utterance(stop_rx).await? else {
                continue;
            };

            let normalized = crate::normalize_voice_answer(&text);
            let is_affirmative = AFFIRMATIVE.iter().any(|w| normalized == *w);
            let is_negative = NEGATIVE.iter().any(|w| normalized == *w);
            let mut answer = match (is_affirmative, is_negative) {
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            };

            // If the closed vocabulary did not fire, ask the LLM to interpret the
            // natural/extended answer. This keeps the fast path for crisp yes/no
            // while allowing "はい、お願いします" to pass and "はい、けど" to be rejected.
            if answer.is_none() {
                match tokio::time::timeout(
                    Duration::from_secs(10),
                    self.session.classify_voice_confirmation(&text, prompt),
                )
                .await
                {
                    Ok(Ok(classification)) => answer = classification,
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "voice confirmation LLM classification failed");
                    }
                    Err(_) => {
                        tracing::warn!("voice confirmation LLM classification timed out");
                    }
                }
            }

            // Both yes and no must pass speaker verification to prevent a third
            // party from spoofing an approval or a cancellation.
            if let Some(answer) = answer {
                match self.verify_last_captured_speaker() {
                    Some(true) => return Ok(Some(answer)),
                    Some(false) | None => {
                        tracing::info!("voice confirmation failed speaker verification");
                    }
                }
            }

            if attempt < MAX_ATTEMPTS - 1 {
                self.speak_text("もう一度お答えください。はい、または、いいえ、でお答えください。")
                    .await?;
            }
        }

        // Fall back to the screen on silence, repeated unrecognized input, or
        // repeated verification failures.
        self.speak_text("確認が取れませんでした。画面で承認をお待ちします。")
            .await?;
        Ok(None)
    }

    /// Synthesize and play a single text block for voice interaction.
    async fn speak_text(&mut self, text: &str) -> Result<(), AudioError> {
        if self.is_stopped() {
            return Ok(());
        }
        if self.last_audio.tts.mute {
            return Ok(());
        }

        let mut stop = self.stop.clone();
        stop.mark_changed();

        self.audio_callback(AudioCallback::Speaking);
        let stream = tokio::select! {
            stream = synthesize_stream_with_timeout(
                self.tts.as_ref(),
                text,
                &self.tts_voice_id,
                &self.last_audio.tts.language,
                self.tts_speed,
                Duration::from_secs(120),
            ) => stream?,
            _ = stop.changed() => return Err(AudioError::UserCancelled),
        };

        let mut stop = self.stop.clone();
        stop.mark_changed();
        tokio::select! {
            result = play_stream_with_timeout(
                stream,
                self.tts_format,
                Arc::clone(&self.stop_tts),
                Duration::from_secs(120),
            ) => {
                result?;
            }
            _ = stop.changed() => {
                self.stop_tts.store(true, Ordering::Relaxed);
                return Err(AudioError::UserCancelled);
            }
        }

        self.audio_callback(AudioCallback::PlaybackFinished);
        Ok(())
    }

    fn is_stopped(&self) -> bool {
        *self.stop.borrow()
    }

    async fn reconfigure_if_needed(&mut self) -> Result<(), AudioError> {
        let current = {
            let config = self.session.config.read()?;
            config.audio.clone()
        };
        if current == self.last_audio {
            return Ok(());
        }
        let (stt, tts, voice_id, speed, tts_format) = Self::build_audio(&current).await?;
        let speaker_verifier = Self::build_speaker_verifier(current.speaker.as_ref()).await?;

        // Rebuild the cached VAD endpoint if the energy threshold changed, so
        // runtime config edits take effect on the next utterance.
        if current.vad.energy_threshold != self.last_audio.vad.energy_threshold {
            let vad_config = takusu_audio::VadEndpointConfig {
                energy_threshold: current.vad.energy_threshold,
                ..Default::default()
            };
            self.endpoint =
                Some(takusu_audio::default_endpoint_async_with_config(vad_config).await);
        }

        // Drop the cached ambient endpoint so it is rebuilt with the latest
        // max utterance length / energy threshold on the next capture.
        if current.ambient.max_utterance_seconds != self.last_audio.ambient.max_utterance_seconds
            || current.vad.energy_threshold != self.last_audio.vad.energy_threshold
        {
            self.ambient_endpoint = None;
        }

        self.stt = stt;
        self.tts = tts;
        self.tts_voice_id = voice_id;
        self.tts_speed = speed;
        self.tts_format = tts_format;
        self.speaker_verifier = speaker_verifier;
        self.last_audio = current;
        Ok(())
    }

    async fn build_audio(
        audio: &AudioConfig,
    ) -> Result<
        (
            Arc<dyn StreamingSpeechToText>,
            Arc<dyn TextToSpeech>,
            String,
            Option<f32>,
            StreamedAudioFormat,
        ),
        AudioError,
    > {
        let stt_config = audio.stt.clone();
        let stt = tokio::task::spawn_blocking(move || build_stt(&stt_config))
            .await
            .map_err(|e| AudioError::Transcribe(format!("stt build task failed: {e}")))??;
        let supported = tokio::task::spawn_blocking(takusu_audio::play::default_output_config)
            .await
            .map_err(|e| AudioError::Play(format!("output config task failed: {e}")))??;
        let output_rate = supported.sample_rate();
        // The TTS backend may clamp or override the requested sample rate, so
        // build_tts returns the rate that will actually be produced.
        let tts_api_sample_rate = if audio.tts.sample_rate == 0 {
            output_rate
        } else {
            audio.tts.sample_rate
        };
        let (tts, voice_id, speed, tts_sample_rate) = build_tts(&audio.tts, tts_api_sample_rate)?;
        let tts_format = StreamedAudioFormat {
            sample_rate: tts_sample_rate,
            channels: 1,
            pcm_format: PcmFormat::I16,
        };
        Ok((stt, tts, voice_id, speed, tts_format))
    }

    async fn record_latency_shared(
        latency: &Arc<tokio::sync::Mutex<LatencyBudget>>,
        record: bool,
        checkpoint: LatencyCheckpoint,
    ) {
        if !record {
            return;
        }
        let mut budget = latency.lock().await;
        budget.record(checkpoint);
    }

    pub(crate) async fn build_speaker_verifier(
        speaker: Option<&SpeakerConfig>,
    ) -> Result<Option<Arc<SpeakerVerifier>>, AudioError> {
        let Some(config) = speaker else {
            return Ok(None);
        };
        let config = config.clone();

        let model_id = if config.model_id.is_empty() {
            DEFAULT_SPEAKER_MODEL_ID.to_string()
        } else {
            config.model_id.clone()
        };

        let model_id_for_ensure = model_id.clone();
        let config_for_cache = config.clone();
        let (model_path, voice_dir) = tokio::task::spawn_blocking(move || {
            let cache = ModelCache::default_dir().map_err(|e| AudioError::Other(e.to_string()))?;
            let model_dir = cache.ensure(&model_id_for_ensure).map_err(|e| {
                AudioError::Other(format!(
                    "failed to download speaker model {model_id_for_ensure}: {e}"
                ))
            })?;
            let model_path = model_dir.join("model.onnx");
            let voice_dir = config_for_cache
                .voice_dir
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(default_voice_dir);
            Ok::<_, AudioError>((model_path, voice_dir))
        })
        .await
        .map_err(|e| AudioError::Other(format!("speaker cache task failed: {e}")))??;

        let mut speaker_config = config;
        speaker_config.model_id = model_id;

        let verifier = tokio::task::spawn_blocking(move || {
            SpeakerVerifier::new(speaker_config, &model_path, Some(voice_dir))
                .map_err(|e| AudioError::Other(format!("failed to create speaker verifier: {e}")))
        })
        .await
        .map_err(|e| AudioError::Other(format!("speaker verifier task failed: {e}")))??;

        Ok(Some(Arc::new(verifier)))
    }

    // Speaker management surface.
    //
    // These methods are exposed as a public API for surfaces that need explicit
    // speaker enrollment / verification (CLI, mobile settings, tests, etc.).
    // Automatic verification during the voice session capture loop is not wired
    // yet and is planned for a follow-up work item.

    /// Enroll the user from a 16 kHz mono f32 sample.
    pub fn enroll_speaker(&self, name: &str, samples: &[f32]) -> Result<(), AudioError> {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.enroll(name, samples).map_err(Into::into),
            None => Err(AudioError::Other(
                "speaker verification is not configured".to_string(),
            )),
        }
    }

    /// Enroll the user from multiple 16 kHz mono f32 samples.
    pub fn enroll_speaker_list(
        &self,
        name: &str,
        samples_list: &[&[f32]],
    ) -> Result<(), AudioError> {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.enroll_list(name, samples_list).map_err(Into::into),
            None => Err(AudioError::Other(
                "speaker verification is not configured".to_string(),
            )),
        }
    }

    /// Verify a 16 kHz mono f32 sample against the enrolled speaker.
    pub fn verify_speaker(
        &self,
        name: &str,
        samples: &[f32],
    ) -> Result<VerificationResult, AudioError> {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.verify(name, samples).map_err(Into::into),
            None => Err(AudioError::Other(
                "speaker verification is not configured".to_string(),
            )),
        }
    }

    /// Search the enrolled speakers for the best match.
    pub fn search_speaker(
        &self,
        samples: &[f32],
    ) -> Result<Option<SpeakerEmbeddingMatch>, AudioError> {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.search(samples).map_err(Into::into),
            None => Err(AudioError::Other(
                "speaker verification is not configured".to_string(),
            )),
        }
    }

    /// Delete an enrolled speaker.
    pub fn delete_speaker(&self, name: &str) -> Result<(), AudioError> {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.remove(name).map_err(Into::into),
            None => Err(AudioError::Other(
                "speaker verification is not configured".to_string(),
            )),
        }
    }

    /// List enrolled speaker names.
    pub fn list_speakers(&self) -> Vec<String> {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.list(),
            None => Vec::new(),
        }
    }

    /// Check whether a speaker is enrolled.
    pub fn is_speaker_enrolled(&self, name: &str) -> bool {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.is_enrolled(name),
            None => false,
        }
    }

    /// Keep the microphone open during TTS, feed the TTS reference through the
    /// AEC, and return any interruption text.
    ///
    /// The TTS reference is delayed by `audio.barge_in.reference_delay_ms` so
    /// that the AEC sees the echo at the same time as the microphone signal,
    /// accounting for playback buffering and device latency.
    async fn barge_in_loop(
        mut reference_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<f32>>,
        stop_tts: Arc<AtomicBool>,
        mut stop: tokio::sync::watch::Receiver<bool>,
        stt: Arc<dyn StreamingSpeechToText>,
        audio: AudioConfig,
    ) -> Result<(Option<String>, Vec<f32>), AudioError> {
        // A cloned watch receiver is initialized with the current value, so
        // changed() would miss a stop signal that was already sent. Mark the
        // current value as unseen so changed() fires immediately if stop is true.
        stop.mark_changed();

        const FRAME_MS: u64 = 10;
        const FRAME_SIZE: usize = (SHERPA_SAMPLE_RATE as usize * FRAME_MS as usize) / 1000;

        let (recorder, mut chunk_rx) = StreamingRecorder::start(RecordConfig {
            max_duration: Duration::from_secs(60),
            normalize_audio: false,
            ..Default::default()
        })
        .map_err(|e| AudioError::Record(e.to_string()))?;

        let language = audio.stt.language.clone();
        let mut asr_stream = stt
            .start_stream(&language)
            .await
            .map_err(|e| AudioError::Transcribe(e.to_string()))?;

        let mut aec: Box<dyn Aec> = if audio.barge_in.use_aec {
            Box::new(NlmsAec::new(audio.aec))
        } else {
            Box::new(NoOpAec)
        };

        // When the AEC is not active, fall back to tap-to-stop if configured:
        // detect a loud utterance and stop TTS playback, but do not feed the
        // microphone into ASR because the assistant's own voice is still in the
        // signal.
        let use_aec = audio.barge_in.use_aec;
        let tap_to_stop = !use_aec && audio.barge_in.tap_to_stop;
        let vad_config = takusu_audio::VadEndpointConfig {
            energy_threshold: audio.vad.energy_threshold,
            min_speech: Duration::from_millis(50),
            max_silence: Duration::from_millis(200),
            ..Default::default()
        };
        let endpoint: Box<dyn takusu_audio::Endpoint> = if use_aec {
            Box::new(takusu_audio::default_endpoint_async_with_config(vad_config).await)
        } else if tap_to_stop {
            Box::new(takusu_audio::VadEndpoint::new(
                takusu_audio::EnergyVad::new(audio.vad.energy_threshold),
                SHERPA_SAMPLE_RATE,
                vad_config,
            ))
        } else {
            // Neither AEC nor tap-to-stop: keep the same shape but use a quiet
            // threshold so barge-in is effectively disabled.
            Box::new(takusu_audio::VadEndpoint::new(
                takusu_audio::EnergyVad::new(audio.vad.energy_threshold),
                SHERPA_SAMPLE_RATE,
                vad_config,
            ))
        };

        let warm_up = Duration::from_millis(audio.barge_in.warm_up_ms);
        let mut detector = BargeInDetector::new(endpoint, warm_up);
        detector.start();

        // How far ahead the reference signal is relative to the actual echo in
        // the microphone. We read the reference buffer `ref_delay_samples`
        // behind the latest available sample to align with the echo.
        let ref_delay_samples =
            (audio.barge_in.reference_delay_ms as usize * SHERPA_SAMPLE_RATE as usize) / 1000;

        let mut mic_buf = Vec::new();
        let mut ref_buf = Vec::new();
        let mut residual = Vec::new();
        let mut ref_consumed = 0usize;
        let mut ref_initialized = false;
        let mut capturing = false;
        let mut reference_closed = false;
        let mut barge_in_samples = Vec::new();

        loop {
            tokio::select! {
                chunk = chunk_rx.recv() => match chunk {
                    Some(c) => mic_buf.extend(c),
                    None => break,
                },
                ref_chunk = reference_rx.recv() => match ref_chunk {
                    Some(r) => ref_buf.extend(r),
                    None => {
                        // TTS finished. If the user was not speaking, stop
                        // listening; otherwise keep recording until SpeechEnd.
                        reference_closed = true;
                        if !capturing {
                            break;
                        }
                    }
                },
                _ = stop.changed() => break,
            }

            while mic_buf.len() >= FRAME_SIZE {
                let capture = mic_buf[..FRAME_SIZE].to_vec();
                mic_buf.drain(..FRAME_SIZE);

                if !ref_initialized {
                    ref_initialized = true;
                    // play_stream_with_reference may have already queued
                    // reference samples before the microphone opened. Start
                    // consuming from the end of the buffer so the first
                    // microphone frame aligns with the most recent reference
                    // frame, accounting for the playback-to-mic delay.
                    let ref_lead_samples = ref_buf.len().saturating_sub(ref_delay_samples);
                    ref_consumed = ref_lead_samples / FRAME_SIZE;
                }

                let ref_consumed_samples = ref_consumed * FRAME_SIZE;
                let ref_start = ref_consumed_samples.saturating_sub(ref_delay_samples);
                let mut reference = vec![0.0f32; FRAME_SIZE];
                if ref_start < ref_buf.len() {
                    let available = ref_buf.len() - ref_start;
                    let to_copy = available.min(FRAME_SIZE);
                    reference[..to_copy].copy_from_slice(&ref_buf[ref_start..ref_start + to_copy]);
                }
                ref_consumed += 1;

                aec.process(&reference, &capture, &mut residual);

                if let Some(event) = detector.push(&residual) {
                    match event {
                        VadEvent::SpeechStart if !capturing => {
                            capturing = true;
                            stop_tts.store(true, Ordering::Relaxed);
                            if tap_to_stop {
                                // Tap-to-stop: the user touched the
                                // microphone while the assistant was speaking.
                                // Stop playback and do not try to transcribe.
                                recorder.stop();
                                break;
                            }
                        }
                        VadEvent::SpeechEnd if capturing => {
                            recorder.stop();
                            break;
                        }
                        _ => {}
                    }
                }

                if capturing && use_aec && !tap_to_stop {
                    asr_stream.accept_waveform(&normalize(&residual, 0.1));
                    barge_in_samples.extend_from_slice(&residual);
                }

                // When TTS has ended and we have processed the last samples
                // that could still contain an echo, stop listening. Use the
                // count after consuming this frame to avoid one extra loop.
                let ref_consumed_after = ref_consumed * FRAME_SIZE;
                if reference_closed
                    && ref_consumed_after >= ref_buf.len() + ref_delay_samples
                    && !capturing
                {
                    break;
                }
            }
        }

        recorder.stop();
        tokio::task::spawn_blocking(move || recorder.join())
            .await
            .map_err(|e| AudioError::Record(format!("barge-in recorder join task failed: {e}")))?
            .map_err(|e| AudioError::Record(format!("barge-in recorder thread panicked: {e}")))?;

        if tap_to_stop || barge_in_samples.is_empty() {
            return Ok((None, Vec::new()));
        }

        let text = asr_stream
            .finish()
            .await
            .map_err(|e| AudioError::Transcribe(e.to_string()))?;
        Ok((
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            },
            barge_in_samples,
        ))
    }
}

#[async_trait::async_trait]
impl VoiceSessionIo for AudioAdapter {
    async fn capture(&mut self, origin: InputOrigin) -> Result<Option<String>, VoiceSessionError> {
        if *self.stop.borrow() {
            return Err(VoiceSessionError::UserCancelled);
        }

        let result = match origin {
            InputOrigin::Ambient => self.capture_ambient().await,
            _ => {
                // A never-signalled stop channel: the voice session uses the
                // watch receiver for out-of-band stop requests instead.
                let (_stop_tx, stop_rx) = tokio::sync::mpsc::channel(1);
                self.capture_utterance(stop_rx).await
            }
        };

        result.map_err(|e| match e {
            AudioError::UserCancelled => VoiceSessionError::UserCancelled,
            _ => VoiceSessionError::Capture(e.to_string()),
        })
    }

    async fn process(
        &mut self,
        text: &str,
        origin: InputOrigin,
        input_path: InputPath,
    ) -> Result<ProcessedTurn, VoiceSessionError> {
        // Do not start a new turn while the previous turn is still waiting for
        // approval. Without this guard, the next utterance would grab the turn
        // lock and overwrite the pending approval before the surface could
        // resolve it. Wait instead of failing so the session can continue once
        // the user resolves the approval.
        while self.session.pending_approval().is_some() {
            if *self.stop.borrow() {
                return Err(VoiceSessionError::UserCancelled);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        self.reconfigure_if_needed()
            .await
            .map_err(AgentError::Audio)?;

        if *self.stop.borrow() {
            return Err(VoiceSessionError::UserCancelled);
        }

        let (mut result, barge_in) = match self
            .run_agent_turn(text, origin.auto_speaks(), input_path)
            .await
        {
            Ok(r) => r,
            Err(AgentError::Audio(AudioError::UserCancelled)) => {
                return Err(VoiceSessionError::UserCancelled);
            }
            Err(e) => return Err(VoiceSessionError::Agent(e.to_string())),
        };
        self.resolve_voice_approval(&mut result, input_path)
            .await
            .map_err(|e: AudioError| match e {
                AudioError::UserCancelled => VoiceSessionError::UserCancelled,
                _ => VoiceSessionError::Agent(AgentError::Audio(e).to_string()),
            })?;

        Ok(ProcessedTurn { result, barge_in })
    }
}

fn default_voice_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("takusu").join("voiceprint");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("takusu")
            .join("voiceprint");
    }
    PathBuf::from("takusu").join("voiceprint")
}

pub(crate) fn build_stt(config: &SttConfig) -> Result<Arc<dyn StreamingSpeechToText>, AudioError> {
    let runtime_config = takusu_audio::SttRuntimeConfig {
        backend: config.backend,
        model: config.model,
        model_dir: if config.model_dir.is_empty() {
            None
        } else {
            Some(PathBuf::from(&config.model_dir))
        },
        language: config.language.clone(),
        use_itn: config.use_itn,
        num_threads: config.num_threads,
        provider: config.provider,
        sample_rate: config.sample_rate,
    };
    runtime_config
        .build_streaming()
        .map_err(|e| AudioError::Transcribe(e.to_string()))
}

type TtsBuildResult = Result<(Arc<dyn TextToSpeech>, String, Option<f32>, u32), AudioError>;

fn build_tts(config: &TtsConfig, api_sample_rate: u32) -> TtsBuildResult {
    match config.backend {
        TtsBackend::Cartesia => {
            let api_key = if config.api_key.is_empty() {
                std::env::var(&config.api_key_env).unwrap_or_default()
            } else {
                config.api_key.clone()
            };
            if api_key.is_empty() && !config.mute {
                return Err(AudioError::Tts("missing Cartesia API key".to_string()));
            }
            let mut tts_config = CartesiaSonicConfig::new(api_key);
            tts_config.voice_id = config.voice_id.clone();
            if !config.model.is_empty() {
                tts_config.model_id = config.model.clone();
            }
            tts_config.language = Some(config.language.clone());
            tts_config.output_format.sample_rate = api_sample_rate;
            tts_config.mute = config.mute;
            let voice_id = config.voice_id.clone();
            let speed = config.speed;
            Ok((
                Arc::new(CartesiaSonic::new(tts_config)),
                voice_id,
                speed,
                api_sample_rate,
            ))
        }
        TtsBackend::Fish => {
            let api_key = if config.api_key.is_empty() {
                std::env::var(&config.api_key_env).unwrap_or_default()
            } else {
                config.api_key.clone()
            };
            if api_key.is_empty() && !config.mute {
                return Err(AudioError::Tts("missing Fish Audio API key".to_string()));
            }
            let mut tts_config = FishAudioConfig::new(api_key);
            tts_config.voice_id = config.voice_id.clone();
            if !config.model.is_empty() {
                tts_config.model = config.model.clone();
            }
            tts_config.sample_rate = api_sample_rate;
            tts_config.mute = config.mute;
            let voice_id = config.voice_id.clone();
            let speed = config.speed;
            Ok((
                Arc::new(FishAudio::new(tts_config)),
                voice_id,
                speed,
                api_sample_rate,
            ))
        }
        // Android TTS is handled by the native mobile module, not by the
        // generic tokio-based AudioAdapter used on desktop.
        TtsBackend::Android => Err(AudioError::UnsupportedBackend("android".to_string())),
    }
}

async fn synthesize_stream_with_timeout(
    tts: &dyn TextToSpeech,
    text: &str,
    voice_id: &str,
    language: &str,
    speed: Option<f32>,
    timeout: Duration,
) -> Result<TtsStream, AudioError> {
    // Lindera dictionary initialization can block on first use, so run the
    // normalization off the async runtime worker threads.
    let text = text.to_string();
    let language = language.to_string();
    let text =
        tokio::task::spawn_blocking(move || normalize_for_tts(&text, &language).into_owned())
            .await
            .map_err(|e| AudioError::Play(format!("tts normalization panicked: {e}")))?;
    let request = TtsRequest {
        text,
        voice: if voice_id.is_empty() {
            None
        } else {
            Some(voice_id.to_string())
        },
        reference_audio_path: None,
        options: TtsOptions {
            response_format: Some("pcm_s16le".to_string()),
            speed,
        },
    };

    tokio::time::timeout(timeout, tts.synthesize_stream(&request))
        .await
        .map_err(|_| AudioError::Timeout)?
        .map_err(|e| AudioError::Tts(e.to_string()))
}

async fn play_stream_with_timeout(
    stream: TtsStream,
    format: StreamedAudioFormat,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<(), AudioError> {
    tokio::time::timeout(timeout, play_stream(stream, format, cancel))
        .await
        .map_err(|_| AudioError::Timeout)?
        .map_err(|e| AudioError::Play(e.to_string()))
}

/// Decode a TTS PCM chunk to 16 kHz mono f32 reference samples for the AEC.
fn decode_tts_reference(
    bytes: &Bytes,
    format: &StreamedAudioFormat,
    pending: &mut BytesMut,
    decoded: &mut Vec<f32>,
) -> Result<Vec<f32>, AudioError> {
    pending.extend_from_slice(bytes);
    decode_pcm_chunk(pending, format, decoded).map_err(|e| AudioError::Play(e.to_string()))?;
    if decoded.is_empty() {
        return Ok(Vec::new());
    }
    let mono = if format.channels > 1 {
        mix_to_mono(decoded, format.channels as usize)
    } else {
        decoded.to_vec()
    };
    let reference = if format.sample_rate != SHERPA_SAMPLE_RATE {
        resample(&mono, format.sample_rate, SHERPA_SAMPLE_RATE)
    } else {
        mono
    };
    Ok(reference)
}

/// Play a TTS PCM stream while also sending decoded 16 kHz mono reference
/// samples to `reference_tx` so a parallel barge-in listener can AEC out the
/// assistant's own voice.
async fn play_stream_with_reference(
    mut stream: TtsStream,
    format: StreamedAudioFormat,
    reference_tx: tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<(), AudioError> {
    tokio::time::timeout(timeout, async {
        let (play_tx, play_rx) = tokio::sync::mpsc::channel::<Result<Bytes, TtsError>>(4);
        let playback_stream: TtsStream =
            Box::pin(tokio_stream::wrappers::ReceiverStream::new(play_rx));
        let mut pending = BytesMut::new();
        let mut decoded = Vec::new();

        let cancel_for_decoder = Arc::clone(&cancel);
        let decoder = tokio::spawn(async move {
            loop {
                if cancel_for_decoder.load(Ordering::Acquire) {
                    break;
                }
                let chunk = tokio::select! {
                    chunk = stream.next() => chunk,
                    _ = async {
                        while !cancel_for_decoder.load(Ordering::Acquire) {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    } => break,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                match chunk {
                    Ok(bytes) => {
                        if let Some(reference) =
                            decode_tts_reference(&bytes, &format, &mut pending, &mut decoded)
                                .ok()
                                .filter(|r| !r.is_empty())
                        {
                            let _ = reference_tx.send(reference);
                        }
                        if play_tx.send(Ok(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = play_tx.send(Err(e)).await;
                        break;
                    }
                }
            }
        });

        play_stream(playback_stream, format, Arc::clone(&cancel))
            .await
            .map_err(|e| AudioError::Play(e.to_string()))?;
        // Stop the decoder if playback finishes early (e.g. barge-in).
        decoder.abort();
        Ok::<(), AudioError>(())
    })
    .await
    .map_err(|_| AudioError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_config_defaults_to_sherpa() {
        use takusu_audio::{ExecutionProvider, SherpaOnnxModel, SttBackend};

        let config = SttConfig::default();
        assert_eq!(config.backend, SttBackend::Sherpa);
        assert_eq!(config.language, "ja");
        assert_eq!(config.model, SherpaOnnxModel::SenseVoice);
        assert!(config.use_itn);
        assert_eq!(config.num_threads, 2);
        assert_eq!(config.provider, ExecutionProvider::Cpu);
        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn tts_config_defaults_to_cartesia() {
        let config = TtsConfig::default();
        assert_eq!(config.backend, TtsBackend::Cartesia);
        assert_eq!(config.api_key_env, "CARTESIA_API_KEY");
        assert_eq!(config.sample_rate, 44100);
        assert!(!config.mute);
    }

    #[test]
    fn stt_config_rejects_unknown_backend_at_parse_time() {
        // With enum-typed backend, unknown values are rejected by serde
        // at config load time rather than at build time.
        let toml = r#"
[stt]
backend = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn stt_config_rejects_unknown_model_at_parse_time() {
        let toml = r#"
[stt]
backend = "sherpa"
model = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn stt_config_rejects_unknown_provider_at_parse_time() {
        let toml = r#"
[stt]
backend = "sherpa"
provider = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn tts_config_rejects_unknown_backend_at_parse_time() {
        let toml = r#"
[tts]
backend = "unknown"
"#;
        let result: Result<AudioConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn build_tts_rejects_android_backend() {
        let config = TtsConfig {
            backend: TtsBackend::Android,
            ..TtsConfig::default()
        };
        let result = build_tts(&config, 44100);
        match result {
            Err(e) => assert!(e.to_string().contains("android")),
            Ok(_) => panic!("expected android backend to be rejected"),
        }
    }

    #[test]
    fn build_tts_allows_missing_api_key_when_muted() {
        let config = TtsConfig {
            api_key: String::new(),
            api_key_env: "NONEXISTENT_API_KEY".to_string(),
            mute: true,
            ..TtsConfig::default()
        };
        assert!(build_tts(&config, 44100).is_ok());
    }

    #[test]
    fn build_tts_rejects_missing_api_key_when_not_muted() {
        let config = TtsConfig {
            api_key: String::new(),
            api_key_env: "NONEXISTENT_API_KEY".to_string(),
            ..TtsConfig::default()
        };
        assert!(build_tts(&config, 44100).is_err());
    }

    #[test]
    fn decode_tts_reference_resamples_and_downmixes() {
        let format = StreamedAudioFormat {
            sample_rate: 22050,
            channels: 2,
            pcm_format: PcmFormat::F32,
        };
        let samples: Vec<f32> = vec![0.25, -0.25];
        let mut raw = Vec::new();
        for s in &samples {
            raw.extend_from_slice(&s.to_le_bytes());
        }
        let bytes = bytes::Bytes::from(raw);
        let mut pending = BytesMut::new();
        let mut decoded = Vec::new();

        let reference = decode_tts_reference(&bytes, &format, &mut pending, &mut decoded)
            .expect("decode should succeed");

        assert!(!reference.is_empty());
        assert!(reference.iter().all(|s| s.abs() <= 1.0));
    }
}
