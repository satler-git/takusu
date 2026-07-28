use wasm_bindgen::JsValue;
use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::db;
use crate::handlers::d1::safe_all;
use crate::handlers::id_resolver::resolve_task_id;
use crate::handlers::settings::get_timezone;
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::models::{
    CreateTask, HabitRow, Quantity, ScheduleEntry, ScheduleRow, TaskRow, UpdateTask,
};
use crate::validate::{
    validate_minutes, validate_quantity, validate_task_datetimes, validate_title,
};
use takusu_util::{
    Minutes,
    search::{EvalContext, filter_tasks},
};

const TASK_COLS: &str = "id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, habit_id, ical_uid, user_edited, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at, tam.actual_minutes";
const TASK_FROM: &str = "tasks LEFT JOIN task_actual_minutes tam ON tam.task_id = tasks.id";
/// SQL predicate for tasks whose deadline has passed but are not finished.
const OVERDUE_SQL: &str =
    "status NOT IN ('completed', 'skipped') AND datetime(end_at) < datetime('now')";
/// SQL predicate that excludes overdue tasks (completed/skipped or end_at is now or later).
const NOT_OVERDUE_SQL: &str =
    "(status IN ('completed', 'skipped') OR datetime(end_at) >= datetime('now'))";

/// Parse a boolean query parameter. Accepts `true`/`false` (case-insensitive)
/// and the numeric strings `1`/`0` for compatibility with common clients.
pub(crate) fn parse_boolish(s: &str) -> bool {
    let s = s.trim();
    s.eq_ignore_ascii_case("true") || s == "1"
}

pub(crate) fn select_tasks() -> String {
    format!("SELECT {TASK_COLS} FROM {TASK_FROM}")
}

pub async fn list(req: Request, env: Env) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let url = req.url()?;
    let mut sql = format!("{select} WHERE 1=1", select = select_tasks());
    let mut bindings: Vec<JsValue> = Vec::new();
    let mut q: Option<String> = None;
    let mut limit: Option<i64> = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "status" => {
                if v == "overdue" {
                    sql.push_str(" AND ");
                    sql.push_str(OVERDUE_SQL);
                } else {
                    sql.push_str(" AND status = ?");
                    bindings.push(JsValue::from_str(&v));
                }
            }
            "from" => {
                // end_at is NOT NULL, so a simple >= is safe.
                sql.push_str(" AND end_at >= ?");
                bindings.push(JsValue::from_str(&v));
            }
            "until" => {
                // start_at is nullable: NULL <= value evaluates to NULL
                // (excluded). Include tasks with no explicit start time so
                // range queries don't silently drop them.
                sql.push_str(" AND (start_at IS NULL OR start_at <= ?)");
                bindings.push(JsValue::from_str(&v));
            }
            "no_overdue" => {
                if parse_boolish(&v) {
                    sql.push_str(" AND ");
                    sql.push_str(NOT_OVERDUE_SQL);
                }
            }
            "habit_id" => {
                sql.push_str(" AND habit_id = ?");
                bindings.push(JsValue::from_str(&v));
            }
            "ical_uid" => {
                sql.push_str(" AND ical_uid = ?");
                bindings.push(JsValue::from_str(&v));
            }
            "q" => {
                q = Some(v.into_owned());
            }
            "limit" => {
                if let Ok(n) = v.parse::<i64>() {
                    limit = Some(n);
                }
            }
            _ => continue,
        }
    }
    sql.push_str(" ORDER BY created_at DESC");

    // When no post-fetch filter is needed we can push the limit into SQL.
    let post_filter_limit = if q.is_some() {
        limit
    } else {
        if let Some(n) = limit {
            sql.push_str(" LIMIT ?");
            bindings.push(JsValue::from_f64(n as f64));
        }
        None
    };

    let stmt = if bindings.is_empty() {
        database.prepare(&sql)
    } else {
        database.prepare(&sql).bind(&bindings)?
    };
    let mut rows: Vec<TaskRow> = safe_all(&stmt).await?;

    if let Some(ref query_str) = q {
        rows = filter_rows_with_query(&database, rows, query_str).await?;
    }

    if let Some(n) = post_filter_limit {
        rows.truncate(n as usize);
    }

    json_ok(&rows)
}

async fn filter_rows_with_query(
    database: &worker::D1Database,
    rows: Vec<TaskRow>,
    q: &str,
) -> Result<Vec<TaskRow>, WorkerError> {
    let tz = get_timezone(database).await?;
    let now = takusu_util::now_timestamp()
        .map_err(|e| WorkerError::Internal(format!("current time unavailable: {e}")))?;

    let habits_stmt = database.prepare(
        "SELECT id, display_id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed, window_mode, created_at, updated_at FROM habits",
    );
    let habits: Vec<HabitRow> = safe_all(&habits_stmt).await?;

    let schedule_entries: Vec<ScheduleEntry> = {
        let stmt = database.prepare(
            "SELECT id, created_at, updated_at, schedule FROM schedules WHERE id = 'active'",
        );
        let rows = safe_all::<ScheduleRow>(&stmt).await?;
        rows.into_iter()
            .next()
            .map(|r| r.schedule.into_inner())
            .unwrap_or_default()
    };

    let schedule: Vec<(String, (String, String))> = schedule_entries
        .into_iter()
        .map(|e| (e.task_id, (e.start_at.to_string(), e.end_at.to_string())))
        .collect();

    let ctx = EvalContext::new(tz, now, schedule, &rows, &habits);
    filter_tasks(rows, q, &ctx).map_err(WorkerError::BadRequest)
}

pub async fn create(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let body: CreateTask = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_title(&body.title)?;
    // Treat quantity_total / original_quantity_total 0 as unset (same as None) server-side.
    let quantity_total = body.quantity_total.filter(|t| *t != 0);
    let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
    validate_quantity(quantity_total, body.quantity_done, original_quantity_total)?;
    let database = db(&env)?;
    let tz = get_timezone(&database).await?;
    validate_task_datetimes(
        body.start_at.as_ref().map(Some),
        Some(&body.end_at),
        &tz,
        None,
        None,
    )?;
    let id = uuid::Uuid::now_v7().to_string();
    let resolved_depends = resolve_depends(&database, body.depends.as_deref()).await?;
    let depends = crate::models::DependencyList::new(resolved_depends);
    let depends_json = depends.to_json_string();
    let sigma = body
        .sigma_minutes
        .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
    let parallelizable = body.parallelizable.unwrap_or(false);
    let allows_parallel = body.allows_parallel.unwrap_or(false);
    let abandonability = body.abandonability.unwrap_or(0.5.into());

    // Atomically reserve a monotonic display_id from the sequence table.
    // This prevents display_id reuse after task deletion (#186).
    // For habit tasks, use a habit-specific sequence (#380).
    let display_id = if let Some(ref habit_id) = body.habit_id {
        // Use habit-specific sequence. Ensure the sequence entry exists first.
        let insert_stmt = database.prepare(
            "INSERT OR IGNORE INTO habit_task_display_id_seq (habit_id, next_id) VALUES (?1, 1)",
        );
        insert_stmt
            .bind(&[JsValue::from_str(habit_id)])?
            .run()
            .await
            .map_err(WorkerError::Worker)?;
        let seq_stmt = database.prepare(
            "UPDATE habit_task_display_id_seq SET next_id = next_id + 1 WHERE habit_id = ?1 RETURNING next_id - 1 AS display_id",
        );
        let bindings = vec![JsValue::from_str(habit_id)];
        let seq_row: Option<DisplayIdRow> = seq_stmt
            .bind(&bindings)?
            .first(None)
            .await
            .map_err(WorkerError::Worker)?;
        seq_row
            .ok_or_else(|| WorkerError::Internal("habit display_id sequence is empty".into()))?
            .display_id
    } else {
        // Use global task sequence
        let seq_stmt = database.prepare(
            "UPDATE task_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1 AS display_id",
        );
        let seq_row: Option<DisplayIdRow> =
            seq_stmt.first(None).await.map_err(WorkerError::Worker)?;
        seq_row
            .ok_or_else(|| WorkerError::Internal("display_id sequence is empty".into()))?
            .display_id
    };

    let quantity_done = body.quantity_done.unwrap_or_default();
    // A title that fails NFKC normalization stores NULL, excluding the task from
    // similar-task search rather than matching on a misleading empty string (#942).
    let normalized_title = takusu_util::memory::normalize_text(
        &body.title,
        Some(takusu_util::memory::MAX_CONTENT_SCALARS),
    )
    .ok();
    let stmt = database.prepare(
        "INSERT INTO tasks (id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, normalized_title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))"
    );
    stmt.bind(&[
        JsValue::from_str(&id),
        JsValue::from_f64(display_id as f64),
        JsValue::from_str(&body.title),
        body.description
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.start_at
            .map(|t| JsValue::from_str(&t.to_string()))
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&body.end_at.to_string()),
        JsValue::from_f64(body.avg_minutes as f64),
        JsValue::from_f64(sigma as f64),
        JsValue::from_str(&depends_json),
        JsValue::from_bool(parallelizable),
        JsValue::from_bool(allows_parallel),
        JsValue::from_f64(abandonability.into()),
        body.ical_uid
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.habit_id
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::from_bool(body.fixed.unwrap_or(false)),
        body.habit_step_id
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        quantity_total
            .map(|n| JsValue::from_f64(f64::from(n)))
            .unwrap_or(JsValue::NULL),
        JsValue::from_f64(f64::from(quantity_done)),
        body.quantity_unit
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::NULL,
        JsValue::NULL,
        original_quantity_total
            .map(|n| JsValue::from_f64(f64::from(n)))
            .unwrap_or(JsValue::NULL),
        normalized_title
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;

    let row = select_one(&database, &id).await?;
    json_created(&row)
}

pub async fn get(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_task_id(&database, id).await?;
    let row = select_one(&database, &full).await?;
    json_ok(&row)
}

pub async fn update(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: UpdateTask = parse_json(&mut req).await?;
    if let Some(avg) = body.avg_minutes {
        validate_minutes(avg, body.sigma_minutes)?;
    } else if let Some(sigma) = body.sigma_minutes {
        validate_minutes(0, Some(sigma))?;
    }
    if let Some(ref t) = body.title {
        validate_title(t)?;
    }
    let database = db(&env)?;
    let full = resolve_task_id(&database, id).await?;
    let existing = select_one(&database, &full).await?;
    if body.start_at.is_some() || body.end_at.is_some() {
        let tz = get_timezone(&database).await?;
        validate_task_datetimes(
            body.start_at.as_ref().map(|o| o.as_ref()),
            body.end_at.as_ref(),
            &tz,
            existing.start_at.as_ref(),
            Some(&existing.end_at),
        )?;
    }
    // Treat original_quantity_total 0 as unset (same as None) server-side.
    // quantity_total 0 is a clear sentinel handled by the CASE WHEN in the
    // UPDATE below (matches the SQLite backend — #1250).
    let existing_total = existing.quantity_total.filter(|t| *t != 0);
    let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
    validate_quantity(
        body.quantity_total.or(existing_total),
        body.quantity_done.or(Some(existing.quantity_done)),
        original_quantity_total,
    )?;

    let status = body.status.unwrap_or(existing.status);

    let depends_json = if let Some(ref deps) = body.depends {
        let resolved = resolve_depends(&database, Some(deps)).await?;
        Some(crate::models::DependencyList::new(resolved).to_json_string())
    } else {
        None
    };

    // Recompute the normalized title only when the title changes; bind NULL
    // otherwise (or when normalization fails) so COALESCE keeps the stored value
    // (#942).
    let normalized_title = body.title.as_deref().and_then(|t| {
        takusu_util::memory::normalize_text(t, Some(takusu_util::memory::MAX_CONTENT_SCALARS)).ok()
    });

    // Unpack Option<Option<Timestamp>> for start_at.
    // None = no change, Some(None) = clear, Some(Some(ts)) = set.
    // end_at is NOT NULL so it stays Option<Timestamp> with COALESCE.
    let (upd_start, start_val) = match body.start_at {
        None => (0i32, JsValue::NULL),
        Some(None) => (1i32, JsValue::NULL),
        Some(Some(ref ts)) => (1i32, JsValue::from_str(&ts.to_string())),
    };

    let main_stmt = database.prepare(
        "UPDATE tasks SET title=COALESCE(?1,title), description=CASE WHEN ?2='' THEN NULL ELSE COALESCE(?2,description) END, start_at=CASE WHEN ?3=0 THEN start_at ELSE ?4 END, end_at=COALESCE(?5,end_at), avg_minutes=COALESCE(?6,avg_minutes), sigma_minutes=COALESCE(?7,sigma_minutes), depends=COALESCE(?8,depends), parallelizable=COALESCE(?9,parallelizable), allows_parallel=COALESCE(?10,allows_parallel), abandonability=COALESCE(?11,abandonability), status=?12, habit_id=COALESCE(?14,habit_id), user_edited=COALESCE(?15,user_edited), fixed=COALESCE(?16,fixed), habit_step_id=COALESCE(?17,habit_step_id), quantity_total=CASE WHEN ?18=0 THEN NULL ELSE COALESCE(?18,quantity_total) END, quantity_done=COALESCE(?19,quantity_done), quantity_unit=CASE WHEN ?20='' THEN NULL ELSE COALESCE(?20,quantity_unit) END, original_quantity_total=COALESCE(?21,original_quantity_total), normalized_title=COALESCE(?22,normalized_title), updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?13"
    );
    let main_stmt = main_stmt.bind(&[
        body.title
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.description
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::from_f64(upd_start as f64),
        start_val,
        body.end_at
            .map(|t| JsValue::from_str(&t.to_string()))
            .unwrap_or(JsValue::NULL),
        body.avg_minutes
            .map(|n| JsValue::from_f64(n as f64))
            .unwrap_or(JsValue::NULL),
        body.sigma_minutes
            .map(|n| JsValue::from_f64(n as f64))
            .unwrap_or(JsValue::NULL),
        depends_json
            .as_deref()
            .map(JsValue::from_str)
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
        JsValue::from_str(&status.to_string()),
        JsValue::from_str(&full),
        body.habit_id
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.user_edited
            .map(JsValue::from_bool)
            .unwrap_or(JsValue::NULL),
        body.fixed.map(JsValue::from_bool).unwrap_or(JsValue::NULL),
        body.habit_step_id
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.quantity_total
            .map(|n| JsValue::from_f64(f64::from(n)))
            .unwrap_or(JsValue::NULL),
        body.quantity_done
            .map(|n| JsValue::from_f64(f64::from(n)))
            .unwrap_or(JsValue::NULL),
        body.quantity_unit
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        original_quantity_total
            .map(|n| JsValue::from_f64(f64::from(n)))
            .unwrap_or(JsValue::NULL),
        normalized_title
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    ])?;

    // completed_at must follow explicit status transitions: set on
    // completion, clear when leaving completed.
    let mut stmts = vec![main_stmt];
    if body.status.is_some() {
        let completed_stmt = database.prepare(
            "UPDATE tasks SET completed_at = CASE WHEN ?1 = 'completed' AND completed_at IS NULL THEN strftime('%Y-%m-%dT%H:%M:%SZ','now') WHEN ?1 != 'completed' AND completed_at IS NOT NULL THEN NULL ELSE completed_at END WHERE id = ?2",
        );
        stmts.push(completed_stmt.bind(&[
            JsValue::from_str(&status.to_string()),
            JsValue::from_str(&full),
        ])?);

        // #1044: moving to a terminal status should close any open work
        // session so active time is not left dangling.
        if status == takusu_util::TaskStatus::Skipped
            || status == takusu_util::TaskStatus::Completed
        {
            let now = takusu_util::now_rfc3339();
            let session_stmt = database.prepare(
                "UPDATE task_work_sessions SET ended_at = ?1 WHERE task_id = ?2 AND ended_at IS NULL",
            );
            stmts.push(session_stmt.bind(&[JsValue::from_str(&now), JsValue::from_str(&full)])?);
        }
    }

    database.batch(stmts).await.map_err(WorkerError::Worker)?;

    let row = select_one(&database, &full).await?;
    json_ok(&row)
}

pub async fn replace(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: CreateTask = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_title(&body.title)?;
    // Treat quantity_total / original_quantity_total 0 as unset (same as None) server-side.
    let quantity_total = body.quantity_total.filter(|t| *t != 0);
    let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
    validate_quantity(
        quantity_total,
        Some(Quantity::default()),
        original_quantity_total,
    )?;
    let database = db(&env)?;
    let tz = get_timezone(&database).await?;
    validate_task_datetimes(
        body.start_at.as_ref().map(Some),
        Some(&body.end_at),
        &tz,
        None,
        None,
    )?;
    let full = resolve_task_id(&database, id).await?;
    let resolved_depends = resolve_depends(&database, body.depends.as_deref()).await?;
    let depends_json = crate::models::DependencyList::new(resolved_depends).to_json_string();
    let sigma = body
        .sigma_minutes
        .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
    let parallelizable = body.parallelizable.unwrap_or(false);
    let allows_parallel = body.allows_parallel.unwrap_or(false);
    let abandonability = body.abandonability.unwrap_or(0.5.into());

    let normalized_title = takusu_util::memory::normalize_text(
        &body.title,
        Some(takusu_util::memory::MAX_CONTENT_SCALARS),
    )
    .ok();

    let stmt = database.prepare(
        "UPDATE tasks SET title=?1, description=?2, start_at=?3, end_at=?4, avg_minutes=?5, sigma_minutes=?6, depends=?7, parallelizable=?8, allows_parallel=?9, abandonability=?10, status='pending', habit_id=COALESCE(?11,habit_id), fixed=?12, habit_step_id=?13, quantity_total=COALESCE(?14, quantity_total), quantity_done=0, quantity_unit=COALESCE(?15, quantity_unit), completed_at=?16, split_from_task_id=COALESCE(?17, split_from_task_id), original_quantity_total=COALESCE(?18, original_quantity_total), user_edited=CASE WHEN habit_id IS NOT NULL THEN 1 ELSE user_edited END, normalized_title=?19, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?20"
    );
    stmt.bind(&[
        JsValue::from_str(&body.title),
        body.description
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        body.start_at
            .map(|t| JsValue::from_str(&t.to_string()))
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&body.end_at.to_string()),
        JsValue::from_f64(body.avg_minutes as f64),
        JsValue::from_f64(sigma as f64),
        JsValue::from_str(&depends_json),
        JsValue::from_bool(parallelizable),
        JsValue::from_bool(allows_parallel),
        JsValue::from_f64(abandonability.into()),
        body.habit_id
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::from_bool(body.fixed.unwrap_or(false)),
        body.habit_step_id
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        quantity_total
            .map(|n| JsValue::from_f64(f64::from(n)))
            .unwrap_or(JsValue::NULL),
        body.quantity_unit
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::NULL,
        JsValue::NULL,
        original_quantity_total
            .map(|n| JsValue::from_f64(f64::from(n)))
            .unwrap_or(JsValue::NULL),
        normalized_title
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&full),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;

    let row = select_one(&database, &full).await?;
    json_ok(&row)
}

pub async fn delete(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let full = resolve_task_id(&database, id).await?;
    // Break split-task self-references, then delete child rows before the
    // parent so D1's foreign-key enforcement stays consistent with SQLite.
    let stmts = vec![
        database
            .prepare("UPDATE tasks SET split_from_task_id = NULL WHERE split_from_task_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM google_cal_events WHERE task_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM task_work_sessions WHERE task_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM progress_events WHERE task_id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
        database
            .prepare("DELETE FROM tasks WHERE id = ?1")
            .bind(&[JsValue::from_str(&full)])?,
    ];
    database.batch(stmts).await.map_err(WorkerError::Worker)?;
    Ok(Response::empty()?)
}

pub async fn select_one(database: &worker::D1Database, id: &str) -> Result<TaskRow, WorkerError> {
    let stmt = database.prepare(format!("{select} WHERE id = ?1", select = select_tasks()));
    let row: Option<TaskRow> = stmt
        .bind(&[JsValue::from_str(id)])?
        .first(None)
        .await
        .map_err(WorkerError::Worker)?;
    row.ok_or_else(|| WorkerError::NotFound(format!("task {id} not found")))
}

/// Resolve a list of dependency references to full UUID strings.
pub(crate) async fn resolve_depends(
    database: &worker::D1Database,
    deps: Option<&[String]>,
) -> Result<Vec<String>, WorkerError> {
    let Some(deps) = deps else {
        return Ok(Vec::new());
    };
    let mut resolved = Vec::with_capacity(deps.len());
    for d in deps {
        resolved.push(resolve_task_id(database, d).await?);
    }
    Ok(resolved)
}

/// Allocate the next monotonic display_id from the sequence table.
pub(crate) async fn allocate_display_id(
    database: &worker::D1Database,
    habit_id: Option<&str>,
) -> Result<i64, WorkerError> {
    if let Some(habit_id) = habit_id {
        let insert_stmt = database.prepare(
            "INSERT OR IGNORE INTO habit_task_display_id_seq (habit_id, next_id) VALUES (?1, 1)",
        );
        insert_stmt
            .bind(&[JsValue::from_str(habit_id)])?
            .run()
            .await
            .map_err(WorkerError::Worker)?;
        let seq_stmt = database.prepare(
            "UPDATE habit_task_display_id_seq SET next_id = next_id + 1 WHERE habit_id = ?1 RETURNING next_id - 1 AS display_id",
        );
        let row: Option<DisplayIdRow> = seq_stmt
            .bind(&[JsValue::from_str(habit_id)])?
            .first(None)
            .await
            .map_err(WorkerError::Worker)?;
        row.ok_or_else(|| WorkerError::Internal("habit display_id sequence is empty".into()))
            .map(|r| r.display_id)
    } else {
        let seq_stmt = database.prepare(
            "UPDATE task_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1 AS display_id",
        );
        let row: Option<DisplayIdRow> = seq_stmt.first(None).await.map_err(WorkerError::Worker)?;
        row.ok_or_else(|| WorkerError::Internal("display_id sequence is empty".into()))
            .map(|r| r.display_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_boolish_accepts_numeric_and_literal_true() {
        assert!(parse_boolish("true"));
        assert!(parse_boolish("True"));
        assert!(parse_boolish("TRUE"));
        assert!(parse_boolish("1"));
        assert!(parse_boolish(" 1 "));
        assert!(!parse_boolish("false"));
        assert!(!parse_boolish("False"));
        assert!(!parse_boolish("0"));
        assert!(!parse_boolish("no"));
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct DisplayIdRow {
    pub(crate) display_id: i64,
}
