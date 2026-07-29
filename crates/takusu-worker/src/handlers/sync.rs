use serde::Deserialize;
use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_ok, parse_json};
use crate::models::UpdateGoogleCalSettings;
use takusu_storage::Storage;

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
    let store = storage(&env)?;
    let row = store.get_gcal_settings().await?;
    json_ok(&row)
}

pub async fn update_settings(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: UpdateGoogleCalSettings = parse_json(&mut req).await?;
    let store = storage(&env)?;
    let row = store.update_gcal_settings(&body).await?;
    json_ok(&row)
}

pub async fn list_mappings(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_gcal_mappings().await?;
    json_ok(&rows)
}

pub async fn upsert_mappings(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: UpsertMappingsBody = parse_json(&mut req).await?;
    let store = storage(&env)?;
    let mappings: Vec<(String, String)> = body
        .mappings
        .into_iter()
        .map(|m| (m.task_id, m.google_event_id))
        .collect();
    store.upsert_gcal_mappings(&mappings).await?;
    Ok(Response::empty()?)
}

pub async fn delete_mappings(req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let url = req.url()?;
    if url.query_pairs().any(|(k, v)| k == "all" && v == "1") {
        store.clear_gcal_mappings().await?;
        return Ok(Response::empty()?);
    }
    let mut req = req;
    let body: DeleteMappingsBody = parse_json(&mut req).await?;
    store.delete_gcal_mappings(&body.task_ids).await?;
    Ok(Response::empty()?)
}
