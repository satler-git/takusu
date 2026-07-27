//! Input validation at the API boundary.
//!
//! These helpers mirror the validation performed by `takusu-local-lib` so the
//! Worker rejects bad input with `400 Bad Request` instead of storing it and
//! crashing later (e.g. during schedule generation).

use serde::Deserialize;
use takusu_util::{Quantity, parse_timezone};

use crate::error::WorkerError;
use crate::models::UpdateSettings;

/// Mirror of `takusu_habit::RecurrenceRule` used only for JSON validation.
/// We duplicate the shape here to avoid pulling `takusu-habit` (and its
/// `jiff` / `takusu-core` / `rand` dependencies) into the WASM bundle, which
/// matches the existing convention of duplicating storage row types in
/// `models.rs`.
///
/// Field optionality matches the canonical type exactly: the canonical
/// `RecurrenceRule` declares every field as required (no `#[serde(default)]`),
/// so this mirror does the same — JSON missing any field is rejected, keeping
/// the worker as strict as the local server.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RecurrenceRule {
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

/// Reject negative or unrealistically large `avg_minutes` / `sigma_minutes`,
/// which would wrap to a huge `u64` slot count in the planner and break the
/// schedule (#269, #604).
pub(crate) fn validate_minutes(avg: i64, sigma: Option<i64>) -> Result<(), WorkerError> {
    // Roughly one year in minutes.  This keeps the converted slot count well
    // within the range where `duration_score`, `total_avg`, and timestamp
    // arithmetic cannot overflow, while still allowing long-running tasks.
    const MAX_MINUTES: i64 = 60 * 24 * 365;

    if avg < 0 {
        return Err(WorkerError::BadRequest(format!(
            "avg_minutes must be >= 0 (got {avg})"
        )));
    }
    if avg > MAX_MINUTES {
        return Err(WorkerError::BadRequest(format!(
            "avg_minutes must be at most {MAX_MINUTES} (got {avg})"
        )));
    }
    if let Some(s) = sigma
        && s < 0
    {
        return Err(WorkerError::BadRequest(format!(
            "sigma_minutes must be >= 0 (got {s})"
        )));
    }
    if let Some(s) = sigma
        && s > MAX_MINUTES
    {
        return Err(WorkerError::BadRequest(format!(
            "sigma_minutes must be at most {MAX_MINUTES} (got {s})"
        )));
    }
    Ok(())
}

/// Reject titles that cannot be NFKC-normalized for similar-task search (empty,
/// control-character only, or exceeding the normalized-title scalar limit).
/// Validating at the boundary keeps `normalized_title` always populated for
/// stored tasks, so a task is never silently excluded from similar-task search
/// (#942). Mirrors `takusu-local-lib`'s `validate_title`.
pub(crate) fn validate_title(title: &str) -> Result<(), WorkerError> {
    takusu_util::memory::normalize_text(title, Some(takusu_util::memory::MAX_CONTENT_SCALARS))
        .map_err(|e| WorkerError::BadRequest(format!("invalid title: {e}")))?;
    Ok(())
}

/// Reject nonsensical quantity values and ensure `done <= total` when both
/// sides are provided.
pub(crate) fn validate_quantity(
    total: Option<Quantity>,
    done: Option<Quantity>,
    original: Option<Quantity>,
) -> Result<(), WorkerError> {
    if let Some(t) = total
        && t <= 0
    {
        return Err(WorkerError::BadRequest(format!(
            "quantity_total must be > 0 (got {t})"
        )));
    }
    if let Some(o) = original
        && o <= 0
    {
        return Err(WorkerError::BadRequest(format!(
            "original_quantity_total must be > 0 (got {o})"
        )));
    }
    if let (Some(t), Some(d)) = (total, done)
        && d > t
    {
        return Err(WorkerError::BadRequest(format!(
            "quantity_done cannot exceed quantity_total ({d} > {t})"
        )));
    }
    Ok(())
}

/// Verify the recurrence string parses as a `RecurrenceRule` so that bad JSON
/// is rejected at the API boundary instead of crashing later (#285).
pub(crate) fn validate_recurrence(recurrence: &str) -> Result<(), WorkerError> {
    serde_json::from_str::<RecurrenceRule>(recurrence)
        .map_err(|e| WorkerError::BadRequest(format!("invalid recurrence: {e}")))?;
    Ok(())
}

/// Validate that `start` and `end` are real `YYYY-MM-DD` calendar dates and
/// that `start <= end`. Mirrors the sqlite-side `validate_scheduled_span_dates`.
pub(crate) fn validate_scheduled_span_dates(start: &str, end: &str) -> Result<(), WorkerError> {
    let s = parse_calendar_date(start)
        .ok_or_else(|| WorkerError::BadRequest(format!("invalid start_date: {start}")))?;
    let e = parse_calendar_date(end)
        .ok_or_else(|| WorkerError::BadRequest(format!("invalid end_date: {end}")))?;
    if s > e {
        return Err(WorkerError::BadRequest(format!(
            "start_date ({start}) must be <= end_date ({end})"
        )));
    }
    Ok(())
}

/// Validate a user-supplied timezone string. Accepts IANA identifiers and
/// fixed-offset strings supported by `jiff`. Bad input is reported as
/// `400 Bad Request`.
pub(crate) fn validate_timezone(tz: &str) -> Result<(), WorkerError> {
    parse_timezone(tz)
        .map_err(WorkerError::BadRequest)
        .map(|_| ())
}

/// Validate the fields of a `PUT /api/settings` request. Only fields that
/// are present in the body are checked; missing fields inherit valid values
/// from the existing row.
pub(crate) fn validate_settings(body: &UpdateSettings) -> Result<(), WorkerError> {
    if let Some(tz) = &body.tz {
        validate_timezone(tz)?;
    }
    // sleep_start / sleep_end are TimeOfDay and validated at deserialization.
    Ok(())
}

/// Validate `start_at` / `end_at` datetime values and that the effective
/// start is not after the effective end. Missing fields are filled from the
/// existing row for comparison when one side is being updated (#934).
///
/// Uses `Option<Option<Timestamp>>` semantics:
/// `None` = no change, `Some(None)` = clear, `Some(Some(ts))` = set.
pub(crate) fn validate_task_datetimes(
    start_at: Option<Option<&takusu_util::Timestamp>>,
    end_at: Option<&takusu_util::Timestamp>,
    _tz: &jiff::tz::TimeZone,
    existing_start: Option<&takusu_util::Timestamp>,
    existing_end: Option<&takusu_util::Timestamp>,
) -> Result<(), WorkerError> {
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
        return Err(WorkerError::BadRequest(format!(
            "start_at must be <= end_at ({s} > {e})"
        )));
    }
    Ok(())
}

/// Validate a bulk-replace step array (#95): per-field sanity + DAG integrity
/// (intra-habit references, cycle detection). Mirrors the app-side
/// `validate_steps`.
pub(crate) fn validate_steps(steps: &[crate::models::HabitStepInput]) -> Result<(), WorkerError> {
    use std::collections::HashMap;

    // Per-field validation.
    for s in steps {
        validate_minutes(s.avg_minutes, s.sigma_minutes)?;
        // start_time / end_time are TimeOfDay and validated at deserialization.
    }

    // Build id → index map for steps that carry an id. A depends_on reference
    // must point at a sibling step with a known id.
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, s) in steps.iter().enumerate() {
        if let Some(ref id) = s.id {
            id_to_idx.insert(id.clone(), i);
        }
    }

    // Build adjacency (depends_on → indices) and validate references.
    let mut adj = vec![Vec::new(); steps.len()];
    for (i, s) in steps.iter().enumerate() {
        for dep in &s.depends_on {
            let Some(&dep_idx) = id_to_idx.get(dep) else {
                return Err(WorkerError::BadRequest(format!(
                    "step depends_on references unknown step id: {dep}"
                )));
            };
            adj[i].push(dep_idx);
        }
    }

    detect_cycle(&adj)?;
    Ok(())
}

/// DFS cycle detection over an adjacency list. Returns an error if a cycle
/// exists. Mirrors `takusu_local_lib::app::detect_cycle`.
fn detect_cycle(adj: &[Vec<usize>]) -> Result<(), WorkerError> {
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
            return Err(WorkerError::BadRequest(
                "habit steps に循環依存が検出されました".into(),
            ));
        }
    }
    Ok(())
}

/// Parse a `YYYY-MM-DD` string into a `(year, month, day)` tuple if it is a
/// real calendar date, else `None`.
///
/// Enforces zero-padded fields (4-digit year, 2-digit month/day) so that
/// lexicographic comparison against `jiff`'s zero-padded `Date::to_string()`
/// works correctly during pause matching (#303).
fn parse_calendar_date(s: &str) -> Option<(i64, u32, u32)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    if parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    if !(1..=12).contains(&m) {
        return None;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let max_day = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=max_day).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn scheduled_span_dates_accepts_valid_range() {
        assert!(validate_scheduled_span_dates("2026-08-01", "2026-08-07").is_ok());
        assert!(validate_scheduled_span_dates("2026-08-07", "2026-08-07").is_ok());
    }

    #[test]
    fn scheduled_span_dates_rejects_reversed() {
        assert!(validate_scheduled_span_dates("2026-08-07", "2026-08-01").is_err());
    }

    #[test]
    fn scheduled_span_dates_rejects_bad_format() {
        assert!(validate_scheduled_span_dates("2026/08/01", "2026-08-07").is_err());
        assert!(validate_scheduled_span_dates("2026-08-01", "notadate").is_err());
        assert!(validate_scheduled_span_dates("2026-13-01", "2026-08-07").is_err());
        assert!(validate_scheduled_span_dates("2026-02-30", "2026-08-07").is_err());
    }

    #[test]
    fn scheduled_span_dates_rejects_non_zero_padded() {
        // Non-zero-padded dates would pass numeric parsing but break the
        // lexicographic comparison against jiff's zero-padded Date::to_string,
        // so they must be rejected (#303).
        assert!(validate_scheduled_span_dates("2026-8-1", "2026-08-07").is_err());
        assert!(validate_scheduled_span_dates("2026-08-01", "2026-8-7").is_err());
        assert!(validate_scheduled_span_dates("026-08-01", "2026-08-07").is_err());
    }

    use crate::models::HabitStepInput;

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
        let steps = vec![step("a", vec![]), step("b", vec!["a"])];
        assert!(validate_steps(&steps).is_ok());
    }

    #[test]
    fn steps_reject_cycle() {
        let steps = vec![step("a", vec!["b"]), step("b", vec!["a"])];
        assert!(validate_steps(&steps).is_err());
    }

    #[test]
    fn steps_reject_unknown_dep() {
        let steps = vec![step("a", vec!["nope"])];
        assert!(validate_steps(&steps).is_err());
    }

    #[test]
    fn steps_reject_bad_time() {
        // TimeOfDay cannot represent invalid times like "25:00" — the type
        // itself rejects them at parse time. Here we verify that a valid
        // TimeOfDay passes validation (the bad-time case is now impossible
        // to construct).
        let s = step("a", vec![]);
        assert!(validate_steps(&[s]).is_ok());
    }

    #[test]
    fn steps_reject_negative_avg() {
        let mut s = step("a", vec![]);
        s.avg_minutes = -1;
        assert!(validate_steps(&[s]).is_err());
    }

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

    #[test]
    fn validate_settings_rejects_invalid_sleep_time() {
        // TimeOfDay cannot represent "25:00" — invalid times are rejected at
        // parse/deserialization time, not by validate_settings. Here we verify
        // that a valid TimeOfDay passes (the bad-time case is now impossible
        // to construct via the type system).
        let body = UpdateSettings {
            sleep_start: Some("23:00".parse().unwrap()),
            ..Default::default()
        };
        assert!(validate_settings(&body).is_ok());
    }

    #[test]
    fn validate_settings_accepts_valid_partial_update() {
        let body = UpdateSettings {
            sleep_start: Some("23:00".parse().unwrap()),
            sleep_end: Some("07:00".parse().unwrap()),
            ..Default::default()
        };
        assert!(validate_settings(&body).is_ok());
    }

    #[test]
    fn validate_task_datetimes_accepts_valid_range() {
        let tz = jiff::tz::TimeZone::UTC;
        let s: takusu_util::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        let e: takusu_util::Timestamp = "2026-07-22T12:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(Some(Some(&s)), Some(&e), &tz, None, None,).is_ok());
    }

    #[test]
    fn validate_task_datetimes_rejects_reversed() {
        let tz = jiff::tz::TimeZone::UTC;
        let s: takusu_util::Timestamp = "2026-07-22T12:00:00Z".parse().unwrap();
        let e: takusu_util::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(Some(Some(&s)), Some(&e), &tz, None, None,).is_err());
    }

    #[test]
    fn validate_task_datetimes_fills_existing_for_partial_update() {
        let tz = jiff::tz::TimeZone::UTC;
        let e: takusu_util::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        let existing: takusu_util::Timestamp = "2026-07-22T08:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e), &tz, Some(&existing), None,).is_ok());
        let e2: takusu_util::Timestamp = "2026-07-22T07:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e2), &tz, Some(&existing), None,).is_err());
    }

    #[test]
    fn validate_task_datetimes_rejects_invalid_existing() {
        // With typed Timestamp, invalid strings cannot be constructed.
        // This test now verifies that a None existing value with a Some end
        // still works (no existing to compare against).
        let tz = jiff::tz::TimeZone::UTC;
        let e: takusu_util::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e), &tz, None, None,).is_ok());
    }
}
