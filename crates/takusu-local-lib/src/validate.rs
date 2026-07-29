//! Validation trait and impls for input structs.
//!
//! The `Validate` trait ties validation logic to the struct it validates so
//! the compiler can detect forgotten calls: `body.validate()?` instead of
//! remembering to call `validate_xxx(&body)?` at every entry point (#1255).
//!
//! Self-contained validators (minutes, title, recurrence, skill, memory,
//! steps, timezone, scheduled-span dates, task datetimes for create) are
//! expressed as `Validate` impls on the corresponding input structs.
//!
//! Context-dependent validators that need an existing row or a timezone
//! (e.g. `UpdateTask` datetime checks against an existing task) remain as
//! `pub(crate)` free functions and are called explicitly at the call site.
//!
//! `parse_sleep` / `parse_workload` have been moved to the
//! [`SettingsPlannerExt`] extension trait so they are tied to `SettingsRow`
//! rather than floating as standalone functions.

use takusu_core::{Minutes, SleepConfig, WorkloadConfig};
use takusu_storage::{
    CreateHabit, CreateHabitScheduledSpan, CreateMemory, CreateSkill, CreateTask,
    HabitPreviewRequest, HabitStepInput, SettingsRow, UpdateHabit, UpdateSettings, UpdateTask,
};
use takusu_util::{EnumLabel, SleepInput};

use crate::error::{AppError, BadRequestKind};

// ── helper free functions ─────────────────────────────────────────────

/// Reject negative or unrealistically large `avg_minutes` / `sigma_minutes`,
/// which would wrap to a huge `u64` slot count in the planner and break the
/// schedule (#269, #604).
pub(crate) fn validate_minutes(avg: i64, sigma: Option<i64>) -> Result<(), AppError> {
    // Roughly one year in minutes.  This keeps the converted slot count well
    // within the range where `duration_score`, `total_avg`, and timestamp
    // arithmetic cannot overflow, while still allowing long-running tasks.
    const MAX_MINUTES: i64 = 60 * 24 * 365;

    if avg < 0 {
        return Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "avg_minutes must be >= 0 (got {avg})"
        ))));
    }
    if avg > MAX_MINUTES {
        return Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "avg_minutes must be at most {MAX_MINUTES} (got {avg})"
        ))));
    }
    if let Some(s) = sigma
        && s < 0
    {
        return Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "sigma_minutes must be >= 0 (got {s})"
        ))));
    }
    if let Some(s) = sigma
        && s > MAX_MINUTES
    {
        return Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "sigma_minutes must be at most {MAX_MINUTES} (got {s})"
        ))));
    }
    Ok(())
}

/// Reject titles that cannot be NFKC-normalized for similar-task search (empty,
/// control-character only, or exceeding the normalized-title scalar limit).
/// Validating at the boundary keeps `normalized_title` always populated for
/// stored tasks, so a task is never silently excluded from similar-task search
/// (#942).
pub(crate) fn validate_title(title: &str) -> Result<(), AppError> {
    takusu_util::memory::normalize_text(title, Some(takusu_util::memory::MAX_CONTENT_SCALARS))
        .map_err(|e| AppError::BadRequest(BadRequestKind::Other(format!("invalid title: {e}"))))?;
    Ok(())
}

/// Parse a recurrence JSON string into a `RecurrenceRule`.
/// This is the single point inside `takusu-local-lib` where storage/client strings
/// become `takusu_habit` types (see `doc/type-safety-issues.md` §3.4 / §8.6).
/// `takusu-worker` validates recurrences separately without converting to `takusu_habit`.
pub(crate) fn parse_recurrence(recurrence: &str) -> Result<takusu_habit::RecurrenceRule, AppError> {
    serde_json::from_str::<takusu_habit::RecurrenceRule>(recurrence).map_err(|e| {
        AppError::BadRequest(BadRequestKind::Other(format!("invalid recurrence: {e}")))
    })
}

/// Verify the timezone string resolves to a real `jiff::tz::TimeZone` so that
/// typos don't silently fall back to UTC (#277). User-supplied timezones are
/// reported as BadRequest.
pub(crate) fn validate_timezone(tz: &str) -> Result<(), AppError> {
    takusu_util::parse_timezone(tz)
        .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))
        .map(|_| ())
}

/// Parse the timezone stored in settings. A corrupt stored timezone is a
/// server-side data error, so it is reported as Internal.
pub(crate) fn parse_settings_timezone(tz: &str) -> Result<jiff::tz::TimeZone, AppError> {
    takusu_util::parse_timezone(tz).map_err(AppError::Internal)
}

/// Validate `start_at` / `end_at` datetime values and that the effective
/// start is not after the effective end. Missing fields are filled from the
/// existing row for comparison when one side is being updated (#934). If an
/// existing value is needed for comparison but cannot be parsed, it is treated
/// as a data-corruption error rather than silently ignored.
///
/// `start_at`: `None` = no change → use existing; `Some(None)` = clear;
/// `Some(Some(ts))` = set.
/// `end_at`: `None` = no change → use existing; `Some(ts)` = set (cannot be
/// cleared).
pub(crate) fn validate_task_datetimes(
    start_at: Option<Option<&takusu_util::Timestamp>>,
    end_at: Option<&takusu_util::Timestamp>,
    existing_start: Option<&takusu_util::Timestamp>,
    existing_end: Option<&takusu_util::Timestamp>,
) -> Result<(), AppError> {
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
        return Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "start_at must be <= end_at ({s} > {e})"
        ))));
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
    fn validate(&self) -> Result<(), AppError>;
}

impl Validate for CreateSkill {
    fn validate(&self) -> Result<(), AppError> {
        const MAX_SLUG_LEN: usize = 64;
        const MAX_NAME_LEN: usize = 100;
        const MAX_DESC_LEN: usize = 500;
        const MAX_BODY_LEN: usize = 64 * 1024;

        if self.slug.is_empty() || self.slug.len() > MAX_SLUG_LEN {
            return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                "slug must be 1..{MAX_SLUG_LEN} characters"
            ))));
        }
        if self.slug.starts_with('.') || self.slug.contains('/') || self.slug.contains("..") {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "slug must not contain path components".into(),
            )));
        }
        if !self
            .slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "slug must contain only ASCII letters, digits, '-', '_'".into(),
            )));
        }
        if self.name.is_empty() || self.name.len() > MAX_NAME_LEN {
            return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                "name must be 1..{MAX_NAME_LEN} characters"
            ))));
        }
        if self.description.len() > MAX_DESC_LEN {
            return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                "description must be at most {MAX_DESC_LEN} characters"
            ))));
        }
        if self.body.is_empty() || self.body.len() > MAX_BODY_LEN {
            return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                "body must be 1..{MAX_BODY_LEN} characters"
            ))));
        }
        Ok(())
    }
}

impl Validate for CreateMemory {
    fn validate(&self) -> Result<(), AppError> {
        if !matches!(
            self.kind,
            takusu_util::MemoryKind::ProperNoun
                | takusu_util::MemoryKind::Fact
                | takusu_util::MemoryKind::TaskNote
        ) {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "kind must be 'proper_noun', 'fact', or 'task_note'".into(),
            )));
        }
        if takusu_util::memory::normalize_key(&self.key).is_err() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "invalid key".into(),
            )));
        }
        if takusu_util::memory::normalize_content(&self.content).is_err() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "invalid content".into(),
            )));
        }
        if self
            .subject_type
            .as_ref()
            .is_some_and(|s| s.as_str().len() > 64)
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "subject_type too long".into(),
            )));
        }
        if self.subject_id.as_ref().is_some_and(|s| s.len() > 64) {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "subject_id too long".into(),
            )));
        }
        if self.kind == takusu_util::MemoryKind::TaskNote {
            if self.subject_type != Some(takusu_util::SubjectType::Task) {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "task_note requires subject_type='task'".into(),
                )));
            }
            if self.subject_id.as_ref().is_none_or(|s| s.is_empty()) {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "task_note requires subject_id".into(),
                )));
            }
        }
        Ok(())
    }
}

impl Validate for CreateTask {
    fn validate(&self) -> Result<(), AppError> {
        validate_minutes(self.avg_minutes, self.sigma_minutes)?;
        validate_title(&self.title)?;
        // For create/replace there is no existing row to compare against.
        validate_task_datetimes(Some(self.start_at.as_ref()), Some(&self.end_at), None, None)?;
        Ok(())
    }
}

impl Validate for UpdateTask {
    fn validate(&self) -> Result<(), AppError> {
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

impl Validate for CreateHabit {
    fn validate(&self) -> Result<(), AppError> {
        validate_minutes(self.avg_minutes, self.sigma_minutes)?;
        parse_recurrence(&self.recurrence).map(|_| ())?;
        Ok(())
    }
}

impl Validate for UpdateHabit {
    fn validate(&self) -> Result<(), AppError> {
        if let Some(avg) = self.avg_minutes {
            validate_minutes(avg, self.sigma_minutes)?;
        } else if let Some(sigma) = self.sigma_minutes {
            validate_minutes(0, Some(sigma))?;
        }
        if let Some(recurrence) = &self.recurrence {
            parse_recurrence(recurrence).map(|_| ())?;
        }
        Ok(())
    }
}

impl Validate for CreateHabitScheduledSpan {
    fn validate(&self) -> Result<(), AppError> {
        crate::date_utils::validate_scheduled_span_dates(&self.start_date, &self.end_date)
            .map_err(|msg| AppError::BadRequest(BadRequestKind::Other(msg)))
    }
}

impl Validate for UpdateSettings {
    fn validate(&self) -> Result<(), AppError> {
        if let Some(tz) = &self.tz {
            validate_timezone(tz)?;
        }
        // sleep_start/sleep_end are Option<TimeOfDay>, already validated by deserialization.
        Ok(())
    }
}

/// Validate a bulk-replace step array (#95): per-field sanity + DAG integrity
/// (intra-habit references, cycle detection). Mirrors the worker-side
/// `validate_steps`.
impl Validate for [HabitStepInput] {
    fn validate(&self) -> Result<(), AppError> {
        use std::collections::HashMap;

        for s in self {
            validate_minutes(s.avg_minutes, s.sigma_minutes)?;
            // start_time/end_time are TimeOfDay, already validated by deserialization.
        }

        // Build id → index map for steps that carry an id. A depends_on reference
        // must point at a sibling step with a known id.
        let mut id_to_idx: HashMap<String, usize> = HashMap::new();
        for (i, s) in self.iter().enumerate() {
            if let Some(ref id) = s.id {
                id_to_idx.insert(id.clone(), i);
            }
        }

        let mut adj = vec![Vec::new(); self.len()];
        for (i, s) in self.iter().enumerate() {
            for dep in &s.depends_on {
                let Some(&dep_idx) = id_to_idx.get(dep) else {
                    return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                        "step depends_on references unknown step id: {dep}"
                    ))));
                };
                adj[i].push(dep_idx);
            }
        }

        crate::graph::detect_cycle(&adj)
            .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;
        Ok(())
    }
}

impl Validate for HabitPreviewRequest {
    fn validate(&self) -> Result<(), AppError> {
        validate_minutes(self.avg_minutes, self.sigma_minutes)?;
        parse_recurrence(&self.recurrence).map(|_| ())?;
        self.steps.validate()?;
        Ok(())
    }
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
