//! Embedded `takusu-local` server for the desktop daemon.
//!
//! When the desktop is started without an explicit `local_url` it spawns the
//! same axum router used by `takusu-local` on a random loopback port. This
//! removes the need to run a separate `takusu-local` process manually while
//! still reusing all of the existing business logic and agent routes.

use std::sync::Arc;
use std::time::Duration;

use axum::serve;
use takusu_contracts::Storage;
use takusu_local::router::router;
use takusu_local::state::{AppState, build_agent_state};
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::config::{LocalConfig, StorageKind};
use takusu_local_lib::storage_sqlite::SqliteStorage;
use takusu_local_lib::storage_workers::WorkersStorage;
use takusu_local_lib::token_cache::TokenCache;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::time::{Instant, sleep};

use crate::config::Config;
use crate::state::DesktopError;

/// Start an embedded `takusu-local` server and update `config.local_url` and
/// `config.token` to point at it.
pub async fn start(config: &mut Config) -> Result<(), DesktopError> {
    let mut cfg = LocalConfig {
        bind: "127.0.0.1:0".into(),
        ..LocalConfig::default()
    };

    if let Ok(v) = std::env::var("TAKUSU_DB")
        && !v.is_empty()
    {
        cfg.db = v;
    } else {
        let data_dir = dirs::state_dir()
            .or_else(dirs::data_dir)
            .ok_or_else(|| DesktopError::Transport("no user state or data directory".into()))?
            .join("takusu");
        cfg.db = format!("sqlite:{}", data_dir.join("takusu.db").display());
    }

    if let Ok(v) = std::env::var("TAKUSU_STORAGE")
        && !v.is_empty()
    {
        cfg.storage = v
            .parse()
            .map_err(|e| DesktopError::Transport(format!("invalid TAKUSU_STORAGE: {e}")))?;
    }

    if let Ok(v) = std::env::var("TAKUSU_WORKERS_URL")
        && !v.is_empty()
    {
        cfg.worker_url = v;
    } else if let Ok(v) = std::env::var("TAKUSU_WORKER_URL")
        && !v.is_empty()
    {
        cfg.worker_url = v;
    }

    if let Ok(v) = std::env::var("TAKUSU_JWT_SECRET")
        && !v.is_empty()
    {
        cfg.jwt_secret = v;
    } else if let Ok(v) = std::env::var("TAKUSU_JWT_SECRET_FILE")
        && !v.is_empty()
    {
        cfg.jwt_secret = std::fs::read_to_string(&v)
            .map_err(|e| {
                DesktopError::Transport(format!("failed to read TAKUSU_JWT_SECRET_FILE: {e}"))
            })?
            .trim()
            .to_string();
    }

    let mut workers_token = String::new();
    if let Ok(v) = std::env::var("TAKUSU_WORKERS_TOKEN")
        && !v.is_empty()
    {
        workers_token = v;
    } else if let Ok(v) = std::env::var("TAKUSU_WORKERS_TOKEN_FILE")
        && !v.is_empty()
    {
        workers_token = std::fs::read_to_string(&v)
            .map_err(|e| {
                DesktopError::Transport(format!("failed to read TAKUSU_WORKERS_TOKEN_FILE: {e}"))
            })?
            .trim()
            .to_string();
    }

    let storage: Arc<dyn Storage> = match cfg.storage {
        StorageKind::Sqlite => {
            if cfg.jwt_secret.is_empty() {
                return Err(DesktopError::Transport(
                    "TAKUSU_JWT_SECRET or TAKUSU_JWT_SECRET_FILE is required for the sqlite backend"
                        .into(),
                ));
            }
            let storage = SqliteStorage::init(&cfg).await.map_err(|e| {
                DesktopError::Transport(format!("failed to initialize sqlite storage: {e}"))
            })?;
            Arc::new(storage)
        }
        StorageKind::Workers => {
            let workers_url = cfg.workers_url().to_string();
            if workers_url.is_empty() {
                return Err(DesktopError::Transport(
                    "TAKUSU_WORKERS_URL is required for the workers backend".into(),
                ));
            }
            if workers_token.is_empty() {
                return Err(DesktopError::Transport(
                    "TAKUSU_WORKERS_TOKEN or TAKUSU_WORKERS_TOKEN_FILE is required for the workers backend"
                        .into(),
                ));
            }
            Arc::new(WorkersStorage::new_with(workers_url, workers_token.clone()))
        }
    };

    let token_cache = Arc::new(TokenCache::with_default_ttl());
    let app = Arc::new(TakusuApp::new(storage, token_cache));

    let bind_addr = cfg.bind_addr().to_string();
    let listener = TcpListener::bind(&bind_addr).await.map_err(|e| {
        DesktopError::Transport(format!("failed to bind embedded local server: {e}"))
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| DesktopError::Transport(format!("failed to get local port: {e}")))?
        .port();
    let local_url = format!("http://127.0.0.1:{port}");

    // Use the configured bearer token as the root token. If none is configured
    // but a workers token is, reuse that token so other clients can connect with
    // the same credential. Otherwise generate an ephemeral token.
    let root_token = if !config.token.is_empty() {
        config.token.clone()
    } else if !workers_token.is_empty() {
        tracing::info!(
            "TAKUSU_TOKEN is not set; using TAKUSU_WORKERS_TOKEN as the local root-token fallback"
        );
        workers_token
    } else {
        let token = format!("tsk_{}", uuid::Uuid::now_v7());
        tracing::warn!(
            "no bearer token configured; using an ephemeral root token; set TAKUSU_TOKEN_FILE to make it persistent"
        );
        token
    };

    let root_token: Arc<str> = Arc::from(root_token.into_boxed_str());
    let agent = build_agent_state(root_token.clone(), &local_url);
    let state = AppState::new(app, Arc::new(RwLock::new(root_token.clone())), agent);
    let app_router = router(state);

    tokio::spawn(async move {
        if let Err(e) = serve(listener, app_router).await {
            tracing::error!(error = %e, "embedded local server exited");
        }
    });

    // Wait until the server is actually accepting connections before the desktop
    // daemon starts issuing requests to it.
    wait_for_ready(&local_url).await?;

    config.local_url = local_url;
    if config.token.is_empty() {
        config.token = root_token.to_string();
    }

    Ok(())
}

async fn wait_for_ready(url: &str) -> Result<(), DesktopError> {
    let health_url = format!("{url}/health");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => sleep(Duration::from_millis(50)).await,
        }
    }

    Err(DesktopError::Transport(
        "embedded local server did not become ready".into(),
    ))
}
