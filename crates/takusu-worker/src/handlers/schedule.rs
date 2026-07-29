use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::models::SaveScheduleRequest;
use takusu_storage::Storage;

pub async fn get(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    match store.get_schedule().await? {
        Some(row) => json_ok(&row),
        None => Err(WorkerError::NotFound("no active schedule".into())),
    }
}

pub async fn save(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: SaveScheduleRequest = parse_json(&mut req).await?;
    let store = storage(&env)?;
    let row = store.save_schedule(&body).await?;
    json_created(&row)
}

pub async fn clear(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    store.clear_schedule().await?;
    Ok(Response::empty()?)
}
