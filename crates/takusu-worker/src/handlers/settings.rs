use wasm_bindgen::JsValue;
use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::db;
use crate::handlers::d1::safe_all;
use crate::handlers::tokens::{json_ok, parse_json};
use crate::models::{SettingsRow, UpdateSettings};
use takusu_storage::Validate;
use takusu_types::parse_timezone;

pub async fn get(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let row = get_inner(&database).await?;
    json_ok(&row)
}

pub async fn update(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: UpdateSettings = parse_json(&mut req).await?;
    body.validate()?;
    let database = db(&env)?;
    let existing = get_inner(&database).await?;
    let tz = body.tz.clone().unwrap_or(existing.tz);
    let sleep_start = body.sleep_start.unwrap_or(existing.sleep_start);
    let sleep_end = body.sleep_end.unwrap_or(existing.sleep_end);
    let comfortable_minutes = body.comfortable_minutes.or(existing.comfortable_minutes);
    let maximum_minutes = body.maximum_minutes.or(existing.maximum_minutes);
    let solver = body.solver.unwrap_or(existing.solver);
    let solver = solver.to_string();
    let time_budget_ms = body
        .time_budget_ms
        .filter(|&v| v > 0)
        .or(existing.time_budget_ms);
    let seed = body.seed.filter(|&v| v >= 0).or(existing.seed);
    let warm_start = body.warm_start.unwrap_or(existing.warm_start);
    let stmt = database.prepare(
        "UPDATE settings SET tz = ?1, sleep_start = ?2, sleep_end = ?3, comfortable_minutes = ?4, maximum_minutes = ?5, solver = ?6, time_budget_ms = ?7, seed = ?8, warm_start = ?9, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = 'active'",
    );
    stmt.bind(&[
        JsValue::from_str(&tz),
        JsValue::from_str(&sleep_start.to_string()),
        JsValue::from_str(&sleep_end.to_string()),
        comfortable_minutes
            .map(|v| JsValue::from_f64(v as f64))
            .unwrap_or(JsValue::NULL),
        maximum_minutes
            .map(|v| JsValue::from_f64(v as f64))
            .unwrap_or(JsValue::NULL),
        JsValue::from_str(&solver),
        time_budget_ms
            .map(|v| JsValue::from_f64(v as f64))
            .unwrap_or(JsValue::NULL),
        seed.map(|v| JsValue::from_f64(v as f64))
            .unwrap_or(JsValue::NULL),
        JsValue::from_bool(warm_start),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;
    let row = get_inner(&database).await?;
    json_ok(&row)
}

pub(crate) async fn get_inner(database: &worker::D1Database) -> Result<SettingsRow, WorkerError> {
    let stmt = database
        .prepare("SELECT id, tz, sleep_start, sleep_end, comfortable_minutes, maximum_minutes, solver, time_budget_ms, seed, warm_start, created_at, updated_at FROM settings WHERE id = 'active'");
    let rows: Vec<SettingsRow> = safe_all(&stmt).await?;
    rows.into_iter()
        .next()
        .ok_or_else(|| WorkerError::NotFound("settings not found".into()))
}

/// Return the configured timezone, falling back to UTC if the settings row
/// has not been created yet. Mirrors `takusu-local-lib` `get_settings_or_default`.
pub(crate) async fn get_timezone(
    database: &worker::D1Database,
) -> Result<jiff::tz::TimeZone, WorkerError> {
    match get_inner(database).await {
        Ok(settings) => parse_timezone(&settings.tz)
            .map_err(|e| WorkerError::Internal(format!("stored timezone is invalid: {e}"))),
        Err(WorkerError::NotFound(_)) => Ok(jiff::tz::TimeZone::UTC),
        Err(e) => Err(e),
    }
}
