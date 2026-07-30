//! Shared input validation for API boundary checks.
//!
//! Both `takusu-local-lib` (the local server) and `takusu-worker` (the
//! Cloudflare Worker) need to reject bad input with `400 Bad Request` before
//! it reaches storage. This module centralises the validation logic so the
//! two backends cannot drift (#1322).
//!
//! All validators return [`StorageError::BadRequest`]; each backend maps that
//! to its own error type (`AppError` / `WorkerError`).
//!
//! ## Recurrence validation
//!
//! `validate_recurrence` validates the JSON *shape* of a recurrence string
//! using [`RecurrenceRuleMirror`], a lightweight mirror of
//! `takusu_habit::RecurrenceRule` that avoids pulling `takusu-habit` (and its
//! `jiff` / `takusu-core` / `rand` dependencies) into the WASM bundle. The
//! mirror's serde shape matches the canonical type exactly, so JSON rejected
//! by one side is rejected by the other.
//!
//! `takusu-local-lib` additionally provides `parse_recurrence`, which
//! deserialises into the real `takusu_habit::RecurrenceRule` when the parsed
//! rule is needed downstream.

use serde::Deserialize;
use takusu_types::{Date, EnumLabel, Quantity, Timestamp, parse_timezone};

use crate::error::StorageError;

// ── mirror types for recurrence JSON shape validation ─────────────────

/// Mirror of `takusu_habit::RecurrenceRule` used only for JSON validation.
/// We duplicate the shape here to avoid pulling `takusu-habit` (and its
/// `jiff` / `takusu-core` / `rand` dependencies) into the WASM bundle.
///
/// Field optionality matches the canonical type exactly: the canonical
/// `RecurrenceRule` declares every field as required (no `#[serde(default)]`),
/// so this mirror does the same — JSON missing any field is rejected, keeping
/// the worker as strict as the local server.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RecurrenceRuleMirror {
    freq: Frequency,
    interval: u32,
    by_day: Vec<NWeekday>,
    by_month: Vec<i8>,
    by_month_day: Vec<i8>,
    count: Option<u32>,
    #[serde(with = "date_strings")]
    exdates: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct NWeekday {
    n: Option<i8>,
    weekday: Weekday,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// Mirror of `takusu_habit::date_strings` that validates each entry is a
/// real `YYYY-MM-DD` calendar date (matching `jiff::civil::Date::strptime`
/// with `%Y-%m-%d`). We avoid `jiff` here to keep the WASM bundle lean.
mod date_strings {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let strings: Vec<String> = Vec::<String>::deserialize(deserializer)?;
        for s in &strings {
            validate_calendar_date(s).map_err(serde::de::Error::custom)?;
        }
        Ok(strings)
    }

    /// Validate that `s` is a real calendar date in `YYYY-MM-DD` form.
    fn validate_calendar_date(s: &str) -> Result<(), String> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 3 {
            return Err(format!("invalid date: {s}"));
        }
        let y: i64 = parts[0].parse().map_err(|_| format!("invalid date: {s}"))?;
        let m: u32 = parts[1].parse().map_err(|_| format!("invalid date: {s}"))?;
        let d: u32 = parts[2].parse().map_err(|_| format!("invalid date: {s}"))?;
        if !(1..=12).contains(&m) {
            return Err(format!("invalid date: {s}"));
        }
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let max_day = match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => 0,
        };
        if !(1..=max_day).contains(&d) {
            return Err(format!("invalid date: {s}"));
        }
        Ok(())
    }
}

// ── helper free functions ─────────────────────────────────────────────

/// Reject negative or unrealistically large `avg_minutes` / `sigma_minutes`,
/// which would wrap to a huge `u64` slot count in the planner and break the
/// schedule (#269, #604).
pub fn validate_minutes(avg: i64, sigma: Option<i64>) -> Result<(), StorageError> {
    // Roughly one year in minutes.  This keeps the converted slot count well
    // within the range where `duration_score`, `total_avg`, and timestamp
    // arithmetic cannot overflow, while still allowing long-running tasks.
    const MAX_MINUTES: i64 = 60 * 24 * 365;

    if avg < 0 {
        return Err(StorageError::BadRequest(format!(
            "avg_minutes must be >= 0 (got {avg})"
        )));
    }
    if avg > MAX_MINUTES {
        return Err(StorageError::BadRequest(format!(
            "avg_minutes must be at most {MAX_MINUTES} (got {avg})"
        )));
    }
    if let Some(s) = sigma
        && s < 0
    {
        return Err(StorageError::BadRequest(format!(
            "sigma_minutes must be >= 0 (got {s})"
        )));
    }
    if let Some(s) = sigma
        && s > MAX_MINUTES
    {
        return Err(StorageError::BadRequest(format!(
            "sigma_minutes must be at most {MAX_MINUTES} (got {s})"
        )));
    }
    Ok(())
}

/// Reject titles that cannot be NFKC-normalized for similar-task search (empty,
/// control-character only, or exceeding the normalized-title scalar limit).
/// Validating at the boundary keeps `normalized_title` always populated for
/// stored tasks, so a task is never silently excluded from similar-task search
/// (#942).
pub fn validate_title(title: &str) -> Result<(), StorageError> {
    takusu_search::memory::normalize_text(title, Some(takusu_search::memory::MAX_CONTENT_SCALARS))
        .map_err(|e| StorageError::BadRequest(format!("invalid title: {e}")))?;
    Ok(())
}

/// Reject nonsensical quantity values and ensure `done <= total` when both
/// sides are provided.
pub fn validate_quantity(
    total: Option<Quantity>,
    done: Option<Quantity>,
    original: Option<Quantity>,
) -> Result<(), StorageError> {
    if let Some(t) = total
        && t <= 0
    {
        return Err(StorageError::BadRequest(format!(
            "quantity_total must be > 0 (got {t})"
        )));
    }
    if let Some(o) = original
        && o <= 0
    {
        return Err(StorageError::BadRequest(format!(
            "original_quantity_total must be > 0 (got {o})"
        )));
    }
    if let (Some(t), Some(d)) = (total, done)
        && d > t
    {
        return Err(StorageError::BadRequest(format!(
            "quantity_done cannot exceed quantity_total ({d} > {t})"
        )));
    }
    Ok(())
}

/// Verify the recurrence string parses as a valid `RecurrenceRule` shape so
/// that bad JSON is rejected at the API boundary instead of crashing later
/// (#285).
pub fn validate_recurrence(recurrence: &str) -> Result<(), StorageError> {
    serde_json::from_str::<RecurrenceRuleMirror>(recurrence)
        .map_err(|e| StorageError::BadRequest(format!("invalid recurrence: {e}")))?;
    Ok(())
}

/// Validate that `start <= end` for a scheduled span (#303).
///
/// `takusu_types::Date` already enforces strict `YYYY-MM-DD` formatting and
/// real calendar dates at parse/deserialization time, so the only remaining
/// check here is ordering.
pub fn validate_scheduled_span_dates(start: &Date, end: &Date) -> Result<(), StorageError> {
    if start > end {
        return Err(StorageError::BadRequest(format!(
            "start_date ({start}) must be <= end_date ({end})"
        )));
    }
    Ok(())
}

/// Validate a user-supplied timezone string. Accepts IANA identifiers and
/// fixed-offset strings supported by `jiff`. Bad input is reported as
/// `400 Bad Request`.
pub fn validate_timezone(tz: &str) -> Result<(), StorageError> {
    parse_timezone(tz)
        .map_err(|e| StorageError::BadRequest(e.to_string()))
        .map(|_| ())
}

/// Validate `start_at` / `end_at` datetime values and that the effective
/// start is not after the effective end. Missing fields are filled from the
/// existing row for comparison when one side is being updated (#934).
///
/// Uses `Option<Option<Timestamp>>` semantics:
/// `None` = no change, `Some(None)` = clear, `Some(Some(ts))` = set.
pub fn validate_task_datetimes(
    start_at: Option<Option<&Timestamp>>,
    end_at: Option<&Timestamp>,
    existing_start: Option<&Timestamp>,
    existing_end: Option<&Timestamp>,
) -> Result<(), StorageError> {
    // start_at: None = no change → use existing; Some(None) = clear; Some(Some(ts)) = set.
    // end_at:   None = no change → use existing; Some(ts) = set (cannot be cleared).
    let effective_start = match &start_at {
        None => existing_start.copied(),
        Some(inner) => inner.copied(),
    };
    let effective_end = match &end_at {
        None => existing_end.copied(),
        Some(e) => Some(**e),
    };

    if let (Some(s), Some(e)) = (effective_start, effective_end)
        && s > e
    {
        return Err(StorageError::BadRequest(format!(
            "start_at must be <= end_at ({s} > {e})"
        )));
    }
    Ok(())
}

// ── Validate trait ─────────────────────────────────────────────────────

/// Validate self-contained input before it reaches storage.
///
/// Implementations check only fields that can be validated without external
/// context (no existing row, no timezone from settings). Context-dependent
/// checks (e.g. `UpdateTask` datetime ordering against an existing task) are
/// kept as explicit free-function calls at the call site.
pub trait Validate {
    fn validate(&self) -> Result<(), StorageError>;
}

impl Validate for crate::CreateSkill {
    fn validate(&self) -> Result<(), StorageError> {
        const MAX_SLUG_LEN: usize = 64;
        const MAX_NAME_LEN: usize = 100;
        const MAX_DESC_LEN: usize = 500;
        const MAX_BODY_LEN: usize = 64 * 1024;

        if self.slug.is_empty() || self.slug.len() > MAX_SLUG_LEN {
            return Err(StorageError::BadRequest(format!(
                "slug must be 1..{MAX_SLUG_LEN} characters"
            )));
        }
        if self.slug.starts_with('.') || self.slug.contains('/') || self.slug.contains("..") {
            return Err(StorageError::BadRequest(
                "slug must not contain path components".into(),
            ));
        }
        if !self
            .slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(StorageError::BadRequest(
                "slug must contain only ASCII letters, digits, '-', '_'".into(),
            ));
        }
        if self.name.is_empty() || self.name.len() > MAX_NAME_LEN {
            return Err(StorageError::BadRequest(format!(
                "name must be 1..{MAX_NAME_LEN} characters"
            )));
        }
        if self.description.len() > MAX_DESC_LEN {
            return Err(StorageError::BadRequest(format!(
                "description must be at most {MAX_DESC_LEN} characters"
            )));
        }
        if self.body.is_empty() || self.body.len() > MAX_BODY_LEN {
            return Err(StorageError::BadRequest(format!(
                "body must be 1..{MAX_BODY_LEN} characters"
            )));
        }
        Ok(())
    }
}

impl Validate for crate::CreateMemory {
    fn validate(&self) -> Result<(), StorageError> {
        if !matches!(
            self.kind,
            takusu_types::MemoryKind::ProperNoun
                | takusu_types::MemoryKind::Fact
                | takusu_types::MemoryKind::TaskNote
        ) {
            return Err(StorageError::BadRequest(
                "kind must be 'proper_noun', 'fact', or 'task_note'".into(),
            ));
        }
        if takusu_search::memory::normalize_key(&self.key).is_err() {
            return Err(StorageError::BadRequest("invalid key".into()));
        }
        if takusu_search::memory::normalize_content(&self.content).is_err() {
            return Err(StorageError::BadRequest("invalid content".into()));
        }
        if self
            .subject_type
            .as_ref()
            .is_some_and(|s| s.as_str().len() > 64)
        {
            return Err(StorageError::BadRequest("subject_type too long".into()));
        }
        if self.subject_id.as_ref().is_some_and(|s| s.len() > 64) {
            return Err(StorageError::BadRequest("subject_id too long".into()));
        }
        if self.kind == takusu_types::MemoryKind::TaskNote {
            if self.subject_type != Some(takusu_types::SubjectType::Task) {
                return Err(StorageError::BadRequest(
                    "task_note requires subject_type='task'".into(),
                ));
            }
            if self.subject_id.as_ref().is_none_or(|s| s.is_empty()) {
                return Err(StorageError::BadRequest(
                    "task_note requires subject_id".into(),
                ));
            }
        }
        Ok(())
    }
}

impl Validate for crate::CreateTask {
    fn validate(&self) -> Result<(), StorageError> {
        validate_minutes(self.avg_minutes, self.sigma_minutes)?;
        validate_title(&self.title)?;
        // For create/replace there is no existing row to compare against.
        validate_task_datetimes(Some(self.start_at.as_ref()), Some(&self.end_at), None, None)?;
        Ok(())
    }
}

impl Validate for crate::UpdateTask {
    fn validate(&self) -> Result<(), StorageError> {
        // Validate minutes if provided. avg_minutes is required to be present
        // only when it is actually set in the update body.
        if let Some(avg) = self.avg_minutes {
            validate_minutes(avg, self.sigma_minutes)?;
        } else if let Some(sigma) = self.sigma_minutes {
            validate_minutes(0, Some(sigma))?;
        }
        if let Some(ref t) = self.title {
            validate_title(t)?;
        }
        // Datetime ordering against an existing row is context-dependent and
        // validated explicitly at the call site.
        Ok(())
    }
}

impl Validate for crate::CreateHabit {
    fn validate(&self) -> Result<(), StorageError> {
        validate_minutes(self.avg_minutes, self.sigma_minutes)?;
        validate_recurrence(&self.recurrence)?;
        Ok(())
    }
}

impl Validate for crate::UpdateHabit {
    fn validate(&self) -> Result<(), StorageError> {
        if let Some(avg) = self.avg_minutes {
            validate_minutes(avg, self.sigma_minutes)?;
        } else if let Some(sigma) = self.sigma_minutes {
            validate_minutes(0, Some(sigma))?;
        }
        if let Some(recurrence) = &self.recurrence {
            validate_recurrence(recurrence)?;
        }
        Ok(())
    }
}

impl Validate for crate::CreateHabitScheduledSpan {
    fn validate(&self) -> Result<(), StorageError> {
        validate_scheduled_span_dates(&self.start_date, &self.end_date)
    }
}

impl Validate for crate::UpdateSettings {
    fn validate(&self) -> Result<(), StorageError> {
        if let Some(tz) = &self.tz {
            validate_timezone(tz)?;
        }
        // sleep_start/sleep_end are Option<TimeOfDay>, already validated by deserialization.
        Ok(())
    }
}

/// Validate a bulk-replace step array (#95): per-field sanity + DAG integrity
/// (intra-habit references, cycle detection).
impl Validate for [crate::HabitStepInput] {
    fn validate(&self) -> Result<(), StorageError> {
        use std::collections::HashMap;

        for s in self {
            validate_minutes(s.avg_minutes, s.sigma_minutes)?;
            // start_time/end_time are TimeOfDay, already validated by deserialization.
        }

        // Build id → index map for steps that carry an id. A depends_on reference
        // must point at a sibling step with a known id.
        let mut id_to_idx: HashMap<&str, usize> = HashMap::new();
        for (i, s) in self.iter().enumerate() {
            if let Some(ref id) = s.id {
                id_to_idx.insert(id.as_str(), i);
            }
        }

        let mut adj = vec![Vec::new(); self.len()];
        for (i, s) in self.iter().enumerate() {
            for dep in &s.depends_on {
                let Some(&dep_idx) = id_to_idx.get(dep.as_str()) else {
                    return Err(StorageError::BadRequest(format!(
                        "step depends_on references unknown step id: {dep}"
                    )));
                };
                adj[i].push(dep_idx);
            }
        }

        detect_cycle(&adj).map_err(|_| StorageError::BadRequestCycle)?;
        Ok(())
    }
}

impl Validate for crate::HabitPreviewRequest {
    fn validate(&self) -> Result<(), StorageError> {
        validate_minutes(self.avg_minutes, self.sigma_minutes)?;
        validate_recurrence(&self.recurrence)?;
        self.steps.validate()?;
        Ok(())
    }
}

// ── cycle detection ───────────────────────────────────────────────────

/// DFS cycle detection over an adjacency list. Returns `Err(())` if a cycle
/// exists. Uses a lightweight 3-color DFS so the shared crate does not need
/// `petgraph` (which `takusu-local-lib` pulls in separately for its richer
/// graph utilities).
#[allow(clippy::result_unit_err)]
pub(crate) fn detect_cycle(adj: &[Vec<usize>]) -> Result<(), ()> {
    let n = adj.len();
    let mut color = vec![0u8; n];
    fn dfs(v: usize, adj: &[Vec<usize>], color: &mut [u8]) -> bool {
        color[v] = 1;
        for &u in &adj[v] {
            if color[u] == 1 {
                return true;
            }
            if color[u] == 0 && dfs(u, adj, color) {
                return true;
            }
        }
        color[v] = 2;
        false
    }
    for v in 0..n {
        if color[v] == 0 && dfs(v, adj, &mut color) {
            return Err(());
        }
    }
    Ok(())
}

// ── tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HabitStepInput;

    // ── validate_minutes ──────────────────────────────────────────────

    #[test]
    fn minutes_reject_negative_avg() {
        assert!(validate_minutes(-1, None).is_err());
        assert!(validate_minutes(0, None).is_ok());
    }

    #[test]
    fn minutes_reject_negative_sigma() {
        assert!(validate_minutes(10, Some(-1)).is_err());
        assert!(validate_minutes(10, Some(0)).is_ok());
    }

    #[test]
    fn minutes_reject_excessive_avg() {
        let max_minutes = 60 * 24 * 365;
        assert!(validate_minutes(max_minutes, None).is_ok());
        assert!(validate_minutes(max_minutes + 1, None).is_err());
    }

    #[test]
    fn minutes_reject_excessive_sigma() {
        let max_minutes = 60 * 24 * 365;
        assert!(validate_minutes(10, Some(max_minutes)).is_ok());
        assert!(validate_minutes(10, Some(max_minutes + 1)).is_err());
    }

    // ── validate_recurrence ───────────────────────────────────────────

    #[test]
    fn recurrence_rejects_garbage() {
        assert!(validate_recurrence("not json").is_err());
    }

    #[test]
    fn recurrence_accepts_valid_rule() {
        let rule = r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#;
        assert!(validate_recurrence(rule).is_ok());
    }

    #[test]
    fn recurrence_rejects_missing_required_field() {
        // Missing interval/by_day/etc. — the canonical type requires them.
        let rule = r#"{"freq":"daily"}"#;
        assert!(validate_recurrence(rule).is_err());
    }

    #[test]
    fn recurrence_rejects_invalid_freq() {
        let rule = r#"{"freq":"hourly","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#;
        assert!(validate_recurrence(rule).is_err());
    }

    #[test]
    fn recurrence_rejects_invalid_exdate() {
        let rule = r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":["notadate"]}"#;
        assert!(validate_recurrence(rule).is_err());
    }

    #[test]
    fn recurrence_rejects_impossible_calendar_date() {
        // 2026-02-30 is not a real date.
        let rule = r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":["2026-02-30"]}"#;
        assert!(validate_recurrence(rule).is_err());
    }

    #[test]
    fn recurrence_accepts_leap_day() {
        let rule = r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":["2024-02-29"]}"#;
        assert!(validate_recurrence(rule).is_ok());
    }

    // ── validate_scheduled_span_dates ─────────────────────────────────

    #[test]
    fn scheduled_span_dates_accepts_valid_range() {
        let s: Date = "2026-08-01".parse().unwrap();
        let e: Date = "2026-08-07".parse().unwrap();
        assert!(validate_scheduled_span_dates(&s, &e).is_ok());
        assert!(validate_scheduled_span_dates(&e, &e).is_ok());
    }

    #[test]
    fn scheduled_span_dates_rejects_reversed() {
        let s: Date = "2026-08-07".parse().unwrap();
        let e: Date = "2026-08-01".parse().unwrap();
        assert!(validate_scheduled_span_dates(&s, &e).is_err());
    }

    // ── validate_steps ────────────────────────────────────────────────

    fn step(id: &str, deps: Vec<&str>) -> HabitStepInput {
        HabitStepInput {
            id: Some(id.to_string()),
            position: 0,
            title: "s".into(),
            description: None,
            start_time: "08:00".parse().unwrap(),
            end_time: "09:00".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: Some(5),
            parallelizable: None,
            allows_parallel: None,
            abandonability: None,
            fixed: None,
            depends_on: deps.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn steps_accept_valid_dag() {
        let steps = [step("a", vec![]), step("b", vec!["a"])];
        assert!(steps.validate().is_ok());
    }

    #[test]
    fn steps_reject_cycle() {
        let steps = [step("a", vec!["b"]), step("b", vec!["a"])];
        assert!(steps.validate().is_err());
    }

    #[test]
    fn steps_reject_unknown_dep() {
        let steps = [step("a", vec!["nope"])];
        assert!(steps.validate().is_err());
    }

    #[test]
    fn steps_reject_negative_avg() {
        let mut s = step("a", vec![]);
        s.avg_minutes = -1;
        assert!([s].validate().is_err());
    }

    // ── validate_timezone ─────────────────────────────────────────────

    #[test]
    fn timezone_accepts_iana_and_offsets() {
        assert!(validate_timezone("Asia/Tokyo").is_ok());
        assert!(validate_timezone("UTC").is_ok());
        assert!(validate_timezone("+09:00").is_ok());
        assert!(validate_timezone("-05:30").is_ok());
        assert!(validate_timezone(" +09:00").is_ok());
        assert!(validate_timezone("+0900").is_ok());
        assert!(validate_timezone("+09").is_ok());
    }

    #[test]
    fn timezone_rejects_unknown() {
        assert!(validate_timezone("Asia/Tokyoo").is_err());
        assert!(validate_timezone("not/a/tz").is_err());
        // UTC±14 is the widest real-world offset.
        assert!(validate_timezone("+14:00:00").is_ok());
        assert!(validate_timezone("-14:00:00").is_ok());
        assert!(validate_timezone("+14:00:01").is_err());
        assert!(validate_timezone("+24:00:00").is_err());
        assert!(validate_timezone("+25:59:59").is_err());
        assert!(validate_timezone("+26:00:00").is_err());
    }

    // ── validate_task_datetimes ───────────────────────────────────────

    #[test]
    fn validate_task_datetimes_accepts_valid_range() {
        let s: Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        let e: Timestamp = "2026-07-22T12:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(Some(Some(&s)), Some(&e), None, None).is_ok());
    }

    #[test]
    fn validate_task_datetimes_rejects_reversed() {
        let s: Timestamp = "2026-07-22T12:00:00Z".parse().unwrap();
        let e: Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(Some(Some(&s)), Some(&e), None, None).is_err());
    }

    #[test]
    fn validate_task_datetimes_fills_existing_for_partial_update() {
        let e: Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        let existing: Timestamp = "2026-07-22T08:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e), Some(&existing), None).is_ok());
        let e2: Timestamp = "2026-07-22T07:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e2), Some(&existing), None).is_err());
    }

    #[test]
    fn validate_task_datetimes_accepts_none_existing() {
        let e: Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e), None, None).is_ok());
    }

    // ── detect_cycle ──────────────────────────────────────────────────

    #[test]
    fn detect_cycle_empty_graph() {
        assert!(detect_cycle(&[]).is_ok());
    }

    #[test]
    fn detect_cycle_acyclic() {
        let adj = vec![vec![1], vec![2], vec![]];
        assert!(detect_cycle(&adj).is_ok());
    }

    #[test]
    fn detect_cycle_simple() {
        let adj = vec![vec![1], vec![0]];
        assert!(detect_cycle(&adj).is_err());
    }

    #[test]
    fn detect_cycle_self_loop() {
        let adj = vec![vec![0]];
        assert!(detect_cycle(&adj).is_err());
    }
}
