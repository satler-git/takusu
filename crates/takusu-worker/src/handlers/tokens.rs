use web_time::{SystemTime, UNIX_EPOCH};
use worker::{Env, Request, Response};

use crate::auth;
use crate::error::WorkerError;
use crate::handlers::auth::db;
use crate::handlers::d1::safe_all;
use crate::models::{TokenCreateResponse, TokenRow};

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

fn token_expires_at(ttl_seconds: i64) -> Option<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let exp = now.saturating_add(ttl_seconds);
    jiff::Timestamp::from_second(exp)
        .ok()
        .map(|t| t.to_string())
}

pub async fn create(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    require_root(&req, &env)?;
    let body: CreateTokenBody = parse_json(&mut req).await?;
    let label_str = body.label.clone().unwrap_or_default();
    let label_opt: Option<String> = if label_str.is_empty() {
        None
    } else {
        Some(label_str)
    };

    let secret = auth::jwt_secret(&env)?;
    let (new_token, jti) = takusu_types::jwt::generate_token_jwt(
        &secret,
        takusu_types::SCOPE_READ_WRITE,
        label_opt.as_deref(),
        None,
    )
    .map_err(|e| WorkerError::Internal(e.to_string()))?;
    let database = db(&env)?;

    let expires_at = token_expires_at(takusu_types::jwt::DEFAULT_TOKEN_TTL_SECONDS);
    let stmt = worker::query!(
        &database,
        "INSERT INTO tokens (jti, scope, label, created_by, created_at, expires_at) VALUES (?1, ?2, ?3, 'authenticated', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?4)",
        jti,
        takusu_types::SCOPE_READ_WRITE,
        label_opt,
        expires_at
    )?;
    stmt.run().await.map_err(WorkerError::Worker)?;

    let lookup = worker::query!(
        &database,
        "SELECT id, jti, scope, label, created_by, created_at, revoked_at, expires_at FROM tokens WHERE jti = ?1",
        jti
    )?;
    let row: Option<TokenRow> = lookup.first(None).await.map_err(WorkerError::Worker)?;
    let row = row.ok_or_else(|| WorkerError::Internal("inserted token not found".into()))?;
    let resp = TokenCreateResponse {
        id: row.id,
        token: new_token,
        scope: row.scope,
        label: row.label,
        created_at: row.created_at,
        expires_at: row.expires_at,
    };
    json_created(&resp)
}

pub async fn list(req: Request, env: Env) -> Result<Response, WorkerError> {
    require_root(&req, &env)?;
    let database = db(&env)?;
    let stmt = worker::query!(
        &database,
        "SELECT id, jti, scope, label, created_by, created_at, revoked_at, expires_at FROM tokens ORDER BY created_at DESC"
    );
    let rows: Vec<TokenRow> = safe_all(&stmt).await?;
    json_ok(&rows)
}

pub async fn revoke(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    require_root(&req, &env)?;
    let id_num: i64 = id
        .parse()
        .map_err(|_| WorkerError::BadRequest(format!("invalid token id: {id}")))?;
    let database = db(&env)?;
    let stmt = worker::query!(
        &database,
        "UPDATE tokens SET revoked_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1 AND revoked_at IS NULL",
        id_num
    )?;
    let result = stmt.run().await.map_err(WorkerError::Worker)?;
    let affected = result
        .meta()
        .map_err(WorkerError::Worker)?
        .and_then(|m| m.rows_written)
        .unwrap_or(0);
    if affected == 0 {
        return Err(WorkerError::NotFound(format!(
            "token {id} not found or already revoked"
        )));
    }
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
