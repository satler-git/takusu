use std::sync::Arc;

use takusu_agent::transport::{AgentApiState, ApiUserInputProvider, SessionFactory};
use takusu_agent::{AgentConfig, AgentError, AgentSession, InvalidArgsError, ToolError};
use takusu_local_lib::app::TakusuApp;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub app: Arc<TakusuApp>,
    /// Root token configured for this server. Requests bearing this token are
    /// treated as root even if the storage backend cannot verify them, which
    /// lets runtime worker credential updates succeed when the current worker
    /// is unreachable or the token is intended for a new worker.
    pub root_token: Arc<RwLock<Arc<str>>>,
    /// In-process agent state shared with the desktop resident daemon.
    pub agent: Arc<AgentApiState>,
}

impl AppState {
    pub fn new(
        app: Arc<TakusuApp>,
        root_token: Arc<RwLock<Arc<str>>>,
        agent: Arc<AgentApiState>,
    ) -> Self {
        Self {
            app,
            root_token,
            agent,
        }
    }
}

/// A headless session factory for the agent routes mounted in `takusu-local`.
///
/// Full voice / turn sessions are out of scope for the desktop daemon in WI-7;
/// the desktop only needs surface state, notifications, and quick actions.
/// Session creation returns an error so callers get a clear signal if they hit
/// the wrong endpoint.
#[derive(Clone, Debug)]
pub struct HeadlessSessionFactory;

impl SessionFactory for HeadlessSessionFactory {
    fn create(
        &self,
        _config: &AgentConfig,
        _token: Arc<RwLock<Arc<str>>>,
    ) -> Result<AgentSession, AgentError> {
        Err(AgentError::Tool(ToolError::InvalidArgs(
            InvalidArgsError::no_field("headless session factory"),
        )))
    }
}

/// Build an agent state using the local server token and planner URL.
pub fn build_agent_state(token: impl AsRef<str>, worker_url: impl AsRef<str>) -> Arc<AgentApiState> {
    let mut config = AgentConfig::default();
    config.server.url = worker_url.as_ref().to_string();
    Arc::new(AgentApiState::new(
        token,
        Arc::new(HeadlessSessionFactory),
        Arc::new(ApiUserInputProvider::new()),
        config,
    ))
}
