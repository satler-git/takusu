pub mod config;
pub mod embed;

use std::borrow::Cow;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use config::Settings;
use embed::Assets;
use takusu_local::state::AppState;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::config::{LocalConfig, StorageKind};
#[cfg(feature = "sqlite")]
use takusu_local_lib::storage_sqlite::SqliteStorage;
use takusu_local_lib::storage_workers::WorkersStorage;
use takusu_local_lib::token_cache::TokenCache;
use takusu_contracts::Storage;
use tokio::sync::RwLock;

/// Build the storage backend described by `cfg`. `workers_token` (resolved from
/// env/config by `config::load`) authenticates against the workers backend.
async fn build_storage(
    cfg: &LocalConfig,
    workers_token: &str,
) -> Result<Arc<dyn Storage>, Box<dyn std::error::Error>> {
    let workers_url = cfg.workers_url();
    let storage: Arc<dyn Storage> = match cfg.storage {
        #[cfg(feature = "sqlite")]
        StorageKind::Sqlite => {
            if cfg.jwt_secret.is_empty() {
                return Err(
                    "jwt_secret (config) or TAKUSU_JWT_SECRET is required for the sqlite backend"
                        .into(),
                );
            }
            tracing::info!("storage backend: sqlite ({})", cfg.db_url());
            Arc::new(SqliteStorage::init(cfg).await?)
        }
        #[allow(unreachable_patterns)]
        _ => {
            if workers_url.is_empty() {
                return Err(
                    "worker_url (config) or TAKUSU_WORKERS_URL is required for the workers backend"
                        .into(),
                );
            }
            if workers_token.is_empty() {
                return Err(
                    "workers_token/root_token (config) or TAKUSU_WORKERS_TOKEN/TAKUSU_ROOT_TOKEN is required for the workers backend"
                        .into(),
                );
            }
            tracing::info!("storage backend: workers ({workers_url})");
            Arc::new(WorkersStorage::new_with(
                workers_url.to_string(),
                workers_token.to_string(),
            ))
        }
    };
    Ok(storage)
}

/// Build the full web router: the takusu REST API under `/api`, an
/// unauthenticated `/bootstrap` endpoint that hands the localhost client a
/// root token, and an SPA fallback that serves the embedded frontend.
///
/// The web UI is localhost-only and trusts the local machine, so the user never
/// enters a token: the frontend fetches `/bootstrap` once on load and uses the
/// returned token for all `/api` calls. `run` enforces a loopback bind so the
/// unauthenticated `/bootstrap` token is never exposed off-machine.
pub async fn build_router(settings: &Settings) -> Result<Router, Box<dyn std::error::Error>> {
    let storage = build_storage(&settings.local, &settings.workers_token).await?;
    let token_cache = Arc::new(TokenCache::with_default_ttl());
    let app = Arc::new(TakusuApp::new(storage, token_cache));

    // Issue a root token the localhost frontend can use for `/api` calls.
    let token = app.create_token(Some("takusu-web")).await?.token;

    let state = AppState::new(app, Arc::new(RwLock::new(Arc::from(token.as_str()))));

    let api = takusu_local::router::router(state);

    let bootstrap_token = token.clone();
    let router = Router::new()
        .route(
            "/bootstrap",
            get(move || async move { axum::Json(serde_json::json!({ "token": bootstrap_token })) }),
        )
        .merge(api)
        .fallback(static_handler);

    Ok(router)
}

/// Serve the embedded frontend and API. Loads config from the shared TOML file
/// and `TAKUSU_*` env vars, then binds `cfg.bind_addr()` (or `bind_override`).
///
/// Refuses to bind a non-loopback address: `/bootstrap` returns an
/// unauthenticated root token, which is only safe on the local machine.
pub async fn run(bind_override: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let settings = config::load();
    let bind_addr = bind_override.unwrap_or_else(|| settings.local.bind_addr().to_string());
    ensure_loopback(&bind_addr)?;

    let router = build_router(&settings).await?;

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("takusu-web listening on http://{bind_addr}");
    axum::serve(listener, router).await?;
    Ok(())
}

/// Reject any bind address that does not resolve exclusively to loopback
/// interfaces. Every resolved address must be loopback: a hostname that yields
/// both a loopback and a non-loopback address is refused, otherwise
/// `TcpListener::bind` could fall back to the non-loopback one. `0.0.0.0` /
/// `::` (all interfaces) and LAN addresses are refused so the unauthenticated
/// `/bootstrap` token cannot leak onto the network.
fn ensure_loopback(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::net::ToSocketAddrs;
    let resolved: Vec<_> = addr
        .to_socket_addrs()
        .map_err(|e| format!("invalid bind address '{addr}': {e}"))?
        .collect();
    if resolved.is_empty() {
        return Err(format!("could not resolve bind address '{addr}'").into());
    }
    if !resolved.iter().all(|sock| sock.ip().is_loopback()) {
        return Err(format!(
            "takusu-web serves an unauthenticated /bootstrap token and must bind to a loopback address; refusing '{addr}'. Bind e.g. 127.0.0.1:3000."
        )
        .into());
    }
    Ok(())
}

/// Wrap an embedded asset's bytes without copying in the common (release,
/// `Cow::Borrowed`) case.
fn asset_body(data: Cow<'static, [u8]>) -> Body {
    match data {
        Cow::Borrowed(b) => Body::from(Bytes::from_static(b)),
        Cow::Owned(o) => Body::from(Bytes::from(o)),
    }
}

/// Serve a static asset from the embedded bundle, falling back to `index.html`
/// for client-side routing (SPA). Unknown `/api` paths return 404 instead of
/// the SPA shell so API typos surface as errors, not HTML.
async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if path == "api" || path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref())],
            asset_body(file.data),
        )
            .into_response();
    }

    match Assets::get("index.html") {
        Some(file) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            asset_body(file.data),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
