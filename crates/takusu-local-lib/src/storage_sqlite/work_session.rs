//! Work session / progress event storage implementation for `SqliteStorage`.
//!
//! Work sessions are top-level entities; a task is an optional attachment.
//! This module holds the `Storage` trait methods that operate on work sessions
//! and the helpers used by task lifecycle code in `super`.

use takusu_contracts::{
    AttachWorkSession, ConvertWorkSession, EstimatorBand, EstimatorResult, EstimatorStateRow,
    EvaluationEstimator, EvaluationTaskProgress, ProgressEventRow, RecordWorkSessionProgress,
    StartWorkSession, StorageError, TaskProgress, TaskRow, WorkSessionProgressResult,
    WorkSessionRow, storage::StorageResult,
};
use takusu_types::estimator::{
    DurationDistribution, InterventionBand, effective_distribution, next_crossing_time,
    progress_posterior, survival_probability,
};
use takusu_types::{Quantity, TaskStatus, Timestamp};

use super::{SELECT_TASK_BY_ID, map_err, progress_request_hash, resolve_task_id};

const SELECT_WORK_SESSION_BY_ID: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions WHERE id = ?";
const SELECT_WORK_SESSIONS_BY_TASK: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions WHERE task_id = ? ORDER BY started_at ASC";
const SELECT_ALL_WORK_SESSIONS: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions ORDER BY started_at DESC";
const SELECT_OPEN_WORK_SESSION_BY_TASK: &str = "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions WHERE task_id = ? AND ended_at IS NULL";

const SELECT_PROGRESS_EVENT_BY_ID: &str = "SELECT id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE id = ?";
const SELECT_PROGRESS_EVENTS_BY_TASK: &str = "SELECT id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE task_id = ? ORDER BY at ASC, id ASC";
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

async fn load_task_kind_prior<'c, E>(
    executor: E,
    task: &TaskRow,
) -> StorageResult<Option<DurationDistribution>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    let kind = task.habit_id.as_deref().unwrap_or("default");
    let prior: Option<(f64, f64)> = sqlx::query_as(
        "SELECT mean_minutes, sigma_minutes FROM estimator_task_priors WHERE kind = ?",
    )
    .bind(kind)
    .fetch_optional(executor)
    .await
    .map_err(map_err)?;
    Ok(prior.map(|(mu, sigma)| DurationDistribution::new(mu, sigma)))
}

async fn ensure_estimator_state(
    tx: &mut sqlx::SqliteConnection,
    task: &TaskRow,
) -> StorageResult<EstimatorStateRow> {
    let task_kind_prior = if task.sigma_minutes > 0 {
        None
    } else {
        load_task_kind_prior(&mut *tx, task).await?
    };
    let source = if task.sigma_minutes > 0 {
        "task"
    } else if task_kind_prior.is_some() {
        "task_kind_prior"
    } else {
        "fallback"
    };
    let distribution = effective_distribution(
        task.avg_minutes as f64,
        task.sigma_minutes as f64,
        task_kind_prior,
    );
    sqlx::query(
        "INSERT OR IGNORE INTO estimator_state (task_id, revision, mean_minutes, sigma_minutes, source) VALUES (?, 0, ?, ?, ?)",
    )
    .bind(&task.id)
    .bind(distribution.mu)
    .bind(distribution.sigma)
    .bind(source)
    .execute(&mut *tx)
    .await
    .map_err(map_err)?;
    sqlx::query_as::<_, EstimatorStateRow>(
        "SELECT task_id, revision, mean_minutes, sigma_minutes, source, updated_at, band, next_crossing_time FROM estimator_state WHERE task_id = ?",
    )
    .bind(&task.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_err)
}

fn estimator_band(band: InterventionBand) -> EstimatorBand {
    match band {
        InterventionBand::Usual => EstimatorBand::Usual,
        InterventionBand::Attention => EstimatorBand::Attention,
        InterventionBand::Replan => EstimatorBand::Replan,
    }
}

fn crossing_timestamp(now: Timestamp, crossing_delay_minutes: Option<f64>) -> Option<Timestamp> {
    let delay_seconds = (crossing_delay_minutes? * 60.0).ceil();
    if !delay_seconds.is_finite() || delay_seconds < 0.0 {
        return None;
    }
    now.to_jiff()
        .checked_add(jiff::Span::new().seconds(delay_seconds as i64))
        .ok()
        .map(Timestamp::from)
}

async fn record_estimator_observation(
    tx: &mut sqlx::SqliteConnection,
    task: &TaskRow,
    active_minutes: i64,
    quantity_fraction: f64,
    now: Timestamp,
    kind: &str,
) -> StorageResult<EstimatorResult> {
    if task.fixed {
        return Err(StorageError::BadRequest(
            "fixed tasks do not have estimator observations".into(),
        ));
    }
    let state = ensure_estimator_state(tx, task).await?;
    let prior = DurationDistribution::new(state.mean_minutes, state.sigma_minutes);
    let posterior = progress_posterior(prior, active_minutes.max(0) as f64, quantity_fraction)
        .map_err(|e| StorageError::BadRequest(e.to_string()))?;
    let revision = state.revision + 1;
    let observation_id = uuid::Uuid::now_v7().to_string();
    let band = estimator_band(posterior.band);
    let next_crossing_time = crossing_timestamp(
        now,
        next_crossing_time(posterior.posterior, active_minutes.max(0) as f64, 0.0),
    );

    sqlx::query(
        "INSERT INTO estimator_observations (id, task_id, revision, kind, active_minutes, quantity_fraction, projection_minutes, prior_mean_minutes, prior_sigma_minutes, posterior_mean_minutes, posterior_sigma_minutes, band, next_crossing_time) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&observation_id)
    .bind(&task.id)
    .bind(revision)
    .bind(kind)
    .bind(active_minutes.max(0) as f64)
    .bind(quantity_fraction)
    .bind(posterior.projection_minutes)
    .bind(prior.mu)
    .bind(prior.sigma)
    .bind(posterior.posterior.mu)
    .bind(posterior.posterior.sigma)
    .bind(band.to_string())
    .bind(next_crossing_time)
    .execute(&mut *tx)
    .await
    .map_err(map_err)?;

    sqlx::query(
        "UPDATE estimator_state SET revision = ?, mean_minutes = ?, sigma_minutes = ?, source = 'observation', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), band = ?, next_crossing_time = ? WHERE task_id = ?",
    )
    .bind(revision)
    .bind(posterior.posterior.mu)
    .bind(posterior.posterior.sigma)
    .bind(band.to_string())
    .bind(next_crossing_time)
    .bind(&task.id)
    .execute(&mut *tx)
    .await
    .map_err(map_err)?;

    let result = EstimatorResult {
        band,
        revision,
        next_crossing_time,
        survival_probability: survival_probability(
            posterior.posterior,
            active_minutes.max(0) as f64,
        ),
        prior_shift_z: Some(posterior.prior_shift_z),
        observation_id,
    };
    Ok(result)
}

async fn compensate_last_estimator_observation(
    tx: &mut sqlx::SqliteConnection,
    task: &TaskRow,
) -> StorageResult<Option<(EstimatorResult, i64, i64)>> {
    let latest: Option<(String, String, f64, f64)> = sqlx::query_as(
        "SELECT id, kind, prior_mean_minutes, prior_sigma_minutes FROM estimator_observations WHERE task_id = ? ORDER BY revision DESC LIMIT 1",
    )
    .bind(&task.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(map_err)?;
    let Some((compensates_id, kind, target_mu, target_sigma)) = latest else {
        return Ok(None);
    };
    if kind == "compensation" {
        return Ok(None);
    }
    let state = ensure_estimator_state(tx, task).await?;
    let target = DurationDistribution::new(target_mu, target_sigma);
    let revision = state.revision + 1;
    let observation_id = uuid::Uuid::now_v7().to_string();
    let band = EstimatorBand::Usual;
    let next_crossing_time: Option<Timestamp> = None;
    sqlx::query(
        "INSERT INTO estimator_observations (id, task_id, revision, kind, active_minutes, prior_mean_minutes, prior_sigma_minutes, posterior_mean_minutes, posterior_sigma_minutes, compensates_observation_id, band, next_crossing_time) VALUES (?, ?, ?, 'compensation', 0, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&observation_id)
    .bind(&task.id)
    .bind(revision)
    .bind(state.mean_minutes)
    .bind(state.sigma_minutes)
    .bind(target.mu)
    .bind(target.sigma)
    .bind(&compensates_id)
    .bind(band.to_string())
    .bind(next_crossing_time)
    .execute(&mut *tx)
    .await
    .map_err(map_err)?;
    sqlx::query(
        "UPDATE estimator_state SET revision = ?, mean_minutes = ?, sigma_minutes = ?, source = 'compensation', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), band = ?, next_crossing_time = ? WHERE task_id = ?",
    )
    .bind(revision)
    .bind(target.mu)
    .bind(target.sigma)
    .bind(band.to_string())
    .bind(next_crossing_time)
    .bind(&task.id)
    .execute(&mut *tx)
    .await
    .map_err(map_err)?;
    let result = EstimatorResult {
        band: EstimatorBand::Usual,
        revision,
        next_crossing_time: None,
        survival_probability: survival_probability(target, 0.0),
        prior_shift_z: None,
        observation_id,
    };
    Ok(Some((
        result,
        target.mean_minutes().round() as i64,
        target.stddev_minutes().round() as i64,
    )))
}

async fn read_estimator_state(
    storage: &super::SqliteStorage,
    task: &TaskRow,
) -> StorageResult<EstimatorStateRow> {
    let state = sqlx::query_as::<_, EstimatorStateRow>(
        "SELECT task_id, revision, mean_minutes, sigma_minutes, source, updated_at, band, next_crossing_time FROM estimator_state WHERE task_id = ?",
    )
    .bind(&task.id)
    .fetch_optional(storage.pool())
    .await
    .map_err(map_err)?;
    if let Some(state) = state {
        return Ok(state);
    }
    let kind = task.habit_id.as_deref().unwrap_or("default");
    let prior: Option<(f64, f64)> = sqlx::query_as(
        "SELECT mean_minutes, sigma_minutes FROM estimator_task_priors WHERE kind = ?",
    )
    .bind(kind)
    .fetch_optional(storage.pool())
    .await
    .map_err(map_err)?;
    let task_kind_prior = prior.map(|(mu, sigma)| DurationDistribution::new(mu, sigma));
    let distribution = effective_distribution(
        task.avg_minutes as f64,
        task.sigma_minutes as f64,
        task_kind_prior,
    );
    Ok(EstimatorStateRow {
        task_id: task.id.clone(),
        revision: 0,
        mean_minutes: distribution.mu,
        sigma_minutes: distribution.sigma,
        source: if task.sigma_minutes > 0 {
            "task".into()
        } else if task_kind_prior.is_some() {
            "task_kind_prior".into()
        } else {
            "fallback".into()
        },
        updated_at: task.updated_at,
        band: None,
        next_crossing_time: None,
    })
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
    if let Some(task) = linked_task.as_ref()
        && !task.fixed
    {
        ensure_estimator_state(&mut tx, task).await?;
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

            let (new_avg, new_sigma) = if total_active_minutes > 0 && !task.fixed {
                let quantity_fraction = task
                    .quantity_total
                    .map(|total| quantity_done.get() as f64 / total.get() as f64)
                    .unwrap_or(1.0)
                    .clamp(f64::EPSILON, 1.0);
                record_estimator_observation(
                    &mut tx,
                    &task,
                    total_active_minutes,
                    quantity_fraction,
                    now_ts,
                    "completion",
                )
                .await?;
                let state = ensure_estimator_state(&mut tx, &task).await?;
                (
                    state.mean_minutes.round() as i64,
                    state.sigma_minutes.round() as i64,
                )
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
            estimator: None,
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
    let mut estimator_result = None;

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
        if task_delta > 0
            && active_minutes > 0
            && let Some(total) = task.quantity_total
            && !task.fixed
        {
            let sessions: Vec<WorkSessionRow> =
                sqlx::query_as::<_, WorkSessionRow>(SELECT_WORK_SESSIONS_BY_TASK)
                    .bind(&task_id)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(map_err)?;
            let total_active_minutes = sessions.iter().map(session_minutes).sum::<i64>();
            if total_active_minutes > 0 {
                let quantity_fraction = new_done.get() as f64 / total.get() as f64;
                estimator_result = Some(
                    record_estimator_observation(
                        &mut tx,
                        task,
                        total_active_minutes,
                        quantity_fraction.min(1.0),
                        Timestamp::now(),
                        "progress",
                    )
                    .await?,
                );
                let state = ensure_estimator_state(&mut tx, task).await?;
                new_avg = state.mean_minutes.round() as i64;
                new_sigma = state.sigma_minutes.round() as i64;
            }
        } else if task_delta < 0
            && !task.fixed
            && let Some((result, avg, sigma)) =
                compensate_last_estimator_observation(&mut tx, task).await?
        {
            estimator_result = Some(result);
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
        estimator: estimator_result,
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
    let avg_minutes = active_minutes.max(1);
    let sigma_minutes = 0;
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

    if !task.fixed && active_minutes > 0 {
        record_estimator_observation(&mut tx, &task, active_minutes, 1.0, now_ts, "completion")
            .await?;
    }

    if let Some(op_id) = operation_id {
        super::SqliteStorage::record_progress_operation(&mut *tx, op_id, &request_hash, &task)
            .await?;
    }
    tx.commit().await.map_err(map_err)?;
    Ok(task)
}

pub(crate) async fn get_estimator_state(
    storage: &super::SqliteStorage,
    id: &str,
) -> StorageResult<Option<EstimatorStateRow>> {
    let task = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
        .bind(resolve_task_id(storage.pool(), id).await?)
        .fetch_one(storage.pool())
        .await
        .map_err(map_err)?;
    if task.fixed {
        return Ok(None);
    }
    Ok(Some(read_estimator_state(storage, &task).await?))
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

    let estimator = if task.fixed {
        None
    } else {
        Some(read_estimator_state(storage, &task).await?)
    };

    Ok(TaskProgress {
        task,
        open_session,
        sessions,
        events,
        total_active_minutes,
        estimator,
    })
}

/// Fetch progress for multiple tasks in a fixed number of queries.
///
/// Returns one [`EvaluationTaskProgress`] per supplied task. Active minutes are
/// summed from work sessions (not progress events), estimator state and
/// task-kind priors are loaded with one query each, and all are grouped in
/// memory. This avoids the N+1 `get_task_progress` round-trips inside the
/// snapshot transaction.
pub(crate) async fn batch_evaluation_progress(
    conn: &mut sqlx::SqliteConnection,
    tasks: &[TaskRow],
) -> StorageResult<Vec<EvaluationTaskProgress>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    let sessions_sql = format!(
        "SELECT id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at FROM work_sessions WHERE task_id IN ({placeholders}) ORDER BY started_at ASC"
    );
    let mut sessions_q =
        sqlx::query_as::<_, WorkSessionRow>(sqlx::AssertSqlSafe(sessions_sql.as_str()));
    for id in &ids {
        sessions_q = sessions_q.bind(id);
    }
    let sessions: Vec<WorkSessionRow> = sessions_q.fetch_all(&mut *conn).await.map_err(map_err)?;

    let state_sql = format!(
        "SELECT task_id, revision, mean_minutes, sigma_minutes, source, updated_at, band, next_crossing_time FROM estimator_state WHERE task_id IN ({placeholders})"
    );
    let mut state_q =
        sqlx::query_as::<_, EstimatorStateRow>(sqlx::AssertSqlSafe(state_sql.as_str()));
    for id in &ids {
        state_q = state_q.bind(id);
    }
    let states: Vec<EstimatorStateRow> = state_q.fetch_all(&mut *conn).await.map_err(map_err)?;

    let mut state_by_task: std::collections::HashMap<String, EstimatorStateRow> =
        std::collections::HashMap::new();
    for state in states {
        state_by_task.insert(state.task_id.clone(), state);
    }

    let mut active_minutes_by_task: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for session in sessions {
        if let Some(task_id) = session.task_id.as_ref() {
            *active_minutes_by_task.entry(task_id.clone()).or_default() +=
                session_minutes(&session);
        }
    }

    let mut result = Vec::with_capacity(tasks.len());
    for task in tasks {
        let total_active_minutes = active_minutes_by_task.get(&task.id).copied().unwrap_or(0);

        let estimator = if task.fixed {
            None
        } else if let Some(state) = state_by_task.get(&task.id) {
            Some(EvaluationEstimator {
                revision: state.revision,
                mean_minutes: state.mean_minutes,
                sigma_minutes: state.sigma_minutes,
                band: state.band,
                next_crossing_time: state.next_crossing_time,
            })
        } else {
            let task_kind_prior = load_task_kind_prior(&mut *conn, task).await?;
            let distribution = effective_distribution(
                task.avg_minutes as f64,
                task.sigma_minutes as f64,
                task_kind_prior,
            );
            Some(EvaluationEstimator {
                revision: 0,
                mean_minutes: distribution.mu,
                sigma_minutes: distribution.sigma,
                band: None,
                next_crossing_time: None,
            })
        };

        result.push(EvaluationTaskProgress {
            task_id: task.id.clone(),
            total_active_minutes,
            estimator,
        });
    }
    Ok(result)
}
