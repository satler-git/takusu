//! Shared ID resolution logic for task and habit references.
//!
//! Handlers accept task/habit references in several human-friendly forms
//! (display ids like `#42`, `h1#3`, `h2`, or full UUIDs). This module
//! centralises the parsing and D1 lookup so that `tasks`, `habits`,
//! `progress`, and `memory` handlers all resolve ids identically.
//!
//! UUID prefixes are not accepted (#1251).

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
    // Allow display ids with a leading `#` (e.g. `#42`) written by the LLM.
    let id = id.strip_prefix('#').unwrap_or(id);

    // `h{habit_display_id}#{task_display_id}` → habit task lookup (#380).
    if let Some(rest) = id.strip_prefix(['h', 'H'])
        && let Some((hdisp, tdisp)) = rest.split_once('#')
        && let (Ok(hnum), Ok(tnum)) = (hdisp.parse::<i64>(), tdisp.parse::<i64>())
    {
        let stmt = database.prepare(
            "SELECT t.id AS id FROM tasks t JOIN habits h ON t.habit_id = h.id \
             WHERE h.display_id = ?1 AND t.display_id = ?2",
        );
        let rows: Vec<IdRow> = safe_all(&stmt.bind(&[
            JsValue::from_f64(hnum as f64),
            JsValue::from_f64(tnum as f64),
        ])?)
        .await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::NotFound(format!("task {id} not found")));
    }

    // Numeric → display_id lookup for non-habit tasks only (#380).
    if let Ok(num) = id.parse::<i64>() {
        let stmt =
            database.prepare("SELECT id FROM tasks WHERE display_id = ?1 AND habit_id IS NULL");
        let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_f64(num as f64)])?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::NotFound(format!("task {id} not found")));
    }

    // Full UUID — verify it exists before accepting it.
    if id.contains('-') {
        let stmt = database.prepare("SELECT id FROM tasks WHERE id = ?1");
        let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_str(id)])?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::NotFound(format!("task {id} not found")));
    }

    // Anything else (e.g. a UUID prefix) is not a valid reference (#1251).
    Err(WorkerError::NotFound(format!("task {id} not found")))
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
    // `h<N>` → habit display_id lookup (#305).
    if let Some(rest) = id.strip_prefix(['h', 'H'])
        && let Ok(num) = rest.parse::<i64>()
    {
        let stmt = database.prepare("SELECT id FROM habits WHERE display_id = ?1");
        let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_f64(num as f64)])?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::NotFound(format!("habit {id} not found")));
    }

    // Full UUID — verify it exists before accepting it (#1271).
    if id.contains('-') {
        let stmt = database.prepare("SELECT id FROM habits WHERE id = ?1");
        let rows: Vec<IdRow> = safe_all(&stmt.bind(&[JsValue::from_str(id)])?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::NotFound(format!("habit {id} not found")));
    }

    // Anything else (e.g. a UUID prefix) is not a valid reference (#1251).
    Err(WorkerError::NotFound(format!("habit {id} not found")))
}
