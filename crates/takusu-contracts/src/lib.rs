//! # takusu-contracts — pluggable storage backend
//!
//! Async `Storage` trait with shared request/response types. The local server
//! (`takusu-local`) is the only consumer; backends are `SqliteStorage` (direct
//! `sqlx`) and `WorkersStorage` (reqwest → Cloudflare Worker + D1).

pub mod error;
pub mod model;
pub mod sleep;
pub mod storage;
pub mod validate;
pub mod workload;

pub use error::StorageError;
pub use model::*;
pub use storage::{Storage, resolve_resident_authority_from_rows};
pub use validate::Validate;
