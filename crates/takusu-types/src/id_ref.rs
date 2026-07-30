//! Parsed task and habit references (`#42`, `h1#3`, `h2`, full UUID).
//!
//! Each backend (`SqliteStorage`, `WorkersStorage`, `takusu-worker`) accepts
//! human-friendly references and resolves them to full UUIDs. The parsing
//! rules are identical across backends, so they live here as
//! [`TaskRef`] / [`HabitRef`] enums with [`TryFrom<&str>`]. Each backend then
//! only implements the "resolved reference → UUID" lookup.
//!
//! UUID prefixes are not accepted (#1251); only full UUIDs (strings
//! containing `-`) are treated as UUID references.

use thiserror::Error;

/// Error returned when a string cannot be parsed as a task or habit
/// reference.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid reference: {0}")]
pub struct IdRefError(pub String);

/// A parsed task reference.
///
/// Accepted forms:
/// - `#42` / `42` — non-habit task display_id
/// - `h1#3` — habit task (`h{habit_display_id}#{task_display_id}`, #380)
/// - full UUID (contains `-`)
///
/// UUID prefixes are not accepted (#1251).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRef {
    /// Non-habit task display_id (e.g. `#42` or `42`).
    Display(i64),
    /// Habit task reference `h{habit}#{task}` (#380).
    HabitTask { habit: i64, task: i64 },
    /// Full UUID string (contains `-`).
    Uuid(String),
}

/// A parsed habit reference.
///
/// Accepted forms:
/// - `h2` / `H2` — habit display_id (#305)
/// - full UUID (contains `-`)
///
/// UUID prefixes are not accepted (#1251).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HabitRef {
    /// Habit display_id (e.g. `h2`).
    Display(i64),
    /// Full UUID string (contains `-`).
    Uuid(String),
}

impl TryFrom<&str> for TaskRef {
    type Error = IdRefError;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        // Allow display ids with a leading `#` (e.g. `#42`) written by the LLM.
        let id = id.strip_prefix('#').unwrap_or(id);

        // `h{habit_display_id}#{task_display_id}` → habit task lookup (#380).
        if let Some(rest) = id.strip_prefix(['h', 'H'])
            && let Some((hdisp, tdisp)) = rest.split_once('#')
            && let (Ok(hnum), Ok(tnum)) = (hdisp.parse::<i64>(), tdisp.parse::<i64>())
        {
            return Ok(Self::HabitTask {
                habit: hnum,
                task: tnum,
            });
        }

        // Numeric input → display_id for non-habit tasks only (#380).
        if let Ok(num) = id.parse::<i64>() {
            return Ok(Self::Display(num));
        }

        // Full UUID — contains `-`. UUID prefixes are not accepted (#1251).
        if id.contains('-') {
            return Ok(Self::Uuid(id.to_string()));
        }

        Err(IdRefError(id.to_string()))
    }
}

impl TryFrom<&str> for HabitRef {
    type Error = IdRefError;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        // `h<N>` → habit display_id lookup (#305).
        if let Some(rest) = id.strip_prefix(['h', 'H'])
            && let Ok(num) = rest.parse::<i64>()
        {
            return Ok(Self::Display(num));
        }

        // Full UUID — contains `-`. UUID prefixes are not accepted (#1251).
        if id.contains('-') {
            return Ok(Self::Uuid(id.to_string()));
        }

        Err(IdRefError(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── TaskRef ─────────────────────────────────────────────────────────

    #[test]
    fn task_ref_display_with_hash() {
        assert_eq!(TaskRef::try_from("#42").unwrap(), TaskRef::Display(42));
    }

    #[test]
    fn task_ref_display_bare_number() {
        assert_eq!(TaskRef::try_from("42").unwrap(), TaskRef::Display(42));
    }

    #[test]
    fn task_ref_habit_task() {
        assert_eq!(
            TaskRef::try_from("h1#3").unwrap(),
            TaskRef::HabitTask { habit: 1, task: 3 }
        );
    }

    #[test]
    fn task_ref_habit_task_uppercase() {
        assert_eq!(
            TaskRef::try_from("H2#10").unwrap(),
            TaskRef::HabitTask { habit: 2, task: 10 }
        );
    }

    #[test]
    fn task_ref_habit_task_with_hash_prefix() {
        assert_eq!(
            TaskRef::try_from("#h1#3").unwrap(),
            TaskRef::HabitTask { habit: 1, task: 3 }
        );
    }

    #[test]
    fn task_ref_full_uuid() {
        let uuid = "01957d8a-3c7a-7abc-8123-456789abcdef";
        assert_eq!(
            TaskRef::try_from(uuid).unwrap(),
            TaskRef::Uuid(uuid.to_string())
        );
    }

    #[test]
    fn task_ref_garbage_errors() {
        assert!(TaskRef::try_from("hello").is_err());
        assert!(TaskRef::try_from("").is_err());
        assert!(TaskRef::try_from("habc").is_err());
        assert!(TaskRef::try_from("h1#").is_err());
        assert!(TaskRef::try_from("h#3").is_err());
    }

    // ── HabitRef ────────────────────────────────────────────────────────

    #[test]
    fn habit_ref_display() {
        assert_eq!(HabitRef::try_from("h2").unwrap(), HabitRef::Display(2));
    }

    #[test]
    fn habit_ref_display_uppercase() {
        assert_eq!(HabitRef::try_from("H2").unwrap(), HabitRef::Display(2));
    }

    #[test]
    fn habit_ref_full_uuid() {
        let uuid = "01957d8a-3c7a-7abc-8123-456789abcdef";
        assert_eq!(
            HabitRef::try_from(uuid).unwrap(),
            HabitRef::Uuid(uuid.to_string())
        );
    }

    #[test]
    fn habit_ref_garbage_errors() {
        assert!(HabitRef::try_from("hello").is_err());
        assert!(HabitRef::try_from("").is_err());
        assert!(HabitRef::try_from("habc").is_err());
        assert!(HabitRef::try_from("42").is_err());
    }
}
