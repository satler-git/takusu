use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::sync::LazyLock;
use takusu_local::router::router as build_router;
use takusu_local::state::AppState;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::config::LocalConfig;
use takusu_local_lib::storage_sqlite::SqliteStorage;
use takusu_local_lib::token_cache::TokenCache;
use takusu_local_lib::{DEFAULT_AUD, generate_root_jwt, jwt};
use tokio::sync::RwLock;
use tower::ServiceExt;

const JWT_SECRET: &str = "test-secret-do-not-use-in-production";
static ROOT_TOKEN: LazyLock<String> = LazyLock::new(|| {
    generate_root_jwt(JWT_SECRET, None).expect("root token generation should not fail")
});

fn root_token() -> &'static str {
    ROOT_TOKEN.as_str()
}

async fn setup() -> (AppState, SqlitePool) {
    let cfg = LocalConfig {
        db: "sqlite::memory:".into(),
        jwt_secret: JWT_SECRET.into(),
        ..Default::default()
    };
    let storage = SqliteStorage::init(&cfg).await.unwrap();
    let pool = storage.pool().clone();
    let token_cache = Arc::new(TokenCache::with_default_ttl());
    let app = Arc::new(TakusuApp::new(Arc::new(storage), token_cache));
    let state = AppState::new(app, Arc::new(RwLock::new(Arc::from(root_token()))));
    (state, pool)
}

fn auth_req(method: Method, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {}", root_token()))
        .body(Body::empty())
        .unwrap()
}

fn auth_req_body(method: Method, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {}", root_token()))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn auth_req_body_idempotency(
    method: Method,
    uri: &str,
    body: serde_json::Value,
    operation_id: &str,
) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {}", root_token()))
        .header("content-type", "application/json")
        .header("idempotency-key", operation_id)
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_str(body: Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn health_check() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_str(res.into_body()).await, "ok");
}

#[tokio::test]
async fn unauthorized_without_token() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = Request::builder()
        .uri("/api/tasks")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unauthorized_with_wrong_token() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = Request::builder()
        .uri("/api/tasks")
        .header("authorization", "Bearer wrong_token")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn authorized_with_root_token() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req(Method::GET, "/api/tasks");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn token_crud() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tokens",
        json!({ "label": "test-device" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let new_token = body["token"].as_str().unwrap();
    assert_eq!(new_token.split('.').count(), 3, "token should be a JWT");

    let claims = jwt::verify(JWT_SECRET, new_token, DEFAULT_AUD).unwrap();
    let jti = claims.jti;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tokens WHERE jti = ?")
        .bind(&jti)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let (state2, pool2) = setup().await;
    sqlx::query(
        "INSERT INTO tokens (jti, scope, label, created_by) VALUES (?, 'read-write', ?, 'root')",
    )
    .bind(&jti)
    .bind("test-device")
    .execute(&pool2)
    .await
    .unwrap();
    let app2 = build_router(state2);
    let req = Request::builder()
        .uri("/api/tasks")
        .header("authorization", format!("Bearer {new_token}"))
        // new_token is a JWT signed with JWT_SECRET, so it is still valid.
        .body(Body::empty())
        .unwrap();
    let res = app2.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_req = auth_req(Method::GET, "/api/tokens");
    let res = app2.oneshot(list_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn task_create_and_list() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "テストタスク",
            "description": "テスト用",
            "start_at": "2026-06-05T09:00:00+09:00",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 60,
            "sigma_minutes": 15,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.3
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["title"], "テストタスク");
    assert_eq!(body["status"], "pending");

    let list_req = auth_req(Method::GET, "/api/tasks");
    let res = app.oneshot(list_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn task_create_rejects_reversed_datetimes() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "bad",
            "start_at": "2026-06-05T18:00:00+09:00",
            "end_at": "2026-06-05T09:00:00+09:00",
            "avg_minutes": 30,
            "depends": []
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn task_update_rejects_reversed_datetimes() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "original",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": []
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = body["id"].as_str().unwrap();

    let update = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({
            "start_at": "2026-06-05T19:00:00+09:00"
        }),
    );
    let res = app.oneshot(update).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn split_rejects_end_at_before_start_at() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "split-me",
            "start_at": "2026-07-22T10:00:00+09:00",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "quantity_done": 0
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/split"),
        json!({
            "retained_quantity": 5,
            "end_at": "2026-07-22T09:00:00+09:00"
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn task_get_update_delete() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "original",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = body["id"].as_str().unwrap();

    let get_req = auth_req(Method::GET, &format!("/api/tasks/{task_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let update_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "title": "updated" }),
    );
    let res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let updated: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(updated["title"], "updated");

    let not_found_req = auth_req(Method::GET, "/api/tasks/nonexistent");
    let res = app.clone().oneshot(not_found_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let delete_req = auth_req(Method::DELETE, &format!("/api/tasks/{task_id}"));
    let res = app.oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn task_replace() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "original",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = body["id"].as_str().unwrap();

    let replace_req = auth_req_body(
        Method::PUT,
        &format!("/api/tasks/{task_id}"),
        json!({
            "title": "replaced",
            "end_at": "2026-06-06T12:00:00+09:00",
            "avg_minutes": 45,
            "depends": [],
            "parallelizable": true,
            "allows_parallel": false,
            "abandonability": 0.8
        }),
    );
    let res = app.oneshot(replace_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let replaced: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(replaced["title"], "replaced");
    assert_eq!(replaced["avg_minutes"], 45);
    assert_eq!(replaced["parallelizable"], true);
}

#[tokio::test]
async fn replace_task_resets_lifecycle_fields() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "original",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "quantity_unit": "問"
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = body["id"].as_str().unwrap();

    let complete = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({"status": "completed", "quantity_done": 10}),
    );
    let res = app.clone().oneshot(complete).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let replace = auth_req_body(
        Method::PUT,
        &format!("/api/tasks/{task_id}"),
        json!({
            "title": "replaced",
            "end_at": "2026-06-06T18:00:00+09:00",
            "avg_minutes": 30,
        }),
    );
    let res = app.clone().oneshot(replace).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let replaced: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(replaced["status"], "pending");
    assert!(replaced["completed_at"].is_null());
    assert_eq!(replaced["quantity_done"], 0);
}

#[tokio::test]
async fn task_list_filter_by_status() {
    let (state, _) = setup().await;
    let app = build_router(state);

    for i in 0..3 {
        let req = auth_req_body(
            Method::POST,
            "/api/tasks",
            json!({
                "title": format!("task-{i}"),
                "end_at": "2026-06-05T18:00:00+09:00",
                "avg_minutes": 30,
                "depends": [],
                "parallelizable": false,
                "allows_parallel": false,
                "abandonability": 0.5
            }),
        );
        app.clone().oneshot(req).await.unwrap();
    }

    let req = auth_req(Method::GET, "/api/tasks?status=pending");
    let res = app.oneshot(req).await.unwrap();
    let list: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn task_list_filter_by_q_status() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let mut ids = Vec::new();
    for title in ["alpha", "beta", "gamma"] {
        let req = auth_req_body(
            Method::POST,
            "/api/tasks",
            json!({
                "title": title,
                "end_at": "2026-06-05T18:00:00+09:00",
                "avg_minutes": 30,
                "depends": [],
                "parallelizable": false,
                "allows_parallel": false,
                "abandonability": 0.5,
            }),
        );
        let res = app.clone().oneshot(req).await.unwrap();
        let body: serde_json::Value =
            serde_json::from_str(&body_str(res.into_body()).await).unwrap();
        ids.push(body["id"].as_str().unwrap().to_string());
    }

    for (id, status) in ids.iter().zip(["scheduled", "completed"].iter()) {
        let update_req = auth_req_body(
            Method::PATCH,
            &format!("/api/tasks/{id}"),
            json!({ "status": status }),
        );
        let res = app.clone().oneshot(update_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    let req = auth_req(Method::GET, "/api/tasks?q=status%3Ascheduled");
    let res = app.clone().oneshot(req).await.unwrap();
    let list: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let titles: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["alpha"]);

    let req = auth_req(Method::GET, "/api/tasks?q=status%3Acompleted");
    let res = app.clone().oneshot(req).await.unwrap();
    let list: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let titles: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["beta"]);
}

#[tokio::test]
async fn habit_crud() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "description": "30分走る",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();
    // create_habit assigns a monotonic display_id (#305).
    let display_id = body["display_id"].as_i64().unwrap();
    assert!(display_id >= 1);

    let get_req = auth_req(Method::GET, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let habit: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(habit["title"], "朝のランニング");
    assert_eq!(habit["display_id"], display_id);
    assert_eq!(
        habit["recurrence"],
        r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#
    );

    // Habit can be fetched by `h{display_id}` (#305).
    let h_req = auth_req(Method::GET, &format!("/api/habits/h{display_id}"));
    let res = app.clone().oneshot(h_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let h_habit: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(h_habit["id"], habit_id);

    let update_req = auth_req_body(
        Method::PATCH,
        &format!("/api/habits/{habit_id}"),
        json!({ "title": "夜のランニング", "active": false }),
    );
    let res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let updated: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(updated["title"], "夜のランニング");

    let list_req = auth_req(Method::GET, "/api/habits");
    let res = app.clone().oneshot(list_req).await.unwrap();
    let list: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    let delete_req = auth_req(Method::DELETE, &format!("/api/habits/{habit_id}"));
    let res = app.oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn habit_display_id_is_monotonic() {
    // #305: habit display_id is assigned from a monotonic sequence.
    let (state, _) = setup().await;
    let app = build_router(state);

    let mk = || {
        auth_req_body(
            Method::POST,
            "/api/habits",
            json!({
                "title": "h",
                "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
                "start_time": "06:00",
                "end_time": "07:00",
                "avg_minutes": 30,
            }),
        )
    };

    let r1 = app.clone().oneshot(mk()).await.unwrap();
    let b1: serde_json::Value = serde_json::from_str(&body_str(r1.into_body()).await).unwrap();
    let r2 = app.clone().oneshot(mk()).await.unwrap();
    let b2: serde_json::Value = serde_json::from_str(&body_str(r2.into_body()).await).unwrap();
    let d1 = b1["display_id"].as_i64().unwrap();
    let d2 = b2["display_id"].as_i64().unwrap();
    assert_eq!(d2, d1 + 1, "habit display_id must be monotonic");

    // Fetch the second habit by h{d2}.
    let h_req = auth_req(Method::GET, &format!("/api/habits/h{d2}"));
    let res = app.oneshot(h_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn habit_delete_cascades_to_tasks() {
    // #240: deleting a habit that has already generated tasks must
    // succeed (cascade-delete the associated tasks) instead of failing
    // on the foreign-key constraint.
    let (state, _) = setup().await;
    let app = build_router(state);

    // Create a habit.
    let habit_req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "description": "30分走る",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(habit_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();

    // Create a task referencing the habit.
    let task_req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "ランニングタスク",
            "end_at": "2026-07-06T07:00:00Z",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1,
            "habit_id": habit_id,
        }),
    );
    let res = app.clone().oneshot(task_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task_body: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = task_body["id"].as_str().unwrap();

    // Delete the habit — must succeed and cascade-delete the task.
    let delete_req = auth_req(Method::DELETE, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // Habit is gone.
    let get_req = auth_req(Method::GET, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // Associated task is also gone.
    let task_get = auth_req(Method::GET, &format!("/api/tasks/{task_id}"));
    let res = app.oneshot(task_get).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ical_import() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260605T090000Z\r\nDTEND:20260605T110000Z\r\nSUMMARY:会議\r\nUID:meeting-001@example.com\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20260606T140000Z\r\nDTEND:20260606T150000Z\r\nSUMMARY:レビュー\r\nUID:review-001@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR";

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/tasks/import/ical")
        .header("authorization", format!("Bearer {}", root_token()))
        .header("content-type", "text/calendar")
        .body(Body::from(ical.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["imported"], 2);
    let ids = body["task_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 2);

    for id in ids {
        let id = id.as_str().unwrap();
        let get_req = auth_req(Method::GET, &format!("/api/tasks/{id}"));
        let res = app.clone().oneshot(get_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let task: serde_json::Value =
            serde_json::from_str(&body_str(res.into_body()).await).unwrap();
        assert!(
            !task["title"].as_str().unwrap().contains('\r'),
            "title should not contain \\r"
        );
        assert!(task["start_at"].as_str().unwrap().contains('Z'));
        assert!(task["end_at"].as_str().unwrap().contains('Z'));
        assert_eq!(task["fixed"], true);
        // 120 min (会議) or 60 min (レビュー)
        let avg = task["avg_minutes"].as_i64().unwrap();
        assert!(avg == 120 || avg == 60, "unexpected avg_minutes: {avg}");
    }
}

#[tokio::test]
async fn ical_import_skips_duplicate() {
    let (state, pool) = setup().await;
    sqlx::query("INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid) VALUES ('existing', '会議', '2026-06-05T11:00:00Z', 120, 0, '[]', 0, 0, 0.5, 'pending', 'meeting-001@example.com')")
        .execute(&pool)
        .await.unwrap();

    let app = build_router(state);
    let ical = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260605T090000Z\r\nDTEND:20260605T110000Z\r\nSUMMARY:会議\r\nUID:meeting-001@example.com\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART:20260606T140000Z\r\nDTEND:20260606T150000Z\r\nSUMMARY:レビュー\r\nUID:review-001@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR";

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/tasks/import/ical")
        .header("authorization", format!("Bearer {}", root_token()))
        .header("content-type", "text/calendar")
        .body(Body::from(ical.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["imported"], 1);
}

#[tokio::test]
async fn schedule_generate_and_get() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "作業A",
            "start_at": "2026-06-05T09:00:00+09:00",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 60,
            "sigma_minutes": 10,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    app.clone().oneshot(create_req).await.unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["schedule"].is_string());

    let get_req = auth_req(Method::GET, "/api/schedule");
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let clear_req = auth_req(Method::DELETE, "/api/schedule");
    let res = app.clone().oneshot(clear_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let get_req = auth_req(Method::GET, "/api/schedule");
    let res = app.oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn schedule_not_found_initially() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req(Method::GET, "/api/schedule");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn token_revoke() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tokens (jti, scope, label, created_by) VALUES (?, 'read-write', 'to-revoke', 'root')",
    )
    .bind("tsk_test_revoke_token")
    .execute(&pool)
    .await
    .unwrap();

    let token_id: i64 = sqlx::query_scalar("SELECT id FROM tokens WHERE label = 'to-revoke'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let app = build_router(state);
    let revoke_req = auth_req(Method::DELETE, &format!("/api/tokens/{token_id}"));
    let res = app.oneshot(revoke_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn delete_nonexistent_task() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req(Method::DELETE, "/api/tasks/nonexistent-id");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn task_prefix_lookup() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "prefix-test",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let full_id = body["id"].as_str().unwrap();
    assert!(full_id.contains('-'));
    let short_id = &full_id[..8];

    let get_req = auth_req(Method::GET, &format!("/api/tasks/{short_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(task["id"], full_id);
    assert_eq!(task["title"], "prefix-test");

    let update_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{short_id}"),
        json!({ "title": "prefix-updated" }),
    );
    let res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let replace_req = auth_req_body(
        Method::PUT,
        &format!("/api/tasks/{short_id}"),
        json!({
            "title": "prefix-replaced",
            "end_at": "2026-06-06T12:00:00+09:00",
            "avg_minutes": 45,
            "depends": [],
            "parallelizable": true,
            "allows_parallel": false,
            "abandonability": 0.8
        }),
    );
    let res = app.clone().oneshot(replace_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let not_found_req = auth_req(Method::GET, "/api/tasks/00000000");
    let res = app.clone().oneshot(not_found_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let delete_req = auth_req(Method::DELETE, &format!("/api/tasks/{short_id}"));
    let res = app.oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn task_update_status() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "status-test",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = body["id"].as_str().unwrap();

    for status in &["in_progress", "completed", "skipped"] {
        let update_req = auth_req_body(
            Method::PATCH,
            &format!("/api/tasks/{task_id}"),
            json!({ "status": status }),
        );
        let res = app.clone().oneshot(update_req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let updated: serde_json::Value =
            serde_json::from_str(&body_str(res.into_body()).await).unwrap();
        assert_eq!(updated["status"], *status);
    }

    let bad_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "status": "invalid" }),
    );
    let res = app.oneshot(bad_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn update_task_to_terminal_status_closes_open_work_session() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    async fn create_task(app: &axum::Router, title: &str) -> String {
        let req = auth_req_body(
            Method::POST,
            "/api/tasks",
            json!({
                "title": title,
                "end_at": "2026-06-05T18:00:00+09:00",
                "avg_minutes": 30,
            }),
        );
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body: serde_json::Value =
            serde_json::from_str(&body_str(res.into_body()).await).unwrap();
        body["id"].as_str().unwrap().to_string()
    }

    async fn start_work(app: &axum::Router, id: &str) {
        let req = auth_req(Method::POST, &format!("/api/tasks/{id}/work/start"));
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    async fn open_session_count(pool: &SqlitePool, id: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_work_sessions WHERE task_id = ? AND ended_at IS NULL",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn update_status(app: &axum::Router, id: &str, status: &str) {
        let req = auth_req_body(
            Method::PATCH,
            &format!("/api/tasks/{id}"),
            json!({ "status": status }),
        );
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    // Skipped closes the open session.
    let skipped_id = create_task(&app, "skip-me").await;
    start_work(&app, &skipped_id).await;
    assert_eq!(open_session_count(&pool, &skipped_id).await, 1);
    update_status(&app, &skipped_id, "skipped").await;
    assert_eq!(open_session_count(&pool, &skipped_id).await, 0);

    // Completed also closes the open session.
    let completed_id = create_task(&app, "complete-me").await;
    start_work(&app, &completed_id).await;
    assert_eq!(open_session_count(&pool, &completed_id).await, 1);
    update_status(&app, &completed_id, "completed").await;
    assert_eq!(open_session_count(&pool, &completed_id).await, 0);
}

#[tokio::test]
async fn task_complete_query_limits() {
    let (state, _) = setup().await;
    let app = build_router(state);

    for title in ["alpha-test", "beta-test", "gamma-test"] {
        create_task_simple(&app, title).await;
    }

    // Default limit is respected.
    let req = auth_req(Method::GET, "/api/tasks/complete?q=test");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let comps = body.as_array().unwrap();
    assert!(comps.len() <= 30, "default limit should be 30 or less");

    // Explicit small limit.
    let req = auth_req(Method::GET, "/api/tasks/complete?q=test&limit=2");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body.as_array().unwrap().len(), 2);

    // limit=0 is invalid.
    let req = auth_req(Method::GET, "/api/tasks/complete?q=test&limit=0");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Max allowed limit.
    let req = auth_req(Method::GET, "/api/tasks/complete?q=test&limit=100");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Over max limit is invalid.
    let req = auth_req(Method::GET, "/api/tasks/complete?q=test&limit=101");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Empty limit value is rejected.
    let req = auth_req(Method::GET, "/api/tasks/complete?q=test&limit=");
    let res = app.oneshot(req).await.unwrap();
    assert!(
        res.status().is_client_error(),
        "empty limit should be rejected"
    );
}

#[tokio::test]
async fn generate_excludes_in_progress() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task1', 'pending-task', '2026-06-05T18:00:00+09:00', 60, 0, '[]', 0, 0, 0.5, 'pending')"
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task2', 'in-progress-task', '2026-06-05T18:00:00+09:00', 30, 0, '[]', 0, 0, 0.5, 'in_progress')"
    ).execute(&pool).await.unwrap();

    let app = build_router(state);

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let schedule: Vec<serde_json::Value> =
        serde_json::from_str(body["schedule"].as_str().unwrap()).unwrap();
    assert_eq!(
        schedule.len(),
        1,
        "should include pending but exclude in_progress tasks"
    );
    assert_eq!(schedule[0]["task_id"], "task1");
}

#[tokio::test]
async fn generate_preserves_in_progress_schedule_entry() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task1', 'pending-task', '2026-06-05T18:00:00+09:00', 60, 0, '[]', 0, 0, 0.5, 'pending')"
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task2', 'will-be-in-progress', '2026-06-05T18:00:00+09:00', 30, 0, '[]', 0, 0, 0.5, 'pending')"
    ).execute(&pool).await.unwrap();

    let app = build_router(state.clone());

    // First generation: both pending tasks get scheduled.
    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({ "sleep": "disabled" }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let schedule: Vec<serde_json::Value> =
        serde_json::from_str(body["schedule"].as_str().unwrap()).unwrap();
    let task2_entry: serde_json::Value = schedule
        .iter()
        .find(|e| e["task_id"] == "task2")
        .expect("task2 should be scheduled initially")
        .clone();

    // Mark task2 as in_progress (user started working on it).
    let upd_req = auth_req_body(
        Method::PATCH,
        "/api/tasks/task2",
        json!({ "status": "in_progress" }),
    );
    let res = app.clone().oneshot(upd_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Regenerate: task2 is in_progress so it's excluded from the planner,
    // but its previous schedule entry must be preserved (#354).
    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({ "sleep": "disabled" }),
    );
    let res = app.oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let schedule: Vec<serde_json::Value> =
        serde_json::from_str(body["schedule"].as_str().unwrap()).unwrap();
    let task_ids: Vec<&str> = schedule
        .iter()
        .map(|e| e["task_id"].as_str().unwrap())
        .collect();
    assert!(
        task_ids.contains(&"task1"),
        "pending task should be scheduled"
    );
    assert!(
        task_ids.contains(&"task2"),
        "in_progress task entry should be preserved (#354)"
    );
    let preserved = schedule.iter().find(|e| e["task_id"] == "task2").unwrap();
    assert_eq!(preserved["start_at"], task2_entry["start_at"]);
    assert_eq!(preserved["end_at"], task2_entry["end_at"]);
}

#[tokio::test]
async fn generate_excludes_completed_and_skipped() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task1', 'pending-task', '2026-06-05T18:00:00+09:00', 60, 0, '[]', 0, 0, 0.5, 'pending')"
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task2', 'completed-task', '2020-01-01T12:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'completed')"
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task3', 'skipped-task', '2026-06-05T18:00:00+09:00', 30, 0, '[]', 0, 0, 0.5, 'skipped')"
    ).execute(&pool).await.unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task4', 'in-progress-task', '2026-06-05T18:00:00+09:00', 30, 0, '[]', 0, 0, 0.5, 'in_progress')"
    ).execute(&pool).await.unwrap();

    let app = build_router(state);

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let schedule: Vec<serde_json::Value> =
        serde_json::from_str(body["schedule"].as_str().unwrap()).unwrap();
    let task_ids: Vec<&str> = schedule
        .iter()
        .map(|e| e["task_id"].as_str().unwrap())
        .collect();
    assert!(task_ids.contains(&"task1"), "should include pending task");
    assert!(
        !task_ids.contains(&"task2"),
        "should exclude completed task"
    );
    assert!(!task_ids.contains(&"task3"), "should exclude skipped task");
    assert!(
        !task_ids.contains(&"task4"),
        "should exclude in_progress task"
    );
}

#[tokio::test]
async fn settings_get_default() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req(Method::GET, "/api/settings");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["tz"], "UTC");
    assert_eq!(body["sleep_start"], "22:00");
    assert_eq!(body["sleep_end"], "06:00");
    assert_eq!(body["solver"], "sa");
}

#[tokio::test]
async fn settings_update() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::PUT,
        "/api/settings",
        json!({
            "tz": "Asia/Tokyo",
            "sleep_start": "23:00",
            "sleep_end": "07:00"
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["tz"], "Asia/Tokyo");
    assert_eq!(body["sleep_start"], "23:00");
    assert_eq!(body["sleep_end"], "07:00");

    let get_req = auth_req(Method::GET, "/api/settings");
    let res = app.oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["tz"], "Asia/Tokyo");
    assert_eq!(body["sleep_start"], "23:00");
}

#[tokio::test]
async fn settings_update_partial() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::PUT,
        "/api/settings",
        json!({ "tz": "Europe/Berlin" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["tz"], "Europe/Berlin");
    assert_eq!(body["sleep_start"], "22:00");
    assert_eq!(body["sleep_end"], "06:00");
}

#[tokio::test]
async fn settings_update_workload() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::PUT,
        "/api/settings",
        json!({
            "comfortable_minutes": 480,
            "maximum_minutes": 720
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["comfortable_minutes"], 480);
    assert_eq!(body["maximum_minutes"], 720);

    // Clear with 0 (the mobile default sentinel) — the server stores 0 and
    // parse_workload treats it as default.
    let req = auth_req_body(
        Method::PUT,
        "/api/settings",
        json!({
            "comfortable_minutes": 0,
            "maximum_minutes": 0
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["comfortable_minutes"], 0);
    assert_eq!(body["maximum_minutes"], 0);
}

#[tokio::test]
async fn update_workers_config_updates_token_with_root() {
    let (state, _) = setup().await;
    let app = build_router(state.clone());
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/api/workers/config")
        .header("authorization", format!("Bearer {}", root_token()))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "url": "https://new.example.com", "token": "new_token" }).to_string(),
        ))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // Worker credentials are updated inside WorkersStorage; no AppState.token to assert.
}

#[tokio::test]
async fn update_workers_config_rejects_normal_token() {
    let (state, _) = setup().await;
    let token_row = state.app.create_token(None).await.unwrap();
    let app = build_router(state.clone());
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/api/workers/config")
        .header("authorization", format!("Bearer {}", token_row.token))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "url": "https://new.example.com", "token": "new_token" }).to_string(),
        ))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn update_workers_config_allows_empty_url_and_token() {
    let (state, _) = setup().await;
    let app = build_router(state.clone());
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/api/workers/config")
        .header("authorization", format!("Bearer {}", root_token()))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "url": "", "token": "" }).to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn schedule_generate_with_custom_sleep() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('task1', 'test-task', '2026-06-05T18:00:00+09:00', 60, 0, '[]', 0, 0, 0.5, 'pending')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let set_req = auth_req_body(
        Method::PUT,
        "/api/settings",
        json!({ "tz": "Asia/Tokyo", "sleep_start": "23:00", "sleep_end": "07:00" }),
    );
    let res = app.clone().oneshot(set_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "recommended"
        }),
    );
    let res = app.oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn move_entry_with_force_overrides_warnings() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('t1', 'Task1', '2026-06-10T12:00:00Z', 60, 0, '[]', 0, 0, 0.5, 'scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO schedules (id, schedule, created_at, updated_at) VALUES ('active', '[{\"task_id\":\"t1\",\"start_at\":\"2026-06-10T10:00:00Z\",\"end_at\":\"2026-06-10T11:00:00Z\"}]', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let req = auth_req_body(
        Method::PATCH,
        "/api/schedule/entries/t1",
        json!({
            "start_at": "2026-06-10T13:00:00Z",
            "force": true
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["task_id"], "t1");
    assert!(
        body["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("deadline_violation"))
    );
}

#[tokio::test]
async fn move_entry_without_force_rejects_violations() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('t1', 'Task1', '2026-06-10T12:00:00Z', 60, 0, '[]', 0, 0, 0.5, 'scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO schedules (id, schedule, created_at, updated_at) VALUES ('active', '[{\"task_id\":\"t1\",\"start_at\":\"2026-06-10T10:00:00Z\",\"end_at\":\"2026-06-10T11:00:00Z\"}]', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let req = auth_req_body(
        Method::PATCH,
        "/api/schedule/entries/t1",
        json!({
            "start_at": "2026-06-10T13:00:00Z",
            "force": false
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn move_entry_no_violation_succeeds() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('t1', 'Task1', '2026-06-10T14:00:00Z', 60, 0, '[]', 0, 0, 0.5, 'scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO schedules (id, schedule, created_at, updated_at) VALUES ('active', '[{\"task_id\":\"t1\",\"start_at\":\"2026-06-10T10:00:00Z\",\"end_at\":\"2026-06-10T11:00:00Z\"}]', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let req = auth_req_body(
        Method::PATCH,
        "/api/schedule/entries/t1",
        json!({
            "start_at": "2026-06-10T12:00:00Z",
            "force": false
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["task_id"], "t1");
}

#[tokio::test]
async fn move_entry_task_not_in_schedule_errors() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('t1', 'Task1', '2026-06-10T14:00:00Z', 60, 0, '[]', 0, 0, 0.5, 'scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO schedules (id, schedule, created_at, updated_at) VALUES ('active', '[]', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let req = auth_req_body(
        Method::PATCH,
        "/api/schedule/entries/t1",
        json!({ "start_at": "2026-06-10T12:00:00Z" }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn clear_schedule_when_empty_is_noop() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req(Method::DELETE, "/api/schedule");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn reschedule_range_mode() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('t1', 'Task1', '2026-06-10T14:00:00Z', 60, 0, '[]', 0, 0, 0.5, 'scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO schedules (id, schedule, created_at, updated_at) VALUES ('active', '[{\"task_id\":\"t1\",\"start_at\":\"2026-06-10T10:00:00Z\",\"end_at\":\"2026-06-10T11:00:00Z\"}]', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/schedule/reschedule",
        json!({
            "mode": "range",
            "from": "2026-06-10T08:00:00Z",
            "until": "2026-06-10T18:00:00Z",
            "sleep": "disabled"
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn preview_range_mode() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('t1', 'Task1', '2026-06-10T14:00:00Z', 60, 0, '[]', 0, 0, 0.5, 'scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO schedules (id, schedule, created_at, updated_at) VALUES ('active', '[{\"task_id\":\"t1\",\"start_at\":\"2026-06-10T10:00:00Z\",\"end_at\":\"2026-06-10T11:00:00Z\"}]', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/schedule/preview",
        json!({
            "mode": "range",
            "from": "2026-06-10T08:00:00Z",
            "until": "2026-06-10T18:00:00Z",
            "sleep": "disabled"
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["entries"].is_array());
}

#[tokio::test]
async fn sync_settings_flow() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let get_req = auth_req(Method::GET, "/api/sync/settings");
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["enabled"], false);

    let put_req = auth_req_body(
        Method::PUT,
        "/api/sync/settings",
        json!({
            "enabled": true,
            "calendar_id": "test@calendar.com",
            "client_id": "fake-client-id",
            "client_secret": "fake-secret"
        }),
    );
    let res = app.clone().oneshot(put_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["enabled"], true);
    assert_eq!(body["has_client_secret"], true);
}

#[tokio::test]
async fn generate_schedule_excludes_completed_in_progress_skipped() {
    let (state, pool) = setup().await;

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('p1', 'Pending', '2026-06-10T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'pending')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('s1', 'Scheduled', '2026-06-10T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('c1', 'Completed', '2026-06-10T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'completed')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let schedule: Vec<serde_json::Value> =
        serde_json::from_str(body["schedule"].as_str().unwrap()).unwrap();
    let task_ids: Vec<&str> = schedule
        .iter()
        .map(|e| e["task_id"].as_str().unwrap())
        .collect();
    assert!(task_ids.contains(&"p1"));
    assert!(task_ids.contains(&"s1"));
    assert!(!task_ids.contains(&"c1"));
}

#[tokio::test]
async fn habit_sync_marks_generated_task_unedited() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(!tasks.is_empty());
    for t in &tasks {
        assert_eq!(t["user_edited"], false);
    }
}

#[tokio::test]
async fn task_edit_marks_habit_task_user_edited() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = tasks[0]["id"].as_str().unwrap();
    assert_eq!(tasks[0]["user_edited"], false);

    let edit_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "title": " edited" }),
    );
    let res = app.clone().oneshot(edit_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let updated: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(updated["user_edited"], true);
}

#[tokio::test]
async fn task_status_update_keeps_user_edited_false() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = tasks[0]["id"].as_str().unwrap();

    let status_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "status": "completed" }),
    );
    let res = app.clone().oneshot(status_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let updated: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(updated["user_edited"], false);
}

#[tokio::test]
async fn habit_change_respects_user_edited_flag() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(tasks.len() >= 3, "expected at least 3 generated tasks");
    // Pick tasks near the middle of the generated range so they are included
    // in both sync runs even if the exact window shifts slightly.
    let task_id = tasks[tasks.len() / 3]["id"].as_str().unwrap();
    let other_id = tasks[tasks.len() * 2 / 3]["id"].as_str().unwrap();

    // After generation tasks are scheduled; sync only mutates pending tasks.
    // Set both targets back to pending so the next sync can update them.
    let pend_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "status": "pending" }),
    );
    let res = app.clone().oneshot(pend_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pend_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{other_id}"),
        json!({ "status": "pending" }),
    );
    let res = app.clone().oneshot(pend_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let edit_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "avg_minutes": 99 }),
    );
    let res = app.clone().oneshot(edit_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let habit_update = auth_req_body(
        Method::PATCH,
        &format!("/api/habits/{habit_id}"),
        json!({ "avg_minutes": 45 }),
    );
    let res = app.clone().oneshot(habit_update).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let get_req = auth_req(Method::GET, &format!("/api/tasks/{task_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    let edited: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(edited["avg_minutes"], 99);
    assert_eq!(edited["user_edited"], true);

    let get_req = auth_req(Method::GET, &format!("/api/tasks/{other_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    let other: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(other["avg_minutes"], 45);
    assert_eq!(other["user_edited"], false);
}

#[tokio::test]
async fn revert_to_habit_clears_user_edited_flag() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(tasks.len() >= 3, "expected at least 3 generated tasks");
    // Pick a task near the middle of the generated range to survive both syncs.
    let task_id = tasks[tasks.len() / 2]["id"].as_str().unwrap();

    // sync only mutates pending tasks; set target back to pending before the next sync.
    let pend_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "status": "pending" }),
    );
    let res = app.clone().oneshot(pend_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let edit_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "avg_minutes": 99 }),
    );
    let res = app.clone().oneshot(edit_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let revert_req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{task_id}"),
        json!({ "user_edited": false }),
    );
    let res = app.clone().oneshot(revert_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let reverted: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(reverted["user_edited"], false);

    let habit_update = auth_req_body(
        Method::PATCH,
        &format!("/api/habits/{habit_id}"),
        json!({ "avg_minutes": 45 }),
    );
    let res = app.clone().oneshot(habit_update).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let get_req = auth_req(Method::GET, &format!("/api/tasks/{task_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(task["avg_minutes"], 45);
}

#[tokio::test]
async fn stale_user_edited_task_is_not_deleted() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "朝のランニング",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let habit_id = body["id"].as_str().unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, habit_id, user_edited, start_at) VALUES ('stale1', 'Stale edited', '2030-01-01T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'pending', ?, 1, '2030-01-01T09:00:00Z')",
    )
    .bind(habit_id)
    .execute(&pool)
    .await
    .unwrap();

    // Use a different date so the two stale tasks do not collide in the
    // (habit_id, date) key used by sync_habit_tasks.
    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, habit_id, user_edited, start_at) VALUES ('stale2', 'Stale unedited', '2030-01-02T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'pending', ?, 0, '2030-01-02T09:00:00Z')",
    )
    .bind(habit_id)
    .execute(&pool)
    .await
    .unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({
            "sleep": "disabled"
        }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let get_req = auth_req(Method::GET, "/api/tasks/stale1");
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let get_req = auth_req(Method::GET, "/api/tasks/stale2");
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn habit_sync_uses_local_date_in_non_utc_timezone() {
    // Regression test: sync_habit_tasks used to compute the (habit_id, date)
    // key and the title date from the UTC date of start_at. In Asia/Tokyo
    // (UTC+9), a habit starting at 08:40 JST maps to 23:40 UTC on the
    // *previous* day, so a Mon-Fri habit generated a task titled
    // "habit (2026-07-05)" (Sunday, UTC date) instead of
    // "habit (2026-07-06)" (Monday, local date). This also caused the
    // weekday filter to appear broken and tasks to be skipped/duplicated
    // across day boundaries.
    let (state, pool) = setup().await;
    let app = build_router(state);

    // Set timezone to Asia/Tokyo.
    let req = auth_req_body(Method::PUT, "/api/settings", json!({ "tz": "Asia/Tokyo" }));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Insert a habit with by_day=[Mon-Fri] and start_time=08:40 JST.
    // 08:40 JST = 23:40 UTC (previous day), which is the exact case that
    // exposed the bug.
    let habit_id = sqlx::query_scalar::<_, String>(
        "INSERT INTO habits (id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed) VALUES ('habit-tz', '平日朝', '', '{\"freq\":\"daily\",\"interval\":1,\"by_day\":[{\"n\":null,\"weekday\":\"mon\"},{\"n\":null,\"weekday\":\"tue\"},{\"n\":null,\"weekday\":\"wed\"},{\"n\":null,\"weekday\":\"thu\"},{\"n\":null,\"weekday\":\"fri\"}],\"by_month\":[],\"by_month_day\":[],\"count\":null,\"exdates\":[]}', '08:40', '16:30', 470, 15, 0, 0, 0.2, 1, 0) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Trigger habit sync via schedule/generate.
    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({ "sleep": "disabled" }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // List generated tasks for this habit.
    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(!tasks.is_empty(), "habit should generate at least one task");

    // The title includes the date in parentheses. Before the fix, the date
    // in the title was the UTC date (1 day behind the JST date of start_at).
    // Verify that the title date matches the JST date of start_at.
    let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
    for t in &tasks {
        let title = t["title"].as_str().unwrap();
        let start_at = t["start_at"].as_str().unwrap();
        let ts: jiff::Timestamp = start_at.parse().unwrap();
        let jst_date = ts.to_zoned(tz.clone()).date().to_string();
        assert!(
            title.contains(&jst_date),
            "title '{}' should contain JST date '{}' (start_at={})",
            title,
            jst_date,
            start_at
        );
    }
}

#[tokio::test]
async fn task_create_rejects_negative_avg_minutes() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "bad",
            "end_at": "2026-07-06T18:00:00Z",
            "avg_minutes": -10,
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn task_create_rejects_negative_sigma_minutes() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "bad",
            "end_at": "2026-07-06T18:00:00Z",
            "avg_minutes": 30,
            "sigma_minutes": -5,
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_create_rejects_invalid_recurrence() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "bad",
            "recurrence": "not json",
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── Habit preview (#1033) ─────────────────────────────────────────────

#[tokio::test]
async fn habit_preview_daily_without_persisting() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/habits/preview",
        json!({
            "title": "preview habit",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_str(res.into_body()).await;
    let tasks: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
    assert!(!tasks.is_empty());
    assert_eq!(tasks[0]["title"], "preview habit");
    assert!(tasks[0]["start_at"].as_str().is_some());
    assert!(tasks[0]["end_at"].as_str().is_some());

    // No habit row is created.
    let list_req = auth_req(Method::GET, "/api/habits");
    let res = app.oneshot(list_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let habits: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(habits.is_empty());
}

#[tokio::test]
async fn habit_preview_period_window_uses_next_occurrence_deadline() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/habits/preview",
        json!({
            "title": "period preview",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "window_mode": "period",
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(!tasks.is_empty());
    let start_at = tasks[0]["start_at"].as_str().unwrap();
    let end_at = tasks[0]["end_at"].as_str().unwrap();
    assert!(end_at > start_at);
}

#[tokio::test]
async fn habit_preview_with_steps() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/habits/preview",
        json!({
            "title": "step preview",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 60,
            "steps": [
                {
                    "position": 0,
                    "title": "step A",
                    "start_time": "06:00",
                    "end_time": "06:30",
                    "avg_minutes": 30,
                },
                {
                    "position": 1,
                    "title": "step B",
                    "start_time": "06:30",
                    "end_time": "07:00",
                    "avg_minutes": 30,
                },
            ],
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(tasks.len() >= 2);
    let titles: Vec<&str> = tasks.iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"step A"));
    assert!(titles.contains(&"step B"));
}

#[tokio::test]
async fn habit_preview_rejects_invalid_recurrence() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/habits/preview",
        json!({
            "title": "bad",
            "recurrence": "not json",
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn settings_update_rejects_invalid_timezone() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(Method::PUT, "/api/settings", json!({ "tz": "Asia/Tokyoo" }));
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn settings_update_rejects_invalid_sleep_time() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::PUT,
        "/api/settings",
        json!({ "sleep_start": "25:00" }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn task_replace_rejects_negative_avg_minutes() {
    let (state, _) = setup().await;
    let app = build_router(state);
    // First create a valid task to replace.
    let create_req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "good",
            "end_at": "2026-07-06T18:00:00Z",
            "avg_minutes": 30,
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_id = body["id"].as_str().unwrap();

    let replace_req = auth_req_body(
        Method::PUT,
        &format!("/api/tasks/{task_id}"),
        json!({
            "title": "bad",
            "end_at": "2026-07-06T18:00:00Z",
            "avg_minutes": -10,
        }),
    );
    let res = app.oneshot(replace_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── Habit spans (#303) ────────────────────────────────────────────────

/// Create a daily habit and return its id.
async fn create_daily_habit(app: &axum::Router, title: &str) -> String {
    let req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": title,
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "abandonability": 0.1
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    body["id"].as_str().unwrap().to_string()
}

/// Trigger habit sync via schedule/generate and return the habit's tasks.
async fn sync_habit_tasks(app: &axum::Router, habit_id: &str) -> Vec<serde_json::Value> {
    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({ "sleep": "disabled" }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    serde_json::from_str(&body_str(res.into_body()).await).unwrap()
}

#[tokio::test]
async fn habit_scheduled_span_skips_occurrences_in_range() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "朝のランニング").await;

    // Pick a date 3 days from today as the span start, 5 days as the end.
    let today = jiff::Zoned::now().date();
    let span_start = today
        .checked_add(jiff::Span::new().days(3))
        .unwrap()
        .to_string();
    let span_end = today
        .checked_add(jiff::Span::new().days(5))
        .unwrap()
        .to_string();

    // Add the scheduled span before generating tasks.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": span_start, "end_date": span_end, "reason": "休暇" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let tasks = sync_habit_tasks(&app, &habit_id).await;
    assert!(
        !tasks.is_empty(),
        "habit should still generate tasks outside the span"
    );

    // No task title should contain a date within the span range.
    for t in &tasks {
        let title = t["title"].as_str().unwrap();
        for d in 3..=5 {
            let date = today
                .checked_add(jiff::Span::new().days(d))
                .unwrap()
                .to_string();
            assert!(
                !title.contains(&date),
                "task title '{title}' should not contain skipped date {date}"
            );
        }
    }
}

#[tokio::test]
async fn habit_scheduled_span_generates_only_in_range_for_disabled_habit() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "夜ジム").await;

    // Disable the habit.
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/habits/{habit_id}"),
        json!({ "active": false }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // No tasks should be generated while the habit is disabled and spanless.
    let tasks_before = sync_habit_tasks(&app, &habit_id).await;
    assert!(
        tasks_before.is_empty(),
        "disabled habit without spans should generate no tasks"
    );

    // Pick a date 3 days from today as the span start, 5 days as the end.
    let today = jiff::Zoned::now().date();
    let span_start = today
        .checked_add(jiff::Span::new().days(3))
        .unwrap()
        .to_string();
    let span_end = today
        .checked_add(jiff::Span::new().days(5))
        .unwrap()
        .to_string();

    // Add the scheduled span.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": span_start, "end_date": span_end, "reason": "集中ウィーク" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    let tasks = sync_habit_tasks(&app, &habit_id).await;
    assert!(
        !tasks.is_empty(),
        "disabled habit should generate tasks inside the span"
    );
    // The span covers three inclusive days and the habit is daily, so exactly
    // three tasks should be generated (and none outside the span).
    assert_eq!(tasks.len(), 3, "expected exactly 3 tasks for a 3-day span");

    // Every task title should contain a date within the span range.
    for t in &tasks {
        let title = t["title"].as_str().unwrap();
        let mut in_range = false;
        for d in 3..=5 {
            let date = today
                .checked_add(jiff::Span::new().days(d))
                .unwrap()
                .to_string();
            if title.contains(&date) {
                in_range = true;
                break;
            }
        }
        assert!(
            in_range,
            "task title '{title}' should be within the scheduled span range"
        );
    }
}

#[tokio::test]
async fn habit_scheduled_span_deletes_existing_pending_unedited_tasks() {
    let (state, pool) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "ジム").await;

    // Generate tasks first (no span yet). generate_schedule marks tasks as
    // 'scheduled', so reset the target task to 'pending' + unedited
    // afterwards to make it eligible for the sync cleanup loop.
    let tasks_before = sync_habit_tasks(&app, &habit_id).await;
    assert!(!tasks_before.is_empty());

    // Find a generated task's date to skip. Use the first task's date.
    let first_title = tasks_before[0]["title"].as_str().unwrap();
    // Title format: "ジム (YYYY-MM-DD)"
    let span_date = first_title
        .split('(')
        .nth(1)
        .map(|s| s.trim_end_matches(')').trim())
        .unwrap();
    let span_date = span_date.to_string();
    let first_id = tasks_before[0]["id"].as_str().unwrap().to_string();

    // Reset to pending + unedited so the cleanup loop can delete it.
    sqlx::query("UPDATE tasks SET status = 'pending', user_edited = 0 WHERE id = ?")
        .bind(&first_id)
        .execute(&pool)
        .await
        .unwrap();

    // Add a span covering that single date.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": span_date, "end_date": span_date }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // Re-sync; the skipped date's pending unedited task should be deleted.
    let tasks_after = sync_habit_tasks(&app, &habit_id).await;
    for t in &tasks_after {
        let title = t["title"].as_str().unwrap();
        assert!(
            !title.contains(&span_date),
            "task for skipped date {span_date} should have been deleted"
        );
    }
}

#[tokio::test]
async fn habit_scheduled_span_protects_edited_and_nonpending_tasks() {
    let (state, pool) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "読書").await;

    // Generate tasks.
    let tasks = sync_habit_tasks(&app, &habit_id).await;
    assert!(!tasks.is_empty());

    // Pick the first task and mark it user_edited + completed via direct SQL
    // so the cleanup loop must protect it.
    let first_id = tasks[0]["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE tasks SET user_edited = 1, status = 'completed' WHERE id = ?")
        .bind(&first_id)
        .execute(&pool)
        .await
        .unwrap();

    // Derive the date from the title to build a span covering it.
    let first_title = tasks[0]["title"].as_str().unwrap();
    let span_date = first_title
        .split('(')
        .nth(1)
        .map(|s| s.trim_end_matches(')').trim())
        .unwrap()
        .to_string();

    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": span_date, "end_date": span_date }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // Re-sync; the protected task must still exist.
    let _ = sync_habit_tasks(&app, &habit_id).await;
    let get_req = auth_req(Method::GET, &format!("/api/tasks/{first_id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "completed/edited task should be protected from span cleanup"
    );
}

#[tokio::test]
async fn habit_scheduled_span_rejects_reversed_dates() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "散歩").await;
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": "2026-08-07", "end_date": "2026-08-01" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_scheduled_span_rejects_bad_date_format() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "瞑想").await;
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": "2026/08/01", "end_date": "2026-08-07" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_scheduled_span_rejects_non_zero_padded_dates() {
    // Non-zero-padded dates like "2026-8-1" would pass numeric parsing
    // but break the lexicographic span-matching comparison against
    // jiff's zero-padded Date::to_string, so they must be rejected (#303).
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "ストレッチ").await;
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": "2026-8-1", "end_date": "2026-08-07" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // End date non-zero-padded should also fail.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": "2026-08-01", "end_date": "2026-8-7" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_scheduled_span_list_and_delete() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "日記").await;

    // Add a span.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": "2026-08-01", "end_date": "2026-08-07", "reason": "夏休み" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let span_body: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let span_id = span_body["id"].as_str().unwrap().to_string();

    // List spans.
    let req = auth_req(
        Method::GET,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let spans: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0]["id"].as_str().unwrap(), span_id);
    assert_eq!(spans[0]["reason"].as_str().unwrap(), "夏休み");

    // Delete the span.
    let req = auth_req(
        Method::DELETE,
        &format!("/api/habits/{habit_id}/scheduled-spans/{span_id}"),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // List should now be empty.
    let req = auth_req(
        Method::GET,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let spans: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(spans.is_empty());
}

#[tokio::test]
async fn habit_scheduled_span_list_all_endpoint() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let h1 = create_daily_habit(&app, "習慣A").await;
    let h2 = create_daily_habit(&app, "習慣B").await;

    // Add a span to each.
    for (hid, start) in [(&h1, "2026-09-01"), (&h2, "2026-10-01")] {
        let req = auth_req_body(
            Method::POST,
            &format!("/api/habits/{hid}/scheduled-spans"),
            json!({ "start_date": start, "end_date": start }),
        );
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
    }

    // GET /api/habits/spans must return both (and not be shadowed by
    // the /api/habits/{id} route).
    let req = auth_req(Method::GET, "/api/habits/scheduled-spans");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let spans: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(spans.len(), 2);
}

#[tokio::test]
async fn habit_delete_removes_its_scheduled_spans() {
    // Regression: deleting a habit must also delete its scheduled span rows so
    // they don't accumulate as orphans in list_all_habit_scheduled_spans (#303 / #503).
    // SQLite does not enable PRAGMA foreign_keys, so the ON DELETE
    // CASCADE in the schema does not fire — the cleanup must be explicit.
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "一時停止削除対象").await;

    // Add a span.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": "2026-08-01", "end_date": "2026-08-07" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // Confirm it shows up in list-all.
    let req = auth_req(Method::GET, "/api/habits/scheduled-spans");
    let res = app.clone().oneshot(req).await.unwrap();
    let spans: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(spans.len(), 1);

    // Delete the habit.
    let req = auth_req(Method::DELETE, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // list-all must now be empty — no orphaned span rows.
    let req = auth_req(Method::GET, "/api/habits/scheduled-spans");
    let res = app.clone().oneshot(req).await.unwrap();
    let spans: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(
        spans.is_empty(),
        "deleting a habit should remove its span rows, but found {spans:?}"
    );
}

// ── Habit steps (#95) ────────────────────────────────────────────────

#[tokio::test]
async fn habit_steps_replace_and_get() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "マルチステップ習慣").await;

    // GET detail initially has no steps.
    let req = auth_req(Method::GET, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let detail: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(detail["id"], habit_id);
    assert!(detail["steps"].as_array().unwrap().is_empty());

    // PUT two steps.
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "準備",
                "start_time": "06:00",
                "end_time": "06:15",
                "avg_minutes": 15
            },
            {
                "position": 1,
                "title": "実行",
                "start_time": "06:15",
                "end_time": "06:45",
                "avg_minutes": 30,
                "depends_on": []
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let steps: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["title"], "準備");
    assert_eq!(steps[1]["title"], "実行");
    let step0_id = steps[0]["id"].as_str().unwrap().to_string();
    let step1_id = steps[1]["id"].as_str().unwrap().to_string();

    // GET detail now shows the steps.
    let req = auth_req(Method::GET, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(req).await.unwrap();
    let detail: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let steps = detail["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["id"], step0_id);
    assert_eq!(steps[1]["id"], step1_id);

    // GET /habits/steps returns all steps for all habits.
    let req = auth_req(Method::GET, "/api/habits/steps");
    let res = app.clone().oneshot(req).await.unwrap();
    let all: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(all.len(), 2);

    // GET /habits/:id/steps returns just this habit's steps.
    let req = auth_req(Method::GET, &format!("/api/habits/{habit_id}/steps"));
    let res = app.clone().oneshot(req).await.unwrap();
    let steps: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(steps.len(), 2);

    // Replace with a single step (bulk replace semantics).
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "統合",
                "start_time": "06:00",
                "end_time": "06:30",
                "avg_minutes": 30
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let steps: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["title"], "統合");
}

#[tokio::test]
async fn habit_steps_reject_cycle() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "サイクル習慣").await;

    // Two steps that depend on each other → cycle.
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "id": "step-a",
                "position": 0,
                "title": "A",
                "start_time": "06:00",
                "end_time": "06:30",
                "avg_minutes": 30,
                "depends_on": ["step-b"]
            },
            {
                "id": "step-b",
                "position": 1,
                "title": "B",
                "start_time": "06:30",
                "end_time": "07:00",
                "avg_minutes": 30,
                "depends_on": ["step-a"]
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_steps_reject_unknown_dep() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "不明依存習慣").await;

    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "A",
                "start_time": "06:00",
                "end_time": "06:30",
                "avg_minutes": 30,
                "depends_on": ["nonexistent"]
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_steps_reject_bad_time() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "時間フォーマット習慣").await;

    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "A",
                "start_time": "25:00",
                "end_time": "06:30",
                "avg_minutes": 30
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_steps_reject_negative_avg() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "負の時間習慣").await;

    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "A",
                "start_time": "06:00",
                "end_time": "06:30",
                "avg_minutes": -5
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_delete_removes_its_steps() {
    // Regression: deleting a habit must also delete its step rows (#95).
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "ステップ削除対象").await;

    // Add steps.
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "A",
                "start_time": "06:00",
                "end_time": "06:30",
                "avg_minutes": 30
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Confirm steps exist in list-all.
    let req = auth_req(Method::GET, "/api/habits/steps");
    let res = app.clone().oneshot(req).await.unwrap();
    let all: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(all.len(), 1);

    // Delete the habit.
    let req = auth_req(Method::DELETE, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // list-all must now be empty.
    let req = auth_req(Method::GET, "/api/habits/steps");
    let res = app.clone().oneshot(req).await.unwrap();
    let all: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(
        all.is_empty(),
        "deleting a habit should remove its step rows, but found {all:?}"
    );
}

#[tokio::test]
async fn habit_steps_sync_generates_one_task_per_step() {
    // #95: a multi-step habit should generate one task per step per
    // occurrence, with each task carrying the corresponding habit_step_id.
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "ステップ同期習慣").await;

    // Add two steps.
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "準備",
                "start_time": "06:00",
                "end_time": "06:15",
                "avg_minutes": 15
            },
            {
                "position": 1,
                "title": "実行",
                "start_time": "06:15",
                "end_time": "06:45",
                "avg_minutes": 30
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let steps: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let step0_id = steps[0]["id"].as_str().unwrap();
    let step1_id = steps[1]["id"].as_str().unwrap();

    // Sync via schedule/generate.
    let tasks = sync_habit_tasks(&app, &habit_id).await;

    // Should have at least 2 tasks (one per step) for the first occurrence.
    // (The exact count depends on the planning horizon, but each occurrence
    // produces one task per step, so the total is a multiple of 2.)
    assert!(
        tasks.len() >= 2,
        "expected at least 2 tasks for a 2-step habit, got {}",
        tasks.len()
    );
    assert_eq!(tasks.len() % 2, 0, "task count should be a multiple of 2");

    // Each task should have a non-null habit_step_id, and both step ids
    // should appear among the generated tasks.
    let step_ids: Vec<&str> = tasks
        .iter()
        .filter_map(|t| t["habit_step_id"].as_str())
        .collect();
    assert!(step_ids.contains(&step0_id), "step 0 task not found");
    assert!(step_ids.contains(&step1_id), "step 1 task not found");
    // Every task for this habit should have a habit_step_id.
    for t in &tasks {
        assert!(
            t["habit_step_id"].as_str().is_some(),
            "task {:?} missing habit_step_id",
            t["id"]
        );
    }
}

#[tokio::test]
async fn habit_steps_replace_on_simple_habit_cleans_up_original_tasks() {
    // #505: a habit that was originally simple (no steps) and already had
    // generated tasks must clean up the original simple tasks when steps are
    // added and the schedule is regenerated.
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "シンプル→ステップ").await;

    // Generate tasks as a simple habit; generate_schedule marks them 'scheduled'.
    let tasks_before = sync_habit_tasks(&app, &habit_id).await;
    assert!(
        !tasks_before.is_empty(),
        "simple habit should generate tasks"
    );
    assert!(
        tasks_before.iter().all(|t| t["habit_step_id"].is_null()),
        "simple tasks should not have a habit_step_id"
    );

    // Add steps to the previously simple habit.
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            {
                "position": 0,
                "title": "準備",
                "start_time": "06:00",
                "end_time": "06:15",
                "avg_minutes": 15
            },
            {
                "position": 1,
                "title": "実行",
                "start_time": "06:15",
                "end_time": "06:45",
                "avg_minutes": 30,
                "depends_on": []
            }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Re-generate the schedule. The original simple tasks should be deleted
    // and replaced by step tasks.
    let tasks_after = sync_habit_tasks(&app, &habit_id).await;
    let simple_tasks: Vec<&serde_json::Value> = tasks_after
        .iter()
        .filter(|t| t["habit_step_id"].is_null())
        .collect();
    assert!(
        simple_tasks.is_empty(),
        "original simple tasks should be cleaned up after adding steps: {simple_tasks:?}"
    );
    assert!(
        tasks_after
            .iter()
            .all(|t| t["habit_step_id"].as_str().is_some()),
        "all tasks after cleanup should carry a habit_step_id"
    );
}

/// Create a weekly habit with `window_mode = "period"` and return its id.
async fn create_weekly_period_habit(app: &axum::Router, title: &str) -> String {
    let req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": title,
            "recurrence": r#"{"freq":"weekly","interval":1,"by_day":[{"n":null,"weekday":"mon"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "09:00",
            "end_time": "10:00",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "abandonability": 0.1,
            "window_mode": "period"
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let raw = body_str(res.into_body()).await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {raw}");
    let body: serde_json::Value = serde_json::from_str(&raw).unwrap();
    body["id"].as_str().unwrap().to_string()
}
fn iso_to_ts(iso: &str) -> jiff::Timestamp {
    iso.parse().unwrap()
}

#[tokio::test]
async fn habit_window_mode_validation_rejects_unknown() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "bad window",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "06:00",
            "end_time": "07:00",
            "avg_minutes": 30,
            "window_mode": "weekly"
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn habit_window_mode_defaults_to_day() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "デフォルト").await;
    let req = auth_req(Method::GET, &format!("/api/habits/{habit_id}"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    // HabitDetail uses #[serde(flatten)] so habit fields are top-level.
    assert_eq!(body["window_mode"].as_str().unwrap(), "day");
}

#[tokio::test]
async fn habit_period_window_spans_to_next_occurrence() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_weekly_period_habit(&app, "週次period").await;

    let tasks = sync_habit_tasks(&app, &habit_id).await;
    assert!(!tasks.is_empty(), "period habit should generate tasks");

    // For a weekly habit in period mode, the deadline of the first occurrence
    // is the start of the next occurrence (≈7 days later). Verify the window
    // spans multiple days rather than a single day.
    let first = &tasks[0];
    let start = iso_to_ts(first["start_at"].as_str().unwrap());
    let end = iso_to_ts(first["end_at"].as_str().unwrap());
    let span_secs = (end.as_second() - start.as_second()) as i64;
    // 7 days = 604800 secs; allow a tolerance because the next occurrence
    // start is at 09:00 next week while the (clamped) start may be today 00:00.
    assert!(
        span_secs >= 6 * 24 * 3600,
        "period window should span ~7 days, got {} secs ({})",
        span_secs,
        first
    );
}

#[tokio::test]
async fn habit_period_clamps_today_start_to_midnight() {
    let (state, _) = setup().await;
    let app = build_router(state);
    // Use a daily period habit with a late start_time (23:59) so that
    // today's occurrence hasn't passed yet regardless of when the test
    // runs, ensuring the window-start clamping logic is exercised.
    let req = auth_req_body(
        Method::POST,
        "/api/habits",
        json!({
            "title": "日次period",
            "recurrence": r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            "start_time": "23:59",
            "end_time": "23:59",
            "avg_minutes": 30,
            "sigma_minutes": 5,
            "abandonability": 0.1,
            "window_mode": "period"
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let raw = body_str(res.into_body()).await;
    assert_eq!(status, StatusCode::CREATED, "create failed: {raw}");
    let body: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let habit_id = body["id"].as_str().unwrap().to_string();

    let tasks = sync_habit_tasks(&app, &habit_id).await;
    assert!(!tasks.is_empty());

    // The first occurrence's window start should be clamped to today's 00:00
    // (the occurrence's 23:59 start time is ignored for the window start).
    // Sort by start_at to find today's occurrence reliably — list_tasks
    // returns ORDER BY created_at DESC, which is unstable when multiple
    // tasks share the same created_at second (#374).
    let mut sorted = tasks.clone();
    sorted.sort_by(|a, b| {
        a["start_at"]
            .as_str()
            .unwrap_or("")
            .cmp(b["start_at"].as_str().unwrap_or(""))
    });
    let first = &sorted[0];
    let start = iso_to_ts(first["start_at"].as_str().unwrap());
    let zdt = start.to_zoned(jiff::tz::TimeZone::UTC);
    assert_eq!(
        zdt.hour(),
        0,
        "period window start for today's occurrence should be clamped to 00:00, got {first}"
    );
    assert_eq!(zdt.minute(), 0);
}

#[tokio::test]
async fn habit_period_scheduled_span_skips_occurrence() {
    let (state, pool) = setup().await;
    let app = build_router(state);
    let habit_id = create_weekly_period_habit(&app, "週次period休止").await;

    // First sync to materialise tasks and discover the first occurrence date.
    let tasks = sync_habit_tasks(&app, &habit_id).await;
    assert!(!tasks.is_empty());
    let first_title = tasks[0]["title"].as_str().unwrap();
    let first_date = first_title
        .split('(')
        .nth(1)
        .map(|s| s.trim_end_matches(')').trim())
        .unwrap()
        .to_string();
    let first_id = tasks[0]["id"].as_str().unwrap().to_string();

    // generate_schedule marks tasks as 'scheduled', so reset to 'pending' +
    // unedited to make the task eligible for the sync cleanup loop.
    sqlx::query("UPDATE tasks SET status = 'pending', user_edited = 0 WHERE id = ?")
        .bind(&first_id)
        .execute(&pool)
        .await
        .unwrap();

    // Add a span covering the first occurrence date.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/habits/{habit_id}/scheduled-spans"),
        json!({ "start_date": first_date, "end_date": first_date }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    // Re-sync; no task should carry the skipped date in its title.
    let tasks = sync_habit_tasks(&app, &habit_id).await;
    for t in &tasks {
        let title = t["title"].as_str().unwrap();
        assert!(
            !title.contains(&format!("({first_date})")),
            "skipped occurrence should not generate a task: {title}"
        );
    }
}

#[tokio::test]
async fn habit_period_update_changes_window_mode() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "day→period").await;

    // Update to period mode.
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/habits/{habit_id}"),
        json!({ "window_mode": "period" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["window_mode"].as_str().unwrap(), "period");

    // Update back to day mode.
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/habits/{habit_id}"),
        json!({ "window_mode": "day" }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["window_mode"].as_str().unwrap(), "day");
}

// ── Dependency analysis (#355) ─────────────────────────────────────────

/// Helper: create a bare task and return its id.
async fn create_task_simple(app: &axum::Router, title: &str) -> String {
    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": title,
            "end_at": "2026-07-08T18:00:00Z",
            "avg_minutes": 30,
            "depends": [],
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn task_dependency_analysis_detects_redundant_edge() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let t1 = create_task_simple(&app, "資料集め").await;
    let t2 = create_task_simple(&app, "下書き").await;
    let t3 = create_task_simple(&app, "レポート提出").await;

    // t2 depends on t1; t3 depends on t2 and t1 (t3→t1 redundant via t3→t2→t1).
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{t2}"),
        json!({ "depends": [t1] }),
    );
    app.clone().oneshot(req).await.unwrap();
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{t3}"),
        json!({ "depends": [t2, t1] }),
    );
    app.clone().oneshot(req).await.unwrap();

    let req = auth_req(Method::GET, "/api/tasks/dependency-analysis");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let redundant = body["redundant"].as_array().unwrap();
    assert_eq!(redundant.len(), 1);
    assert_eq!(redundant[0]["from"], t3);
    assert_eq!(redundant[0]["to"], t1);
    let via = redundant[0]["via"].as_array().unwrap();
    assert_eq!(via.len(), 3);
    assert_eq!(via[0]["id"], t3);
    assert_eq!(via[2]["id"], t1);

    // Remove the redundant edge (t3→t1) → analysis becomes empty.
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{t3}"),
        json!({ "depends": [t2] }),
    );
    app.clone().oneshot(req).await.unwrap();
    let req = auth_req(Method::GET, "/api/tasks/dependency-analysis");
    let res = app.clone().oneshot(req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["redundant"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_dependency_analysis_excludes_done_tasks() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let t1 = create_task_simple(&app, "資料集め").await;
    let t2 = create_task_simple(&app, "下書き").await;
    let t3 = create_task_simple(&app, "レポート提出").await;

    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{t2}"),
        json!({ "depends": [t1] }),
    );
    app.clone().oneshot(req).await.unwrap();
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{t3}"),
        json!({ "depends": [t2, t1] }),
    );
    app.clone().oneshot(req).await.unwrap();

    // Mark t1 completed — the redundant edge t3→t1 should disappear
    // because t1 is excluded from analysis.
    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{t1}"),
        json!({ "status": "completed" }),
    );
    app.clone().oneshot(req).await.unwrap();

    let req = auth_req(Method::GET, "/api/tasks/dependency-analysis");
    let res = app.clone().oneshot(req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["redundant"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn task_dependency_analysis_empty_when_no_deps() {
    let (state, _) = setup().await;
    let app = build_router(state);
    create_task_simple(&app, "単独タスク").await;

    let req = auth_req(Method::GET, "/api/tasks/dependency-analysis");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["redundant"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn habit_step_dependency_analysis_detects_redundant_edge() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "ステップ依存習慣").await;

    // 3 steps: s1 (no deps), s2 depends on s1, s3 depends on s2 and s1.
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            { "id": "s1", "position": 0, "title": "資料集め", "start_time": "06:00", "end_time": "06:30", "avg_minutes": 30 },
            { "id": "s2", "position": 1, "title": "下書き", "start_time": "06:30", "end_time": "07:00", "avg_minutes": 30, "depends_on": ["s1"] },
            { "id": "s3", "position": 2, "title": "提出", "start_time": "07:00", "end_time": "07:30", "avg_minutes": 30, "depends_on": ["s2", "s1"] }
        ]),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let req = auth_req(
        Method::GET,
        &format!("/api/habits/{habit_id}/steps/dependency-analysis"),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let redundant = body["redundant"].as_array().unwrap();
    assert_eq!(redundant.len(), 1);
    assert_eq!(redundant[0]["from"], "s3");
    assert_eq!(redundant[0]["to"], "s1");
    let via = redundant[0]["via"].as_array().unwrap();
    assert_eq!(via.len(), 3);
    assert_eq!(via[0]["id"], "s3");
    assert_eq!(via[2]["id"], "s1");

    // Remove the redundant edge (s3→s1) → analysis becomes empty.
    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            { "id": "s1", "position": 0, "title": "資料集め", "start_time": "06:00", "end_time": "06:30", "avg_minutes": 30 },
            { "id": "s2", "position": 1, "title": "下書き", "start_time": "06:30", "end_time": "07:00", "avg_minutes": 30, "depends_on": ["s1"] },
            { "id": "s3", "position": 2, "title": "提出", "start_time": "07:00", "end_time": "07:30", "avg_minutes": 30, "depends_on": ["s2"] }
        ]),
    );
    app.clone().oneshot(req).await.unwrap();
    let req = auth_req(
        Method::GET,
        &format!("/api/habits/{habit_id}/steps/dependency-analysis"),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["redundant"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn habit_step_dependency_analysis_empty_when_no_deps() {
    let (state, _) = setup().await;
    let app = build_router(state);
    let habit_id = create_daily_habit(&app, "無依存習慣").await;

    let req = auth_req_body(
        Method::PUT,
        &format!("/api/habits/{habit_id}/steps"),
        json!([
            { "id": "s1", "position": 0, "title": "A", "start_time": "06:00", "end_time": "06:30", "avg_minutes": 30 },
            { "id": "s2", "position": 1, "title": "B", "start_time": "06:30", "end_time": "07:00", "avg_minutes": 30 }
        ]),
    );
    app.clone().oneshot(req).await.unwrap();

    let req = auth_req(
        Method::GET,
        &format!("/api/habits/{habit_id}/steps/dependency-analysis"),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["redundant"].as_array().unwrap().is_empty());
}

// ── Schedule regression tests (#582) ─────────────────────────────────

#[tokio::test]
async fn generate_schedule_allows_task_depending_on_done_task() {
    // A pending task whose dependency is already completed should still be
    // schedulable; the completed dependency is treated as satisfied.
    let (state, pool) = setup().await;
    let app = build_router(state);

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('done', 'done-task', '2030-01-01T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'completed')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status) VALUES ('pending', 'pending-task', '2030-01-01T18:00:00Z', 30, 0, '[\"done\"]', 0, 0, 0.5, 'pending')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let gen_req = auth_req_body(
        Method::POST,
        "/api/schedule/generate",
        json!({ "sleep": "disabled" }),
    );
    let res = app.clone().oneshot(gen_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let schedule: Vec<serde_json::Value> =
        serde_json::from_str(body["schedule"].as_str().unwrap()).unwrap();
    let ids: Vec<&str> = schedule
        .iter()
        .map(|e| e["task_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"pending"), "pending task should be scheduled");
    assert!(
        !ids.contains(&"done"),
        "completed dependency should not be in schedule"
    );
}

#[tokio::test]
async fn generate_schedule_does_not_duplicate_habit_entries() {
    // Regenerating a schedule should not create duplicate entries for the same
    // habit task (#582).
    let (state, _) = setup().await;
    let app = build_router(state);

    let habit_id = create_daily_habit(&app, "朝のランニング").await;

    let gen_req = || {
        auth_req_body(
            Method::POST,
            "/api/schedule/generate",
            json!({ "sleep": "disabled" }),
        )
    };
    let res = app.clone().oneshot(gen_req()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app.clone().oneshot(gen_req()).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let schedule: Vec<serde_json::Value> =
        serde_json::from_str(body["schedule"].as_str().unwrap()).unwrap();
    let ids: Vec<&str> = schedule
        .iter()
        .map(|e| e["task_id"].as_str().unwrap())
        .collect();
    let unique: std::collections::HashSet<String> = ids.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        ids.len(),
        unique.len(),
        "schedule should not contain duplicate task ids: {ids:?}"
    );

    // Sanity check: habit tasks are actually present in the schedule.
    let list_req = auth_req(Method::GET, &format!("/api/tasks?habit_id={habit_id}"));
    let res = app.clone().oneshot(list_req).await.unwrap();
    let tasks: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let task_ids: std::collections::HashSet<String> = tasks
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect();
    let overlap = unique.intersection(&task_ids).count();
    assert!(overlap > 0, "habit tasks should appear in the schedule");
}

#[tokio::test]
async fn delete_all_gcal_events_rejects_unconfigured_settings() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req(Method::POST, "/api/sync/delete-all");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(
        body["message"].as_str().unwrap().contains("client_id"),
        "error should mention missing client_id: {body:?}"
    );
}

#[tokio::test]
async fn delete_all_gcal_events_rejects_missing_client_secret() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let settings_req = auth_req_body(
        Method::PUT,
        "/api/sync/settings",
        json!({
            "client_id": "fake-client-id",
            "refresh_token": "fake-refresh-token"
        }),
    );
    app.clone().oneshot(settings_req).await.unwrap();

    let req = auth_req(Method::POST, "/api/sync/delete-all");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(
        body["message"].as_str().unwrap().contains("client_secret"),
        "error should mention missing client_secret: {body:?}"
    );
}

#[tokio::test]
async fn delete_all_gcal_events_rejects_missing_refresh_token() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let settings_req = auth_req_body(
        Method::PUT,
        "/api/sync/settings",
        json!({
            "client_id": "fake-client-id",
            "client_secret": "fake-secret"
        }),
    );
    app.clone().oneshot(settings_req).await.unwrap();

    let req = auth_req(Method::POST, "/api/sync/delete-all");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(
        body["message"].as_str().unwrap().contains("refresh token"),
        "error should mention missing refresh_token: {body:?}"
    );
}

#[tokio::test]
async fn delete_all_gcal_events_returns_zero_when_no_mappings() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let settings_req = auth_req_body(
        Method::PUT,
        "/api/sync/settings",
        json!({
            "client_id": "fake-client-id",
            "client_secret": "fake-secret",
            "refresh_token": "fake-refresh-token"
        }),
    );
    app.clone().oneshot(settings_req).await.unwrap();

    let req = auth_req(Method::POST, "/api/sync/delete-all");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["deleted"], 0);
    assert!(body["failed"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn list_tasks_overdue_status_returns_unfinished_past_deadline() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let overdue = create_task_simple(&app, "overdue task").await;
    let completed = create_task_simple(&app, "completed overdue").await;
    let completed_patch = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{completed}"),
        json!({"status": "completed"}),
    );
    let res = app.clone().oneshot(completed_patch).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let future_req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "future task",
            "end_at": "2030-01-01T00:00:00Z",
            "avg_minutes": 30,
            "depends": [],
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(future_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let future = body["id"].as_str().unwrap().to_string();

    let req = auth_req(Method::GET, "/api/tasks?status=overdue");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let ids: Vec<&str> = list.iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(ids, vec![overdue.as_str()]);

    let req = auth_req(Method::GET, "/api/tasks?no_overdue=true");
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let ids: std::collections::HashSet<&str> =
        list.iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(future.as_str()));
    assert!(ids.contains(completed.as_str()));
}

#[tokio::test]
async fn memory_crud_and_search() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create_req = auth_req_body(
        Method::POST,
        "/api/memory",
        json!({
            "kind": "proper_noun",
            "key": "研究室",
            "content": "大学の研究室"
        }),
    );
    let res = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let row: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = row["id"].as_str().unwrap().to_owned();
    assert_eq!(row["key"], "研究室");

    let get_req = auth_req(Method::GET, &format!("/api/memory/{id}"));
    let res = app.clone().oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let got: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(got["content"], "大学の研究室");

    let update_req = auth_req_body(
        Method::PATCH,
        &format!("/api/memory/{id}"),
        json!({
            "observed_revision": got["revision"],
            "content": "大学の研究室（更新）"
        }),
    );
    let res = app.clone().oneshot(update_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let updated: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(updated["content"], "大学の研究室（更新）");

    let search_req = auth_req(Method::GET, "/api/memory/search?q=大学&limit=10");
    let res = app.clone().oneshot(search_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let found: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["id"], id);

    let rev = updated["revision"].as_i64().unwrap();
    let delete_req = auth_req(
        Method::DELETE,
        &format!("/api/memory/{id}?observed_revision={rev}"),
    );
    let res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let get_req = auth_req(Method::GET, &format!("/api/memory/{id}"));
    let res = app.oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn find_similar_tasks_orders_completed_tasks() {
    let (state, _) = setup().await;
    let app = build_router(state);

    // Create and complete two tasks with distinct titles.
    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "数学の演習問題",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let math_id = body["id"].as_str().unwrap().to_owned();

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "読書",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let _res = app.clone().oneshot(create).await.unwrap();

    // Complete the math task.
    let patch = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{math_id}"),
        json!({ "status": "completed" }),
    );
    let res = app.clone().oneshot(patch).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let req = auth_req(Method::GET, "/api/tasks/similar?q=数学&limit=5");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let similar: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(!similar.is_empty());
    assert_eq!(similar[0]["title"], "数学の演習問題");
    // similarity now carries the real Dice score, not a hardcoded label (#942).
    let sim = similar[0]["similarity"].as_str().unwrap();
    assert!(sim.starts_with("dice:"), "unexpected similarity: {sim}");
    assert!(sim["dice:".len()..].parse::<f64>().is_ok());
}

#[tokio::test]
async fn find_similar_tasks_recall_bigram_only_match() {
    let (state, _) = setup().await;
    let app = build_router(state);

    // A reordered title shares bigrams with the query but contains no query
    // substring; the bigram-OR pre-filter must not drop it (#942).
    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "演習数学",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = body["id"].as_str().unwrap().to_owned();

    let patch = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{id}"),
        json!({ "status": "completed" }),
    );
    let res = app.clone().oneshot(patch).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let req = auth_req(Method::GET, "/api/tasks/similar?q=数学演習&limit=5");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let similar: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(
        similar.iter().any(|r| r["title"] == "演習数学"),
        "bigram-only match was dropped by the pre-filter: {similar:?}"
    );
}

#[tokio::test]
async fn find_similar_tasks_returns_old_completed_match() {
    let (state, _) = setup().await;
    let app = build_router(state);

    // Complete the matching task first, then add many newer completed tasks with
    // unrelated titles. The old match must still surface (no recency-only cap,
    // #942).
    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "线性代数のレポート",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let old_id = body["id"].as_str().unwrap().to_owned();
    let patch = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{old_id}"),
        json!({ "status": "completed" }),
    );
    assert_eq!(
        app.clone().oneshot(patch).await.unwrap().status(),
        StatusCode::OK
    );

    for i in 0..20 {
        let create = auth_req_body(
            Method::POST,
            "/api/tasks",
            json!({
                "title": format!("無関係なタスク{i}"),
                "end_at": "2026-06-05T18:00:00+09:00",
                "avg_minutes": 10,
                "depends": [],
                "parallelizable": false,
                "allows_parallel": false,
                "abandonability": 0.5
            }),
        );
        let res = app.clone().oneshot(create).await.unwrap();
        let body: serde_json::Value =
            serde_json::from_str(&body_str(res.into_body()).await).unwrap();
        let tid = body["id"].as_str().unwrap().to_owned();
        let patch = auth_req_body(
            Method::PATCH,
            &format!("/api/tasks/{tid}"),
            json!({ "status": "completed" }),
        );
        assert_eq!(
            app.clone().oneshot(patch).await.unwrap().status(),
            StatusCode::OK
        );
    }

    let req = auth_req(Method::GET, "/api/tasks/similar?q=线性代数&limit=5");
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let similar: Vec<serde_json::Value> =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(
        similar.iter().any(|r| r["title"] == "线性代数のレポート"),
        "old completed match was not returned: {similar:?}"
    );
}

#[tokio::test]
async fn create_task_rejects_unnormalizable_title() {
    let (state, _) = setup().await;
    let app = build_router(state);

    // A control-character-only title cannot be NFKC-normalized for similar-task
    // search and must be rejected at the boundary (#942).
    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "\u{0001}\u{0002}",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // An empty title is likewise rejected.
    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "   ",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn replace_task_rejects_unnormalizable_title() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "有効なタイトル",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = body["id"].as_str().unwrap().to_owned();

    // Replacing with an unnormalizable title is rejected, so the stored
    // normalized_title is never silently nulled out (#942).
    let put = auth_req_body(
        Method::PUT,
        &format!("/api/tasks/{id}"),
        json!({
            "title": "\u{0001}\u{0002}",
            "end_at": "2026-06-05T18:00:00+09:00",
            "avg_minutes": 30,
            "depends": [],
            "parallelizable": false,
            "allows_parallel": false,
            "abandonability": 0.5
        }),
    );
    let res = app.oneshot(put).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn progress_lifecycle() {
    let (state, _pool) = setup().await;
    let app = build_router(state);

    // Create a quantitative task.
    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "study",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "quantity_unit": "pages"
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    // Start work.
    let req = auth_req(Method::POST, &format!("/api/tasks/{id}/work/start"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let started: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(started["status"], "in_progress");

    // Record progress.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/progress"),
        json!({"quantity_done": 4}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let progress: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(progress["task"]["quantity_done"], 4);
    assert_eq!(progress["event"]["delta_quantity"], 4);
    assert!(!progress["event"]["id"].as_str().unwrap().is_empty());

    // Idempotent progress call with the same value returns a null event.
    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/progress"),
        json!({"quantity_done": 4}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let no_op: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(no_op["event"].is_null());

    // Get progress.
    let req = auth_req(Method::GET, &format!("/api/tasks/{id}/progress"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let detail: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(detail["events"].as_array().unwrap().len(), 1);
    assert!(detail["open_session"].is_object());

    // Complete the task.
    let req = auth_req(Method::POST, &format!("/api/tasks/{id}/work/complete"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let completed: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(completed["status"], "completed");
    assert_eq!(completed["quantity_done"], 10);
}

#[tokio::test]
async fn progress_and_pause_cannot_share_operation_id() {
    let (state, _pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "study",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "quantity_unit": "pages"
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    let req = auth_req(Method::POST, &format!("/api/tasks/{id}/work/start"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Reusing the same operationId for a different progress endpoint must fail
    // with CONFLICT, because the idempotency key is bound to the request hash.
    let operation_id = "shared-op-1";
    let req = auth_req_body_idempotency(
        Method::POST,
        &format!("/api/tasks/{id}/progress"),
        json!({"quantity_done": 4}),
        operation_id,
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let req = auth_req_body_idempotency(
        Method::POST,
        &format!("/api/tasks/{id}/work/pause"),
        json!({}),
        operation_id,
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Using a fresh operationId for pause succeeds.
    let req = auth_req_body_idempotency(
        Method::POST,
        &format!("/api/tasks/{id}/work/pause"),
        json!({}),
        "pause-op-1",
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn progress_status_validation() {
    let (state, _pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "done",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 5
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    // Complete it first.
    let req = auth_req(Method::POST, &format!("/api/tasks/{id}/work/complete"));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Attempting to start/record/complete/split a completed task fails.
    for uri in [
        format!("/api/tasks/{id}/work/start"),
        format!("/api/tasks/{id}/work/pause"),
        format!("/api/tasks/{id}/work/complete"),
    ] {
        let req = auth_req(Method::POST, &uri);
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{uri} should fail");
    }
    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/progress"),
        json!({"quantity_done": 1}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/split"),
        json!({"retained_quantity": 2}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn split_task_works() {
    let (state, _pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "split-me",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "quantity_done": 3
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/split"),
        json!({"retained_quantity": 4}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let split: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(split["original"]["quantity_total"], 4);
    assert_eq!(split["original"]["quantity_done"], 3);
    assert_eq!(split["remainder"]["quantity_total"], 6);
    assert_eq!(split["remainder"]["quantity_done"], 0);
    assert_eq!(split["remainder"]["status"], "pending");
}

#[tokio::test]
async fn split_rejects_retained_less_than_done() {
    let (state, _pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "split-done",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "quantity_done": 4
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/split"),
        json!({"retained_quantity": 2}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn task_create_normalizes_zero_quantity_total_to_null() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "zero-total",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 0
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(body["quantity_total"].is_null());
    assert_eq!(body["quantity_done"], 0);
}

#[tokio::test]
async fn task_create_normalizes_zero_original_quantity_total_to_null() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let req = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "zero-original",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "original_quantity_total": 0
        }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(body["quantity_total"], 10);
    assert!(body["original_quantity_total"].is_null());
}

#[tokio::test]
async fn split_rejects_zero_quantity_total() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "split-zero",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 0
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();
    assert!(task["quantity_total"].is_null());

    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/split"),
        json!({"retained_quantity": 1}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn split_uses_total_when_original_quantity_total_is_zero() {
    let (state, _) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "split-zero-original",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "original_quantity_total": 0
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();
    assert!(task["original_quantity_total"].is_null());

    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/split"),
        json!({"retained_quantity": 4}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let split: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(split["original"]["quantity_total"], 4);
    assert_eq!(split["original"]["quantity_done"], 0);
    assert_eq!(split["original"]["original_quantity_total"], 10);
    assert_eq!(split["remainder"]["quantity_total"], 6);
    assert_eq!(split["remainder"]["quantity_done"], 0);
    assert_eq!(split["remainder"]["original_quantity_total"], 10);
}

#[tokio::test]
async fn progress_active_minutes_are_incremental() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "progressive",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    let now = jiff::Timestamp::now();
    let hour_ago = now
        .checked_sub(jiff::SignedDuration::from_hours(1))
        .unwrap()
        .to_string();
    let seconds_ago = now
        .checked_sub(jiff::SignedDuration::from_secs(5))
        .unwrap()
        .to_string();

    sqlx::query("INSERT INTO task_work_sessions (id, task_id, started_at) VALUES (?, ?, ?)")
        .bind(uuid::Uuid::now_v7().to_string())
        .bind(id)
        .bind(hour_ago)
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO progress_events (id, task_id, at, quantity_done, delta_quantity, active_minutes, note) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(id)
    .bind(seconds_ago)
    .bind(3i64)
    .bind(3i64)
    .bind(60i64)
    .bind(None::<String>)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("UPDATE tasks SET quantity_done = ? WHERE id = ?")
        .bind(3i64)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/progress"),
        json!({"quantity_done": 5}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let result: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(result["event"]["delta_quantity"], 2);
    let active_minutes = result["event"]["active_minutes"].as_i64().unwrap();
    assert!(
        active_minutes < 60,
        "active_minutes should be incremental ({active_minutes})"
    );
}

#[tokio::test]
async fn update_task_completed_at_follows_status() {
    let (state, _pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "status-cycle",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap();

    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{id}"),
        json!({"status": "completed"}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let completed: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(completed["status"], "completed");
    assert!(!completed["completed_at"].is_null());

    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{id}"),
        json!({"title": "renamed"}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let renamed: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(renamed["title"], "renamed");
    assert!(!renamed["completed_at"].is_null());

    let req = auth_req_body(
        Method::PATCH,
        &format!("/api/tasks/{id}"),
        json!({"status": "pending"}),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let pending: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(pending["status"], "pending");
    assert!(pending["completed_at"].is_null());
}

#[tokio::test]
async fn estimate_habit_from_completed_task_actuals() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    sqlx::query(
        "INSERT INTO habits (id, title, recurrence, start_time, end_time, avg_minutes, sigma_minutes, fixed) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("habit-1")
    .bind("Morning run")
    .bind(r#"{"rrule":"FREQ=DAILY"}"#)
    .bind("07:00")
    .bind("08:00")
    .bind(60i64)
    .bind(10i64)
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, status, habit_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("task-1")
    .bind("Morning run instance")
    .bind("2026-07-22T08:00:00Z")
    .bind(60i64)
    .bind(10i64)
    .bind("completed")
    .bind("habit-1")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO task_work_sessions (id, task_id, started_at, ended_at) VALUES (?, ?, ?, ?)",
    )
    .bind("ws-1")
    .bind("task-1")
    .bind("2026-07-22T07:00:00Z")
    .bind("2026-07-22T07:50:00Z")
    .execute(&pool)
    .await
    .unwrap();

    let req = auth_req_body(
        Method::POST,
        "/api/habits/habit-1/estimate",
        json!({ "apply": true }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let result: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(result["sample_count"], 1);
    assert_eq!(result["avg_minutes"], 50);
    assert_eq!(result["sigma_minutes"], 0);
    assert!(result["steps"].as_array().unwrap().is_empty());
    assert!(result["applied"].as_bool().unwrap());
    assert!(result["habit"].is_object());
    assert_eq!(result["habit"]["avg_minutes"], 50);
}

#[tokio::test]
async fn estimate_habit_detects_outliers_when_enabled() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    sqlx::query(
        "INSERT INTO habits (id, title, recurrence, start_time, end_time, avg_minutes, sigma_minutes, fixed) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("habit-2")
    .bind("Reading")
    .bind(r#"{"freq":"Daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"exdates":[]}"#)
    .bind("20:00")
    .bind("21:00")
    .bind(60i64)
    .bind(10i64)
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();

    for (task, started, ended) in [
        ("task-a", "2026-07-20T20:00:00Z", "2026-07-20T20:30:00Z"),
        ("task-b", "2026-07-21T20:00:00Z", "2026-07-21T20:30:00Z"),
        ("task-c", "2026-07-22T20:00:00Z", "2026-07-22T20:30:00Z"),
        ("task-d", "2026-07-23T20:00:00Z", "2026-07-23T23:30:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, status, habit_id) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(task)
        .bind(task)
        .bind("2026-07-22T08:00:00Z")
        .bind(60i64)
        .bind(10i64)
        .bind("completed")
        .bind("habit-2")
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO task_work_sessions (id, task_id, started_at, ended_at) VALUES (?, ?, ?, ?)",
        )
        .bind(format!("ws-{}", task))
        .bind(task)
        .bind(started)
        .bind(ended)
        .execute(&pool)
        .await
        .unwrap();
    }

    let req = auth_req_body(
        Method::POST,
        "/api/habits/habit-2/estimate",
        json!({ "detect_outliers": true, "apply": false }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let result: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(result["sample_count"], 4);
    assert_eq!(result["excluded_count"], 1);
    // 210 is the outlier; remaining three are 30 minutes each
    assert_eq!(result["avg_minutes"], 30);
    assert!(!result["applied"].as_bool().unwrap());
}

#[tokio::test]
async fn estimate_habit_updates_per_step_estimates() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    sqlx::query(
        "INSERT INTO habits (id, title, recurrence, start_time, end_time, avg_minutes, sigma_minutes, fixed) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("habit-4")
    .bind("Evening routine")
    .bind(r#"{"freq":"Daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"exdates":[]}"#)
    .bind("20:00")
    .bind("21:00")
    .bind(60i64)
    .bind(10i64)
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO habit_steps (id, habit_id, position, title, start_time, end_time, avg_minutes, sigma_minutes, depends_on, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("step-1")
    .bind("habit-4")
    .bind(0i64)
    .bind("Stretch")
    .bind("20:00")
    .bind("20:10")
    .bind(10i64)
    .bind(2i64)
    .bind("[]")
    .bind("2026-07-22T00:00:00Z")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, status, habit_id, habit_step_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("task-step-1")
    .bind("Stretch")
    .bind("2026-07-22T08:00:00Z")
    .bind(10i64)
    .bind(2i64)
    .bind("completed")
    .bind("habit-4")
    .bind("step-1")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO task_work_sessions (id, task_id, started_at, ended_at) VALUES (?, ?, ?, ?)",
    )
    .bind("ws-step-1")
    .bind("task-step-1")
    .bind("2026-07-22T20:00:00Z")
    .bind("2026-07-22T20:15:00Z")
    .execute(&pool)
    .await
    .unwrap();

    let req = auth_req_body(
        Method::POST,
        "/api/habits/habit-4/estimate",
        json!({ "apply": true }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let result: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(result["steps"].as_array().unwrap().len(), 1);
    assert_eq!(result["steps"][0]["avg_minutes"], 15);
    assert!(result["steps"][0]["applied"].as_bool().unwrap());
    assert_eq!(result["avg_minutes"], 15);
    assert_eq!(result["habit"]["avg_minutes"], 15);

    // Verify the habit_steps row was actually updated.
    let updated: (i64, i64) =
        sqlx::query_as("SELECT avg_minutes, sigma_minutes FROM habit_steps WHERE id = ?")
            .bind("step-1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(updated.0, 15);
}

#[tokio::test]
async fn estimate_habit_preserves_fixed_step() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    sqlx::query(
        "INSERT INTO habits (id, title, recurrence, start_time, end_time, avg_minutes, sigma_minutes, fixed) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("habit-5")
    .bind("Evening routine")
    .bind(r#"{"freq":"Daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"exdates":[]}"#)
    .bind("20:00")
    .bind("21:00")
    .bind(60i64)
    .bind(10i64)
    .bind(false)
    .execute(&pool)
    .await
    .unwrap();

    for (step_id, position, title, fixed) in [
        ("step-a", 0i64, "Stretch", false),
        ("step-b", 1i64, "Cooldown", true),
    ] {
        sqlx::query(
            "INSERT INTO habit_steps (id, habit_id, position, title, start_time, end_time, avg_minutes, sigma_minutes, fixed, depends_on, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(step_id)
        .bind("habit-5")
        .bind(position)
        .bind(title)
        .bind("20:00")
        .bind("20:10")
        .bind(10i64)
        .bind(2i64)
        .bind(fixed)
        .bind("[]")
        .bind("2026-07-22T00:00:00Z")
        .execute(&pool)
        .await
        .unwrap();
    }

    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, status, habit_id, habit_step_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("task-step-a")
    .bind("Stretch")
    .bind("2026-07-22T08:00:00Z")
    .bind(10i64)
    .bind(2i64)
    .bind("completed")
    .bind("habit-5")
    .bind("step-a")
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO task_work_sessions (id, task_id, started_at, ended_at) VALUES (?, ?, ?, ?)",
    )
    .bind("ws-step-a")
    .bind("task-step-a")
    .bind("2026-07-22T20:00:00Z")
    .bind("2026-07-22T20:15:00Z")
    .execute(&pool)
    .await
    .unwrap();

    let req = auth_req_body(
        Method::POST,
        "/api/habits/habit-5/estimate",
        json!({ "apply": true }),
    );
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let result: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(result["steps"].as_array().unwrap().len(), 2);
    // The fixed step's avg should still be present and unchanged.
    let fixed = result["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step_id"] == "step-b")
        .unwrap();
    assert_eq!(fixed["avg_minutes"], 10);
    assert!(!fixed["applied"].as_bool().unwrap());

    // Verify the fixed row was not deleted.
    let fixed_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM habit_steps WHERE id = ? AND habit_id = ?")
            .bind("step-b")
            .bind("habit-5")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fixed_count, 1);
}

#[tokio::test]
async fn estimate_habit_rejects_fixed_habit() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    sqlx::query(
        "INSERT INTO habits (id, title, recurrence, start_time, end_time, avg_minutes, sigma_minutes, fixed) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("habit-3")
    .bind("Fixed lunch")
    .bind(r#"{"freq":"Daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"exdates":[]}"#)
    .bind("12:00")
    .bind("13:00")
    .bind(60i64)
    .bind(0i64)
    .bind(true)
    .execute(&pool)
    .await
    .unwrap();

    let req = auth_req_body(Method::POST, "/api/habits/habit-3/estimate", json!({}));
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn delete_task_nullifies_split_from_task_id() {
    let (state, _pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "split-parent",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
            "quantity_done": 3,
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap().to_string();

    let req = auth_req_body(
        Method::POST,
        &format!("/api/tasks/{id}/split"),
        json!({"retained_quantity": 4}),
    );
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let split: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let remainder_id = split["remainder"]["id"].as_str().unwrap().to_string();
    assert_eq!(split["remainder"]["split_from_task_id"], id);

    let delete_req = auth_req(Method::DELETE, &format!("/api/tasks/{id}"));
    let res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let get_req = auth_req(Method::GET, &format!("/api/tasks/{remainder_id}"));
    let res = app.oneshot(get_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let remainder: serde_json::Value =
        serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert!(remainder["split_from_task_id"].is_null());
}

#[tokio::test]
async fn delete_task_removes_progress_and_work_session_rows() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    let create = auth_req_body(
        Method::POST,
        "/api/tasks",
        json!({
            "title": "with-progress",
            "end_at": "2026-07-22T18:00:00+09:00",
            "avg_minutes": 30,
            "quantity_total": 10,
        }),
    );
    let res = app.clone().oneshot(create).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let task: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    let id = task["id"].as_str().unwrap().to_string();

    let now = jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    sqlx::query(
        "INSERT INTO task_work_sessions (id, task_id, started_at, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(&id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO progress_events (id, task_id, at, active_minutes) VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(&id)
    .bind(&now)
    .bind(0i64)
    .execute(&pool)
    .await
    .unwrap();

    let delete_req = auth_req(Method::DELETE, &format!("/api/tasks/{id}"));
    let res = app.oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_work_sessions WHERE task_id = ?")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM progress_events WHERE task_id = ?")
        .bind(&id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn delete_habit_breaks_split_reference_from_other_habit_task() {
    let (state, pool) = setup().await;
    let app = build_router(state);

    let habit_a = create_daily_habit(&app, "habit-a").await;
    let habit_b = create_daily_habit(&app, "habit-b").await;

    // task_a belongs to habit_a, task_b belongs to habit_b and references task_a.
    let task_a_id = "task-a-uuid";
    let task_b_id = "task-b-uuid";
    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, \
         parallelizable, allows_parallel, abandonability, status, habit_id, quantity_done, quantity_total) \
         VALUES (?, 'task-a', '2030-01-01T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'pending', ?, 0, 0)",
    )
    .bind(task_a_id)
    .bind(&habit_a)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, depends, \
         parallelizable, allows_parallel, abandonability, status, habit_id, split_from_task_id, quantity_done, quantity_total) \
         VALUES (?, 'task-b', '2030-01-01T18:00:00Z', 30, 0, '[]', 0, 0, 0.5, 'pending', ?, ?, 0, 0)",
    )
    .bind(task_b_id)
    .bind(&habit_b)
    .bind(task_a_id)
    .execute(&pool)
    .await
    .unwrap();

    let delete_req = auth_req(Method::DELETE, &format!("/api/habits/{habit_a}"));
    let res = app.clone().oneshot(delete_req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let get_a = auth_req(Method::GET, &format!("/api/habits/{habit_a}"));
    let res = app.clone().oneshot(get_a).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let get_b = auth_req(Method::GET, &format!("/api/tasks/{task_b_id}"));
    let res = app.oneshot(get_b).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let task_b: serde_json::Value = serde_json::from_str(&body_str(res.into_body()).await).unwrap();
    assert_eq!(task_b["id"], task_b_id);
    assert!(task_b["split_from_task_id"].is_null());
}
