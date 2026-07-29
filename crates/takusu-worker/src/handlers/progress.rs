use std::str::FromStr;

use wasm_bindgen::JsValue;
use worker::{Env, Request, Response};

use crate::auth;
use crate::error::WorkerError;
use crate::handlers::auth::db;
use crate::handlers::d1::safe_all;
use crate::handlers::id_resolver::resolve_task_id;
use crate::handlers::settings::get_timezone;
use crate::handlers::tasks::{allocate_display_id, select_one};
use crate::handlers::tokens::{json_ok, parse_json};
use crate::models::{
    ProgressEventRow, ProgressResult, Quantity, RecordProgress, SplitResult, SplitTask,
    TaskProgress, TaskRow, TaskWorkSessionRow,
};
use crate::validate::validate_task_datetimes;

#[derive(serde::Deserialize)]
struct ProgressOpRow {
    request_hash: String,
    response_json: String,
}

fn operation_id(req: &Request) -> Option<String> {
    req.headers()
        .get("Idempotency-Key")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("idempotency-key").ok().flatten())
}

fn progress_request_hash(payload: &str, operation_id: Option<&str>) -> String {
    auth::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

async fn check_progress_idempotency<T: serde::de::DeserializeOwned>(
    database: &worker::D1Database,
    operation_id: &str,
    request_hash: &str,
) -> Result<Option<T>, WorkerError> {
    let stmt = database.prepare(
        "SELECT request_hash, response_json FROM progress_operations WHERE operation_id = ?1",
    );
    let row: Option<ProgressOpRow> = stmt
        .bind(&[JsValue::from_str(operation_id)])?
        .first(None)
        .await
        .map_err(WorkerError::Worker)?;
    if let Some(row) = row {
        if row.request_hash != request_hash {
            return Err(WorkerError::BadRequest(
                "idempotency key reused with different request".into(),
            ));
        }
        let value: T = serde_json::from_str(&row.response_json)
            .map_err(|e| WorkerError::Internal(format!("corrupt idempotency response: {e}")))?;
        return Ok(Some(value));
    }
    Ok(None)
}

async fn record_progress_operation<T: serde::Serialize>(
    database: &worker::D1Database,
    operation_id: &str,
    request_hash: &str,
    value: &T,
) -> Result<(), WorkerError> {
    let response_json = serde_json::to_string(value)
        .map_err(|e| WorkerError::Internal(format!("serialize idempotency response: {e}")))?;
    let stmt = database.prepare(
        "INSERT INTO progress_operations (operation_id, request_hash, response_json, created_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    );
    stmt.bind(&[
        JsValue::from_str(operation_id),
        JsValue::from_str(request_hash),
        JsValue::from_str(&response_json),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;
    Ok(())
}

fn now_seconds() -> i64 {
    (worker::Date::now().as_millis() / 1000) as i64
}

fn parse_timestamp(s: &str) -> Result<i64, WorkerError> {
    // Accept RFC 3339 (used by progress timestamps) or legacy D1
    // `datetime('now')` output for backward compatibility.
    if let Ok(ts) = jiff::Timestamp::from_str(s) {
        return Ok(ts.as_second());
    }
    let dt = jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M:%S", s)
        .map_err(|e| WorkerError::Internal(format!("invalid timestamp {s}: {e}")))?;
    let zdt = dt
        .to_zoned(jiff::tz::TimeZone::UTC)
        .map_err(|e| WorkerError::Internal(format!("invalid timestamp {s}: {e}")))?;
    Ok(zdt.timestamp().as_second())
}

fn session_minutes(session: &TaskWorkSessionRow) -> i64 {
    match session.ended_at {
        Some(end) => {
            takusu_types::minutes_between(&session.started_at.to_string(), &end.to_string())
        }
        None => {
            let now = now_seconds();
            let start = parse_timestamp(&session.started_at.to_string()).unwrap_or(now);
            ((now - start) / 60).max(1)
        }
    }
}

async fn compute_updated_estimate(
    database: &worker::D1Database,
    task_id: &str,
    avg_minutes: i64,
    sigma_minutes: i64,
    quantity_total: Option<i64>,
    active_minutes: i64,
    delta_quantity: i64,
) -> Result<(i64, i64), WorkerError> {
    let stmt = database.prepare(
        "SELECT id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE task_id = ?1 AND delta_quantity > 0 AND active_minutes > 0 ORDER BY id ASC",
    );
    let events: Vec<ProgressEventRow> =
        safe_all(&stmt.bind(&[JsValue::from_str(task_id)])?).await?;

    let observations: Vec<(i64, i64)> = events
        .iter()
        .map(|e| (e.active_minutes, e.delta_quantity.unwrap_or(1).max(1)))
        .collect();

    Ok(takusu_types::estimate_progress(
        avg_minutes,
        sigma_minutes,
        quantity_total,
        active_minutes,
        delta_quantity,
        &observations,
    ))
}

pub async fn start_task_work(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let op_id = operation_id(&req);
    let payload = serde_json::json!({"op": "start", "id": id}).to_string();
    let request_hash = progress_request_hash(&payload, op_id.as_deref());
    if let Some(ref oid) = op_id
        && let Some(stored) =
            check_progress_idempotency::<TaskRow>(&database, oid, &request_hash).await?
    {
        return json_ok(&stored);
    }

    let full = resolve_task_id(&database, id).await?;

    let status_stmt = database.prepare("SELECT status FROM tasks WHERE id = ?1");
    let status: Option<String> = status_stmt
        .bind(&[JsValue::from_str(&full)])?
        .first(Some("status"))
        .await
        .map_err(WorkerError::Worker)?;
    if status.as_deref() == Some("completed") || status.as_deref() == Some("skipped") {
        return Err(WorkerError::BadRequest(format!(
            "cannot start work on a {} task",
            status.unwrap_or_default()
        )));
    }

    let session_id = uuid::Uuid::now_v7().to_string();
    let insert = database.prepare(
        "INSERT OR IGNORE INTO task_work_sessions (id, task_id, started_at, created_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    );
    insert
        .bind(&[JsValue::from_str(&session_id), JsValue::from_str(&full)])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let update = database.prepare(
        "UPDATE tasks SET status = 'in_progress', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
    );
    update
        .bind(&[JsValue::from_str(&full)])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let task = select_one(&database, &full).await?;
    if let Some(ref oid) = op_id {
        record_progress_operation(&database, oid, &request_hash, &task).await?;
    }
    json_ok(&task)
}

pub async fn pause_task_work(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let op_id = operation_id(&req);
    let payload = serde_json::json!({"op": "pause", "id": id}).to_string();
    let request_hash = progress_request_hash(&payload, op_id.as_deref());
    if let Some(ref oid) = op_id
        && let Some(stored) =
            check_progress_idempotency::<TaskRow>(&database, oid, &request_hash).await?
    {
        return json_ok(&stored);
    }

    let full = resolve_task_id(&database, id).await?;

    let status_stmt = database.prepare("SELECT status FROM tasks WHERE id = ?1");
    let status: Option<String> = status_stmt
        .bind(&[JsValue::from_str(&full)])?
        .first(Some("status"))
        .await
        .map_err(WorkerError::Worker)?;
    if status.as_deref() == Some("completed") || status.as_deref() == Some("skipped") {
        return Err(WorkerError::BadRequest(format!(
            "cannot pause work on a {} task",
            status.unwrap_or_default()
        )));
    }

    let close = database.prepare(
        "UPDATE task_work_sessions SET ended_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE task_id = ?1 AND ended_at IS NULL",
    );
    close
        .bind(&[JsValue::from_str(&full)])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let update = database.prepare(
        "UPDATE tasks SET status = 'scheduled', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
    );
    update
        .bind(&[JsValue::from_str(&full)])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let task = select_one(&database, &full).await?;
    if let Some(ref oid) = op_id {
        record_progress_operation(&database, oid, &request_hash, &task).await?;
    }
    json_ok(&task)
}

pub async fn record_progress(
    mut req: Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: RecordProgress = parse_json(&mut req).await?;

    let database = db(&env)?;
    let op_id = operation_id(&req);
    let payload = serde_json::json!({"op": "progress", "id": id, "body": body}).to_string();
    let request_hash = progress_request_hash(&payload, op_id.as_deref());
    if let Some(ref oid) = op_id
        && let Some(stored) =
            check_progress_idempotency::<ProgressResult>(&database, oid, &request_hash).await?
    {
        return json_ok(&stored);
    }

    let full = resolve_task_id(&database, id).await?;

    let task = select_one(&database, &full).await?;

    if task.status == takusu_types::TaskStatus::Completed
        || task.status == takusu_types::TaskStatus::Skipped
    {
        return Err(WorkerError::BadRequest(format!(
            "cannot record progress on a {} task",
            task.status
        )));
    }
    if let Some(total) = task.quantity_total
        && body.quantity_done > total
    {
        return Err(WorkerError::BadRequest(format!(
            "quantity_done cannot exceed quantity_total ({} > {})",
            body.quantity_done, total
        )));
    }

    let open_stmt = database.prepare(
        "SELECT id, task_id, started_at, ended_at, created_at FROM task_work_sessions WHERE task_id = ?1 AND ended_at IS NULL ORDER BY started_at ASC LIMIT 1",
    );
    let open: Option<TaskWorkSessionRow> = open_stmt
        .bind(&[JsValue::from_str(&full)])?
        .first(None)
        .await
        .map_err(WorkerError::Worker)?;

    // Increasing progress requires an open session to measure active time.
    // Corrections (decreasing or keeping quantity_done) are allowed without one.
    if open.is_none() && body.quantity_done > task.quantity_done {
        return Err(WorkerError::BadRequest(
            "no open work session; start work first".into(),
        ));
    }

    #[derive(serde::Deserialize)]
    struct NowRow {
        now: String,
    }
    let delta_quantity = body.quantity_done.get() - task.quantity_done.get();

    if delta_quantity == 0 {
        let result = ProgressResult {
            task: task.clone(),
            event: None,
            suggests_completion: false,
        };
        if let Some(ref oid) = op_id {
            record_progress_operation(&database, oid, &request_hash, &result).await?;
        }
        return json_ok(&result);
    }

    let now_stmt = database.prepare("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now') AS now");
    let now_row: Option<NowRow> = now_stmt.first(None).await.map_err(WorkerError::Worker)?;
    let now = now_row
        .map(|r| r.now)
        .ok_or_else(|| WorkerError::Internal("failed to get current time".into()))?;

    // Active minutes are measured from the later of the open session start and
    // the most recent progress event so repeated updates in the same session
    // do not accumulate the same time.
    let last_event_stmt = database.prepare(
        "SELECT id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
    );
    let last_event: Option<ProgressEventRow> = last_event_stmt
        .bind(&[JsValue::from_str(&full)])?
        .first(None)
        .await
        .map_err(WorkerError::Worker)?;

    let active_minutes = if let Some(ref session) = open {
        let base = if let Some(ref ev) = last_event {
            std::cmp::max(session.started_at, ev.at)
        } else {
            session.started_at
        };
        takusu_types::minutes_between(&base.to_string(), &now)
    } else {
        0
    };

    let event_id = uuid::Uuid::now_v7().to_string();
    let insert = database.prepare(
        "INSERT INTO progress_events (id, task_id, at, quantity_done, delta_quantity, active_minutes, note) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?3, ?4, ?5, ?6)",
    );
    insert
        .bind(&[
            JsValue::from_str(&event_id),
            JsValue::from_str(&full),
            JsValue::from_f64(f64::from(body.quantity_done)),
            JsValue::from_f64(delta_quantity as f64),
            JsValue::from_f64(active_minutes as f64),
            body.note
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let mut new_avg = task.avg_minutes;
    let mut new_sigma = task.sigma_minutes;
    if delta_quantity > 0 && active_minutes > 0 {
        let (avg, sigma) = compute_updated_estimate(
            &database,
            &full,
            task.avg_minutes,
            task.sigma_minutes,
            task.quantity_total.map(|q| q.get()),
            active_minutes,
            delta_quantity,
        )
        .await?;
        new_avg = avg;
        new_sigma = sigma;
    }

    let status = if task.status == takusu_types::TaskStatus::Completed {
        takusu_types::TaskStatus::Completed
    } else if delta_quantity < 0 {
        task.status
    } else {
        takusu_types::TaskStatus::InProgress
    };

    let suggests_completion = task
        .quantity_total
        .map(|total| body.quantity_done >= total)
        .unwrap_or(false);

    let update = database.prepare(
        "UPDATE tasks SET quantity_done = ?1, avg_minutes = ?2, sigma_minutes = ?3, status = ?4, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?5",
    );
    update
        .bind(&[
            JsValue::from_f64(f64::from(body.quantity_done)),
            JsValue::from_f64(new_avg as f64),
            JsValue::from_f64(new_sigma as f64),
            JsValue::from_str(&status.to_string()),
            JsValue::from_str(&full),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let event_stmt = database.prepare("SELECT id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE id = ?1");
    let event: ProgressEventRow = event_stmt
        .bind(&[JsValue::from_str(&event_id)])?
        .first(None)
        .await
        .map_err(WorkerError::Worker)?
        .ok_or_else(|| WorkerError::Internal("inserted progress event not found".into()))?;

    let task = select_one(&database, &full).await?;

    let result = ProgressResult {
        task,
        event: Some(event),
        suggests_completion,
    };
    if let Some(ref oid) = op_id {
        record_progress_operation(&database, oid, &request_hash, &result).await?;
    }
    json_ok(&result)
}

pub async fn complete_task_work(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let op_id = operation_id(&req);
    let payload = serde_json::json!({"op": "complete", "id": id}).to_string();
    let request_hash = progress_request_hash(&payload, op_id.as_deref());
    if let Some(ref oid) = op_id
        && let Some(stored) =
            check_progress_idempotency::<TaskRow>(&database, oid, &request_hash).await?
    {
        return json_ok(&stored);
    }

    let full = resolve_task_id(&database, id).await?;

    let original = select_one(&database, &full).await?;
    if original.status == takusu_types::TaskStatus::Completed
        || original.status == takusu_types::TaskStatus::Skipped
    {
        return Err(WorkerError::BadRequest(format!(
            "cannot complete a {} task",
            original.status
        )));
    }

    let close = database.prepare(
        "UPDATE task_work_sessions SET ended_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE task_id = ?1 AND ended_at IS NULL",
    );
    close
        .bind(&[JsValue::from_str(&full)])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let original = select_one(&database, &full).await?;

    let session_stmt = database.prepare(
        "SELECT id, task_id, started_at, ended_at, created_at FROM task_work_sessions WHERE task_id = ?1 ORDER BY started_at ASC",
    );
    let sessions: Vec<TaskWorkSessionRow> =
        safe_all(&session_stmt.bind(&[JsValue::from_str(&full)])?).await?;
    let total_active_minutes: i64 = sessions.iter().map(session_minutes).sum();

    let quantity_done = original.quantity_total.unwrap_or(original.quantity_done);
    let delta_quantity = quantity_done.get() - original.quantity_done.get();

    let (new_avg, new_sigma) = if delta_quantity > 0 && total_active_minutes > 0 {
        compute_updated_estimate(
            &database,
            &full,
            original.avg_minutes,
            original.sigma_minutes,
            original.quantity_total.map(|q| q.get()),
            total_active_minutes,
            delta_quantity,
        )
        .await?
    } else if original.quantity_total.is_none() && total_active_minutes > 0 {
        (total_active_minutes, original.sigma_minutes)
    } else {
        (original.avg_minutes, original.sigma_minutes)
    };

    let update = database.prepare(
        "UPDATE tasks SET status = 'completed', completed_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), quantity_done = ?1, avg_minutes = ?2, sigma_minutes = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?4",
    );
    update
        .bind(&[
            JsValue::from_f64(f64::from(quantity_done)),
            JsValue::from_f64(new_avg as f64),
            JsValue::from_f64(new_sigma as f64),
            JsValue::from_str(&full),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    if total_active_minutes > 0 {
        let event_id = uuid::Uuid::now_v7().to_string();
        let insert = database.prepare(
            "INSERT INTO progress_events (id, task_id, at, quantity_done, delta_quantity, active_minutes, note) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?3, ?4, ?5, ?6)",
        );
        insert
            .bind(&[
                JsValue::from_str(&event_id),
                JsValue::from_str(&full),
                JsValue::from_f64(f64::from(quantity_done)),
                JsValue::from_f64(delta_quantity as f64),
                JsValue::from_f64(total_active_minutes as f64),
                JsValue::from_str("completed"),
            ])?
            .run()
            .await
            .map_err(WorkerError::Worker)?;
    }

    let task = select_one(&database, &full).await?;
    if let Some(ref oid) = op_id {
        record_progress_operation(&database, oid, &request_hash, &task).await?;
    }
    json_ok(&task)
}

pub async fn get_task_progress(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_task_id(&database, id).await?;

    let task = select_one(&database, &full).await?;

    let session_stmt = database.prepare(
        "SELECT id, task_id, started_at, ended_at, created_at FROM task_work_sessions WHERE task_id = ?1 ORDER BY started_at ASC",
    );
    let sessions: Vec<TaskWorkSessionRow> =
        safe_all(&session_stmt.bind(&[JsValue::from_str(&full)])?).await?;

    let event_stmt = database.prepare(
        "SELECT id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE task_id = ?1 ORDER BY id ASC",
    );
    let events: Vec<ProgressEventRow> =
        safe_all(&event_stmt.bind(&[JsValue::from_str(&full)])?).await?;

    let open_session = sessions.iter().find(|s| s.ended_at.is_none()).cloned();
    let total_active_minutes = sessions.iter().map(session_minutes).sum();

    json_ok(&TaskProgress {
        task,
        open_session,
        sessions,
        events,
        total_active_minutes,
    })
}

pub async fn split_task(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: SplitTask = parse_json(&mut req).await?;

    let database = db(&env)?;
    let op_id = operation_id(&req);
    let payload = serde_json::json!({"op": "split", "id": id, "body": body}).to_string();
    let request_hash = progress_request_hash(&payload, op_id.as_deref());
    if let Some(ref oid) = op_id
        && let Some(stored) =
            check_progress_idempotency::<SplitResult>(&database, oid, &request_hash).await?
    {
        return json_ok(&stored);
    }

    let full = resolve_task_id(&database, id).await?;

    let original = select_one(&database, &full).await?;
    if body.end_at.is_some() {
        let tz = get_timezone(&database).await?;
        validate_task_datetimes(
            None,
            body.end_at.as_ref(),
            &tz,
            original.start_at.as_ref(),
            None,
        )?;
    }
    if original.status == takusu_types::TaskStatus::Completed
        || original.status == takusu_types::TaskStatus::Skipped
    {
        return Err(WorkerError::BadRequest(format!(
            "cannot split a {} task",
            original.status
        )));
    }

    let total = original.quantity_total.ok_or_else(|| {
        WorkerError::BadRequest("cannot split a task with no quantity_total".into())
    })?;
    if body.retained_quantity <= 0 {
        return Err(WorkerError::BadRequest(
            "retained_quantity must be greater than 0".into(),
        ));
    }
    if body.retained_quantity > total {
        return Err(WorkerError::BadRequest(
            "retained_quantity cannot exceed quantity_total".into(),
        ));
    }
    if body.retained_quantity == total {
        return Err(WorkerError::BadRequest(
            "retained_quantity must be less than quantity_total".into(),
        ));
    }
    if body.retained_quantity < original.quantity_done {
        return Err(WorkerError::BadRequest(
            "retained_quantity cannot be less than quantity_done".into(),
        ));
    }
    let remainder_quantity = Quantity::new(total.get() - body.retained_quantity.get())
        .expect("retained_quantity is less than total, so remainder is non-negative");
    let original_quantity_total = original
        .original_quantity_total
        .filter(|t| *t != 0)
        .unwrap_or(total);

    let remainder_id = uuid::Uuid::now_v7().to_string();
    let display_id = allocate_display_id(&database, None).await?;

    let depends = if body.set_dependency.unwrap_or(false) {
        vec![full.clone()]
    } else {
        Vec::new()
    };
    let depends_json = crate::models::DependencyList::new(depends).to_json_string();

    let remainder_title = body.title.as_ref().unwrap_or(&original.title);
    let normalized_title = takusu_search::memory::normalize_text(
        remainder_title,
        Some(takusu_search::memory::MAX_CONTENT_SCALARS),
    )
    .ok();

    let insert = database.prepare(
        "INSERT INTO tasks (id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, normalized_title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    );
    insert
        .bind(&[
            JsValue::from_str(&remainder_id),
            JsValue::from_f64(display_id as f64),
            JsValue::from_str(remainder_title.as_str()),
            body.description
                .as_ref()
                .or(original.description.as_ref())
                .map(|s| JsValue::from_str(s.as_str()))
                .unwrap_or(JsValue::NULL),
            original
                .start_at
                .as_ref()
                .map(|s| JsValue::from_str(&s.to_string()))
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&body.end_at.unwrap_or(original.end_at).to_string()),
            JsValue::from_f64(original.avg_minutes as f64),
            JsValue::from_f64(original.sigma_minutes as f64),
            JsValue::from_str(&depends_json),
            JsValue::from_bool(original.parallelizable),
            JsValue::from_bool(original.allows_parallel),
            JsValue::from_f64(original.abandonability.into()),
            JsValue::NULL,
            JsValue::NULL,
            JsValue::from_bool(original.fixed),
            JsValue::NULL,
            JsValue::from_f64(f64::from(remainder_quantity)),
            JsValue::from_f64(0.0),
            original
                .quantity_unit
                .as_ref()
                .map(|s| JsValue::from_str(s.as_str()))
                .unwrap_or(JsValue::NULL),
            JsValue::NULL,
            JsValue::from_str(&full),
            JsValue::from_f64(f64::from(original_quantity_total)),
            normalized_title
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let new_done = original.quantity_done.min(body.retained_quantity);
    let update = database.prepare(
        "UPDATE tasks SET quantity_total = ?1, quantity_done = ?2, original_quantity_total = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?4",
    );
    update
        .bind(&[
            JsValue::from_f64(f64::from(body.retained_quantity)),
            JsValue::from_f64(f64::from(new_done)),
            JsValue::from_f64(f64::from(original_quantity_total)),
            JsValue::from_str(&full),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let original = select_one(&database, &full).await?;
    let remainder = select_one(&database, &remainder_id).await?;

    let result = SplitResult {
        original,
        remainder,
    };
    if let Some(ref oid) = op_id {
        record_progress_operation(&database, oid, &request_hash, &result).await?;
    }
    json_ok(&result)
}
