//! D1 implementation of the `Storage` trait.
//!
//! Mirrors `SqliteStorage` (takusu-local-lib) but targets Cloudflare D1 via
//! the `worker` crate's `JsValue` bindings. All SQL and storage-level business
//! logic (display_id allocation, normalized_title, status transitions,
//! idempotency, work-session cleanup, estimate recomputation, etc.) lives here
//! so that handler modules stay as thin parse → validate → serialize wrappers.

use takusu_contracts::storage::StorageResult;
use takusu_contracts::{
    HabitRow, HabitStepRow, MemoryRow, ScheduleRow, SettingsRow, SkillRow, StorageError, TaskRow,
};
use takusu_types::{Quantity, parse_timezone};
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

pub(super) const HABIT_COLS: &str = "id, display_id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed, window_mode, created_at, updated_at";
pub(super) const STEP_COLS: &str = "id, habit_id, position, title, description, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, fixed, depends_on, created_at";
pub(super) const SCHEDULED_SPAN_COLS: &str =
    "id, habit_id, start_date, end_date, reason, created_at";
pub(super) const SKILL_COLS: &str =
    "slug, name, description, body, built_in, created_at, updated_at";
pub(super) const MEMORY_COLS: &str = "id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at";

pub(super) fn select_tasks() -> String {
    format!("SELECT {TASK_COLS} FROM {TASK_FROM}")
}

pub(super) fn select_habits() -> String {
    format!("SELECT {HABIT_COLS} FROM habits")
}

pub(super) fn memory_select() -> String {
    format!("SELECT {MEMORY_COLS} FROM memories")
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
pub(super) struct StatusRow {
    pub(super) status: String,
}

#[derive(serde::Deserialize)]
pub(super) struct NowRow {
    pub(super) now: String,
}

// ── D1Result helpers ────────────────────────────────────────────────────

/// Field names that D1 returns as `0` / `1` (or `0.0` / `1.0`) instead of
/// booleans. These are converted to `true` / `false` before deserializing into
/// the model so that `#[serde(transparent)]` / plain `bool` fields keep working.
const D1_BOOL_FIELDS: &[&str] = &[
    "active",
    "allows_parallel",
    "built_in",
    "enabled",
    "fixed",
    "parallelizable",
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

pub(super) async fn d1_all<T: serde::de::DeserializeOwned>(
    stmt: &worker::D1PreparedStatement,
) -> StorageResult<Vec<T>> {
    let raw: Vec<serde_json::Value> = stmt
        .all()
        .await
        .map_err(d1_err)?
        .results::<serde_json::Value>()
        .map_err(d1_err)?;
    let mut out = Vec::with_capacity(raw.len());
    for mut value in raw {
        normalize_d1_bools(&mut value);
        out.push(serde_json::from_value(value).map_err(|e| {
            StorageError::Internal(format!("D1 row deserialization failed: {e}"))
        })?);
    }
    Ok(out)
}

pub(super) async fn d1_first<T: serde::de::DeserializeOwned>(
    stmt: &worker::D1PreparedStatement,
) -> StorageResult<Option<T>> {
    let value: Option<serde_json::Value> = stmt.first(None).await.map_err(d1_err)?;
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
    let stmt = database
        .prepare("SELECT id, tz, sleep_start, sleep_end, comfortable_minutes, maximum_minutes, solver, time_budget_ms, seed, warm_start, created_at, updated_at FROM settings WHERE id = 'active'");
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
            "SELECT id, created_at, updated_at, schedule FROM schedules WHERE id = 'active'",
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

pub(super) fn session_minutes(session: &takusu_contracts::TaskWorkSessionRow) -> i64 {
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

pub(super) async fn compute_updated_estimate(
    database: &D1Database,
    task_id: &str,
    avg_minutes: i64,
    sigma_minutes: i64,
    quantity_total: Option<i64>,
    active_minutes: i64,
    delta_quantity: i64,
) -> StorageResult<(i64, i64)> {
    let stmt = database.prepare(
        "SELECT id, task_id, at, quantity_done, delta_quantity, active_minutes, note FROM progress_events WHERE task_id = ?1 AND delta_quantity > 0 AND active_minutes > 0 ORDER BY id ASC",
    );
    let events: Vec<takusu_contracts::ProgressEventRow> =
        d1_all(&stmt.bind(&[JsValue::from_str(task_id)]).map_err(d1_err)?).await?;

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

// ── Token helper ────────────────────────────────────────────────────────

pub(super) fn token_expires_at(ttl_seconds: i64) -> Option<String> {
    let now = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let exp = now.saturating_add(ttl_seconds);
    jiff::Timestamp::from_second(exp)
        .ok()
        .map(|t| t.to_string())
}

// Storage trait impl is in storage_d1_impl.rs (registered in lib.rs).
