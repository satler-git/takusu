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

use takusu_audio::play::{PcmFormat, PlayError, StreamedAudioFormat, play_stream};
use takusu_audio::{
    CartesiaSonic, CartesiaSonicConfig, DEFAULT_SPEAKER_MODEL_ID, FishAudio, FishAudioConfig,
    MIN_SPEAKER_AUDIO_SECONDS, ModelCache, RecordConfig, SHERPA_SAMPLE_RATE, SpeakerConfig,
    SpeakerEmbeddingMatch, SpeakerVerifier,
    StreamingRecorder, StreamingSpeechToText, TextToSpeech, TtsBackend, TtsOptions, TtsRequest,
    TtsStream, VadEvent, VerificationResult, normalize, normalize_for_tts,
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
    /// Optional speaker embedding verifier for voiceprint enrollment and
    /// verification. Shared so `&self` methods can run verification.
    speaker_verifier: Option<Arc<SpeakerVerifier>>,
    /// The most recently captured 16 kHz mono f32 samples. This is updated by
    /// `capture_utterance` and is used to verify the speaker for voice
    /// confirmations and ambient-immediate commands.
    last_captured_samples: Vec<f32>,
}

impl AudioAdapter {
    /// Create an audio adapter from an existing agent session.
    pub async fn new(session: Arc<AgentSession>) -> Result<Self, AudioError> {
        let audio = {
            let config = session.config.read()?;
            config.audio.clone()
        };
        let (stt, tts, voice_id, speed, tts_format) = Self::build_audio(&audio).await?;
        let endpoint = takusu_audio::default_endpoint_async().await;
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
            speaker_verifier,
            last_captured_samples: Vec::new(),
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
    //
    // TODO(WI-12): wire this to `SurfaceCommand::StopTts` so tray/mobile
    // surfaces can stop the assistant's speech mid-turn.
    pub fn stop_tts_signal(&self) {
        self.stop_tts.store(true, Ordering::Relaxed);
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

            let mut result = self
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
        if text.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }

    /// Run one agent turn from `text` on the given `input_path`, streaming TTS
    /// when `speak` is `true`, and return the turn result. Approval resolution
    /// is the caller's responsibility.
    async fn run_agent_turn(
        &mut self,
        text: &str,
        speak: bool,
        input_path: InputPath,
    ) -> Result<TurnResult, AgentError> {
        // Reset the tap-to-stop flag for this turn.
        self.stop_tts.store(false, Ordering::Relaxed);

        let no_tts_this_turn = !speak || self.last_audio.tts.mute;

        let (tts_tx, tts_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel::<TtsStream>(3);
        let tts = Arc::clone(&self.tts);
        let tts_format = self.tts_format;
        let voice_id = Arc::new(self.tts_voice_id.clone());
        let speed = self.tts_speed;
        let tts_language = Arc::new(self.last_audio.tts.language.clone());
        let stop_tts = Arc::clone(&self.stop_tts);
        let stop_tts_play = Arc::clone(&stop_tts);

        let tts_synth = tokio::spawn(async move {
            if no_tts_this_turn {
                return Result::<(), AudioError>::Ok(());
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
            Ok(())
        });

        let tts_play = tokio::spawn(async move {
            while let Some(stream) = audio_rx.recv().await {
                if stop_tts_play.load(Ordering::Relaxed) {
                    break;
                }
                play_stream_with_timeout(stream, tts_format, Duration::from_secs(120)).await?;
            }
            Ok::<(), AudioError>(())
        });

        // The turn event callback may be called from a spawned task, so move
        // the cloned event sink into the closure instead of borrowing `self`.
        if !no_tts_this_turn {
            self.audio_callback(AudioCallback::Speaking);
        }
        let on_event = self.on_event.clone();
        let result = match self
            .session
            .run_turn_stream_with_input(
                text,
                input_path,
                move |event| Self::emit_with(&on_event, event),
                |block| {
                    if !no_tts_this_turn {
                        let _ = tts_tx.send(block);
                    }
                },
            )
            .await
        {
            Ok(result) => result,
            Err(e) => {
                drop(tts_tx);
                tts_synth.abort();
                tts_play.abort();
                return Err(e);
            }
        };

        // Drop the text sender so the synthesizer exits after the final block.
        drop(tts_tx);
        let (synth_result, play_result) = tokio::join!(tts_synth, tts_play);
        synth_result
            .map_err(|e| AudioError::Play(format!("tts synthesizer task panicked: {e}")))??;
        play_result.map_err(|e| AudioError::Play(format!("tts player task panicked: {e}")))??;

        if !no_tts_this_turn {
            self.audio_callback(AudioCallback::PlaybackFinished);
        }

        Ok(result)
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
            // speaker and either execute immediately or leave the approval on
            // the surface for manual confirmation.
            match self.verify_last_captured_speaker() {
                Some(true) => {
                    let approved = self
                        .session
                        .resolve_approval(&request.id, true, None)
                        .await
                        .map_err(|e| AudioError::Other(e.to_string()))?;
                    if approved.approved {
                        result.text = "承認しました。".to_string();
                        result.changes = approved.changes;
                    }
                    result.schedule_dirty |= approved.schedule_dirty;
                    result.approval_request = None;
                    result.presentation = None;
                }
                Some(false) | None => {
                    result.text = "話者確認が取れなかったため、画面で承認をお待ちします。".to_string();
                    result.presentation = Some(crate::presentation::Presentation::ChangeProposal(
                        result.approval_request.clone().unwrap(),
                    ));
                }
            }
            return Ok(());
        }

        // VoiceConfirmed: read back and capture yes/no.
        let readback = self.build_voice_readback(request);
        match self.voice_confirm_loop(&readback).await? {
            Some(confirmed) => {
                let approved = self
                    .session
                    .resolve_approval(&request.id, confirmed, None)
                    .await
                    .map_err(|e| AudioError::Other(e.to_string()))?;
                if approved.approved {
                    result.text = format!("{}。承認しました。", readback);
                    result.changes = approved.changes;
                } else {
                    result.text = format!("{}。キャンセルしました。", readback);
                    result.changes = Vec::new();
                }
                result.schedule_dirty |= approved.schedule_dirty;
                result.approval_request = None;
                result.presentation = None;
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

    /// Build a short Japanese readback for a voice-confirmation prompt.
    fn build_voice_readback(&self, request: &crate::ApprovalRequest) -> String {
        let mut summary = String::new();
        for change in &request.changes {
            summary.push_str(&change.description);
            if !summary.ends_with('。') {
                summary.push('。');
            }
        }
        if request.why.is_empty() {
            format!("{}よろしいですか", summary)
        } else if request.why.ends_with('。') {
            format!("{}{}よろしいですか", summary, request.why)
        } else {
            format!("{}{}。よろしいですか", summary, request.why)
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
            self.speak_text(prompt).await?;

            let (_stop_tx, stop_rx) = tokio::sync::mpsc::channel(1);
            let Some(text) = self.capture_utterance(stop_rx).await? else {
                continue;
            };

            let trimmed = text.trim().to_lowercase();
            let answer = if AFFIRMATIVE.iter().any(|w| trimmed == *w) {
                Some(true)
            } else if NEGATIVE.iter().any(|w| trimmed == *w) {
                Some(false)
            } else {
                None
            };

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
        if self.last_audio.tts.mute {
            return Ok(());
        }
        self.audio_callback(AudioCallback::Speaking);
        let stream = synthesize_stream_with_timeout(
            self.tts.as_ref(),
            text,
            &self.tts_voice_id,
            &self.last_audio.tts.language,
            self.tts_speed,
            Duration::from_secs(120),
        )
        .await?;
        play_stream_with_timeout(stream, self.tts_format, Duration::from_secs(120)).await?;
        self.audio_callback(AudioCallback::PlaybackFinished);
        Ok(())
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

    async fn build_speaker_verifier(
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
            SpeakerVerifier::new(speaker_config, &model_path, Some(voice_dir)).map_err(|e| {
                AudioError::Other(format!("failed to create speaker verifier: {e}"))
            })
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
    pub fn verify_speaker(&self, name: &str, samples: &[f32]) -> Result<VerificationResult, AudioError> {
        match self.speaker_verifier.as_ref() {
            Some(v) => v.verify(name, samples).map_err(Into::into),
            None => Err(AudioError::Other(
                "speaker verification is not configured".to_string(),
            )),
        }
    }

    /// Search the enrolled speakers for the best match.
    pub fn search_speaker(&self, samples: &[f32]) -> Result<Option<SpeakerEmbeddingMatch>, AudioError> {
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
}

#[async_trait::async_trait]
impl VoiceSessionIo for AudioAdapter {
    async fn capture(&mut self, _origin: InputOrigin) -> Result<Option<String>, VoiceSessionError> {
        // A never-signalled stop channel: the voice session uses the watch
        // receiver for out-of-band stop requests instead.
        let (_stop_tx, stop_rx) = tokio::sync::mpsc::channel(1);
        self.capture_utterance(stop_rx)
            .await
            .map_err(|e| match e {
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
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let mut result = self
            .run_agent_turn(text, origin.auto_speaks(), input_path)
            .await?;
        self.resolve_voice_approval(&mut result, input_path)
            .await
            .map_err(AgentError::Audio)?;
        Ok(ProcessedTurn { result })
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

fn build_stt(config: &SttConfig) -> Result<Arc<dyn StreamingSpeechToText>, AudioError> {
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
    timeout: Duration,
) -> Result<(), AudioError> {
    tokio::time::timeout(timeout, play_stream(stream, format))
        .await
        .map_err(|_| AudioError::Timeout)?
        .map_err(|e| AudioError::Play(e.to_string()))
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
}
