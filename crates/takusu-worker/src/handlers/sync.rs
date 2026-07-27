use serde::Deserialize;
use wasm_bindgen::JsValue;
use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::db;
use crate::handlers::d1::safe_all;
use crate::handlers::tokens::{json_ok, parse_json};
use crate::models::{GoogleCalEventRow, GoogleCalSettingsRow, UpdateGoogleCalSettings};
use takusu_util::Timestamp;

#[derive(Deserialize)]
pub struct MappingPair {
    pub task_id: String,
    pub google_event_id: String,
}

#[derive(Deserialize, Default)]
pub struct UpsertMappingsBody {
    pub mappings: Vec<MappingPair>,
}

#[derive(Deserialize, Default)]
pub struct DeleteMappingsBody {
    pub task_ids: Vec<String>,
}

pub async fn get_settings(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let row = get_settings_row(&database).await?;
    json_ok(&row)
}

pub async fn update_settings(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: UpdateGoogleCalSettings = parse_json(&mut req).await?;
    let database = db(&env)?;
    let existing = get_settings_row(&database).await?;
    let enabled = body.enabled.unwrap_or(existing.enabled);
    let calendar_id = body
        .calendar_id
        .clone()
        .unwrap_or_else(|| existing.calendar_id.clone());
    let client_id = body
        .client_id
        .clone()
        .unwrap_or_else(|| existing.client_id.clone());
    let client_secret = body
        .client_secret
        .clone()
        .unwrap_or_else(|| existing.client_secret.clone());
    let refresh_token = body
        .refresh_token
        .clone()
        .or_else(|| existing.refresh_token.clone());

    let stmt = database.prepare(
        "INSERT INTO google_cal_settings (id, enabled, calendar_id, client_id, client_secret, refresh_token, created_at, updated_at) VALUES ('active', ?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled, calendar_id=excluded.calendar_id, client_id=excluded.client_id, client_secret=excluded.client_secret, refresh_token=excluded.refresh_token, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"
    );
    stmt.bind(&[
        JsValue::from_bool(enabled),
        JsValue::from_str(&calendar_id),
        JsValue::from_str(&client_id),
        JsValue::from_str(&client_secret),
        refresh_token
            .as_deref()
            .map(JsValue::from_str)
            .unwrap_or(JsValue::NULL),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;
    let row = get_settings_row(&database).await?;
    json_ok(&row)
}

pub async fn list_mappings(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let stmt =
        database.prepare("SELECT task_id, google_event_id, updated_at FROM google_cal_events");
    let rows: Vec<GoogleCalEventRow> = safe_all(&stmt).await?;
    json_ok(&rows)
}

pub async fn upsert_mappings(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: UpsertMappingsBody = parse_json(&mut req).await?;
    let database = db(&env)?;
    for m in &body.mappings {
        let stmt = database.prepare(
            "INSERT INTO google_cal_events (task_id, google_event_id, updated_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(task_id) DO UPDATE SET google_event_id=excluded.google_event_id, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"
        );
        stmt.bind(&[
            JsValue::from_str(&m.task_id),
            JsValue::from_str(&m.google_event_id),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;
    }
    Ok(Response::empty()?)
}

pub async fn delete_mappings(req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let url = req.url()?;
    if url.query_pairs().any(|(k, v)| k == "all" && v == "1") {
        let stmt = database.prepare("DELETE FROM google_cal_events");
        stmt.run().await.map_err(WorkerError::Worker)?;
        return Ok(Response::empty()?);
    }
    let mut req = req;
    let body: DeleteMappingsBody = parse_json(&mut req).await?;
    for id in &body.task_ids {
        let stmt = database.prepare("DELETE FROM google_cal_events WHERE task_id = ?1");
        stmt.bind(&[JsValue::from_str(id)])?
            .run()
            .await
            .map_err(WorkerError::Worker)?;
    }
    Ok(Response::empty()?)
}

async fn get_settings_row(
    database: &worker::D1Database,
) -> Result<GoogleCalSettingsRow, WorkerError> {
    let stmt = database
        .prepare("SELECT id, enabled, calendar_id, client_id, client_secret, refresh_token, created_at, updated_at FROM google_cal_settings WHERE id = 'active'");
    let rows: Vec<GoogleCalSettingsRow> = safe_all(&stmt).await?;
    Ok(rows
        .into_iter()
        .next()
        .unwrap_or_else(|| GoogleCalSettingsRow {
            id: "active".to_string(),
            enabled: false,
            calendar_id: "primary".to_string(),
            client_id: String::new(),
            client_secret: String::new(),
            refresh_token: None,
            created_at: Timestamp::default(),
            updated_at: Timestamp::default(),
        }))
}
