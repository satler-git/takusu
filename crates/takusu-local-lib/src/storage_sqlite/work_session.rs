//! Work session / progress event storage implementation for `SqliteStorage`.
//!
//! Work sessions are top-level entities; a task is an optional attachment.
//! This module holds the `Storage` trait methods that operate on work sessions
//! and the helpers used by task lifecycle code in `super`.

use takusu_contracts::{
    AttachWorkSession, ConvertWorkSession, ProgressEventRow, RecordWorkSessionProgress,
    StartWorkSession, StorageError, TaskProgress, TaskRow, WorkSessionProgressResult,
    WorkSessionRow, storage::StorageResult,
};
use takusu_types::{Quantity, TaskStatus, Timestamp};

use super::{SELECT_TASK_BY_ID, map_err, progress_request_hash, resolve_task_id};

const SELECT_WORK_SESSION_BY_ID: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions WHERE id = ?";
const SELECT_WORK_SESSIONS_BY_TASK: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions WHERE task_id = ? ORDER BY started_at ASC";
const SELECT_ALL_WORK_SESSIONS: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions ORDER BY started_at DESC";
const SELECT_OPEN_WORK_SESSION_BY_TASK: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions WHERE task_id = ? AND ended_at IS NULL";

const SELECT_PROGRESS_EVENT_BY_ID: &str = "SELECT id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE id = ?";
const SELECT_PROGRESS_EVENTS_BY_TASK: &str = "SELECT id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE task_id = ? ORDER BY at ASC, id ASC";
const SELECT_PROGRESS_EVENTS_BY_SESSION: &str = "SELECT id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE work_session_id = ? ORDER BY at ASC, id ASC";
const LAST_PROGRESS_EVENT: &str = "SELECT id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE work_session_id = ? ORDER BY at DESC, id DESC LIMIT 1";

/// Close any open work session for `task_id` so active time is not left
/// dangling when the task is no longer active (#1044, #1438).
pub(crate) async fn cleanup_work_sessions<'c, E>(executor: E, full: &str) -> StorageResult<()>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    let now = takusu_types::now_rfc3339();
    sqlx::query("UPDATE work_sessions SET ended_at = ? WHERE task_id = ? AND ended_at IS NULL")
        .bind(&now)
        .bind(full)
        .execute(executor)
        .await
        .map_err(map_err)?;
    Ok(())
}

/// Active minutes for a work session (closed or open).
pub(crate) fn session_minutes(session: &WorkSessionRow) -> i64 {
    match &session.ended_at {
        Some(end) => {
            takusu_types::minutes_between(&session.started_at.to_string(), &end.to_string())
        }
        None => takusu_types::minutes_between(
            &session.started_at.to_string(),
            &takusu_types::now_rfc3339(),
        ),
    }
}

/// Compute updated avg_minutes / sigma_minutes from a new positive progress
/// observation. See doc/proposal.typ WI-9 for the estimate-update formula.
pub(crate) async fn compute_updated_estimate<'c, E>(
    executor: E,
    task_id: &str,
    avg_minutes: i64,
    sigma_minutes: i64,
    quantity_total: Option<i64>,
    active_minutes: i64,
    delta_quantity: i64,
) -> StorageResult<(i64, i64)>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    // Collect all positive progress observations for this task.
    let events: Vec<ProgressEventRow> = sqlx::query_as(
        "SELECT id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE task_id = ? AND delta_quantity > 0 AND active_minutes > 0 ORDER BY id ASC",
    )
    .bind(task_id)
    .fetch_all(executor)
    .await
    .map_err(map_err)?;

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

/// Split the unfinished remainder off a task that is being completed with only
/// partial progress (#1419). `task.quantity_done` stays on the original (which
/// the caller then completes); `quantity_total - quantity_done` becomes a new
/// pending task. Requires `0 < quantity_done < quantity_total`.
async fn split_remainder_on_complete(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task: &TaskRow,
) -> StorageResult<()> {
    let total = task
        .quantity_total
        .expect("partial completion requires a quantity_total");
    let retained = task.quantity_done;
    let remainder_quantity = Quantity::new(total.get() - retained.get())
        .expect("partial completion guarantees quantity_done < quantity_total");
    let original_quantity_total = task
        .original_quantity_total
        .filter(|t| *t != 0)
        .unwrap_or(total);

    let display_id: i64 = sqlx::query_scalar(
        "UPDATE task_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(map_err)?;

    let remainder_id = uuid::Uuid::now_v7().to_string();
    let normalized_title = takusu_search::memory::normalize_text(
        &task.title,
        Some(takusu_search::memory::MAX_CONTENT_SCALARS),
    )
    .ok();
    let depends = takusu_types::DependencyList::new(Vec::new());

    sqlx::query(
        "INSERT INTO tasks (id, display_id, title, normalized_title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    )
    .bind(&remainder_id)
    .bind(display_id)
    .bind(&task.title)
    .bind(&normalized_title)
    .bind(task.description.as_ref())
    .bind(task.start_at)
    .bind(task.end_at)
    .bind(task.avg_minutes)
    .bind(task.sigma_minutes)
    .bind(&depends)
    .bind(task.parallelizable)
    .bind(task.allows_parallel)
    .bind(task.abandonability)
    .bind(None::<String>) // ical_uid
    .bind(None::<String>) // habit_id
    .bind(task.fixed)
    .bind(None::<String>) // habit_step_id
    .bind(remainder_quantity)
    .bind(0i64)
    .bind(task.quantity_unit.as_ref())
    .bind(None::<String>) // completed_at
    .bind(&task.id)
    .bind(Some(original_quantity_total))
    .execute(&mut **tx)
    .await
    .map_err(map_err)?;

    sqlx::query(
        "UPDATE tasks SET quantity_total = ?, original_quantity_total = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
    )
    .bind(retained)
    .bind(Some(original_quantity_total))
    .bind(&task.id)
    .execute(&mut **tx)
    .await
    .map_err(map_err)?;

    Ok(())
}

pub(crate) async fn start_work_session(
    storage: &super::SqliteStorage,
    body: &StartWorkSession,
    operation_id: Option<&str>,
) -> StorageResult<WorkSessionRow> {
    let payload = serde_json::json!({"op": "start_work_session", "body": body}).to_string();
    let request_hash = progress_request_hash(&payload, operation_id);

    let mut tx = storage.pool().begin().await.map_err(map_err)?;
    if let Some(op_id) = operation_id
        && let Some(stored) =
            super::SqliteStorage::check_progress_idempotency(&mut *tx, op_id, &request_hash).await?
    {
        return stored;
    }

    let mut linked_task: Option<TaskRow> = None;
    let mut task_id: Option<String> = None;

    if let Some(ref id) = body.task_id {
        let full = resolve_task_id(&mut *tx, id).await?;
        let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(&full)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound(format!("task {id} not found")),
                other => StorageError::Internal(other.to_string()),
            })?;
        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(StorageError::BadRequest(format!(
                "cannot start work session for a {} task",
                task.status
            )));
        }
        // At most one open work session per task; concurrent sessions are
        // only allowed across different tasks.
        let existing: Option<WorkSessionRow> =
            sqlx::query_as::<_, WorkSessionRow>(SELECT_OPEN_WORK_SESSION_BY_TASK)
                .bind(&full)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_err)?;
        if existing.is_some() {
            return Err(StorageError::BadRequest(format!(
                "task {id} already has an open work session"
            )));
        }
        task_id = Some(full);
        linked_task = Some(task);
    }

    let now = takusu_types::now_rfc3339();
    let id = uuid::Uuid::now_v7().to_string();
    let title = body
        .title
        .clone()
        .or_else(|| linked_task.as_ref().map(|t| t.title.clone()));
    let note = body.note.clone();
    let quantity_total = body
        .quantity_total
        .or(linked_task.as_ref().and_then(|t| t.quantity_total))
        .filter(|q| *q != 0);
    let quantity_unit = body
        .quantity_unit
        .clone()
        .or_else(|| linked_task.as_ref().and_then(|t| t.quantity_unit.clone()));

    sqlx::query(
            "INSERT INTO work_sessions (id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)",
        )
        .bind(&id)
        .bind(&task_id)
        .bind(&title)
        .bind(&note)
        .bind(quantity_total)
        .bind(Quantity::default())
        .bind(quantity_unit.as_deref())
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    if let Some(ref full) = task_id {
        sqlx::query(
                "UPDATE tasks SET status = 'in_progress', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
            )
            .bind(full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
    }

    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(&id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

    if let Some(op_id) = operation_id {
        super::SqliteStorage::record_progress_operation(&mut *tx, op_id, &request_hash, &session)
            .await?;
    }
    tx.commit().await.map_err(map_err)?;
    Ok(session)
}

pub(crate) async fn pause_work_session(
    storage: &super::SqliteStorage,
    id: &str,
    operation_id: Option<&str>,
) -> StorageResult<WorkSessionRow> {
    let payload = serde_json::json!({"op": "pause_work_session", "id": id}).to_string();
    let request_hash = progress_request_hash(&payload, operation_id);

    let mut tx = storage.pool().begin().await.map_err(map_err)?;
    if let Some(op_id) = operation_id
        && let Some(stored) =
            super::SqliteStorage::check_progress_idempotency(&mut *tx, op_id, &request_hash).await?
    {
        return stored;
    }

    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                StorageError::NotFound(format!("work session {id} not found"))
            }
            other => StorageError::Internal(other.to_string()),
        })?;

    let now = takusu_types::now_rfc3339();
    let was_open = session.ended_at.is_none();
    sqlx::query("UPDATE work_sessions SET ended_at = COALESCE(ended_at, ?) WHERE id = ?")
        .bind(&now)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    if was_open && let Some(ref task_id) = session.task_id {
        let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        if task.status != TaskStatus::Completed && task.status != TaskStatus::Skipped {
            sqlx::query(
                    "UPDATE tasks SET status = 'scheduled', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
                )
                .bind(task_id)
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
        }
    }

    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

    if let Some(op_id) = operation_id {
        super::SqliteStorage::record_progress_operation(&mut *tx, op_id, &request_hash, &session)
            .await?;
    }
    tx.commit().await.map_err(map_err)?;
    Ok(session)
}

pub(crate) async fn complete_work_session(
    storage: &super::SqliteStorage,
    id: &str,
    operation_id: Option<&str>,
) -> StorageResult<WorkSessionRow> {
    let payload = serde_json::json!({"op": "complete_work_session", "id": id}).to_string();
    let request_hash = progress_request_hash(&payload, operation_id);

    let mut tx = storage.pool().begin().await.map_err(map_err)?;
    if let Some(op_id) = operation_id
        && let Some(stored) =
            super::SqliteStorage::check_progress_idempotency(&mut *tx, op_id, &request_hash).await?
    {
        return stored;
    }

    let now_rfc = takusu_types::now_rfc3339();
    let now_ts = Timestamp::now();
    sqlx::query("UPDATE work_sessions SET ended_at = COALESCE(ended_at, ?) WHERE id = ?")
        .bind(&now_rfc)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    // Close the session before measuring active minutes.
    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                StorageError::NotFound(format!("work session {id} not found"))
            }
            other => StorageError::Internal(other.to_string()),
        })?;

    if let Some(ref task_id) = session.task_id {
        let task_before: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;

        if task_before.status != TaskStatus::Completed && task_before.status != TaskStatus::Skipped
        {
            let sessions: Vec<WorkSessionRow> =
                sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSIONS_BY_TASK)
                    .bind(task_id)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(map_err)?;
            let total_active_minutes: i64 = sessions.iter().map(session_minutes).sum();

            // #1419 (P2): genuine partial progress (0 < done < total) splits
            // the unfinished remainder into a new task instead of silently
            // inflating quantity_done to the total. Zero progress keeps the
            // previous "declared done" behaviour.
            if let Some(total) = task_before.quantity_total
                && task_before.quantity_done > 0
                && task_before.quantity_done < total
            {
                split_remainder_on_complete(&mut tx, &task_before).await?;
            }

            // Refetch: after a split the original's quantity_total equals the
            // completed quantity, so completing it inflates nothing.
            let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
                .bind(task_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_err)?;

            let quantity_done = task.quantity_total.unwrap_or(task.quantity_done);
            let delta_quantity = quantity_done.get() - task.quantity_done.get();

            let (new_avg, new_sigma) = if delta_quantity > 0 && total_active_minutes > 0 {
                compute_updated_estimate(
                    &mut *tx,
                    task_id,
                    task.avg_minutes,
                    task.sigma_minutes,
                    task.quantity_total.map(|q| q.get()),
                    total_active_minutes,
                    delta_quantity,
                )
                .await?
            } else if task.quantity_total.is_none() && total_active_minutes > 0 {
                (total_active_minutes, task.sigma_minutes)
            } else {
                (task.avg_minutes, task.sigma_minutes)
            };

            let status = TaskStatus::Completed;
            let completed_at = task.completed_at.or(Some(now_ts));

            sqlx::query(
                    "UPDATE tasks SET status = ?, completed_at = ?, quantity_done = ?, avg_minutes = ?, sigma_minutes = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
                )
                .bind(status.to_string())
                .bind(completed_at)
                .bind(quantity_done)
                .bind(new_avg)
                .bind(new_sigma)
                .bind(task_id)
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;

            if total_active_minutes > 0 {
                let event_id = uuid::Uuid::now_v7().to_string();
                sqlx::query(
                        "INSERT INTO progress_events (id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&event_id)
                    .bind(id)
                    .bind(task_id)
                    .bind(&now_rfc)
                    .bind(Some(quantity_done))
                    .bind(Some(delta_quantity))
                    .bind(total_active_minutes)
                    .bind("completed")
                    .execute(&mut *tx)
                    .await
                    .map_err(map_err)?;
            }
        }
    }

    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

    if let Some(op_id) = operation_id {
        super::SqliteStorage::record_progress_operation(&mut *tx, op_id, &request_hash, &session)
            .await?;
    }
    tx.commit().await.map_err(map_err)?;
    Ok(session)
}

pub(crate) async fn record_work_session_progress(
    storage: &super::SqliteStorage,
    id: &str,
    body: &RecordWorkSessionProgress,
    operation_id: Option<&str>,
) -> StorageResult<WorkSessionProgressResult> {
    let payload = serde_json::json!({"op": "record_work_session_progress", "id": id, "body": body})
        .to_string();
    let request_hash = progress_request_hash(&payload, operation_id);

    let mut tx = storage.pool().begin().await.map_err(map_err)?;
    if let Some(op_id) = operation_id
        && let Some(stored) =
            super::SqliteStorage::check_progress_idempotency(&mut *tx, op_id, &request_hash).await?
    {
        return stored;
    }

    let mut session: WorkSessionRow =
        sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => {
                    StorageError::NotFound(format!("work session {id} not found"))
                }
                other => StorageError::Internal(other.to_string()),
            })?;

    // Update the session's total when the caller supplies a new one.
    // A value of 0 is treated as "clear the total", matching create/update.
    if let Some(raw_total) = body.quantity_total {
        let desired = (raw_total != 0).then_some(raw_total);
        if desired != session.quantity_total {
            sqlx::query("UPDATE work_sessions SET quantity_total = ? WHERE id = ?")
                .bind(desired)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
            session = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(map_err)?;
        }
    }

    if let Some(total) = session.quantity_total
        && body.quantity_done > total
    {
        return Err(StorageError::BadRequest(format!(
            "quantity_done cannot exceed quantity_total ({} > {})",
            body.quantity_done, total
        )));
    }

    let mut linked_task: Option<TaskRow> = None;
    if let Some(ref task_id) = session.task_id {
        let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(StorageError::BadRequest(format!(
                "cannot record progress on a {} task",
                task.status
            )));
        }

        // Keep the linked task's total in sync with the session total.
        if let Some(raw_total) = body.quantity_total {
            let desired = (raw_total != 0).then_some(raw_total);
            if desired != task.quantity_total {
                sqlx::query("UPDATE tasks SET quantity_total = ? WHERE id = ?")
                    .bind(desired)
                    .bind(task_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_err)?;
            }
        }

        // Re-fetch after any total update so validation uses the new total.
        let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        linked_task = Some(task);
    }

    let delta = body.quantity_done.get() - session.quantity_done.get();

    if delta == 0 {
        let suggests_completion = match &linked_task {
            Some(task) => task
                .quantity_total
                .map(|total| task.quantity_done >= total)
                .unwrap_or(false),
            None => session
                .quantity_total
                .map(|total| body.quantity_done >= total)
                .unwrap_or(false),
        };
        let result = WorkSessionProgressResult {
            work_session: session,
            task: linked_task,
            event: None,
            suggests_completion,
        };
        if let Some(op_id) = operation_id {
            super::SqliteStorage::record_progress_operation(
                &mut *tx,
                op_id,
                &request_hash,
                &result,
            )
            .await?;
        }
        tx.commit().await.map_err(map_err)?;
        return Ok(result);
    }

    // Increasing progress requires an open session to measure active time.
    // Corrections (decreasing or keeping quantity_done) are allowed without one.
    if session.ended_at.is_some() && body.quantity_done > session.quantity_done {
        return Err(StorageError::BadRequest(
            "cannot record progress on a closed work session".into(),
        ));
    }

    let now_rfc = takusu_types::now_rfc3339();
    let active_minutes = if session.ended_at.is_none() && delta > 0 {
        let last_event: Option<ProgressEventRow> =
            sqlx::query_as::<_, ProgressEventRow>(LAST_PROGRESS_EVENT)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_err)?;
        let base = if let Some(ref ev) = last_event {
            if ev.at >= session.started_at {
                ev.at
            } else {
                session.started_at
            }
        } else {
            session.started_at
        };
        takusu_types::minutes_between_ts(base, Timestamp::now())
    } else {
        0
    };

    let event_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
            "INSERT INTO progress_events (id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event_id)
        .bind(id)
        .bind(&session.task_id)
        .bind(&now_rfc)
        .bind(Some(body.quantity_done))
        .bind(Some(delta))
        .bind(active_minutes)
        .bind(body.note.as_ref())
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    sqlx::query("UPDATE work_sessions SET quantity_done = ? WHERE id = ?")
        .bind(body.quantity_done)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    let mut result_task: Option<TaskRow> = None;
    let mut suggests_completion = false;

    if let Some(ref task) = linked_task {
        let task_id = task.id.clone();
        let task_delta = body.quantity_done.get() - session.quantity_done.get();
        let new_done_i64 = task.quantity_done.get() + task_delta;
        let new_done = Quantity::new(new_done_i64)
            .map_err(|e| StorageError::BadRequest(format!("invalid task quantity: {e}")))?;

        if let Some(total) = task.quantity_total {
            if new_done > total {
                return Err(StorageError::BadRequest(format!(
                    "quantity_done would exceed quantity_total ({} > {})",
                    new_done, total
                )));
            }
            suggests_completion = new_done >= total;
        }

        let mut new_avg = task.avg_minutes;
        let mut new_sigma = task.sigma_minutes;
        if task_delta > 0 && active_minutes > 0 {
            let (avg, sigma) = compute_updated_estimate(
                &mut *tx,
                &task_id,
                task.avg_minutes,
                task.sigma_minutes,
                task.quantity_total.map(|q| q.get()),
                active_minutes,
                task_delta,
            )
            .await?;
            new_avg = avg;
            new_sigma = sigma;
        }

        let new_status = if task.status == TaskStatus::Completed {
            TaskStatus::Completed
        } else if task_delta < 0 {
            task.status
        } else {
            TaskStatus::InProgress
        };

        sqlx::query(
                "UPDATE tasks SET quantity_done = ?, avg_minutes = ?, sigma_minutes = ?, status = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
            )
            .bind(new_done)
            .bind(new_avg)
            .bind(new_sigma)
            .bind(new_status.to_string())
            .bind(&task_id)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;

        let task_row: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(&task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        result_task = Some(task_row);
    } else if let Some(total) = session.quantity_total {
        suggests_completion = body.quantity_done >= total;
    }

    let event: ProgressEventRow =
        sqlx::query_as::<_, ProgressEventRow>(SELECT_PROGRESS_EVENT_BY_ID)
            .bind(&event_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;

    let work_session: WorkSessionRow =
        sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;

    let result = WorkSessionProgressResult {
        work_session,
        task: result_task,
        event: Some(event),
        suggests_completion,
    };
    if let Some(op_id) = operation_id {
        super::SqliteStorage::record_progress_operation(&mut *tx, op_id, &request_hash, &result)
            .await?;
    }
    tx.commit().await.map_err(map_err)?;
    Ok(result)
}

pub(crate) async fn get_work_session(
    storage: &super::SqliteStorage,
    id: &str,
) -> StorageResult<WorkSessionRow> {
    sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(storage.pool())
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                StorageError::NotFound(format!("work session {id} not found"))
            }
            other => StorageError::Internal(other.to_string()),
        })
}

pub(crate) async fn list_work_sessions(
    storage: &super::SqliteStorage,
    task_id: Option<&str>,
) -> StorageResult<Vec<WorkSessionRow>> {
    if let Some(id) = task_id {
        let full = resolve_task_id(storage.pool(), id).await?;
        sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSIONS_BY_TASK)
            .bind(&full)
            .fetch_all(storage.pool())
            .await
            .map_err(map_err)
    } else {
        sqlx::query_as::<_, WorkSessionRow>(SELECT_ALL_WORK_SESSIONS)
            .fetch_all(storage.pool())
            .await
            .map_err(map_err)
    }
}

pub(crate) async fn attach_work_session(
    storage: &super::SqliteStorage,
    id: &str,
    body: &AttachWorkSession,
    operation_id: Option<&str>,
) -> StorageResult<WorkSessionRow> {
    let payload =
        serde_json::json!({"op": "attach_work_session", "id": id, "body": body}).to_string();
    let request_hash = progress_request_hash(&payload, operation_id);

    let mut tx = storage.pool().begin().await.map_err(map_err)?;
    if let Some(op_id) = operation_id
        && let Some(stored) =
            super::SqliteStorage::check_progress_idempotency(&mut *tx, op_id, &request_hash).await?
    {
        return stored;
    }

    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                StorageError::NotFound(format!("work session {id} not found"))
            }
            other => StorageError::Internal(other.to_string()),
        })?;

    let full = resolve_task_id(&mut *tx, &body.task_id).await?;
    let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
        .bind(&full)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                StorageError::NotFound(format!("task {} not found", body.task_id))
            }
            other => StorageError::Internal(other.to_string()),
        })?;

    if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
        return Err(StorageError::BadRequest(format!(
            "cannot attach a work session to a {} task",
            task.status
        )));
    }

    sqlx::query("UPDATE work_sessions SET task_id = ? WHERE id = ?")
        .bind(&full)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    sqlx::query("UPDATE progress_events SET task_id = ? WHERE work_session_id = ?")
        .bind(&full)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    if session.ended_at.is_none() {
        sqlx::query(
                "UPDATE tasks SET status = 'in_progress', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
            )
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
    }

    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

    if let Some(op_id) = operation_id {
        super::SqliteStorage::record_progress_operation(&mut *tx, op_id, &request_hash, &session)
            .await?;
    }
    tx.commit().await.map_err(map_err)?;
    Ok(session)
}

pub(crate) async fn convert_work_session(
    storage: &super::SqliteStorage,
    id: &str,
    body: &ConvertWorkSession,
    operation_id: Option<&str>,
) -> StorageResult<TaskRow> {
    let payload =
        serde_json::json!({"op": "convert_work_session", "id": id, "body": body}).to_string();
    let request_hash = progress_request_hash(&payload, operation_id);

    let mut tx = storage.pool().begin().await.map_err(map_err)?;
    if let Some(op_id) = operation_id
        && let Some(stored) =
            super::SqliteStorage::check_progress_idempotency(&mut *tx, op_id, &request_hash).await?
    {
        return stored;
    }

    let now_rfc = takusu_types::now_rfc3339();
    let now_ts = Timestamp::now();
    sqlx::query("UPDATE work_sessions SET ended_at = COALESCE(ended_at, ?) WHERE id = ?")
        .bind(&now_rfc)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    // Refetch after possibly closing the session.
    let session: WorkSessionRow = sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSION_BY_ID)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => {
                StorageError::NotFound(format!("work session {id} not found"))
            }
            other => StorageError::Internal(other.to_string()),
        })?;

    let active_minutes = session_minutes(&session);
    let end_at = session.ended_at.unwrap_or(now_ts);
    let status = body.status.unwrap_or(TaskStatus::Completed);

    let display_id: i64 = sqlx::query_scalar(
        "UPDATE task_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1",
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(map_err)?;

    let task_id = uuid::Uuid::now_v7().to_string();
    let title = body
        .title
        .clone()
        .or(session.title.clone())
        .unwrap_or_else(|| "converted session".into());
    let normalized_title = takusu_search::memory::normalize_text(
        &title,
        Some(takusu_search::memory::MAX_CONTENT_SCALARS),
    )
    .ok();
    let description = session.note.clone();
    let quantity_total = session.quantity_total.filter(|q| *q != 0);
    let quantity_unit = session.quantity_unit.as_deref();
    let fixed = body.fixed.unwrap_or(true);
    // Estimate from the session's progress observations, weighting each by
    // the quantity it accomplished (#1419). Falls back to the raw session
    // duration when there is no quantity total or no positive observation.
    let observations: Vec<(i64, i64)> =
        sqlx::query_as::<_, ProgressEventRow>(SELECT_PROGRESS_EVENTS_BY_SESSION)
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_err)?
            .iter()
            .filter(|e| e.active_minutes > 0 && e.delta_quantity.unwrap_or(0) > 0)
            .map(|e| (e.active_minutes, e.delta_quantity.unwrap_or(1).max(1)))
            .collect();
    let (avg_minutes, sigma_minutes) =
        takusu_types::weighted_estimate(&observations, quantity_total.map(|q| q.get())).unwrap_or(
            (
                active_minutes,
                takusu_types::Minutes(active_minutes).to_slots().0.max(1),
            ),
        );
    let completed_at = if status == TaskStatus::Completed {
        Some(now_ts)
    } else {
        None
    };
    let depends = takusu_types::DependencyList::new(Vec::new());

    sqlx::query(
            "INSERT INTO tasks (id, display_id, title, normalized_title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id)
        .bind(display_id)
        .bind(&title)
        .bind(&normalized_title)
        .bind(&description)
        .bind(session.started_at)
        .bind(end_at)
        .bind(avg_minutes)
        .bind(sigma_minutes)
        .bind(&depends)
        .bind(false)
        .bind(false)
        .bind(takusu_types::Abandonability::default())
        .bind(status.to_string())
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(fixed)
        .bind(None::<String>)
        .bind(quantity_total)
        .bind(session.quantity_done)
        .bind(quantity_unit)
        .bind(completed_at)
        .bind(None::<String>)
        .bind(None::<Quantity>)
        .bind(&now_rfc)
        .bind(&now_rfc)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    sqlx::query("UPDATE work_sessions SET task_id = ? WHERE id = ?")
        .bind(&task_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    sqlx::query("UPDATE progress_events SET task_id = ? WHERE work_session_id = ?")
        .bind(&task_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

    let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
        .bind(&task_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

    if let Some(op_id) = operation_id {
        super::SqliteStorage::record_progress_operation(&mut *tx, op_id, &request_hash, &task)
            .await?;
    }
    tx.commit().await.map_err(map_err)?;
    Ok(task)
}

pub(crate) async fn get_task_progress(
    storage: &super::SqliteStorage,
    id: &str,
) -> StorageResult<TaskProgress> {
    let full = resolve_task_id(storage.pool(), id).await?;
    let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
        .bind(&full)
        .fetch_one(storage.pool())
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => StorageError::NotFound(format!("task {id} not found")),
            other => StorageError::Internal(other.to_string()),
        })?;

    let sessions: Vec<WorkSessionRow> =
        sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSIONS_BY_TASK)
            .bind(&full)
            .fetch_all(storage.pool())
            .await
            .map_err(map_err)?;

    let events: Vec<ProgressEventRow> =
        sqlx::query_as::<_, ProgressEventRow>(SELECT_PROGRESS_EVENTS_BY_TASK)
            .bind(&full)
            .fetch_all(storage.pool())
            .await
            .map_err(map_err)?;

    let open_session = sessions.iter().find(|s| s.ended_at.is_none()).cloned();
    let total_active_minutes = sessions.iter().map(session_minutes).sum();

    Ok(TaskProgress {
        task,
        open_session,
        sessions,
        events,
        total_active_minutes,
    })
}
