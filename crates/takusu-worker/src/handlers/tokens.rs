use worker::{Env, Request, Response};

use crate::auth;
use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::models::TokenRow;
use takusu_contracts::Storage;

#[derive(serde::Deserialize)]
pub struct CreateTokenBody {
    pub label: Option<String>,
}

fn require_root(req: &Request, env: &Env) -> Result<(), WorkerError> {
    let claims = auth::verify_token(req, env)?;
    if !claims.is_root() {
        return Err(WorkerError::Unauthorized);
    }
    Ok(())
}

pub async fn create(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    require_root(&req, &env)?;
    let body: CreateTokenBody = parse_json(&mut req).await?;
    let label_str = body.label.clone().unwrap_or_default();
    let label_opt: Option<&str> = if label_str.is_empty() {
        None
    } else {
        Some(label_str.as_str())
    };

    let store = storage(&env)?;
    let resp = store.create_token(label_opt).await?;
    json_created(&resp)
}

pub async fn list(req: Request, env: Env) -> Result<Response, WorkerError> {
    require_root(&req, &env)?;
    let store = storage(&env)?;
    let rows: Vec<TokenRow> = store.list_tokens().await?;
    json_ok(&rows)
}

pub async fn revoke(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    require_root(&req, &env)?;
    let id_num: i64 = id
        .parse()
        .map_err(|_| WorkerError::BadRequest(format!("invalid token id: {id}")))?;
    let store = storage(&env)?;
    store.revoke_token(id_num).await?;
    Ok(Response::empty()?)
}

pub async fn parse_json<T: serde::de::DeserializeOwned>(
    req: &mut Request,
) -> Result<T, WorkerError> {
    let text = req.text().await.map_err(WorkerError::Worker)?;
    serde_json::from_str(&text).map_err(|e| WorkerError::BadRequest(format!("invalid json: {e}")))
}

pub fn json_ok<T: serde::Serialize>(value: &T) -> Result<Response, WorkerError> {
    Response::from_json(value).map_err(WorkerError::Worker)
}

pub fn json_created<T: serde::Serialize>(value: &T) -> Result<Response, WorkerError> {
    let body = serde_json::to_string(value).map_err(|e| WorkerError::Internal(e.to_string()))?;
    let mut resp = Response::ok(body)?;
    resp.headers_mut()
        .set("content-type", "application/json")
        .map_err(WorkerError::Worker)?;
    Ok(resp)
}
