//! Optional daemon voice loop (feature `audio-device`, WI-12).
//!
//! Runs the continuous voice session from the shared agent core against the
//! local microphone and mirrors streaming assistant turn events into a
//! [`SurfaceStateMachine`] so the tray and compact panel render the same state
//! as the agent transport. Enable the `audio-device` feature to pull in the
//! `takusu-agent` audio runtime (cpal/sherpa) for this crate.

use std::sync::Arc;

use takusu_agent::{
    AgentConfig, AgentError, AgentSession, InputOrigin, SessionOutcome, SurfaceSnapshot,
    SurfaceStateMachine, SurfaceEvent,
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

/// Run one continuous voice session against the local microphone, mirroring
/// every streaming `TurnEvent` into `machine` and forwarding each state snapshot
/// to `on_state_changed`.
///
/// The session loops `capture -> process -> speak -> capture ...` until the
/// user exits or the idle timeout fires. `stop` lets an external caller request
/// cancellation.
pub async fn voice_loop_with_surface<S>(
    session: Arc<AgentSession>,
    machine: SurfaceStateMachine,
    origin: InputOrigin,
    stop: tokio::sync::watch::Receiver<bool>,
    on_state_changed: S,
) -> Result<SessionOutcome, AgentError>
where
    S: FnMut(SurfaceSnapshot) + Send + 'static,
{
    let on_state = Arc::new(std::sync::Mutex::new(on_state_changed));
    let on_state_for_audio = Arc::clone(&on_state);
    let machine_for_audio = machine.clone();
    takusu_agent::runner::run_voice_session(
        session,
        origin,
        takusu_agent::VoiceSessionConfig::default(),
        stop,
        move |event| {
            let snapshot = machine.apply_turn_event(&event);
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

/// Start a continuous voice session against the desktop microphone and the
/// configured local agent.
///
/// Spawns the loop on the current tokio runtime and returns a handle that can
/// stop the session. State snapshots are pushed into `state` as
/// `SurfaceEvent::StateChanged` events.
pub fn spawn_voice_session(
    state: DesktopState,
    config: Config,
) -> Result<VoiceSessionHandle, DesktopError> {
    let mut agent_config = AgentConfig::load()
        .map_err(|e| DesktopError::Transport(format!("failed to load agent config: {e}")))?;
    agent_config.server.url = config.local_url.clone();
    agent_config.server.token = config.token.clone();

    let client = takusu_client::Client::new(&config.local_url, &config.token);
    let session = Arc::new(
        takusu_agent::runner::build_session(&agent_config, client)
            .map_err(|e| DesktopError::Transport(format!("failed to build agent session: {e}")))?,
    );

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let handle = VoiceSessionHandle { stop: stop_tx };

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
            InputOrigin::Voice,
            stop_rx,
            on_state,
        )
        .await
    });

    let state_for_cleanup = state.clone();
    tokio::spawn(async move {
        match task.await {
            Ok(Ok(outcome)) => tracing::info!(?outcome, "voice session ended"),
            Ok(Err(error)) => tracing::error!(error=%error, "voice session failed"),
            Err(join_error) => tracing::error!(error=%join_error, "voice session task panicked"),
        }
        state_for_cleanup.set_voice_handle(None);
        state_for_cleanup.set_voice_invite(false);
    });

    state.set_voice_handle(Some(handle.clone()));
    Ok(handle)
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
}
