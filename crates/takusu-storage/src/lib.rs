//! # takusu-storage — pluggable storage backend
//!
//! Async `Storage` trait with shared request/response types. The local server
//! (`takusu-local`) is the only consumer; backends are `SqliteStorage` (direct
//! `sqlx`) and `WorkersStorage` (reqwest → Cloudflare Worker + D1).

pub mod error;
pub mod model;
pub mod storage;
pub mod validate;

pub use error::StorageError;
pub use model::*;
pub use storage::Storage;
pub use validate::Validate;
