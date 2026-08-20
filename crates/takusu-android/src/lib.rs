uniffi::setup_scaffolding!();

mod audio;
mod log_buffer;
mod model;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use tokio::runtime::{Builder, Runtime};
use tokio::sync::RwLock;

use axum::Router;
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
///
/// Each entry also tracks a `stopping` flag so that a runtime that has been
/// asked to shut down is not reused while its `TcpListener` is still being
/// torn down asynchronously.
struct ServerEntry {
    weak: Weak<Runtime>,
    stopping: AtomicBool,
}

static REGISTRY: LazyLock<Mutex<HashMap<u16, ServerEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(uniffi::Object)]
pub struct TakusuServer {
    runtime: Mutex<Option<Arc<Runtime>>>,
    port: Mutex<u16>,
    is_owner: Mutex<bool>,
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
            is_owner: Mutex::new(false),
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
        //
        // If the registry entry is marked as stopping (another instance already
        // called stop()) or the Weak pointer is dead, remove the stale entry and
        // bind a fresh runtime. If we do reuse a live runtime, keep its Arc in
        // our own `runtime` field so this instance holds the server alive.
        {
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            registry.retain(|_, entry| {
                !entry.stopping.load(Ordering::SeqCst) && entry.weak.strong_count() > 0
            });
            if let Some(entry) = registry.get(&port)
                && !entry.stopping.load(Ordering::SeqCst)
                && let Some(runtime) = entry.weak.upgrade()
            {
                *self.port.lock().unwrap_or_else(|e| e.into_inner()) = port;
                *runtime_guard = Some(runtime);
                *self.is_owner.lock().unwrap_or_else(|e| e.into_inner()) = false;
                tracing::info!(
                    "takusu-local already running on port {port}; reusing existing server"
                );
                return Ok(());
            }
            // Stale or stopping entry; remove it so we can bind a fresh runtime.
            registry.remove(&port);
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
        // The check → bind → register sequence must be atomic across all
        // instances, otherwise two concurrent start() calls (e.g. the
        // foreground module and a WorkManager worker) can both pass the
        // "no entry" check and race to bind the same port. Holding the
        // registry lock here serialises them: the first re-check sees no
        // entry and binds; the second re-check sees the entry and reuses.
        let (listener, actual_port) = {
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            registry.retain(|_, w| !w.stopping.load(Ordering::SeqCst) && w.weak.strong_count() > 0);
            if let Some(entry) = registry.get(&port)
                && !entry.stopping.load(Ordering::SeqCst)
                && let Some(runtime) = entry.weak.upgrade()
            {
                // Another instance registered while we were building the
                // router. Reuse it instead of binding.
                *self.port.lock().unwrap_or_else(|e| e.into_inner()) = port;
                *runtime_guard = Some(runtime);
                *self.is_owner.lock().unwrap_or_else(|e| e.into_inner()) = false;
                tracing::info!(
                    "takusu-local already running on port {port}; reusing existing server"
                );
                return Ok(());
            }
            registry.remove(&port);
            let listener = runtime
                .block_on(async { TcpListener::bind(&bind_addr).await })
                .map_err(|e| {
                    let detail = format!("failed to bind {bind_addr}: {e}");
                    tracing::error!("{detail}");
                    TakusuError::Server { detail }
                })?;
            let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
            registry.insert(
                actual_port,
                ServerEntry {
                    weak: Arc::downgrade(&runtime),
                    stopping: AtomicBool::new(false),
                },
            );
            (listener, actual_port)
        };

        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = actual_port;
        *self.is_owner.lock().unwrap_or_else(|e| e.into_inner()) = true;

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
        let is_owner = *self.is_owner.lock().unwrap_or_else(|e| e.into_inner());
        if is_owner {
            // Mark the registry entry as stopping so other instances do not
            // adopt this runtime while its TcpListener is being torn down.
            let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = registry.get(&port) {
                entry.stopping.store(true, Ordering::SeqCst);
            }
            // The spawned axum task may be aborted before it can remove the
            // registry entry (especially when we drop the runtime
            // synchronously below), so deregister now.
            registry.remove(&port);
        }
        *self.is_owner.lock().unwrap_or_else(|e| e.into_inner()) = false;
        *self.port.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        // If another caller still holds a clone of the runtime, let that task
        // finish; the runtime will be dropped once the last clone is released.
        // When we are the last owner, drop the runtime synchronously so the
        // TcpListener is released immediately. This avoids races where the
        // foreground module tries to bind the port while the runtime is still
        // being torn down in the background.
        drop(runtime_guard);
        if let Some(arc) = arc
            && let Ok(runtime) = Arc::try_unwrap(arc)
        {
            drop(runtime);
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
        // Check the registry first, but do not hold the lock while we later
        // lock `self.runtime`; `start()` and `stop()` take the runtime lock
        // first and then the registry lock, so acquiring them in reverse order
        // here can deadlock. The registry guard is dropped at the end of this
        // block, before the runtime lock is taken.
        {
            let registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = registry.get(&port)
                && !entry.stopping.load(Ordering::SeqCst)
                && entry.weak.strong_count() > 0
            {
                return ServerStatus::Running { port };
            }
        }
        // The registry entry may be removed before the Runtime is dropped (or
        // while a reuser is still holding an Arc). As long as this instance's
        // own Arc is still alive, the TcpListener is bound and the server is
        // usable, so report Running. This keeps foreground callers from seeing
        // a stale "stopped" status while they still hold the runtime.
        let runtime_guard = self.runtime.lock().unwrap_or_else(|e| e.into_inner());
        if runtime_guard.is_some() {
            return ServerStatus::Running { port };
        }
        ServerStatus::Stopped
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

/// Global server status independent of any `TakusuServer` instance.
///
/// The Kotlin `TakusuServerModule` uses this when its own `server` reference
/// is null (for example when a WorkManager worker started the server directly
/// through `uniffi.takusu_android.TakusuServer`).
#[uniffi::export]
fn global_server_status() -> ServerStatus {
    let registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    for (port, entry) in registry.iter() {
        if !entry.stopping.load(Ordering::SeqCst) && entry.weak.strong_count() > 0 {
            return ServerStatus::Running { port: *port };
        }
    }
    ServerStatus::Stopped
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

    #[test]
    fn reuser_reports_running_after_owner_stops() {
        let port = free_port();
        let owner = TakusuServer::new();
        let reuser = TakusuServer::new();

        start(&owner, port).expect("owner bind should succeed");
        start(&reuser, port).expect("reuser should adopt running server");

        // When the owner stops it removes the registry entry and drops its Arc.
        // The reuser still holds an Arc, so the runtime is still alive and its
        // status() must still report Running (using its own runtime field).
        owner.stop().expect("owner stop should succeed");
        assert!(matches!(reuser.status(), ServerStatus::Running { port: p } if p == port));

        // Only after the reuser also drops its Arc does the server stop.
        reuser.stop().expect("reuser stop should succeed");
        assert!(matches!(reuser.status(), ServerStatus::Stopped));
    }

    #[test]
    fn stopping_runtime_is_not_reused() {
        let port = free_port();
        let owner = TakusuServer::new();
        start(&owner, port).expect("first bind should succeed");

        // Stop the owner. A fresh start must bind a new runtime rather than
        // adopt the registry entry while the old runtime is still being torn
        // down and its TcpListener is closing.
        owner.stop().expect("owner stop should succeed");
        assert!(matches!(owner.status(), ServerStatus::Stopped));

        let fresh = TakusuServer::new();
        start(&fresh, port).expect("fresh bind should succeed after stop");
        assert!(matches!(fresh.status(), ServerStatus::Running { port: p } if p == port));
        fresh.stop().expect("fresh stop should succeed");
    }
}
