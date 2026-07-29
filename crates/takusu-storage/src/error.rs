use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A dependency cycle was detected among tasks or habit steps. Distinct
    /// from `BadRequest(String)` so backends can map it to a structured
    /// variant (e.g. `AppError::BadRequest(BadRequestKind::CycleDetected)`)
    /// rather than the generic `Other` fallback.
    #[error("cycle detected in dependencies")]
    BadRequestCycle,
    #[error("unauthorized")]
    Unauthorized,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal: {0}")]
    Internal(String),
}
