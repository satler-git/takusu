//! Shared ID resolution logic for task and habit references.
//!
//! Handlers accept task/habit references in several human-friendly forms
//! (display ids like `#42`, `h1#3`, `h2`, or full UUIDs). The parsing rules
//! live in [`takusu_types::TaskRef`] / [`takusu_types::HabitRef`]; this
//! module performs the D1 lookup for each resolved variant so that `tasks`,
//! `habits`, `progress`, and `memory` handlers all resolve ids identically.
//!
//! UUID prefixes are not accepted (#1251).

use takusu_types::{HabitRef, TaskRef};
use wasm_bindgen::JsValue;
use worker::D1Database;

use crate::error::WorkerError;
use crate::handlers::d1::safe_all;

#[derive(serde::Deserialize)]
struct IdRow {
    id: String,
}

/// Resolve a single task reference (display_id number or full UUID) to a full
/// UUID string. UUID prefixes are not accepted (#1251).
///
/// Accepted forms:
/// - `#42` / `42` — non-habit task display_id
/// - `h1#3` — habit task (`h{habit_display_id}#{task_display_id}`, #380)
/// - full UUID (contains `-`)
pub(crate) async fn resolve_task_id(
    database: &D1Database,
    id: &str,
) -> Result<String, WorkerError> {
    let parsed = TaskRef::try_from(id)
        .map_err(|_| WorkerError::NotFound(format!("task {id} not found")))?;
    match parsed {
        TaskRef::HabitTask { habit, task } => {
            let stmt = database.prepare(
                "SELECT t.id AS id FROM tasks t JOIN habits h ON t.habit_id = h.id \
                 WHERE h.display_id = ?1 AND t.display_id = ?2",
            );
            let rows: Vec<IdRow> = safe_all(&stmt.bind(&[
                JsValue::from_f64(habit as f64),
                JsValue::from_f64(task as f64),
            ])?)
            .await?;
            rows.into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| WorkerError::NotFound(format!("task {id} not found")))
        }
        TaskRef::Display(num) => {
            let stmt =
                database.prepare("SELECT id FROM tasks WHERE display_id = ?1 AND habit_id IS NULL");
            let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_f64(num as f64)])?).await?;
            rows.into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| WorkerError::NotFound(format!("task {id} not found")))
        }
        TaskRef::Uuid(uuid) => {
            let stmt = database.prepare("SELECT id FROM tasks WHERE id = ?1");
            let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_str(&uuid)])?).await?;
            rows.into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| WorkerError::NotFound(format!("task {id} not found")))
        }
    }
}

/// Resolve a habit reference (`h<N>` or full UUID) to a full UUID. UUID
/// prefixes are not accepted (#1251).
///
/// Accepted forms:
/// - `h2` / `H2` — habit display_id (#305)
/// - full UUID (contains `-`)
pub(crate) async fn resolve_habit_id(
    database: &D1Database,
    id: &str,
) -> Result<String, WorkerError> {
    let parsed = HabitRef::try_from(id)
        .map_err(|_| WorkerError::NotFound(format!("habit {id} not found")))?;
    match parsed {
        HabitRef::Display(num) => {
            let stmt = database.prepare("SELECT id FROM habits WHERE display_id = ?1");
            let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_f64(num as f64)])?).await?;
            rows.into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| WorkerError::NotFound(format!("habit {id} not found")))
        }
        HabitRef::Uuid(uuid) => {
            let stmt = database.prepare("SELECT id FROM habits WHERE id = ?1");
            let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_str(&uuid)])?).await?;
            rows.into_iter()
                .next()
                .map(|r| r.id)
                .ok_or_else(|| WorkerError::NotFound(format!("habit {id} not found")))
        }
    }
}
