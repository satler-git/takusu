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

impl From<takusu_storage::StorageError> for WorkerError {
    fn from(e: takusu_storage::StorageError) -> Self {
        use takusu_storage::StorageError;
        match e {
            StorageError::NotFound(m) => WorkerError::NotFound(m),
            StorageError::BadRequest(m) => WorkerError::BadRequest(m),
            // Preserve the user-visible message the worker previously
            // produced. The local server maps this to the structured
            // `BadRequestKind::CycleDetected` variant via `storage_to_app`.
            StorageError::BadRequestCycle => {
                WorkerError::BadRequest("habit steps に循環依存が検出されました".into())
            }
            StorageError::Unauthorized => WorkerError::Unauthorized,
            StorageError::Conflict(m) => WorkerError::Conflict(m),
            StorageError::Internal(m) => WorkerError::Internal(m),
            StorageError::Io(m) => WorkerError::Internal(m),
        }
    }
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
