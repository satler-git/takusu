use wasm_bindgen::JsValue;
use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::db;
use crate::handlers::d1::safe_all;
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::models::{
    ApplyHabitEstimateRequest, CreateHabit, CreateHabitScheduledSpan, HabitDetail, HabitRow,
    HabitScheduledSpanRow, HabitStepInput, HabitStepRow, UpdateHabit,
};
use crate::validate::{
    validate_minutes, validate_recurrence, validate_scheduled_span_dates, validate_steps,
};
use takusu_util::Minutes;

const HABIT_COLS: &str = "id, display_id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed, window_mode, created_at, updated_at";
const STEP_COLS: &str = "id, habit_id, position, title, description, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, fixed, depends_on, created_at";

fn select_habits() -> String {
    format!("SELECT {HABIT_COLS} FROM habits")
}

pub async fn list(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let stmt = database.prepare(format!(
        "{select} ORDER BY created_at DESC",
        select = select_habits()
    ));
    let rows: Vec<HabitRow> = safe_all(&stmt).await?;
    json_ok(&rows)
}

pub async fn create(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: CreateHabit = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_recurrence(&body.recurrence)?;
    let window_mode = body.window_mode.unwrap_or(takusu_util::WindowMode::Day);
    let database = db(&env)?;
    let id = uuid::Uuid::now_v7().to_string();
    let sigma = body
        .sigma_minutes
        .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
    let parallelizable = body.parallelizable.unwrap_or(false);
    let allows_parallel = body.allows_parallel.unwrap_or(false);
    let abandonability = body.abandonability.unwrap_or(0.5.into());
    let fixed = body.fixed.unwrap_or(false);

    // Atomically reserve a monotonic display_id from the sequence table
    // (mirrors tasks.display_id, issue #186 / #305).
    let seq_stmt = database.prepare(
        "UPDATE habit_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1 AS display_id",
    );
    let seq_row: Option<DisplayIdRow> = seq_stmt.first(None).await.map_err(WorkerError::Worker)?;
    let display_id = seq_row
        .ok_or_else(|| WorkerError::Internal("habit display_id sequence is empty".into()))?
        .display_id;

    let stmt = database.prepare(
        "INSERT INTO habits (id, display_id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed, window_mode, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))"
    );
    stmt.bind(&[
        JsValue::from_str(&id),
        JsValue::from_f64(display_id as f64),
        JsValue::from_str(&body.title),
        body.description
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&body.recurrence),
        JsValue::from_str(&body.start_time.to_string()),
        JsValue::from_str(&body.end_time.to_string()),
        JsValue::from_f64(body.avg_minutes as f64),
        JsValue::from_f64(sigma as f64),
        JsValue::from_bool(parallelizable),
        JsValue::from_bool(allows_parallel),
        JsValue::from_f64(abandonability.into()),
        JsValue::from_bool(fixed),
        JsValue::from_str(&window_mode.to_string()),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;

    let row = select_one(&database, &id).await?;
    json_created(&row)
}

pub async fn get(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    let habit = select_one(&database, &full).await?;
    // Include steps in the GET response (#95).
    let steps = select_steps_for_habit(&database, &full).await?;
    json_ok(&HabitDetail { habit, steps })
}

pub async fn update(mut req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: UpdateHabit = parse_json(&mut req).await?;
    if let Some(avg) = body.avg_minutes {
        validate_minutes(avg, body.sigma_minutes)?;
    } else if let Some(sigma) = body.sigma_minutes {
        validate_minutes(0, Some(sigma))?;
    }
    if let Some(ref recurrence) = body.recurrence {
        validate_recurrence(recurrence)?;
    }
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    let stmt = database.prepare(
        "UPDATE habits SET title=COALESCE(?1,title), description=COALESCE(?2,description), recurrence=COALESCE(?3,recurrence), start_time=COALESCE(?4,start_time), end_time=COALESCE(?5,end_time), avg_minutes=COALESCE(?6,avg_minutes), sigma_minutes=COALESCE(?7,sigma_minutes), parallelizable=COALESCE(?8,parallelizable), allows_parallel=COALESCE(?9,allows_parallel), abandonability=COALESCE(?10,abandonability), active=COALESCE(?11,active), fixed=COALESCE(?12,fixed), window_mode=COALESCE(?13,window_mode), updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?14"
    );
    stmt.bind(&[
        body.title
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.description
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.recurrence
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.start_time
            .map(|t| JsValue::from_str(&t.to_string()))
            .unwrap_or(JsValue::NULL),
        body.end_time
            .map(|t| JsValue::from_str(&t.to_string()))
            .unwrap_or(JsValue::NULL),
        body.avg_minutes
            .map(|n| JsValue::from_f64(n as f64))
            .unwrap_or(JsValue::NULL),
        body.sigma_minutes
            .map(|n| JsValue::from_f64(n as f64))
            .unwrap_or(JsValue::NULL),
        body.parallelizable
            .map(JsValue::from_bool)
            .unwrap_or(JsValue::NULL),
        body.allows_parallel
            .map(JsValue::from_bool)
            .unwrap_or(JsValue::NULL),
        body.abandonability
            .map(|a| JsValue::from_f64(a.into()))
            .unwrap_or(JsValue::NULL),
        body.active.map(JsValue::from_bool).unwrap_or(JsValue::NULL),
        body.fixed.map(JsValue::from_bool).unwrap_or(JsValue::NULL),
        body.window_mode
            .as_ref()
            .map(|w| JsValue::from_str(&w.to_string()))
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&full),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;

    let row = select_one(&database, &full).await?;
    json_ok(&row)
}

pub async fn replace(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: CreateHabit = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_recurrence(&body.recurrence)?;
    let window_mode = body.window_mode.unwrap_or(takusu_util::WindowMode::Day);
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    let sigma = body
        .sigma_minutes
        .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
    let parallelizable = body.parallelizable.unwrap_or(false);
    let allows_parallel = body.allows_parallel.unwrap_or(false);
    let abandonability = body.abandonability.unwrap_or(0.5.into());
    let fixed = body.fixed.unwrap_or(false);

    let stmt = database.prepare(
        "UPDATE habits SET title=?1, description=?2, recurrence=?3, start_time=?4, end_time=?5, avg_minutes=?6, sigma_minutes=?7, parallelizable=?8, allows_parallel=?9, abandonability=?10, fixed=?11, window_mode=?12, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?13"
    );
    stmt.bind(&[
        JsValue::from_str(&body.title),
        body.description
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&body.recurrence),
        JsValue::from_str(&body.start_time.to_string()),
        JsValue::from_str(&body.end_time.to_string()),
        JsValue::from_f64(body.avg_minutes as f64),
        JsValue::from_f64(sigma as f64),
        JsValue::from_bool(parallelizable),
        JsValue::from_bool(allows_parallel),
        JsValue::from_f64(abandonability.into()),
        JsValue::from_bool(fixed),
        JsValue::from_str(&window_mode.to_string()),
        JsValue::from_str(&full),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;

    let row = select_one(&database, &full).await?;
    json_ok(&row)
}

pub async fn delete(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    // Delete tasks referencing this habit before deleting the habit,
    // so D1's foreign-key constraint does not block deletion of habits
    // that have already generated tasks (#240). The client confirms
    // with the user before issuing the delete when there are
    // associated tasks. All statements run in a single batch() call
    // so D1 executes them atomically (matching the sqlite transaction
    // in storage_sqlite.rs) — a partial failure cannot leave the
    // database with tasks deleted but the habit still present.
    // Break split-task self-references first, including split-off tasks
    // that live outside this habit, then delete child rows and habit rows.
    let stmts = vec![
        database
            .prepare("UPDATE tasks SET split_from_task_id = NULL WHERE split_from_task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM google_cal_events WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM task_work_sessions WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM progress_events WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM tasks WHERE habit_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM habit_scheduled_spans WHERE habit_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM habit_steps WHERE habit_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        // habit_task_display_id_seq: clean up the per-habit sequence (#380).
        database
            .prepare("DELETE FROM habit_task_display_id_seq WHERE habit_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM habits WHERE id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
    ];
    database.batch(stmts).await.map_err(WorkerError::Worker)?;
    Ok(Response::empty()?)
}

pub async fn select_one(database: &worker::D1Database, id: &str) -> Result<HabitRow, WorkerError> {
    let stmt = database.prepare(format!("{select} WHERE id = ?1", select = select_habits()));
    let row: Option<HabitRow> = stmt
        .bind(&[JsValue::from_str(id)])?
        .first(None)
        .await
        .map_err(WorkerError::Worker)?;
    row.ok_or_else(|| WorkerError::NotFound(format!("habit {id} not found")))
}

/// Resolve a habit reference (`h<N>` or full UUID) to a full UUID. UUID
/// prefixes are not accepted (#1251).
async fn resolve_habit_id(database: &worker::D1Database, id: &str) -> Result<String, WorkerError> {
    // `h<N>` → habit display_id lookup (#305).
    if let Some(rest) = id.strip_prefix(['h', 'H'])
        && let Ok(num) = rest.parse::<i64>()
    {
        let stmt = database.prepare(format!(
            "{select} WHERE display_id = ?1",
            select = select_habits()
        ));
        let row: Option<HabitRow> = stmt
            .bind(&[JsValue::from_f64(num as f64)])?
            .first(None)
            .await
            .map_err(WorkerError::Worker)?;
        return row
            .map(|h| h.id)
            .ok_or_else(|| WorkerError::NotFound(format!("habit {id} not found")));
    }
    // Full UUID
    if id.contains('-') {
        return Ok(id.to_string());
    }
    // Anything else (e.g. a UUID prefix) is not a valid reference (#1251).
    Err(WorkerError::NotFound(format!("habit {id} not found")))
}

#[derive(serde::Deserialize)]
struct DisplayIdRow {
    display_id: i64,
}

// ── Habit scheduled spans (#303 / #503) ────────────────────────────────

const SCHEDULED_SPAN_COLS: &str = "id, habit_id, start_date, end_date, reason, created_at";

pub async fn list_scheduled_spans(
    _req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    let stmt = database.prepare(format!(
        "SELECT {SCHEDULED_SPAN_COLS} FROM habit_scheduled_spans WHERE habit_id = ?1 ORDER BY start_date ASC, created_at ASC"
    ));
    let rows: Vec<HabitScheduledSpanRow> =
        safe_all(&stmt.bind(&[JsValue::from_str(&full)])?).await?;
    json_ok(&rows)
}

pub async fn list_all_scheduled_spans(
    _req: worker::Request,
    env: Env,
) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let stmt = database.prepare(format!(
        "SELECT {SCHEDULED_SPAN_COLS} FROM habit_scheduled_spans ORDER BY habit_id, start_date ASC, created_at ASC"
    ));
    let rows: Vec<HabitScheduledSpanRow> = safe_all(&stmt).await?;
    json_ok(&rows)
}

pub async fn create_scheduled_span(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: CreateHabitScheduledSpan = parse_json(&mut req).await?;
    validate_scheduled_span_dates(&body.start_date.to_string(), &body.end_date.to_string())?;
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    let span_id = uuid::Uuid::now_v7().to_string();
    let stmt = database.prepare(
        "INSERT INTO habit_scheduled_spans (id, habit_id, start_date, end_date, reason, created_at) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    );
    stmt.bind(&[
        JsValue::from_str(&span_id),
        JsValue::from_str(&full),
        JsValue::from_str(&body.start_date.to_string()),
        JsValue::from_str(&body.end_date.to_string()),
        body.reason
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;
    let row = select_one_scheduled_span(&database, &span_id).await?;
    json_created(&row)
}

pub async fn delete_scheduled_span(
    _req: worker::Request,
    env: Env,
    id: &str,
    span_id: &str,
) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    let stmt =
        database.prepare("DELETE FROM habit_scheduled_spans WHERE id = ?1 AND habit_id = ?2");
    let result = stmt
        .bind(&[JsValue::from_str(span_id), JsValue::from_str(&full)])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;
    let affected = result
        .meta()
        .map_err(WorkerError::Worker)?
        .and_then(|m| m.rows_written)
        .unwrap_or(0);
    if affected == 0 {
        return Err(WorkerError::NotFound(format!(
            "scheduled span {span_id} not found for habit {id}"
        )));
    }
    Ok(Response::empty()?)
}

async fn select_one_scheduled_span(
    database: &worker::D1Database,
    span_id: &str,
) -> Result<HabitScheduledSpanRow, WorkerError> {
    let stmt = database.prepare(format!(
        "SELECT {SCHEDULED_SPAN_COLS} FROM habit_scheduled_spans WHERE id = ?1"
    ));
    let row: Option<HabitScheduledSpanRow> = stmt
        .bind(&[JsValue::from_str(span_id)])?
        .first(None)
        .await
        .map_err(WorkerError::Worker)?;
    row.ok_or_else(|| WorkerError::Internal("inserted scheduled span not found".into()))
}

// ── Habit steps (#95) ────────────────────────────────────────────────────

async fn select_steps_for_habit(
    database: &worker::D1Database,
    habit_id: &str,
) -> Result<Vec<HabitStepRow>, WorkerError> {
    let stmt = database.prepare(format!(
        "SELECT {STEP_COLS} FROM habit_steps WHERE habit_id = ?1 ORDER BY position ASC, created_at ASC"
    ));
    safe_all(&stmt.bind(&[JsValue::from_str(habit_id)])?).await
}

pub async fn list_steps(
    _req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;
    let rows = select_steps_for_habit(&database, &full).await?;
    json_ok(&rows)
}

pub async fn list_all_steps(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let stmt = database.prepare(format!(
        "SELECT {STEP_COLS} FROM habit_steps ORDER BY habit_id, position ASC, created_at ASC"
    ));
    let rows: Vec<HabitStepRow> = safe_all(&stmt).await?;
    json_ok(&rows)
}

pub async fn replace_steps(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: Vec<HabitStepInput> = parse_json(&mut req).await?;
    validate_steps(&body)?;
    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;

    // Fetch existing step ids for this habit.
    let id_stmt = database.prepare("SELECT id FROM habit_steps WHERE habit_id = ?1");
    let existing: Vec<IdRow> = id_stmt
        .bind(&[JsValue::from_str(&full)])?
        .all()
        .await
        .map_err(WorkerError::Worker)?
        .results()
        .map_err(WorkerError::Worker)?;
    let existing_ids: std::collections::HashSet<String> =
        existing.into_iter().map(|r| r.id).collect();

    let mut input_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stmts: Vec<_> = Vec::new();

    for s in &body {
        let id =
            s.id.clone()
                .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        input_ids.insert(id.clone());
        let sigma = s
            .sigma_minutes
            .unwrap_or(Minutes(s.avg_minutes).to_slots().0.max(1));
        let parallelizable = s.parallelizable.unwrap_or(false);
        let allows_parallel = s.allows_parallel.unwrap_or(false);
        let abandonability = s.abandonability.unwrap_or(0.5.into());
        let fixed = s.fixed.unwrap_or(false);
        let depends_json =
            serde_json::to_string(&s.depends_on).unwrap_or_else(|_| "[]".to_string());

        if existing_ids.contains(&id) {
            let stmt = database.prepare(
                "UPDATE habit_steps SET position=?1, title=?2, description=?3, start_time=?4, end_time=?5, avg_minutes=?6, sigma_minutes=?7, parallelizable=?8, allows_parallel=?9, abandonability=?10, fixed=?11, depends_on=?12 WHERE id = ?13 AND habit_id = ?14",
            );
            stmts.push(
                stmt.bind(&[
                    JsValue::from_f64(s.position as f64),
                    JsValue::from_str(&s.title),
                    s.description
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&s.start_time.to_string()),
                    JsValue::from_str(&s.end_time.to_string()),
                    JsValue::from_f64(s.avg_minutes as f64),
                    JsValue::from_f64(sigma as f64),
                    JsValue::from_bool(parallelizable),
                    JsValue::from_bool(allows_parallel),
                    JsValue::from_f64(abandonability.into()),
                    JsValue::from_bool(fixed),
                    JsValue::from_str(&depends_json),
                    JsValue::from_str(&id),
                    JsValue::from_str(&full),
                ])?,
            );
        } else {
            let stmt = database.prepare(
                "INSERT INTO habit_steps (id, habit_id, position, title, description, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, fixed, depends_on, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            );
            stmts.push(
                stmt.bind(&[
                    JsValue::from_str(&id),
                    JsValue::from_str(&full),
                    JsValue::from_f64(s.position as f64),
                    JsValue::from_str(&s.title),
                    s.description
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&s.start_time.to_string()),
                    JsValue::from_str(&s.end_time.to_string()),
                    JsValue::from_f64(s.avg_minutes as f64),
                    JsValue::from_f64(sigma as f64),
                    JsValue::from_bool(parallelizable),
                    JsValue::from_bool(allows_parallel),
                    JsValue::from_f64(abandonability.into()),
                    JsValue::from_bool(fixed),
                    JsValue::from_str(&depends_json),
                ])?,
            );
        }
    }

    // Delete existing steps not present in the input.
    for old_id in &existing_ids {
        if !input_ids.contains(old_id) {
            let stmt = database.prepare("DELETE FROM habit_steps WHERE id = ?1 AND habit_id = ?2");
            stmts.push(stmt.bind(&[JsValue::from_str(old_id), JsValue::from_str(&full)])?);
        }
    }

    if !stmts.is_empty() {
        database.batch(stmts).await.map_err(WorkerError::Worker)?;
    }

    let rows = select_steps_for_habit(&database, &full).await?;
    json_ok(&rows)
}

pub async fn apply_estimate(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: ApplyHabitEstimateRequest = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, Some(body.sigma_minutes))?;

    let database = db(&env)?;
    let full = resolve_habit_id(&database, id).await?;

    let habit = select_one(&database, &full).await?;
    if habit.fixed {
        return Err(WorkerError::BadRequest(
            "cannot apply estimate to fixed habit".into(),
        ));
    }

    let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
    for step in &body.steps {
        // Only update non-fixed steps; fixed steps are intentionally preserved.
        let stmt = database.prepare(
            "UPDATE habit_steps SET avg_minutes = ?1, sigma_minutes = ?2 WHERE id = ?3 AND habit_id = ?4 AND fixed = 0",
        );
        stmts.push(stmt.bind(&[
            JsValue::from_f64(step.avg_minutes as f64),
            JsValue::from_f64(step.sigma_minutes as f64),
            JsValue::from_str(&step.step_id),
            JsValue::from_str(&full),
        ])?);
    }

    let habit_stmt =
        database.prepare("UPDATE habits SET avg_minutes = ?1, sigma_minutes = ?2 WHERE id = ?3");
    stmts.push(habit_stmt.bind(&[
        JsValue::from_f64(body.avg_minutes as f64),
        JsValue::from_f64(body.sigma_minutes as f64),
        JsValue::from_str(&full),
    ])?);

    database.batch(stmts).await.map_err(WorkerError::Worker)?;
    Ok(Response::empty()?)
}

#[derive(serde::Deserialize)]
struct IdRow {
    id: String,
}
