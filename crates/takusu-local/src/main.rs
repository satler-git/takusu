use std::sync::Arc;

use takusu_contracts::Storage;
use takusu_local::router::router;
use takusu_local::state::{AppState, build_agent_state};
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::config::LocalConfig;
#[cfg(feature = "sqlite")]
use takusu_local_lib::config::StorageKind;
#[cfg(feature = "sqlite")]
use takusu_local_lib::storage_sqlite::SqliteStorage;
use takusu_local_lib::storage_workers::WorkersStorage;
use takusu_local_lib::token_cache::TokenCache;
use tokio::sync::RwLock;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = takusu_local_lib::sentry::init("takusu_local=info", sentry::release_name!());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        let mut cfg = LocalConfig::default();
        if let Ok(v) = std::env::var("TAKUSU_DB") && !v.is_empty() {
            cfg.db = v;
        }
        if let Ok(v) = std::env::var("TAKUSU_BIND") && !v.is_empty() {
            cfg.bind = v;
        }
        if let Ok(v) = std::env::var("TAKUSU_STORAGE") && !v.is_empty() {
            cfg.storage = v.parse().unwrap_or_else(|e| {
                eprintln!("Error: invalid TAKUSU_STORAGE: {e}");
                std::process::exit(1);
            });
        }
        if let Ok(v) = std::env::var("TAKUSU_WORKERS_URL") && !v.is_empty() {
            cfg.worker_url = v;
        } else if let Ok(v) = std::env::var("TAKUSU_WORKER_URL") && !v.is_empty() {
            cfg.worker_url = v;
        }
        if let Ok(v) = std::env::var("TAKUSU_JWT_SECRET") && !v.is_empty() {
            cfg.jwt_secret = v;
        }

        let env_root = std::env::var("TAKUSU_ROOT_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());

        let workers_url = cfg.workers_url().to_string();
        let workers_token = std::env::var("TAKUSU_WORKERS_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| env_root.clone())
            .unwrap_or_default();

        let storage: Arc<dyn Storage> = match cfg.storage {
            #[cfg(feature = "sqlite")]
            StorageKind::Sqlite => {
                if cfg.jwt_secret.is_empty() {
                    return Err("TAKUSU_JWT_SECRET is required for the sqlite backend".into());
                }
                tracing::info!("storage backend: sqlite ({})", cfg.db_url());
                Arc::new(SqliteStorage::init(&cfg).await?)
            }
            #[allow(unreachable_patterns)]
            _ => {
                if workers_url.is_empty() {
                    return Err("TAKUSU_WORKERS_URL is required for the workers backend".into());
                }
                if workers_token.is_empty() {
                    return Err("TAKUSU_WORKERS_TOKEN (or TAKUSU_ROOT_TOKEN) is required for the workers backend".into());
                }
                tracing::info!("storage backend: workers ({workers_url})");
                Arc::new(WorkersStorage::new_with(workers_url.clone(), workers_token.clone()))
            }
        };

        if env_root.is_none() && !workers_token.is_empty() {
            tracing::info!(
                "TAKUSU_ROOT_TOKEN is not set; using TAKUSU_WORKERS_TOKEN as the local root-token fallback"
            );
        }
        let root_token = env_root.unwrap_or(workers_token);
        if root_token.is_empty() {
            tracing::warn!(
                "TAKUSU_ROOT_TOKEN is not set and no worker token is configured; the local root-token bypass is disabled and root-only operations may fail"
            );
        }
        let token_cache = Arc::new(TokenCache::with_default_ttl());
        let app = Arc::new(TakusuApp::new(storage, token_cache));

        // Bind before constructing the agent state so the in-process agent
        // routes call the actual local URL rather than an empty workers URL.
        let bind_addr = cfg.bind_addr().to_string();
        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        let port = listener.local_addr()?.port();
        let local_url = format!("http://127.0.0.1:{port}");
        tracing::info!("listening on {bind_addr}");

        let root_token: Arc<str> = Arc::from(root_token.into_boxed_str());
        let agent = build_agent_state(root_token.clone(), &local_url);
        let state = AppState::new(
            app,
            Arc::new(RwLock::new(root_token)),
            agent,
        );
        let app_router = router(state);

        axum::serve(listener, app_router).await?;

        Ok(())
    })
}
