//! Shared date helpers used by `app.rs` and `storage_sqlite.rs`
//! (extracted from the per-module duplicates, #1258).

/// Validate that `start <= end` (#303).
///
/// `takusu_util::Date` already enforces strict `YYYY-MM-DD` formatting and
/// real calendar dates at parse/deserialization time, so the only remaining
/// check here is ordering.
pub(crate) fn validate_scheduled_span_dates(
    start: &takusu_util::Date,
    end: &takusu_util::Date,
) -> Result<(), String> {
    if start > end {
        return Err(format!("start_date ({start}) must be <= end_date ({end})"));
    }
    Ok(())
}
