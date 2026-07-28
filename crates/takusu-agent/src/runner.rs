use std::sync::Arc;

use crate::llm::OpenAIClient;
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
    let llm = OpenAIClient::new(config.llm.clone())?;
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
pub async fn run_audio(
    session: AgentSession,
    no_tts: bool,
) -> Result<(), AgentError> {
    use crate::audio::AudioAdapter;
    let mut adapter = AudioAdapter::new(session).await?;
    adapter.run(no_tts).await
}
