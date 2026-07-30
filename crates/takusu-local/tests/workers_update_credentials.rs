use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use takusu_contracts::{Storage, TaskQuery, TaskRow};
use takusu_local_lib::storage_workers::WorkersStorage;
use tokio::net::TcpListener;

#[derive(Clone, Default)]
struct RetryState {
    tokens: Arc<Mutex<Vec<String>>>,
}

async fn retry_handler(
    State(state): State<RetryState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TaskRow>>, StatusCode> {
    let count = state.tokens.lock().unwrap().len();
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    state.tokens.lock().unwrap().push(token);
    if count == 0 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(vec![]))
}

#[tokio::test]
async fn update_credentials_applies_during_retry() {
    let state = RetryState::default();
    let app = Router::new()
        .route("/api/tasks", get(retry_handler))
        .with_state(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let url = format!("http://127.0.0.1:{port}");
    let storage = Arc::new(WorkersStorage::new_with(url.clone(), "old_token".into()));
    let update_storage = storage.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        update_storage
            .update_credentials(url.clone(), "new_token".into())
            .await;
    });

    let rows = storage.list_tasks(&TaskQuery::default()).await.unwrap();
    assert!(rows.is_empty());

    let tokens = state.tokens.lock().unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0], "Bearer old_token");
    assert_eq!(tokens[1], "Bearer new_token");
}
