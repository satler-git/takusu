use std::sync::Arc;

use crate::{AgentConfig, AgentSession, ToolRegistry};

pub(crate) fn make_agent(
    config: AgentConfig,
    registry: ToolRegistry,
    llm: impl crate::llm::LlmClient + 'static,
) -> AgentSession {
    let client = takusu_client::Client::new(&config.server.url, &config.server.token);
    make_agent_with_client(config, client, registry, llm)
}

pub(crate) fn make_agent_with_client(
    config: AgentConfig,
    client: takusu_client::Client,
    registry: ToolRegistry,
    llm: impl crate::llm::LlmClient + 'static,
) -> AgentSession {
    let tz_cache = crate::tools::takusu::TimeZoneCache::new(client.clone());
    AgentSession::new_with_client_and_cache(config, client, tz_cache, Arc::new(registry), llm)
}
