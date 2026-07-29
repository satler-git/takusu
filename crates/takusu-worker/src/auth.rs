use worker::{Env, Request};

use crate::error::WorkerError;

pub fn jwt_secret(env: &Env) -> Result<String, WorkerError> {
    env.secret("TAKUSU_JWT_SECRET")
        .map(|s| s.to_string())
        .map_err(|e| WorkerError::Internal(format!("TAKUSU_JWT_SECRET secret not set: {e}")))
}

pub fn verify_token(req: &Request, env: &Env) -> Result<takusu_types::TokenClaims, WorkerError> {
    let header = req
        .headers()
        .get("authorization")
        .map_err(|e| WorkerError::Internal(format!("failed to read authorization header: {e}")))?
        .and_then(|v| v.strip_prefix("Bearer ").map(|s| s.to_string()))
        .ok_or(WorkerError::Unauthorized)?;

    let secret = jwt_secret(env)?;
    takusu_types::jwt::verify(&secret, &header, takusu_types::DEFAULT_AUD)
        .map_err(|_| WorkerError::Unauthorized)
}

pub fn is_root(req: &Request, env: &Env) -> Result<bool, WorkerError> {
    Ok(verify_token(req, env)?.is_root())
}
