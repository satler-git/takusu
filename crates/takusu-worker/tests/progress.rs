mod common;

use common::*;

#[test]
#[ignore]
fn record_work_session_progress_updates_quantity() {
    let _g = start_wrangler();
    let token = root_token();

    let task_body = r#"{
        "title": "regression task for jiff now",
        "end_at": "2030-01-01T00:00:00+00:00",
        "avg_minutes": 30,
        "quantity_total": 10,
        "quantity_unit": "pages"
    }"#;
    let (status, body) = http_post_json("/api/tasks", Some(&token), task_body).unwrap();
    assert_eq!(status, 200, "body: {body}");
    let task: serde_json::Value = serde_json::from_str(&body).expect("task json");
    let task_id = task["id"].as_str().expect("task id");

    let start_body = format!(r#"{{"task_id": "{task_id}"}}"#);
    let (status, body) =
        http_post_json("/api/work-sessions", Some(&token), &start_body).unwrap();
    assert_eq!(status, 200, "body: {body}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("session json");
    let session_id = session["id"].as_str().expect("session id");

    // Record progress calls `Timestamp::now()` in the worker. On
    // wasm32-unknown-unknown this previously panicked because jiff's `js`
    // feature was not enabled, so getting the current time fell through to
    // `std::time::SystemTime::now()`, which panics in that target.
    let progress_body = r#"{"quantity_done": 4}"#;
    let (status, body) = http_post_json(
        &format!("/api/work-sessions/{session_id}/progress"),
        Some(&token),
        progress_body,
    )
    .unwrap();
    assert_eq!(status, 200, "body: {body}");
    let result: serde_json::Value = serde_json::from_str(&body).expect("progress json");
    assert_eq!(result["work_session"]["quantity_done"], 4);
    assert_eq!(result["task"]["quantity_done"], 4);
    assert_eq!(result["event"]["delta_quantity"], 4);
}
