use std::sync::Arc;
use std::time::Duration;

use axum::serve;
use takusu_client::Client;
use takusu_local::router::router;
use takusu_local::state::AppState;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::error::AppError;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub struct LocalServer {
    pub url: String,
    pub token: String,
    _shutdown: oneshot::Sender<()>,
}

pub async fn start_in_process(app: Arc<TakusuApp>) -> Result<LocalServer, AppError> {
    let resp = app.create_token(None).await?;
    let token = resp.token;
    let state = AppState::new(
        app,
        Arc::new(tokio::sync::RwLock::new(Arc::from(token.as_str()))),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AppError::Internal(format!("failed to bind local server: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| AppError::Internal(format!("failed to get local addr: {e}")))?
        .port();
    let (tx, rx) = oneshot::channel();
    let server = serve(listener, router(state)).with_graceful_shutdown(async {
        let _ = rx.await;
    });
    tokio::spawn(async move {
        if let Err(e) = server.await {
            eprintln!("in-process server error: {e}");
        }
    });

    let url = format!("http://127.0.0.1:{port}");
    wait_for_ready(&url, &token).await?;

    Ok(LocalServer {
        url,
        token,
        _shutdown: tx,
    })
}

async fn wait_for_ready(url: &str, token: &str) -> Result<(), AppError> {
    let client = Client::new(url, token);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if client.health().await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(AppError::Internal(
        "in-process server did not become ready".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use takusu_contracts::Storage;
    use takusu_local_lib::config::LocalConfig;
    use takusu_local_lib::storage_sqlite::SqliteStorage;
    use takusu_local_lib::token_cache::TokenCache;

    async fn test_app() -> Arc<TakusuApp> {
        let cfg = LocalConfig {
            db: "sqlite::memory:".to_string(),
            jwt_secret: "test-secret-do-not-use-in-production".to_string(),
            ..Default::default()
        };

        let storage: Arc<dyn Storage> = Arc::new(
            SqliteStorage::init(&cfg)
                .await
                .expect("failed to init test storage"),
        );
        let token_cache = Arc::new(TokenCache::with_default_ttl());
        Arc::new(TakusuApp::new(storage, token_cache))
    }

    #[tokio::test]
    async fn start_in_process_becomes_ready_and_responds_to_health() {
        let app = test_app().await;
        let server = start_in_process(app).await.expect("server should start");

        let client = Client::new(&server.url, &server.token);
        let health = client
            .health()
            .await
            .expect("health endpoint should be reachable");
        assert_eq!(health, "ok");
    }
}
