use axum::Router;
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use serde_json::json;
use takusu_contracts::Storage;
use takusu_local_lib::storage_workers::WorkersStorage;
use tokio::net::TcpListener;

#[tokio::test]
async fn gcal_mappings_round_trip_with_empty_worker_body() {
    let app = Router::new()
        .route(
            "/api/sync/mappings",
            get(|| async { axum::Json(json!([])) }),
        )
        .route("/api/sync/mappings", post(|| async { StatusCode::OK }))
        .route("/api/sync/mappings", delete(|| async { StatusCode::OK }));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    let storage = WorkersStorage::new_with(format!("http://{addr}"), "token".into());

    let list = storage.list_gcal_mappings().await.unwrap();
    assert!(list.is_empty());

    storage
        .upsert_gcal_mappings(&[("t1".into(), "e1".into())])
        .await
        .unwrap();

    storage.delete_gcal_mappings(&["t1".into()]).await.unwrap();
}
