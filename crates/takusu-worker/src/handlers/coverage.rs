use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_created, parse_json};
use crate::models::{CreateCoverageConfirmation, CreateUnsettledInterval, SettleRequest};
use takusu_contracts::{Storage, Validate};

fn operation_id(req: &Request) -> Option<String> {
    req.headers()
        .get("Idempotency-Key")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("idempotency-key").ok().flatten())
}

pub async fn create_confirmation(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let mut body: CreateCoverageConfirmation = parse_json(&mut req).await?;
    body.validate()
        .map_err(|e| WorkerError::BadRequest(e.to_string()))?;
    if body.operation_id.is_none() {
        body.operation_id = operation_id(&req);
    }
    let store = storage(&env)?;
    let row = store.create_coverage_confirmation(&body).await?;
    json_created(&row)
}

pub async fn create_unsettled_interval(
    mut req: Request,
    env: Env,
) -> Result<Response, WorkerError> {
    let mut body: CreateUnsettledInterval = parse_json(&mut req).await?;
    body.validate()
        .map_err(|e| WorkerError::BadRequest(e.to_string()))?;
    if body.operation_id.is_none() {
        body.operation_id = operation_id(&req);
    }
    let store = storage(&env)?;
    let row = store.create_unsettled_interval(&body).await?;
    json_created(&row)
}

pub async fn settle(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let mut body: SettleRequest = parse_json(&mut req).await?;
    body.validate()
        .map_err(|e| WorkerError::BadRequest(e.to_string()))?;
    if body.operation_id.is_none() {
        body.operation_id = operation_id(&req);
    }
    let store = storage(&env)?;
    let response = store.settle(&body).await?;
    crate::handlers::tokens::json_ok(&response)
}
