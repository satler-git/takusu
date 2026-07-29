//! Input validation at the API boundary.
//!
//! All validation logic lives in [`takusu_storage::validate`] so the worker
//! and the local server share a single implementation (#1322). This module
//! re-exports the shared free functions with `WorkerError` mapping via the
//! `From<StorageError>` impl in [`crate::error`].

pub use takusu_storage::validate::{
    validate_minutes, validate_quantity, validate_recurrence, validate_scheduled_span_dates,
    validate_task_datetimes, validate_title,
};
