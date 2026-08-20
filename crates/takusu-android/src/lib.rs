uniffi::setup_scaffolding!();

mod audio;
mod log_buffer;
mod model;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use tokio::runtime::{Builder, Runtime};
use tokio::sync::RwLock;

use takusu_agent::tools::takusu::{TimeZoneCache, register_tools};
use takusu_agent::transport::{AgentApiState, ApiUserInputProvider};
use takusu_agent::{AgentConfig, AgentSession, ToolRegistry};
use takusu_contracts::Storage;
use takusu_local::router::router;
use takusu_local::state::AppState;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::storage_workers::WorkersStorage;
use takusu_local_lib::token_cache::TokenCache;
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

#[derive(Debug, Clone, uniffi::Record)]
pub struct EventEvaluationResult {
    pub due_event_ids: Vec<String>,
    pub next_eval_at_millis: Option<i64>,
}

#[uniffi::export]
pub fn evaluate_and_commit_events(
    workers_url: String,
    root_token: String,
    device_id: String,
) -> Result<EventEvaluationResult, TakusuError> {
    if workers_url.trim().is_empty() || root_token.is_empty() {
        return Err(TakusuError::InvalidConfig {
            detail: "workers_url and root_token are required".into(),
        });
    }
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| TakusuError::Server {
            detail: format!("failed to create evaluator runtime: {error}"),
        })?;
    let result = runtime.block_on(async move {
        let http_client =
            takusu_client::default_http_client(None).map_err(|error| TakusuError::Server {
                detail: format!("failed to build evaluator HTTP client: {error}"),
            })?;
        let storage: Arc<dyn Storage> = Arc::new(WorkersStorage::new_with_client(
            http_client,
            workers_url,
            root_token,
        ));
        let app = TakusuApp::new(storage, Arc::new(TokenCache::with_default_ttl()));
        let evaluation = app
            .evaluate_and_commit_events(&device_id)
            .await
            .map_err(|error| TakusuError::Server {
                detail: format!("event evaluation failed: {error}"),
            })?;
        Ok::<_, TakusuError>(EventEvaluationResult {
            due_event_ids: evaluation
                .due_events
                .into_iter()
                .map(|event| event.id)
                .collect(),
            next_eval_at_millis: evaluation
                .next_eval_at
                .map(|timestamp| timestamp.as_second().saturating_mul(1_000)),
        })
    });
    runtime.shutdown_background();
    result
}

/// Embedded takusu server for Android.
///
/// Spawns an axum server on localhost that serves the full takusu-local REST API.
/// Storage backend is WorkersStorage (HTTP → Cloudflare Worker).
/// Process-wide registry of running servers keyed by bound port.
///
/// The foreground module and the background workers each create their own
/// `TakusuServer` instance, so a per-object `runtime` field alone cannot tell
/// them that another instance already holds the port. Binding the same port
/// twice then fails with "Address already in use". This registry makes the
/// port the single source of truth: `start` is idempotent per port, and only
/// the instance that actually owns the runtime shuts it down on `stop`.
static REGISTRY: LazyLock<Mutex<HashMap<u16, Weak<Runtime>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

        // A live server from another instance (e.g. a background worker) may
        // already own this port. Treat that as success and reuse it instead of
        // attempting a second bind, which would fail with "Address already in
        // use". The caller does not own this instance's runtime, but it records
        // the port so that status() reports the live server as running and a
        // later stop() is an idempotent no-op that cannot tear down the owner.
        {
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            registry.retain(|_, w| w.strong_count() > 0);
            if registry.get(&port).is_some_and(|w| w.strong_count() > 0) {
                *self.port.lock().unwrap_or_else(|e| e.into_inner()) = port;
                tracing::info!(
                    "takusu-local already running on port {port}; reusing existing server"
                );
                return Ok(());
            }
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
            workers_url.clone(),
            root_token.clone(),
        ));
        let token_cache = Arc::new(TokenCache::with_default_ttl());
        let app = Arc::new(TakusuApp::new(storage, token_cache));
        let shared_token: Arc<RwLock<Arc<str>>> =
            Arc::new(RwLock::new(Arc::from(root_token.as_str())));

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
                let llm = takusu_agent::llm::build_llm_client(&config.llm)?;
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
            Arc::clone(&shared_token),
            agent_factory,
            user_input_provider,
            agent_config,
        ));
        // Use the same agent state for both the local API and the agent routes
        // so the server does not construct two separate planner clients.
        let state = AppState::new(app, Arc::clone(&shared_token), Arc::clone(&agent_state));
        let app_router = router(state);

        let bind_addr = format!("127.0.0.1:{port}");
        // The check → bind → register sequence must be atomic across all
        // instances, otherwise two concurrent start() calls (e.g. the
        // foreground module and a WorkManager worker) can both pass the
        // "no entry" check and race to bind the same port. Holding the
        // registry lock here serialises them: the first re-check sees no
        // entry and binds; the second re-check sees the entry and reuses.
        let (listener, actual_port) = {
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            registry.retain(|_, w| w.strong_count() > 0);
            if registry.get(&port).is_some_and(|w| w.strong_count() > 0) {
                // Another instance registered while we were building the
                // router. Reuse it instead of binding.
                *self.port.lock().unwrap_or_else(|e| e.into_inner()) = port;
                tracing::info!(
                    "takusu-local already running on port {port}; reusing existing server"
                );
                return Ok(());
            }
            let listener = runtime
                .block_on(async { TcpListener::bind(&bind_addr).await })
                .map_err(|e| {
                    let detail = format!("failed to bind {bind_addr}: {e}");
                    tracing::error!("{detail}");
                    TakusuError::Server { detail }
                })?;
            let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
            registry.insert(actual_port, Arc::downgrade(&runtime));
            (listener, actual_port)
        };

        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = actual_port;

        tracing::info!("takusu-local listening on 127.0.0.1:{actual_port} (workers storage)");

        let task_port = actual_port;
        runtime.spawn(async move {
            if let Err(e) = axum::serve(listener, app_router).await {
                tracing::error!("server error: {e}");
            }
            // The HTTP listener has shut down. Drop the registry entry so a
            // future start() re-binds instead of reusing a now-dead server.
            REGISTRY
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&task_port);
        });

        *runtime_guard = Some(runtime);
        Ok(())
    }

    /// Stop the server gracefully.
    ///
    /// Idempotent: returns Ok if this instance owned a runtime (shutting it
    /// down) or reused an existing server (orphaning it), and only returns
    /// `NotRunning` for an instance that never started. This lets the Kotlin
    /// module clear its reference even when it adopted a server it does not
    /// own.
    pub fn stop(&self) -> Result<(), TakusuError> {
        let mut runtime_guard = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
        let arc = runtime_guard.take();
        let owned = arc.is_some();
        let port = *self.port.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(arc) = &arc {
            // Only deregister if this instance is the one that registered the
            // port. Instances that reused an existing server never registered,
            // so they cannot tear down another instance's server.
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(weak) = registry.get(&port)
                && weak
                    .upgrade()
                    .is_none_or(|reg_runtime| Arc::ptr_eq(&reg_runtime, arc))
            {
                registry.remove(&port);
            }
        }
        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        // If another caller still holds a clone of the runtime, let that task
        // finish; the runtime will be dropped once the last clone is released.
        drop(runtime_guard);
        if let Some(arc) = arc
            && let Ok(runtime) = Arc::try_unwrap(arc)
        {
            runtime.shutdown_background();
        }
        if owned || port != 0 {
            Ok(())
        } else {
            Err(TakusuError::NotRunning)
        }
    }

    /// Get the current server status.
    ///
    /// The registry is the source of truth for whether the port is live, so an
    /// instance that reused an existing server reports `Running` just like the
    /// owner does, and a server whose HTTP task died reports `Stopped` even if
    /// this instance still holds a runtime.
    pub fn status(&self) -> ServerStatus {
        let port = *self.port.lock().unwrap_or_else(|e| e.into_inner());
        if port == 0 {
            return ServerStatus::Stopped;
        }
        let registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if registry.get(&port).is_some_and(|w| w.strong_count() > 0) {
            ServerStatus::Running { port }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    }

    fn start(server: &TakusuServer, port: u16) -> Result<(), TakusuError> {
        server.start_with_agent_config(
            port,
            "https://workers.example.test".to_string(),
            "root-token".to_string(),
            String::new(),
        )
    }

    #[test]
    fn start_is_idempotent_per_port_and_owner_only_stops() {
        let port = free_port();
        let owner = TakusuServer::new();
        let reuser = TakusuServer::new();

        start(&owner, port).expect("first bind should succeed");
        assert!(matches!(owner.status(), ServerStatus::Running { port: p } if p == port));

        // A second instance starting on the same port must reuse the existing
        // server instead of failing with "Address already in use".
        start(&reuser, port).expect("second start on the same port should reuse");

        // Both the owner and the reuser report Running, so the JS/Kotlin layer
        // that checks status() after start() sees a live server (#reuser-status).
        assert!(matches!(owner.status(), ServerStatus::Running { port: p } if p == port));
        assert!(matches!(reuser.status(), ServerStatus::Running { port: p } if p == port));

        // The reuser does not own the runtime, but its stop() returns Ok so the
        // Kotlin module can clear its reference, and it must not tear down the
        // owner's server.
        assert!(reuser.stop().is_ok());
        assert!(matches!(owner.status(), ServerStatus::Running { port: p } if p == port));

        owner.stop().expect("owner stop should succeed");
        assert!(matches!(owner.status(), ServerStatus::Stopped));
    }

    #[test]
    fn concurrent_start_never_double_binds() {
        let port = free_port();
        let owner = TakusuServer::new();
        let reuser = TakusuServer::new();

        // Race both instances through the check → bind → register path at once.
        std::thread::scope(|s| {
            s.spawn(|| start(&owner, port));
            s.spawn(|| start(&reuser, port));
        });

        // Exactly one instance owns the runtime; the other reused it. Neither
        // failed with "Address already in use", and both report running.
        let owner_running =
            matches!(owner.status(), ServerStatus::Running { port: p } if p == port);
        let reuser_running =
            matches!(reuser.status(), ServerStatus::Running { port: p } if p == port);
        assert!(
            owner_running && reuser_running,
            "both instances should report running, got owner={owner_running} reuser={reuser_running}"
        );

        // Whichever instance won the bind is the owner; the other is a reuser
        // whose stop() is a no-op. Stopping both tears the shared server down
        // exactly once and leaves both instances reported stopped.
        owner.stop().ok();
        reuser.stop().ok();
        assert!(matches!(owner.status(), ServerStatus::Stopped));
        assert!(matches!(reuser.status(), ServerStatus::Stopped));
    }

    #[test]
    fn registry_is_clean_after_stop() {
        let port = free_port();
        {
            let server = TakusuServer::new();
            start(&server, port).expect("bind should succeed");
            server.stop().expect("stop should succeed");
        }
        let registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!registry.contains_key(&port));
    }
}
