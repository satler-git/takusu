uniffi::setup_scaffolding!();

mod audio;
mod log_buffer;
mod model;

use std::sync::{Arc, Mutex};

use tokio::runtime::{Builder, Runtime};
use tokio::sync::RwLock;

use axum::Router;
use takusu_agent::tools::takusu::{TimeZoneCache, register_tools};
use takusu_agent::transport::{AgentApiState, ApiUserInputProvider};
use takusu_agent::{AgentConfig, AgentSession, ToolRegistry};
use takusu_local::router::router;
use takusu_local::state::AppState;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::storage_workers::WorkersStorage;
use takusu_local_lib::token_cache::TokenCache;
use takusu_storage::Storage;
use tokio::net::TcpListener;

/// Error type for FFI
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum TakusuError {
    #[error("server already running")]
    AlreadyRunning,
    #[error("server not running")]
    NotRunning,
    #[error("invalid configuration: {detail}")]
    InvalidConfig { detail: String },
    #[error("server error: {detail}")]
    Server { detail: String },
    #[error("model error: {detail}")]
    Model { detail: String },
    #[error("audio error: {detail}")]
    Audio { detail: String },
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum ServerStatus {
    Stopped,
    Running { port: u16 },
}

/// Embedded takusu server for Android.
///
/// Spawns an axum server on localhost that serves the full takusu-local REST API.
/// Storage backend is WorkersStorage (HTTP → Cloudflare Worker).
#[derive(uniffi::Object)]
pub struct TakusuServer {
    runtime: Mutex<Option<Arc<Runtime>>>,
    port: Mutex<u16>,
}

impl Default for TakusuServer {
    fn default() -> Self {
        Self::new()
    }
}

#[uniffi::export]
impl TakusuServer {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            runtime: Mutex::new(None),
            port: Mutex::new(0),
        }
    }

    /// Backwards-compatible server start used by the widget worker.
    pub fn start(
        &self,
        port: u16,
        workers_url: String,
        root_token: String,
    ) -> Result<(), TakusuError> {
        self.start_with_agent_config(port, workers_url, root_token, String::new())
    }

    /// Start the server and configure the in-process Agent.
    pub fn start_with_agent_config(
        &self,
        port: u16,
        workers_url: String,
        root_token: String,
        agent_config_json: String,
    ) -> Result<(), TakusuError> {
        // Install the in-process log ring buffer first so that validation
        // errors and subsequent server logs are captured. Uses try_init() so
        // restarts (stop → start) don't panic when the global subscriber is
        // already set.
        log_buffer::install();

        let mut runtime_guard = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
        if runtime_guard.is_some() {
            tracing::error!("server already running");
            return Err(TakusuError::AlreadyRunning);
        }

        if workers_url.is_empty() {
            tracing::error!("workers_url must not be empty");
            return Err(TakusuError::InvalidConfig {
                detail: "workers_url must not be empty".to_string(),
            });
        }
        if root_token.is_empty() {
            tracing::error!("root_token must not be empty");
            return Err(TakusuError::InvalidConfig {
                detail: "root_token must not be empty".to_string(),
            });
        }

        let runtime = Arc::new(
            Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    let detail = format!("failed to create runtime: {e}");
                    tracing::error!("{detail}");
                    TakusuError::Server { detail }
                })?,
        );

        // Build a reqwest client that uses bundled Mozilla root certificates
        // (webpki-root-certs) instead of rustls-platform-verifier.  The
        // platform verifier requires JNI initialisation with an Android
        // Context, which is not available inside the embedded UniFFI runtime.
        // Without it, any HTTPS request panics ("Expect rustls-platform-verifier
        // to be initialized"), killing the axum task and surfacing as
        // "unexpected end of stream" on the client side.
        let http_client =
            takusu_client::default_http_client(None).map_err(|e| TakusuError::Server {
                detail: format!("failed to build HTTP client: {e}"),
            })?;

        let storage: Arc<dyn Storage> = Arc::new(WorkersStorage::new_with_client(
            http_client,
            workers_url,
            root_token.clone(),
        ));
        let token_cache = Arc::new(TokenCache::with_default_ttl());
        let app = Arc::new(TakusuApp::new(storage, token_cache));
        let shared_token: Arc<RwLock<Arc<str>>> =
            Arc::new(RwLock::new(Arc::from(root_token.as_str())));
        let state = AppState::new(app, Arc::clone(&shared_token));

        // Agent sessions run in the same process as the planner server. The
        // factory creates a fresh session for each authenticated Mobile
        // session, while keeping provider credentials in the native layer.
        let mut agent_config = if agent_config_json.trim().is_empty() {
            AgentConfig::default()
        } else {
            serde_json::from_str(&agent_config_json).map_err(|e| TakusuError::InvalidConfig {
                detail: format!("invalid agent configuration: {e}"),
            })?
        };
        agent_config.server.url = format!("http://127.0.0.1:{port}");
        let user_input_provider = Arc::new(ApiUserInputProvider::new());
        let agent_factory = Arc::new({
            let user_input_provider = user_input_provider.clone();
            move |config: &AgentConfig, token: Arc<RwLock<Arc<str>>>| {
                let llm = takusu_agent::llm::OpenAIClient::new(config.llm.clone())?;
                let planner_client =
                    takusu_client::Client::new_with_token(&config.server.url, token);
                let tz_cache = TimeZoneCache::new(planner_client.clone());
                let registry = Arc::new_cyclic(|weak| {
                    let mut registry = ToolRegistry::new();
                    register_tools(
                        &mut registry,
                        planner_client.clone(),
                        tz_cache.clone(),
                        user_input_provider.clone(),
                        weak.clone(),
                    );
                    registry
                });
                Ok(AgentSession::new_with_client_and_cache(
                    config.clone(),
                    planner_client,
                    tz_cache,
                    registry,
                    llm,
                ))
            }
        });
        let agent_state = Arc::new(AgentApiState::new_with_token(
            shared_token,
            agent_factory,
            user_input_provider,
            agent_config,
        ));
        let app_router = router(state).merge(Router::new().nest(
            "/api/agent/v1",
            takusu_agent::transport::router(agent_state),
        ));

        let bind_addr = format!("127.0.0.1:{port}");
        let listener = runtime
            .block_on(async { TcpListener::bind(&bind_addr).await })
            .map_err(|e| {
                let detail = format!("failed to bind {bind_addr}: {e}");
                tracing::error!("{detail}");
                TakusuError::Server { detail }
            })?;

        let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = actual_port;

        tracing::info!("takusu-local listening on 127.0.0.1:{actual_port} (workers storage)");

        runtime.spawn(async move {
            if let Err(e) = axum::serve(listener, app_router).await {
                tracing::error!("server error: {e}");
            }
        });

        *runtime_guard = Some(runtime);
        Ok(())
    }

    /// Stop the server gracefully.
    pub fn stop(&self) -> Result<(), TakusuError> {
        let mut runtime_guard = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
        let arc = runtime_guard.take().ok_or(TakusuError::NotRunning)?;
        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        // If another caller still holds a clone of the runtime, let that task
        // finish; the runtime will be dropped once the last clone is released.
        drop(runtime_guard);
        if let Ok(runtime) = Arc::try_unwrap(arc) {
            runtime.shutdown_background();
        }
        Ok(())
    }

    /// Get the current server status.
    pub fn status(&self) -> ServerStatus {
        let runtime_guard = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
        let port_guard = self.port.lock().unwrap_or_else(|e| e.into_inner());
        if runtime_guard.is_some() && *port_guard > 0 {
            ServerStatus::Running { port: *port_guard }
        } else {
            ServerStatus::Stopped
        }
    }
}

// ── Log capture (free functions exported to Kotlin) ──────────────────

/// Snapshot of the captured server log lines (oldest first).
/// Returns an empty list if the server hasn't started or no logs exist.
#[uniffi::export]
fn get_logs() -> Vec<String> {
    log_buffer::get_logs()
}

/// Clear the captured log buffer.
#[uniffi::export]
fn clear_logs() {
    log_buffer::clear_logs();
}

/// Push a client-side log line (e.g. from JS/Expo) into the shared buffer.
#[uniffi::export]
fn push_log(line: String) {
    log_buffer::push_log(line);
}
