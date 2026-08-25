//! Daemon audio runtime.
//!
//! The `audio-device` feature gates this whole module in `lib.rs`, so
//! conditional compilation is at the module boundary. Inside, all items are
//! compiled only when that feature is enabled.
//!
//! - Voice sessions (WI-12) and ambient listening (WI-21) share the same agent
//!   session runner. Ambient runs with `InputOrigin::Ambient`, a long idle
//!   timeout, and a per-wake log.
//! - When `audio-device` is disabled, this module is not compiled and no local
//!   capture is available.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use takusu_agent::{
    AgentConfig, AgentError, AgentSession, InputOrigin, Presentation, SessionOutcome, SurfaceEvent,
    SurfaceSnapshot, SurfaceStateMachine, TurnEvent, VoiceSessionConfig,
};
use takusu_audio::play::{PcmFormat, StreamedAudioFormat, play_stream};
use takusu_audio::{
    CartesiaSonic, CartesiaSonicConfig, FishAudio, FishAudioConfig, TextToSpeech, TtsBackend,
    TtsOptions, TtsRequest, WakeWordBackend, normalize_for_tts,
};
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::state::{DesktopError, DesktopState};

/// Handle to an in-progress daemon voice session.
#[derive(Clone, Debug)]
pub struct VoiceSessionHandle {
    stop: tokio::sync::watch::Sender<bool>,
}

impl VoiceSessionHandle {
    /// Request that the running voice session stop after the current turn.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// Handle to an in-progress ambient listening session.
#[derive(Clone, Debug)]
pub struct AmbientSessionHandle {
    stop: tokio::sync::watch::Sender<bool>,
}

impl AmbientSessionHandle {
    /// Request that the ambient session stop.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// Persistent log for ambient wake-word evaluation.
///
/// Appends one line per wake event with ISO timestamp, backend, wake word, and
/// the transcript the ASR returned (the wake word itself has been stripped by
/// the pipeline). The log is meant to be reviewed by the user; the final
/// command text is stored without raw audio or pre-gate transcript.
#[derive(Clone, Debug)]
pub struct WakeLog {
    path: PathBuf,
    wake_word: String,
    backend: WakeWordBackend,
}

impl WakeLog {
    /// Create a new logger, ensuring the parent directory exists.
    pub fn new(
        path: impl Into<PathBuf>,
        wake_word: impl Into<String>,
        backend: WakeWordBackend,
    ) -> Result<Self, std::io::Error> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Touch the file so we fail early on bad paths.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        drop(file);
        set_private_file_permissions(&path)?;
        Ok(Self {
            path,
            wake_word: wake_word.into(),
            backend,
        })
    }

    /// Append one wake event. `transcript` is the command after the wake word.
    ///
    /// Runs the file write with `tokio::task::block_in_place` so the turn event
    /// callback does not block a tokio worker thread.
    pub fn append_wake(&self, transcript: &str) -> Result<(), std::io::Error> {
        tokio::task::block_in_place(|| self.append_wake_sync(transcript))
    }

    fn append_wake_sync(&self, transcript: &str) -> Result<(), std::io::Error> {
        let ts = jiff::Timestamp::now();
        // Remove control characters that would break the single-line TSV record.
        let transcript = transcript.replace(|c: char| c.is_control(), " ");
        let line = format!(
            "{}\t{:?}\t{}\t{}",
            ts, self.backend, self.wake_word, transcript
        );
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_file_permissions(path: &std::path::Path) -> Result<(), std::io::Error> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &std::path::Path) -> Result<(), std::io::Error> {
    Ok(())
}

/// Load the agent config, override it with the desktop `Config`, and build the
/// shared `AgentSession`.
pub(crate) fn build_agent_session(
    config: &Config,
    agent_config: &AgentConfig,
) -> Result<Arc<AgentSession>, DesktopError> {
    let client = takusu_client::Client::new(&config.local_url, &config.token);
    let session = takusu_agent::runner::build_session(agent_config, client)
        .map_err(|e| DesktopError::Transport(format!("failed to build agent session: {e}")))?;
    Ok(Arc::new(session))
}

/// Run one continuous voice/ambient session against the local microphone,
/// mirroring every streaming `TurnEvent` into `machine` and forwarding each
/// state snapshot to `on_state_changed`.
///
/// When `wake_log` is `Some`, every final `TurnEvent::AsrText` is logged for
/// later false-fire/miss analysis. Voice sessions pass `None`.
pub async fn voice_loop_with_surface<S>(
    session: Arc<AgentSession>,
    machine: SurfaceStateMachine,
    origin: InputOrigin,
    config: VoiceSessionConfig,
    stop: tokio::sync::watch::Receiver<bool>,
    on_state_changed: S,
    wake_log: Option<WakeLog>,
) -> Result<SessionOutcome, AgentError>
where
    S: FnMut(SurfaceSnapshot) + Send + 'static,
{
    let on_state = Arc::new(std::sync::Mutex::new(on_state_changed));
    let on_state_for_audio = Arc::clone(&on_state);
    let machine_for_turn = machine.clone();
    let machine_for_audio = machine.clone();
    takusu_agent::runner::run_voice_session(
        session,
        origin,
        config,
        stop,
        &machine,
        move |event| {
            if let TurnEvent::AsrText(transcript) = &event
                && let Some(log) = wake_log.as_ref()
                && let Err(error) = log.append_wake(transcript)
            {
                tracing::warn!(error=%error, "failed to append wake log");
            }
            let snapshot = machine_for_turn.apply_turn_event(&event);
            if let Ok(mut guard) = on_state.lock() {
                guard(snapshot);
            }
        },
        move |callback| {
            let snapshot = machine_for_audio.apply_audio_callback(callback);
            if let Ok(mut guard) = on_state_for_audio.lock() {
                guard(snapshot);
            }
        },
    )
    .await
}

/// Common implementation for starting a voice or ambient session.
#[allow(clippy::too_many_arguments)]
fn spawn_voice_like_session<H, StartFn, FinishFn>(
    state: DesktopState,
    session: Arc<AgentSession>,
    origin: InputOrigin,
    voice_config: VoiceSessionConfig,
    wake_log: Option<WakeLog>,
    make_handle: impl FnOnce(tokio::sync::watch::Sender<bool>) -> H,
    on_started: StartFn,
    on_finished: FinishFn,
) -> Result<H, DesktopError>
where
    H: Clone + Send + 'static,
    StartFn: FnOnce(&DesktopState, &H) + Send + 'static,
    FinishFn: FnOnce(&DesktopState) + Send + 'static,
{
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let handle = make_handle(stop_tx);

    let on_state = {
        let state = state.clone();
        move |snapshot: SurfaceSnapshot| {
            state.update_surface(SurfaceEvent::StateChanged(snapshot));
        }
    };

    let task: JoinHandle<Result<SessionOutcome, AgentError>> = tokio::spawn(async move {
        voice_loop_with_surface(
            session,
            SurfaceStateMachine::new(),
            origin,
            voice_config,
            stop_rx,
            on_state,
            wake_log,
        )
        .await
    });

    let state_for_cleanup = state.clone();
    tokio::spawn(async move {
        let label = match origin {
            InputOrigin::Voice => "voice",
            InputOrigin::Ambient => "ambient",
            _ => "session",
        };
        match task.await {
            Ok(Ok(outcome)) => tracing::info!(?outcome, "{label} session ended"),
            Ok(Err(error)) => tracing::error!(error=%error, "{label} session failed"),
            Err(join_error) => tracing::error!(error=%join_error, "{label} session task panicked"),
        }
        on_finished(&state_for_cleanup);
    });

    on_started(&state, &handle);
    Ok(handle)
}

/// Start a continuous voice session against the desktop microphone and the
/// configured local agent.
///
/// Spawns the loop on the current tokio runtime and returns a handle that can
/// stop the session. State snapshots are pushed into `state` as
/// `SurfaceEvent::StateChanged` events.
pub fn spawn_voice_session(
    state: DesktopState,
    config: Config,
    agent_config: &AgentConfig,
) -> Result<VoiceSessionHandle, DesktopError> {
    let session = build_agent_session(&config, agent_config)?;
    spawn_voice_like_session(
        state,
        session,
        InputOrigin::Voice,
        VoiceSessionConfig::default(),
        None,
        |stop| VoiceSessionHandle { stop },
        |state, handle| {
            state.set_voice_handle(Some(handle.clone()));
            state.set_voice_invite(true);
        },
        |state| {
            state.set_voice_handle(None);
            state.set_voice_invite(false);
        },
    )
}

/// Start ambient listening against the desktop microphone.
///
/// Respects `audio.ambient.enabled` in the agent config; callers must opt in
/// before this function is invoked. Runs indefinitely (idle timeout of one day)
/// until the user stops it from the tray or notification. Each wake event is
/// appended to `config.ambient_log_path()`.
pub fn spawn_ambient_session(
    state: DesktopState,
    config: Config,
    agent_config: &AgentConfig,
) -> Result<AmbientSessionHandle, DesktopError> {
    if !agent_config.audio.ambient.enabled {
        return Err(DesktopError::Transport(
            "ambient listening is not enabled in agent config".into(),
        ));
    }

    let log_path = config
        .ambient_log_path()
        .map_err(|e| DesktopError::Transport(format!("failed to resolve wake log path: {e}")))?;
    let wake_log = WakeLog::new(
        log_path,
        &agent_config.audio.ambient.wake_word,
        agent_config.audio.ambient.wake_word_backend,
    )
    .map_err(|e| DesktopError::Transport(format!("failed to open wake log: {e}")))?;

    let session = build_agent_session(&config, agent_config)?;

    // The ambient pipeline captures an utterance for at most
    // `max_utterance_seconds`; the session turn timeout must be longer. The idle
    // timeout is set to a day so the daemon keeps listening across quiet hours.
    let turn_timeout = Duration::from_secs(agent_config.audio.ambient.max_utterance_seconds + 15);
    let ambient_config = VoiceSessionConfig {
        idle_timeout: Duration::from_secs(60 * 60 * 24),
        turn_timeout,
        ..VoiceSessionConfig::default()
    };
    let wake_word = agent_config.audio.ambient.wake_word.clone();

    spawn_voice_like_session(
        state,
        session,
        InputOrigin::Ambient,
        ambient_config,
        Some(wake_log),
        |stop| AmbientSessionHandle { stop },
        move |state, handle| {
            // If the user hit stop while the session was still starting, abort
            // immediately without ever showing it as active.
            if state.ambient_stop_requested() {
                handle.stop();
                state.set_ambient_starting(false);
                return;
            }
            state.set_ambient_wake_word(wake_word);
            state.set_ambient_handle(Some(handle.clone()));
            state.set_ambient_active(true);
            state.set_ambient_starting(false);
        },
        |state| {
            state.set_ambient_handle(None);
            state.set_ambient_active(false);
            state.set_ambient_starting(false);
            state.set_ambient_stop_requested(false);
        },
    )
}

/// Synthesize and play the voice template for a planner presentation.
///
/// Loads the agent's audio configuration, builds the configured TTS backend,
/// and streams 16-bit PCM to the default output device. Returns an error on
/// missing keys, synthesis failures, or playback problems so the caller can
/// fall back to a desktop notification.
pub async fn speak_presentation(presentation: &Presentation) -> Result<(), DesktopError> {
    let text = presentation.voice_template();
    if text.trim().is_empty() {
        return Err(DesktopError::Transport("empty voice template".into()));
    }

    let config = AgentConfig::load()
        .map_err(|e| DesktopError::Transport(format!("failed to load agent config: {e}")))?;
    let tts_config = &config.audio.tts;

    let normalized = tokio::task::spawn_blocking({
        let text = text.clone();
        let language = tts_config.language.clone();
        move || normalize_for_tts(&text, &language).into_owned()
    })
    .await
    .map_err(|e| DesktopError::Transport(format!("tts normalization panicked: {e}")))?;

    let (tts, voice_id, speed, sample_rate) = build_tts(tts_config)?;
    let request = TtsRequest {
        text: normalized,
        voice: if voice_id.is_empty() {
            None
        } else {
            Some(voice_id)
        },
        reference_audio_path: None,
        options: TtsOptions {
            response_format: Some("pcm_s16le".into()),
            speed,
        },
    };

    let stream = tts
        .synthesize_stream(&request)
        .await
        .map_err(|e| DesktopError::Transport(format!("tts synthesis failed: {e}")))?;

    let format = StreamedAudioFormat {
        sample_rate,
        channels: 1,
        pcm_format: PcmFormat::I16,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    tokio::time::timeout(
        Duration::from_secs(120),
        play_stream(stream, format, cancel),
    )
    .await
    .map_err(|_| DesktopError::Transport("tts playback timed out".into()))?
    .map_err(|e| DesktopError::Transport(format!("tts playback failed: {e}")))?;

    Ok(())
}

type TtsBuildResult = Result<(Arc<dyn TextToSpeech>, String, Option<f32>, u32), DesktopError>;

fn build_tts(config: &takusu_agent::audio_config::TtsConfig) -> TtsBuildResult {
    let api_key = if config.api_key.is_empty() {
        std::env::var(&config.api_key_env).unwrap_or_default()
    } else {
        config.api_key.clone()
    };

    let sample_rate = if config.sample_rate == 0 {
        44100
    } else {
        config.sample_rate
    };

    match config.backend {
        TtsBackend::Cartesia => {
            if api_key.is_empty() && !config.mute {
                return Err(DesktopError::Transport("missing Cartesia API key".into()));
            }
            let mut tts_config = CartesiaSonicConfig::new(api_key);
            tts_config.voice_id = config.voice_id.clone();
            if !config.model.is_empty() {
                tts_config.model_id = config.model.clone();
            }
            tts_config.language = Some(config.language.clone());
            tts_config.output_format.sample_rate = sample_rate;
            tts_config.mute = config.mute;
            Ok((
                Arc::new(CartesiaSonic::new(tts_config)),
                config.voice_id.clone(),
                config.speed,
                sample_rate,
            ))
        }
        TtsBackend::Fish => {
            if api_key.is_empty() && !config.mute {
                return Err(DesktopError::Transport("missing Fish Audio API key".into()));
            }
            let mut tts_config = FishAudioConfig::new(api_key);
            tts_config.voice_id = config.voice_id.clone();
            if !config.model.is_empty() {
                tts_config.model = config.model.clone();
            }
            tts_config.sample_rate = sample_rate;
            tts_config.mute = config.mute;
            Ok((
                Arc::new(FishAudio::new(tts_config)),
                config.voice_id.clone(),
                config.speed,
                sample_rate,
            ))
        }
        TtsBackend::Android => Err(DesktopError::Transport(
            "android tts backend is not supported on desktop".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_events_drive_the_shared_surface_machine() {
        let machine = SurfaceStateMachine::new();
        machine.apply_turn_event(&takusu_agent::TurnEvent::Thinking("working".into()));
        assert_eq!(
            machine.snapshot().state,
            takusu_agent::SurfaceState::Thinking
        );
    }

    #[test]
    fn wake_log_appends_tsv_line() {
        let tmp = std::env::temp_dir().join(format!("takusu-wake-log-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let log = WakeLog::new(&tmp, "たくす", WakeWordBackend::AsrTextMatch).unwrap();
        log.append_wake("レポートを始める").unwrap();
        let contents = std::fs::read_to_string(&tmp).unwrap();
        assert!(contents.contains("たくす"));
        assert!(contents.contains("レポートを始める"));
        assert!(contents.contains("AsrTextMatch"));
        let _ = std::fs::remove_file(&tmp);
    }
}
