//! Validation trait and helpers for `takusu-local-lib`.
//!
//! The shared validation logic lives in [`takusu_contracts::validate`] so the
//! local server and the Cloudflare Worker use a single implementation
//! (#1322). This module provides:
//!
//! - A local [`Validate`] trait that blanket-impls for every type that
//!   implements `takusu_contracts::Validate`, mapping `StorageError` to
//!   `AppError`. This lets existing `body.validate()?` call sites work
//!   unchanged.
//! - Thin wrappers around the shared free functions (`validate_minutes`,
//!   `validate_task_datetimes`) that return `AppError`.
//! - `parse_recurrence`, which deserialises into the real
//!   `takusu_habit::RecurrenceRule` (the shared crate only validates JSON
//!   shape to avoid pulling `takusu-habit` into WASM).
//! - `parse_settings_timezone` and [`SettingsPlannerExt`], which depend on
//!   `takusu-core` / `jiff` and are local-server-specific.

use takusu_core::{Minutes, SleepConfig, WorkloadConfig};
use takusu_contracts::SettingsRow;
use takusu_types::SleepInput;

use crate::error::{AppError, storage_to_app};

// ── Validate trait (blanket impl over the shared trait) ───────────────

/// Validate self-contained input before it reaches storage.
///
/// This trait is blanket-implemented for every `T: takusu_contracts::Validate`,
/// mapping `StorageError` to `AppError`. Call `body.validate()?` as before;
/// the compiler resolves to this blanket impl.
pub trait Validate {
    fn validate(&self) -> Result<(), AppError>;
}

impl<T: ?Sized + takusu_contracts::Validate> Validate for T {
    fn validate(&self) -> Result<(), AppError> {
        takusu_contracts::Validate::validate(self).map_err(storage_to_app)
    }
}

// ── thin wrappers returning AppError ──────────────────────────────────

/// Reject negative or unrealistically large `avg_minutes` / `sigma_minutes`
/// (#269, #604). Delegates to the shared implementation.
pub(crate) fn validate_minutes(avg: i64, sigma: Option<i64>) -> Result<(), AppError> {
    takusu_contracts::validate::validate_minutes(avg, sigma).map_err(storage_to_app)
}

/// Validate `start_at` / `end_at` datetime values and that the effective
/// start is not after the effective end (#934). Delegates to the shared
/// implementation.
pub(crate) fn validate_task_datetimes(
    start_at: Option<Option<&takusu_types::Timestamp>>,
    end_at: Option<&takusu_types::Timestamp>,
    existing_start: Option<&takusu_types::Timestamp>,
    existing_end: Option<&takusu_types::Timestamp>,
) -> Result<(), AppError> {
    takusu_contracts::validate::validate_task_datetimes(
        start_at,
        end_at,
        existing_start,
        existing_end,
    )
    .map_err(storage_to_app)
}

// ── local-server-specific helpers ─────────────────────────────────────

/// Parse a recurrence JSON string into a `RecurrenceRule`.
/// This is the single point inside `takusu-local-lib` where storage/client strings
/// become `takusu_habit` types (see `doc/type-safety-issues.md` §3.4 / §8.6).
/// `takusu-worker` validates recurrences separately without converting to `takusu_habit`.
pub(crate) fn parse_recurrence(recurrence: &str) -> Result<takusu_habit::RecurrenceRule, AppError> {
    serde_json::from_str::<takusu_habit::RecurrenceRule>(recurrence).map_err(|e| {
        AppError::BadRequest(crate::error::BadRequestKind::Other(format!(
            "invalid recurrence: {e}"
        )))
    })
}

/// Parse the timezone stored in settings. A corrupt stored timezone is a
/// server-side data error, so it is reported as Internal.
pub(crate) fn parse_settings_timezone(tz: &str) -> Result<jiff::tz::TimeZone, AppError> {
    takusu_types::parse_timezone(tz).map_err(AppError::Internal)
}

// ── SettingsPlannerExt ─────────────────────────────────────────────────

/// Extension trait that ties `parse_sleep` / `parse_workload` to `SettingsRow`
/// so they are discovered via method calls rather than standalone functions
/// (#1255).
pub trait SettingsPlannerExt {
    /// #459: 設定から WorkloadConfig を構築する。`None` または `0` の場合はデフォルトを使う。
    /// 1 スロット = 5 分なので、`Minutes` からスロット数に変換する。
    fn workload_config(&self) -> WorkloadConfig;

    /// Parse a [`SleepInput`] into a `SleepConfig` using the settings' sleep
    /// window and the supplied timezone.
    fn sleep_config(
        &self,
        input: &SleepInput,
        tz: &jiff::tz::TimeZone,
    ) -> Result<SleepConfig, AppError>;
}

impl SettingsPlannerExt for SettingsRow {
    fn workload_config(&self) -> WorkloadConfig {
        let comfortable = self.comfortable_minutes.filter(|&m| m > 0);
        let maximum = self.maximum_minutes.filter(|&m| m > 0);
        match (comfortable, maximum) {
            (Some(c), Some(m)) => {
                let c_slots = Minutes(c).to_slots().0;
                let m_slots = Minutes(m).to_slots().0;
                if c_slots <= 0 || m_slots <= 0 {
                    return WorkloadConfig::default();
                }
                if c_slots > m_slots {
                    WorkloadConfig::new(m_slots, c_slots)
                } else {
                    WorkloadConfig::new(c_slots, m_slots)
                }
            }
            (Some(c), None) => {
                let c_slots = Minutes(c).to_slots().0;
                if c_slots <= 0 {
                    return WorkloadConfig::default();
                }
                let m_slots = (c_slots * 3 / 2).max(c_slots + 48);
                WorkloadConfig::new(c_slots, m_slots)
            }
            (None, Some(m)) => {
                let m_slots = Minutes(m).to_slots().0;
                if m_slots <= 0 {
                    return WorkloadConfig::default();
                }
                let c_slots = (m_slots * 2 / 3).min(m_slots - 24).max(1);
                WorkloadConfig::new(c_slots, m_slots)
            }
            (None, None) => WorkloadConfig::default(),
        }
    }

    fn sleep_config(
        &self,
        input: &SleepInput,
        tz: &jiff::tz::TimeZone,
    ) -> Result<SleepConfig, AppError> {
        match input {
            SleepInput::Recommended => {
                let (sh, sm) = (self.sleep_start.hour(), self.sleep_start.minute());
                let (eh, em) = (self.sleep_end.hour(), self.sleep_end.minute());
                Ok(SleepConfig::from_local(5, tz, sh, sm, eh, em))
            }
            SleepInput::Disabled => Ok(SleepConfig::disabled()),
            SleepInput::Custom { start, end } => {
                let (sh, sm) = (start.hour(), start.minute());
                let (eh, em) = (end.hour(), end.minute());
                Ok(SleepConfig::from_local(5, tz, sh, sm, eh, em))
            }
        }
    }
}

// ── tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── recurrence mirror vs. real type agreement (#1322) ─────────────
    //
    // `takusu_contracts::validate::validate_recurrence` validates JSON shape
    // using a lightweight mirror of `takusu_habit::RecurrenceRule` to avoid
    // pulling `takusu-habit` into the WASM bundle. `parse_recurrence` (this
    // crate) deserialises the real `takusu_habit::RecurrenceRule`. If the
    // mirror and the real type ever drift, a recurrence could pass boundary
    // validation but fail at sync/preview time. These tests assert that
    // every JSON string accepted by one side is accepted by the other, and
    // vice versa, for a representative set of inputs.

    #[test]
    fn recurrence_mirror_and_real_type_agree_on_valid() {
        let valid = [
            r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            r#"{"freq":"weekly","interval":2,"by_day":[{"n":null,"weekday":"mon"}],"by_month":[],"by_month_day":[],"count":10,"exdates":[]}"#,
            r#"{"freq":"monthly","interval":1,"by_day":[],"by_month":[],"by_month_day":[15],"count":null,"exdates":["2024-02-29"]}"#,
            r#"{"freq":"yearly","interval":3,"by_day":[],"by_month":[12],"by_month_day":[31],"count":null,"exdates":["2026-01-01","2026-07-04"]}"#,
        ];
        for json in &valid {
            let mirror_ok = takusu_contracts::validate::validate_recurrence(json).is_ok();
            let real_ok = parse_recurrence(json).is_ok();
            assert!(
                mirror_ok && real_ok,
                "mirror={mirror_ok} real={real_ok} for {json}"
            );
        }
    }

    #[test]
    fn recurrence_mirror_and_real_type_agree_on_invalid() {
        let invalid = [
            // Not JSON.
            "not json",
            // Missing required field.
            r#"{"freq":"daily"}"#,
            // Invalid freq.
            r#"{"freq":"hourly","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#,
            // Invalid exdate (not a date).
            r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":["notadate"]}"#,
            // Impossible calendar date.
            r#"{"freq":"daily","interval":1,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":["2026-02-30"]}"#,
        ];
        for json in &invalid {
            let mirror_err = takusu_contracts::validate::validate_recurrence(json).is_err();
            let real_err = parse_recurrence(json).is_err();
            assert!(
                mirror_err && real_err,
                "mirror should reject and real should reject, got mirror_err={mirror_err} real_err={real_err} for {json}"
            );
        }
    }
}

