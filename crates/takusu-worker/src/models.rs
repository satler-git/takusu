//! Shared domain types re-exported from `takusu_storage`.
//!
//! The row types (`TaskRow` / `HabitRow` / `ScheduleRow` / `MemoryRow` / ...)
//! are defined once in `takusu_storage::model` and shared by the server,
//! client, and worker (#1294). `takusu_storage` gates its `sqlx::FromRow`
//! derives behind the `sqlx` feature, which this WASM crate does not enable,
//! so the re-exported types are plain serde structs here and the `sqlx`
//! dependency never enters the WASM bundle.
//!
//! Primitive newtypes (`Quantity` / `Timestamp` / `Date` / ...) are re-exported
//! from `takusu_types` so existing `crate::models::Quantity` references keep
//! resolving.

pub use takusu_storage::model::*;
