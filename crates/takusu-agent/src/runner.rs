use std::sync::Arc;

use crate::llm::build_llm_client;
use crate::tools::takusu::{TimeZoneCache, register_tools};
use crate::{
    AgentConfig, AgentError, AgentSession, StubUserInputProvider, ToolRegistry, TurnResult,
    UserInputProvider,
};
use takusu_client::Client;

pub fn build_session(config: &AgentConfig, client: Client) -> Result<AgentSession, AgentError> {
    build_session_with_provider(config, client, Arc::new(StubUserInputProvider))
}

pub fn build_session_with_provider(
    config: &AgentConfig,
    client: Client,
    user_input_provider: Arc<dyn UserInputProvider>,
) -> Result<AgentSession, AgentError> {
    let llm = build_llm_client(&config.llm)?;
    let tz_cache = TimeZoneCache::new(client.clone());
    let registry = Arc::new_cyclic(|weak| {
        let mut registry = ToolRegistry::new();
        register_tools(
            &mut registry,
            client.clone(),
            tz_cache.clone(),
            user_input_provider,
            weak.clone(),
        );
        registry
    });
    Ok(AgentSession::new_with_client_and_cache(
        config.clone(),
        client,
        tz_cache,
        registry,
        llm,
    ))
}

pub async fn run_text(session: &AgentSession, text: &str) -> Result<TurnResult, AgentError> {
    session.run_turn(text).await
}

#[cfg(feature = "audio-device")]
pub async fn run_audio<E>(
    session: Arc<AgentSession>,
    no_tts: bool,
    yes: bool,
    on_event: E,
) -> Result<(), AgentError>
where
    E: FnMut(crate::TurnEvent) + Send + 'static,
{
    use crate::audio::AudioAdapter;
    let mut adapter = AudioAdapter::new(session).await?.with_events(on_event);
    adapter.run(no_tts, yes).await
}

/// Run a continuous voice session against a real microphone, routing streaming
/// assistant events to `on_turn_event` and audio lifecycle callbacks to
/// `on_audio_callback`.
///
/// This is the concrete desktop/platform entry point for the WI-12 voice
/// session: after it returns, control has already run the session loop
/// (`capture -> process -> speak -> capture ...`) until the user exited or the
/// idle timeout fired. `on_turn_event` lets a surface forward `TurnEvent`s to
/// the shared `SurfaceStateMachine`; `on_audio_callback` does the same for
/// `AudioCallback`s.
#[cfg(feature = "audio-device")]
#[allow(clippy::needless_pass_by_value)]
pub async fn run_voice_session<E, A>(
    session: Arc<AgentSession>,
    origin: crate::voice_session::InputOrigin,
    config: crate::voice_session::VoiceSessionConfig,
    stop: tokio::sync::watch::Receiver<bool>,
    on_turn_event: E,
    on_audio_callback: A,
) -> Result<crate::voice_session::SessionOutcome, AgentError>
where
    E: FnMut(crate::TurnEvent) + Send + 'static,
    A: FnMut(crate::surface::AudioCallback) + Send + 'static,
{
    use crate::audio::AudioAdapter;
    use crate::capability::InputPath;
    use crate::voice_session::VoiceSession;
    let input_path = match origin {
        crate::voice_session::InputOrigin::Voice => InputPath::ExplicitVoiceSession,
        crate::voice_session::InputOrigin::Text => InputPath::PlainText,
        crate::voice_session::InputOrigin::Background => InputPath::NotificationCapability,
    };
    let mut adapter = AudioAdapter::new(session)
        .await?
        .with_events(on_turn_event)
        .with_audio_callback(on_audio_callback)
        .with_stop_signal(stop);
    Ok(VoiceSession::new(config, origin, input_path)
        .run(&mut adapter)
        .await)
}
