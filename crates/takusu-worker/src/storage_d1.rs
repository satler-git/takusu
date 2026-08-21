//! D1 implementation of the `Storage` trait.
//!
//! Mirrors `SqliteStorage` (takusu-local-lib) but targets Cloudflare D1 via
//! the `worker` crate's `JsValue` bindings. All SQL and storage-level business
//! logic (display_id allocation, normalized_title, status transitions,
//! idempotency, work-session cleanup, estimate recomputation, etc.) lives here
//! so that handler modules stay as thin parse → validate → serialize wrappers.

use std::collections::HashMap;

use takusu_contracts::storage::StorageResult;
use takusu_contracts::{
    EstimatorBand, EstimatorResult, EstimatorStateRow, EvaluationEstimator, EvaluationTaskProgress,
    HabitRow, HabitStepRow, MemoryKindCounts, MemoryRow, ScheduleRow, SettingsRow, SkillRow,
    StorageError, TaskRow, WorkSessionRow,
};
use takusu_types::estimator::{
    DurationDistribution, InterventionBand, effective_distribution, next_crossing_time,
    progress_posterior, survival_probability,
};
use takusu_types::{Quantity, TaskStatus, Timestamp, parse_timezone};
use wasm_bindgen::JsValue;
use worker::D1Database;

// ── SQL column constants ────────────────────────────────────────────────

pub(super) const TASK_COLS: &str = "id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, habit_id, ical_uid, user_edited, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at, tam.actual_minutes";
pub(super) const TASK_FROM: &str =
    "tasks LEFT JOIN task_actual_minutes tam ON tam.task_id = tasks.id";
pub(super) const OVERDUE_SQL: &str =
    "status NOT IN ('completed', 'skipped') AND datetime(end_at) < datetime('now')";
pub(super) const NOT_OVERDUE_SQL: &str =
    "(status IN ('completed', 'skipped') OR datetime(end_at) >= datetime('now'))";
pub(super) const ACTIONABLE_SQL: &str = "status IN ('pending', 'scheduled', 'in_progress')";

pub(super) const HABIT_COLS: &str = "id, display_id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed, window_mode, created_at, updated_at";
pub(super) const STEP_COLS: &str = "id, habit_id, position, title, description, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, fixed, depends_on, created_at";
pub(super) const SCHEDULED_SPAN_COLS: &str =
    "id, habit_id, start_date, end_date, reason, created_at";
pub(super) const SKILL_COLS: &str =
    "slug, name, description, body, built_in, created_at, updated_at";
pub(super) const MEMORY_COLS: &str = "id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at";
pub(super) const COMMENT_COLS: &str = "id, task_id, author, content, seq, created_at";

pub(super) const WORK_SESSION_COLS: &str = "id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at";
pub(super) const PROGRESS_EVENT_COLS: &str =
    "id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note";

// ── Snapshot selects used by get_evaluation_inputs / get_coverage_evaluation ──

pub(super) const SETTINGS_SELECT: &str = "SELECT id, tz, sleep_start, sleep_end, comfortable_minutes, maximum_minutes, solver, time_budget_ms, seed, warm_start, plan_length_days, device_priority, created_at, updated_at FROM settings WHERE id = 'active'";
pub(super) const SCHEDULE_SELECT: &str = "SELECT id, created_at, updated_at, schedule, horizon_task_ids FROM schedules WHERE id = 'active'";
pub(super) const SCHEDULE_REVISION_SELECT: &str =
    "SELECT revision FROM schedule_revisions WHERE id = 'active'";
pub(super) const EVENT_LEDGER_SELECT_ORDERED: &str = "SELECT id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at, delivered_at FROM event_ledger ORDER BY created_at, id";
pub(super) const COVERAGE_CONFIRMATIONS_SELECT: &str = "SELECT id, start_at, end_at, timezone, source, schedule_revision, calendar_health, created_at, settled_at, operation_id FROM coverage_confirmations ORDER BY created_at DESC";
pub(super) const UNSETTLED_INTERVALS_SELECT: &str = "SELECT id, start_at, end_at, classification, source, created_at, settled_at, operation_id FROM unsettled_intervals WHERE settled_at IS NULL ORDER BY start_at";
pub(super) const ALL_ESTIMATOR_STATE_SELECT: &str = "SELECT task_id, revision, mean_minutes, sigma_minutes, source, updated_at, band, next_crossing_time FROM estimator_state WHERE task_id IN (SELECT id FROM tasks WHERE status = 'in_progress')";
pub(super) const ALL_ESTIMATOR_PRIORS_SELECT: &str =
    "SELECT kind, mean_minutes, sigma_minutes FROM estimator_task_priors";

pub(super) fn all_work_sessions_sql() -> String {
    format!("SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE task_id IN (SELECT id FROM tasks WHERE status = 'in_progress')")
}

pub(super) fn evaluation_tasks_sql() -> String {
    format!("{} ORDER BY created_at DESC", select_tasks())
}

pub(super) fn select_tasks() -> String {
    format!("SELECT {TASK_COLS} FROM {TASK_FROM}")
}

pub(super) fn select_habits() -> String {
    format!("SELECT {HABIT_COLS} FROM habits")
}

pub(super) fn memory_select() -> String {
    format!("SELECT {MEMORY_COLS} FROM memories")
}

pub(super) fn comment_select() -> String {
    format!("SELECT {COMMENT_COLS} FROM task_comments")
}

pub(super) fn select_skills() -> String {
    format!("SELECT {SKILL_COLS} FROM skills")
}

// ── D1Storage struct ────────────────────────────────────────────────────

/// D1-backed `Storage` implementation. Holds a `D1Database` binding obtained
/// from the Worker `Env`. Created per-request via `D1Storage::new(db(&env)?)`.
pub(crate) struct D1Storage {
    pub(super) db: D1Database,
    pub(super) jwt_secret: String,
}

impl D1Storage {
    pub(crate) fn new(db: D1Database, jwt_secret: String) -> Self {
        Self { db, jwt_secret }
    }

    /// Total `proper_noun` / `fact` memory rows per kind, for the agent's
    /// memory-store hint (WI-4 / #1003).
    pub(super) async fn memory_counts(&self) -> StorageResult<MemoryKindCounts> {
        let stmt = self.db.prepare(
            "SELECT kind, COUNT(*) AS n FROM memories WHERE kind IN ('proper_noun', 'fact') GROUP BY kind",
        );
        let rows: Vec<MemoryCountRow> = d1_all(&stmt).await?;
        let mut counts = MemoryKindCounts {
            proper_noun: 0,
            fact: 0,
        };
        for r in rows {
            match r.kind.as_str() {
                "proper_noun" => counts.proper_noun = r.n,
                "fact" => counts.fact = r.n,
                _ => {}
            }
        }
        Ok(counts)
    }
}

// ── Error mapping ───────────────────────────────────────────────────────

pub(super) fn d1_err(e: worker::Error) -> StorageError {
    StorageError::Internal(format!("D1 error: {e:?}"))
}

pub(super) fn not_found(msg: impl Into<String>) -> StorageError {
    StorageError::NotFound(msg.into())
}

// ── Helper deserialization structs ──────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct IdRow {
    pub(super) id: String,
}

#[derive(serde::Deserialize)]
pub(super) struct DisplayIdRow {
    pub(super) display_id: i64,
}

#[derive(serde::Deserialize)]
pub(super) struct CountRow {
    #[serde(rename = "c")]
    pub(super) c: i64,
}

#[derive(serde::Deserialize)]
pub(super) struct RevisionRow {
    pub(super) revision: i64,
}

#[derive(serde::Deserialize)]
pub(super) struct MemoryCountRow {
    #[serde(rename = "kind")]
    pub(super) kind: String,
    #[serde(rename = "n")]
    pub(super) n: i64,
}

// ── D1Result helpers ────────────────────────────────────────────────────

/// Field names that D1 returns as `0` / `1` (or `0.0` / `1.0`) instead of
/// booleans. These are converted to `true` / `false` before deserializing into
/// the model so that `#[serde(transparent)]` / plain `bool` fields keep working.
const D1_BOOL_FIELDS: &[&str] = &[
    "active",
    "allows_parallel",
    "audio_service_running",
    "built_in",
    "enabled",
    "fixed",
    "parallelizable",
    "private_output_route",
    "user_edited",
    "warm_start",
];

fn normalize_d1_bools(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if D1_BOOL_FIELDS.contains(&key.as_str()) {
                    if let Some(b) = number_as_bool(v) {
                        *v = serde_json::Value::Bool(b);
                    }
                } else {
                    normalize_d1_bools(v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                normalize_d1_bools(v);
            }
        }
        _ => {}
    }
}

fn number_as_bool(value: &serde_json::Value) -> Option<bool> {
    let n = value.as_f64()?;
    if n == 0.0 {
        Some(false)
    } else if n == 1.0 {
        Some(true)
    } else {
        None
    }
}

pub(super) fn d1_parse_all<T: serde::de::DeserializeOwned>(
    mut values: Vec<serde_json::Value>,
) -> StorageResult<Vec<T>> {
    let mut out = Vec::with_capacity(values.len());
    for value in &mut values {
        normalize_d1_bools(value);
    }
    for value in values {
        out.push(
            serde_json::from_value(value).map_err(|e| {
                StorageError::Internal(format!("D1 row deserialization failed: {e}"))
            })?,
        );
    }
    Ok(out)
}

pub(super) fn d1_parse_first<T: serde::de::DeserializeOwned>(
    value: Option<serde_json::Value>,
) -> StorageResult<Option<T>> {
    match value {
        Some(mut v) => {
            normalize_d1_bools(&mut v);
            serde_json::from_value(v)
                .map_err(|e| StorageError::Internal(format!("D1 row deserialization failed: {e}")))
                .map(Some)
        }
        None => Ok(None),
    }
}

pub(super) async fn d1_all<T: serde::de::DeserializeOwned>(
    stmt: &worker::D1PreparedStatement,
) -> StorageResult<Vec<T>> {
    let raw: Vec<serde_json::Value> = stmt
        .all()
        .await
        .map_err(d1_err)?
        .results::<serde_json::Value>()
        .map_err(d1_err)?;
    d1_parse_all(raw)
}

pub(super) async fn d1_first<T: serde::de::DeserializeOwned>(
    stmt: &worker::D1PreparedStatement,
) -> StorageResult<Option<T>> {
    let value: Option<serde_json::Value> = stmt.first(None).await.map_err(d1_err)?;
    d1_parse_first(value)
}

/// Execute a batch of prepared statements in one round trip and return the raw
/// row values for each statement in the same order. Callers deserialize each
/// result with [`d1_parse_all`] / [`d1_parse_first`].
pub(super) async fn d1_batch_results(
    database: &D1Database,
    stmts: Vec<worker::D1PreparedStatement>,
) -> StorageResult<Vec<Vec<serde_json::Value>>> {
    let results = database.batch(stmts).await.map_err(d1_err)?;
    let mut out = Vec::with_capacity(results.len());
    for result in results {
        let rows = result.results::<serde_json::Value>().map_err(d1_err)?;
        out.push(rows);
    }
    Ok(out)
}

#[async_trait::async_trait(?Send)]
pub(crate) trait D1PreparedStatementExt {
    async fn first_t<T: serde::de::DeserializeOwned>(&self) -> StorageResult<Option<T>>;
}

#[async_trait::async_trait(?Send)]
impl D1PreparedStatementExt for worker::D1PreparedStatement {
    async fn first_t<T: serde::de::DeserializeOwned>(&self) -> StorageResult<Option<T>> {
        d1_first::<T>(self).await
    }
}

// ── ID resolution helpers ───────────────────────────────────────────────

pub(super) async fn resolve_task_id(database: &D1Database, id: &str) -> StorageResult<String> {
    let id = id.strip_prefix('#').unwrap_or(id);

    if let Some(rest) = id.strip_prefix(['h', 'H'])
        && let Some((hdisp, tdisp)) = rest.split_once('#')
        && let (Ok(hnum), Ok(tnum)) = (hdisp.parse::<i64>(), tdisp.parse::<i64>())
    {
        let stmt = database.prepare(
            "SELECT t.id AS id FROM tasks t JOIN habits h ON t.habit_id = h.id \
             WHERE h.display_id = ?1 AND t.display_id = ?2",
        );
        let rows: Vec<IdRow> = d1_all(
            &stmt
                .bind(&[
                    JsValue::from_f64(hnum as f64),
                    JsValue::from_f64(tnum as f64),
                ])
                .map_err(d1_err)?,
        )
        .await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| not_found(format!("task {id} not found")));
    }

    if let Ok(num) = id.parse::<i64>() {
        let stmt =
            database.prepare("SELECT id FROM tasks WHERE display_id = ?1 AND habit_id IS NULL");
        let rows: Vec<IdRow> = d1_all(
            &stmt
                .bind(&[JsValue::from_f64(num as f64)])
                .map_err(d1_err)?,
        )
        .await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| not_found(format!("task {id} not found")));
    }

    if id.contains('-') {
        let stmt = database.prepare("SELECT id FROM tasks WHERE id = ?1");
        let rows: Vec<IdRow> =
            d1_all(&stmt.bind(&[JsValue::from_str(id)]).map_err(d1_err)?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| not_found(format!("task {id} not found")));
    }

    Err(not_found(format!("task {id} not found")))
}

pub(super) async fn resolve_habit_id(database: &D1Database, id: &str) -> StorageResult<String> {
    if let Some(rest) = id.strip_prefix(['h', 'H'])
        && let Ok(num) = rest.parse::<i64>()
    {
        let stmt = database.prepare("SELECT id FROM habits WHERE display_id = ?1");
        let rows: Vec<IdRow> = d1_all(
            &stmt
                .bind(&[JsValue::from_f64(num as f64)])
                .map_err(d1_err)?,
        )
        .await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| not_found(format!("habit {id} not found")));
    }

    if id.contains('-') {
        let stmt = database.prepare("SELECT id FROM habits WHERE id = ?1");
        let rows: Vec<IdRow> =
            d1_all(&stmt.bind(&[JsValue::from_str(id)]).map_err(d1_err)?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| not_found(format!("habit {id} not found")));
    }

    Err(not_found(format!("habit {id} not found")))
}

pub(super) async fn resolve_depends(
    database: &D1Database,
    deps: Option<&[String]>,
) -> StorageResult<Vec<String>> {
    let Some(deps) = deps else {
        return Ok(Vec::new());
    };
    let mut resolved = Vec::with_capacity(deps.len());
    for d in deps {
        resolved.push(resolve_task_id(database, d).await?);
    }
    Ok(resolved)
}

pub(super) async fn allocate_display_id(
    database: &D1Database,
    habit_id: Option<&str>,
) -> StorageResult<i64> {
    if let Some(habit_id) = habit_id {
        let insert_stmt = database.prepare(
            "INSERT OR IGNORE INTO habit_task_display_id_seq (habit_id, next_id) VALUES (?1, 1)",
        );
        insert_stmt
            .bind(&[JsValue::from_str(habit_id)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        let seq_stmt = database.prepare(
            "UPDATE habit_task_display_id_seq SET next_id = next_id + 1 WHERE habit_id = ?1 RETURNING next_id - 1 AS display_id",
        );
        let row: Option<DisplayIdRow> = seq_stmt
            .bind(&[JsValue::from_str(habit_id)])
            .map_err(d1_err)?
            .first_t()
            .await?;
        row.ok_or_else(|| StorageError::Internal("habit display_id sequence is empty".into()))
            .map(|r| r.display_id)
    } else {
        let seq_stmt = database.prepare(
            "UPDATE task_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1 AS display_id",
        );
        let row: Option<DisplayIdRow> = seq_stmt.first_t().await?;
        row.ok_or_else(|| StorageError::Internal("display_id sequence is empty".into()))
            .map(|r| r.display_id)
    }
}

pub(super) async fn select_one_task(database: &D1Database, id: &str) -> StorageResult<TaskRow> {
    let stmt = database.prepare(format!("{select} WHERE id = ?1", select = select_tasks()));
    let row: Option<TaskRow> = stmt
        .bind(&[JsValue::from_str(id)])
        .map_err(d1_err)?
        .first_t()
        .await?;
    row.ok_or_else(|| not_found(format!("task {id} not found")))
}

pub(super) async fn select_one_habit(database: &D1Database, id: &str) -> StorageResult<HabitRow> {
    let stmt = database.prepare(format!("{select} WHERE id = ?1", select = select_habits()));
    let row: Option<HabitRow> = stmt
        .bind(&[JsValue::from_str(id)])
        .map_err(d1_err)?
        .first_t()
        .await?;
    row.ok_or_else(|| not_found(format!("habit {id} not found")))
}

pub(super) async fn select_steps_for_habit(
    database: &D1Database,
    habit_id: &str,
) -> StorageResult<Vec<HabitStepRow>> {
    let stmt = database.prepare(format!(
        "SELECT {STEP_COLS} FROM habit_steps WHERE habit_id = ?1 ORDER BY position ASC, created_at ASC"
    ));
    d1_all(&stmt.bind(&[JsValue::from_str(habit_id)]).map_err(d1_err)?).await
}

pub(super) async fn select_one_memory(database: &D1Database, id: &str) -> StorageResult<MemoryRow> {
    let stmt = database.prepare(format!("{select} WHERE id = ?1", select = memory_select()));
    let rows: Vec<MemoryRow> =
        d1_all(&stmt.bind(&[JsValue::from_str(id)]).map_err(d1_err)?).await?;
    rows.into_iter()
        .next()
        .ok_or_else(|| not_found(format!("memory {id} not found")))
}

pub(super) async fn select_one_skill(database: &D1Database, slug: &str) -> StorageResult<SkillRow> {
    let stmt = database.prepare(format!(
        "{select} WHERE slug = ?1",
        select = select_skills()
    ));
    let row: Option<SkillRow> = stmt
        .bind(&[JsValue::from_str(slug)])
        .map_err(d1_err)?
        .first_t()
        .await?;
    row.ok_or_else(|| not_found(format!("skill {slug} not found")))
}

// ── Quantity validation (mirrors SqliteStorage::validate_quantity) ──────

pub(super) fn validate_quantity(
    total: Option<Quantity>,
    done: Option<Quantity>,
    original: Option<Quantity>,
) -> StorageResult<()> {
    if let Some(t) = total
        && t <= 0
    {
        return Err(StorageError::BadRequest(format!(
            "quantity_total must be > 0 (got {t})"
        )));
    }
    if let Some(o) = original
        && o <= 0
    {
        return Err(StorageError::BadRequest(format!(
            "original_quantity_total must be > 0 (got {o})"
        )));
    }
    if let (Some(t), Some(d)) = (total, done)
        && d > t
    {
        return Err(StorageError::BadRequest(format!(
            "quantity_done cannot exceed quantity_total ({d} > {t})"
        )));
    }
    Ok(())
}

// ── Timezone helper ─────────────────────────────────────────────────────

pub(super) async fn get_timezone(database: &D1Database) -> StorageResult<jiff::tz::TimeZone> {
    let stmt = database.prepare(SETTINGS_SELECT);
    let rows: Vec<SettingsRow> = d1_all(&stmt).await?;
    match rows.into_iter().next() {
        Some(settings) => parse_timezone(&settings.tz)
            .map_err(|e| StorageError::Internal(format!("stored timezone is invalid: {e}"))),
        None => Ok(jiff::tz::TimeZone::UTC),
    }
}

// ── Task search filter ──────────────────────────────────────────────────

pub(super) async fn filter_rows_with_query(
    database: &D1Database,
    rows: Vec<TaskRow>,
    q: &str,
) -> StorageResult<Vec<TaskRow>> {
    let tz = get_timezone(database).await?;
    let now = takusu_types::now_timestamp()
        .map_err(|e| StorageError::Internal(format!("current time unavailable: {e}")))?;

    let habits_stmt = database.prepare(format!("SELECT {HABIT_COLS} FROM habits"));
    let habits: Vec<HabitRow> = d1_all(&habits_stmt).await?;

    let schedule_entries: Vec<takusu_contracts::ScheduleEntry> = {
        let stmt = database.prepare(
            "SELECT id, created_at, updated_at, schedule, horizon_task_ids FROM schedules WHERE id = 'active'",
        );
        let rows = d1_all::<ScheduleRow>(&stmt).await?;
        rows.into_iter()
            .next()
            .map(|r| r.schedule.into_inner())
            .unwrap_or_default()
    };

    let schedule: Vec<(String, (String, String))> = schedule_entries
        .into_iter()
        .map(|e| (e.task_id, (e.start_at.to_string(), e.end_at.to_string())))
        .collect();

    let ctx = takusu_search::search::EvalContext::new(tz, now, schedule, &rows, &habits);
    takusu_search::search::filter_tasks(rows, q, &ctx)
        .map_err(|e| StorageError::BadRequest(e.to_string()))
}

// ── Progress helpers ────────────────────────────────────────────────────

pub(super) struct EstimatorMutation {
    pub(super) result: EstimatorResult,
    pub(super) avg_minutes: i64,
    pub(super) sigma_minutes: i64,
    pub(super) statements: Vec<worker::D1PreparedStatement>,
}

fn estimator_band(band: InterventionBand) -> EstimatorBand {
    match band {
        InterventionBand::Usual => EstimatorBand::Usual,
        InterventionBand::Attention => EstimatorBand::Attention,
        InterventionBand::Replan => EstimatorBand::Replan,
    }
}

pub(super) async fn load_task_kind_prior(
    database: &D1Database,
    task: &TaskRow,
) -> StorageResult<Option<DurationDistribution>> {
    let kind = task.habit_id.as_deref().unwrap_or("default");
    let row: Option<(f64, f64)> = d1_first(
        &database
            .prepare(
                "SELECT mean_minutes, sigma_minutes FROM estimator_task_priors WHERE kind = ?1",
            )
            .bind(&[JsValue::from_str(kind)])
            .map_err(d1_err)?,
    )
    .await?;
    Ok(row.map(|(mu, sigma)| DurationDistribution::new(mu, sigma)))
}

/// Row returned from `estimator_task_priors`.
#[derive(serde::Deserialize)]
pub(super) struct PriorRow {
    pub(super) kind: String,
    pub(super) mean_minutes: f64,
    pub(super) sigma_minutes: f64,
}

pub(super) fn prior_map_from_rows(rows: Vec<PriorRow>) -> HashMap<String, DurationDistribution> {
    let mut map = HashMap::with_capacity(rows.len());
    for row in rows {
        map.insert(
            row.kind,
            DurationDistribution::new(row.mean_minutes, row.sigma_minutes),
        );
    }
    map
}

pub(super) async fn estimator_state(
    database: &D1Database,
    task: &TaskRow,
) -> StorageResult<EstimatorStateRow> {
    let state: Option<EstimatorStateRow> = d1_first(
        &database
            .prepare(
                "SELECT task_id, revision, mean_minutes, sigma_minutes, source, updated_at, band, next_crossing_time FROM estimator_state WHERE task_id = ?1",
            )
            .bind(&[JsValue::from_str(&task.id)])
            .map_err(d1_err)?,
    )
    .await?;
    if let Some(state) = state {
        return Ok(state);
    }
    let task_kind_prior = if task.sigma_minutes > 0 {
        None
    } else {
        load_task_kind_prior(database, task).await?
    };
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

/// Build the per-task progress view from rows already fetched in a snapshot.
pub(super) fn build_evaluation_progress(
    tasks: &[TaskRow],
    sessions: Vec<WorkSessionRow>,
    states: Vec<EstimatorStateRow>,
    priors: &HashMap<String, DurationDistribution>,
) -> Vec<EvaluationTaskProgress> {
    let in_progress: Vec<&TaskRow> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .collect();
    if in_progress.is_empty() {
        return Vec::new();
    }

    let mut active_minutes_by_task: HashMap<String, i64> = HashMap::new();
    for session in sessions {
        if let Some(task_id) = session.task_id.as_ref() {
            *active_minutes_by_task.entry(task_id.clone()).or_default() +=
                session_minutes(&session);
        }
    }

    let mut state_by_task: HashMap<String, EstimatorStateRow> = HashMap::new();
    for state in states {
        state_by_task.insert(state.task_id.clone(), state);
    }

    let mut result = Vec::with_capacity(in_progress.len());
    for task in in_progress {
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
            let task_kind_prior = if task.sigma_minutes > 0 {
                None
            } else {
                let kind = task.habit_id.as_deref().unwrap_or("default");
                priors.get(kind).copied()
            };
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
    result
}

pub(super) async fn estimator_observation(
    database: &D1Database,
    task: &TaskRow,
    active_minutes: i64,
    quantity_fraction: f64,
    now: Timestamp,
    kind: &str,
) -> StorageResult<EstimatorMutation> {
    if task.fixed {
        return Err(StorageError::BadRequest(
            "fixed tasks do not have estimator observations".into(),
        ));
    }
    let existing: Option<EstimatorStateRow> = d1_first(
        &database
            .prepare(
                "SELECT task_id, revision, mean_minutes, sigma_minutes, source, updated_at, band, next_crossing_time FROM estimator_state WHERE task_id = ?1",
            )
            .bind(&[JsValue::from_str(&task.id)])
            .map_err(d1_err)?,
    )
    .await?;
    let state = if let Some(state) = existing.clone() {
        state
    } else {
        let task_kind_prior = if task.sigma_minutes > 0 {
            None
        } else {
            load_task_kind_prior(database, task).await?
        };
        let distribution = effective_distribution(
            task.avg_minutes as f64,
            task.sigma_minutes as f64,
            task_kind_prior,
        );
        EstimatorStateRow {
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
        }
    };
    let prior = DurationDistribution::new(state.mean_minutes, state.sigma_minutes);
    let posterior = progress_posterior(prior, active_minutes.max(0) as f64, quantity_fraction)
        .map_err(|e| StorageError::BadRequest(e.to_string()))?;
    let revision = state.revision + 1;
    let observation_id = uuid::Uuid::now_v7().to_string();
    let band = estimator_band(posterior.band);
    let crossing_delay = next_crossing_time(posterior.posterior, active_minutes.max(0) as f64, 0.0);
    let next_crossing_time = crossing_delay.and_then(|delay| {
        let seconds = (delay * 60.0).ceil();
        (seconds.is_finite() && seconds >= 0.0)
            .then(|| Timestamp::from_second(now.as_second().saturating_add(seconds as i64)))
            .flatten()
    });
    let mut statements = Vec::new();
    if existing.is_none() {
        statements.push(
            database
                .prepare(
                    "INSERT OR IGNORE INTO estimator_state (task_id, revision, mean_minutes, sigma_minutes, source) VALUES (?1, 0, ?2, ?3, ?4)",
                )
                .bind(&[
                    JsValue::from_str(&task.id),
                    JsValue::from_f64(prior.mu),
                    JsValue::from_f64(prior.sigma),
                    JsValue::from_str(&state.source),
                ])
                .map_err(d1_err)?,
        );
    }
    statements.push(
        database
            .prepare(
                "INSERT INTO estimator_observations (id, task_id, revision, kind, active_minutes, quantity_fraction, projection_minutes, prior_mean_minutes, prior_sigma_minutes, posterior_mean_minutes, posterior_sigma_minutes, band, next_crossing_time) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )
            .bind(&[
                JsValue::from_str(&observation_id),
                JsValue::from_str(&task.id),
                JsValue::from_f64(revision as f64),
                JsValue::from_str(kind),
                JsValue::from_f64(active_minutes.max(0) as f64),
                JsValue::from_f64(quantity_fraction),
                JsValue::from_f64(posterior.projection_minutes),
                JsValue::from_f64(prior.mu),
                JsValue::from_f64(prior.sigma),
                JsValue::from_f64(posterior.posterior.mu),
                JsValue::from_f64(posterior.posterior.sigma),
                JsValue::from_str(&band.to_string()),
                next_crossing_time
                    .map(|t| JsValue::from_str(&t.to_string()))
                    .unwrap_or(JsValue::NULL),
            ])
            .map_err(d1_err)?,
    );
    statements.push(
        database
            .prepare(
                "UPDATE estimator_state SET revision = ?1, mean_minutes = ?2, sigma_minutes = ?3, source = 'observation', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), band = ?4, next_crossing_time = ?5 WHERE task_id = ?6",
            )
            .bind(&[
                JsValue::from_f64(revision as f64),
                JsValue::from_f64(posterior.posterior.mu),
                JsValue::from_f64(posterior.posterior.sigma),
                JsValue::from_str(&band.to_string()),
                next_crossing_time
                    .map(|t| JsValue::from_str(&t.to_string()))
                    .unwrap_or(JsValue::NULL),
                JsValue::from_str(&task.id),
            ])
            .map_err(d1_err)?,
    );

    Ok(EstimatorMutation {
        result: EstimatorResult {
            band,
            revision,
            next_crossing_time,
            survival_probability: survival_probability(
                posterior.posterior,
                active_minutes.max(0) as f64,
            ),
            prior_shift_z: Some(posterior.prior_shift_z),
            observation_id,
        },
        avg_minutes: posterior.posterior.mean_minutes().round() as i64,
        sigma_minutes: posterior.posterior.stddev_minutes().round() as i64,
        statements,
    })
}

pub(super) async fn compensate_last_estimator_observation(
    database: &D1Database,
    task: &TaskRow,
) -> StorageResult<Option<EstimatorMutation>> {
    let latest: Option<(String, String, f64, f64)> = d1_first(
        &database
            .prepare(
                "SELECT id, kind, prior_mean_minutes, prior_sigma_minutes FROM estimator_observations WHERE task_id = ?1 ORDER BY revision DESC LIMIT 1",
            )
            .bind(&[JsValue::from_str(&task.id)])
            .map_err(d1_err)?,
    )
    .await?;
    let Some((compensates_id, kind, target_mu, target_sigma)) = latest else {
        return Ok(None);
    };
    if kind == "compensation" {
        return Ok(None);
    }
    let state = estimator_state(database, task).await?;
    let target = DurationDistribution::new(target_mu, target_sigma);
    let revision = state.revision + 1;
    let observation_id = uuid::Uuid::now_v7().to_string();
    let band = EstimatorBand::Usual;
    let statements = vec![
        database
            .prepare(
                "INSERT INTO estimator_observations (id, task_id, revision, kind, active_minutes, prior_mean_minutes, prior_sigma_minutes, posterior_mean_minutes, posterior_sigma_minutes, compensates_observation_id, band, next_crossing_time) VALUES (?1, ?2, ?3, 'compensation', 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .bind(&[
                JsValue::from_str(&observation_id),
                JsValue::from_str(&task.id),
                JsValue::from_f64(revision as f64),
                JsValue::from_f64(state.mean_minutes),
                JsValue::from_f64(state.sigma_minutes),
                JsValue::from_f64(target.mu),
                JsValue::from_f64(target.sigma),
                JsValue::from_str(&compensates_id),
                JsValue::from_str(&band.to_string()),
                JsValue::NULL,
            ])
            .map_err(d1_err)?,
        database
            .prepare(
                "UPDATE estimator_state SET revision = ?1, mean_minutes = ?2, sigma_minutes = ?3, source = 'compensation', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), band = ?4, next_crossing_time = ?5 WHERE task_id = ?6",
            )
            .bind(&[
                JsValue::from_f64(revision as f64),
                JsValue::from_f64(target.mu),
                JsValue::from_f64(target.sigma),
                JsValue::from_str(&band.to_string()),
                JsValue::NULL,
                JsValue::from_str(&task.id),
            ])
            .map_err(d1_err)?,
    ];
    Ok(Some(EstimatorMutation {
        result: EstimatorResult {
            band: EstimatorBand::Usual,
            revision,
            next_crossing_time: None,
            survival_probability: survival_probability(target, 0.0),
            prior_shift_z: None,
            observation_id,
        },
        avg_minutes: target.mean_minutes().round() as i64,
        sigma_minutes: target.stddev_minutes().round() as i64,
        statements,
    }))
}

pub(super) fn now_seconds() -> i64 {
    (worker::Date::now().as_millis() / 1000) as i64
}

pub(super) fn parse_timestamp(s: &str) -> StorageResult<i64> {
    use std::str::FromStr;
    if let Ok(ts) = jiff::Timestamp::from_str(s) {
        return Ok(ts.as_second());
    }
    let dt = jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M:%S", s)
        .map_err(|e| StorageError::Internal(format!("invalid timestamp {s}: {e}")))?;
    let zdt = dt
        .to_zoned(jiff::tz::TimeZone::UTC)
        .map_err(|e| StorageError::Internal(format!("invalid timestamp {s}: {e}")))?;
    Ok(zdt.timestamp().as_second())
}

pub(super) fn session_minutes(session: &takusu_contracts::WorkSessionRow) -> i64 {
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

// ── Progress idempotency ────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct ProgressOpRow {
    pub(super) request_hash: String,
    pub(super) response_json: String,
}

pub(super) fn progress_request_hash(payload: &str, operation_id: Option<&str>) -> String {
    crate::util::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

pub(super) async fn check_progress_idempotency<T: serde::de::DeserializeOwned>(
    database: &D1Database,
    operation_id: &str,
    request_hash: &str,
) -> StorageResult<Option<T>> {
    let stmt = database.prepare(
        "SELECT request_hash, response_json FROM progress_operations WHERE operation_id = ?1",
    );
    let row: Option<ProgressOpRow> = stmt
        .bind(&[JsValue::from_str(operation_id)])
        .map_err(d1_err)?
        .first_t()
        .await?;
    if let Some(row) = row {
        if row.request_hash != request_hash {
            return Err(StorageError::BadRequest(
                "idempotency key reused with different request".into(),
            ));
        }
        let value: T = serde_json::from_str(&row.response_json)
            .map_err(|e| StorageError::Internal(format!("corrupt idempotency response: {e}")))?;
        return Ok(Some(value));
    }
    Ok(None)
}

pub(super) async fn record_progress_operation<T: serde::Serialize>(
    database: &D1Database,
    operation_id: &str,
    request_hash: &str,
    value: &T,
) -> StorageResult<()> {
    let response_json = serde_json::to_string(value)
        .map_err(|e| StorageError::Internal(format!("serialize idempotency response: {e}")))?;
    let stmt = database.prepare(
        "INSERT INTO progress_operations (operation_id, request_hash, response_json, created_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    );
    stmt.bind(&[
        JsValue::from_str(operation_id),
        JsValue::from_str(request_hash),
        JsValue::from_str(&response_json),
    ])
    .map_err(d1_err)?
    .run()
    .await
    .map_err(d1_err)?;
    Ok(())
}

// ── Memory idempotency ──────────────────────────────────────────────────

pub(super) fn memory_request_hash(payload: &str, operation_id: Option<&str>) -> String {
    crate::util::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

pub(super) async fn check_memory_idempotency(
    database: &D1Database,
    op_id: &str,
    expected_hash: &str,
) -> StorageResult<Option<String>> {
    let stmt = database.prepare(
        "SELECT request_hash, response_json FROM memory_operations WHERE operation_id = ?1",
    );
    #[derive(serde::Deserialize)]
    struct OpRow {
        pub(super) request_hash: String,
        pub(super) response_json: String,
    }
    let rows: Vec<OpRow> = d1_all(&stmt.bind(&[JsValue::from_str(op_id)]).map_err(d1_err)?).await?;
    if let Some(row) = rows.into_iter().next() {
        if row.request_hash != expected_hash {
            return Err(StorageError::Conflict(
                "idempotency key reused with different request".into(),
            ));
        }
        return Ok(Some(row.response_json));
    }
    Ok(None)
}

pub(super) async fn record_memory_operation(
    database: &D1Database,
    op_id: &str,
    request_hash: &str,
    response_json: &str,
) -> StorageResult<()> {
    let stmt = database.prepare(
        "INSERT INTO memory_operations (operation_id, request_hash, response_json, created_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    );
    stmt.bind(&[
        JsValue::from_str(op_id),
        JsValue::from_str(request_hash),
        JsValue::from_str(response_json),
    ])
    .map_err(d1_err)?
    .run()
    .await
    .map_err(d1_err)?;
    Ok(())
}

// ── Comment idempotency (WI-1) ─────────────────────────────────────────

pub(super) fn comment_request_hash(payload: &str, operation_id: Option<&str>) -> String {
    crate::util::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

pub(super) async fn check_comment_idempotency(
    database: &D1Database,
    op_id: &str,
    expected_hash: &str,
) -> StorageResult<Option<String>> {
    let stmt = database.prepare(
        "SELECT request_hash, response_json FROM comment_operations WHERE operation_id = ?1",
    );
    #[derive(serde::Deserialize)]
    struct OpRow {
        pub(super) request_hash: String,
        pub(super) response_json: String,
    }
    let rows: Vec<OpRow> = d1_all(&stmt.bind(&[JsValue::from_str(op_id)]).map_err(d1_err)?).await?;
    if let Some(row) = rows.into_iter().next() {
        if row.request_hash != expected_hash {
            return Err(StorageError::Conflict(
                "idempotency key reused with different request".into(),
            ));
        }
        return Ok(Some(row.response_json));
    }
    Ok(None)
}

// ── Token helper ────────────────────────────────────────────────────────

pub(super) fn token_expires_at(ttl_seconds: i64) -> Option<String> {
    let now = jiff::Timestamp::now().as_second();
    let exp = now.saturating_add(ttl_seconds);
    jiff::Timestamp::from_second(exp)
        .ok()
        .map(|t| t.to_string())
}

// Storage trait impl is in storage_d1_impl.rs (registered in lib.rs).
