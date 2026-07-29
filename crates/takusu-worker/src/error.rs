use thiserror::Error;
use worker::{Response, ResponseBuilder};

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal: {0}")]
    Internal(String),
    #[error("worker error: {0}")]
    Worker(#[from] worker::Error),
}

impl WorkerError {
    pub fn status(&self) -> u16 {
        match self {
            WorkerError::NotFound(_) => 404,
            WorkerError::BadRequest(_) => 400,
            WorkerError::Unauthorized => 401,
            WorkerError::Conflict(_) => 409,
            WorkerError::Internal(_) | WorkerError::Worker(_) => 500,
        }
    }

    pub fn body(&self) -> serde_json::Value {
        serde_json::json!({ "message": self.to_string() })
    }
}

pub fn error_response(err: WorkerError) -> worker::Result<Response> {
    match &err {
        WorkerError::Internal(_) | WorkerError::Worker(_) => {
            log::error!("{}", err);
        }
        WorkerError::Unauthorized => {
            log::warn!("{}", err);
        }
        _ => {
            log::info!("{}", err);
        }
    }
    ResponseBuilder::new()
        .with_status(err.status())
        .ok(err.body().to_string())
}
