use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_ok, parse_json};
use crate::models::UpdateSettings;
use takusu_contracts::{Storage, Validate};

pub async fn get(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let row = store.get_settings().await?;
    json_ok(&row)
}

pub async fn update(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: UpdateSettings = parse_json(&mut req).await?;
    body.validate()?;
    let store = storage(&env)?;
    let row = store.update_settings(&body).await?;
    json_ok(&row)
}
