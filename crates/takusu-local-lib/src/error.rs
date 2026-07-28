use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(BadRequestKind),
    #[error("unauthorized")]
    Unauthorized,
    #[error("conflict: {0}")]
    Conflict(ConflictKind),
    #[error("internal: {0}")]
    Internal(String),
}

/// Structured bad-request reasons.
///
/// `Other(String)` is the fallback for ad-hoc validation messages that
/// do not yet warrant a dedicated variant. New structured variants should
/// be added when a reason needs to be matched programmatically by callers.
#[derive(Debug, Error)]
pub enum BadRequestKind {
    /// A dependency cycle was detected among tasks or habit steps.
    #[error("cycle detected in dependencies")]
    CycleDetected,
    /// A time, datetime, or start_time value could not be parsed or was out of
    /// range. The inner string preserves the original, context-specific message
    /// (e.g. `"invalid datetime: ..."` / `"invalid step start_time: ..."`) so
    /// callers keep the same user-visible text while still matching on the
    /// variant programmatically.
    #[error("{0}")]
    InvalidTime(String),
    /// Fallback for unstructured bad-request messages.
    #[error("{0}")]
    Other(String),
}

/// Structured conflict reasons.
///
/// `Other(String)` is the fallback for conflicts surfaced from storage or
/// other sources that do not have a dedicated variant.
#[derive(Debug, Error)]
pub enum ConflictKind {
    /// An attempt to mutate a built-in skill was rejected. `op` records the
    /// rejected operation so the original, operation-specific message
    /// ("cannot be overwritten" / "cannot be edited" / "cannot be deleted")
    /// is preserved while still allowing callers to match on the kind.
    #[error("built-in skill {slug} {op}")]
    BuiltInSkill { slug: String, op: SkillOp },
    /// A resource with the given key already exists.
    #[error("skill {0} already exists")]
    AlreadyExists(String),
    /// A schedule move would violate task deadlines.
    #[error("schedule violations detected")]
    ScheduleViolation,
    /// Fallback for unstructured conflict messages.
    #[error("{0}")]
    Other(String),
}

/// The operation attempted on a built-in skill.
#[derive(Debug, Error)]
pub enum SkillOp {
    #[error("cannot be overwritten")]
    Overwrite,
    #[error("cannot be edited")]
    Edit,
    #[error("cannot be deleted")]
    Delete,
}

pub(crate) fn storage_to_app(e: takusu_storage::StorageError) -> AppError {
    use takusu_storage::StorageError;
    match e {
        StorageError::NotFound(m) => AppError::NotFound(m),
        StorageError::BadRequest(m) => AppError::BadRequest(BadRequestKind::Other(m)),
        StorageError::Unauthorized => AppError::Unauthorized,
        StorageError::Conflict(m) => AppError::Conflict(ConflictKind::Other(m)),
        StorageError::Internal(m) => AppError::Internal(m),
    }
}
