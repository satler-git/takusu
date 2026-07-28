use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use takusu_core::{
    Minutes, NormalDist, ParallelMode, Planner, Point, RescheduleRange, SleepConfig, Slots,
    Task as CoreTask, TaskPlacement, WorkloadConfig,
};
use takusu_storage::{
    CreateHabit, CreateHabitBatch, CreateHabitBatchResult, CreateHabitScheduledSpan, CreateMemory,
    CreateSkill, CreateTask, CreateTaskBatch, CreateTaskBatchItem, CreateTaskBatchResult,
    GoogleCalEventRow, GoogleCalSettingsRow, HabitDetail, HabitEstimateRequest,
    HabitEstimateResult, HabitEstimateSample, HabitEstimateStep, HabitPreviewRequest,
    HabitPreviewTask, HabitRow, HabitScheduledSpanRow, HabitStepEstimateInput, HabitStepInput,
    HabitStepRow, MemoryQuery, MemoryRow, ProgressResult, RecordProgress, SaveScheduleRequest,
    ScheduleEntry, ScheduleRow, SettingsRow, SimilarTaskQuery, SimilarTaskRow, SkillRow,
    SplitResult, SplitTask, Storage, TaskProgress, TaskQuery, TaskRow, TokenCreateResponse,
    TokenRow, UpdateGoogleCalSettings, UpdateHabit, UpdateMemory, UpdateSettings, UpdateSkill,
    UpdateTask,
};
use takusu_util::search::{Completion, complete};
use takusu_util::{Abandonability, EnumLabel, MemoryKind, TaskStatus, WindowMode};

use crate::error::AppError;
use crate::error::storage_to_app;
use crate::token_cache::TokenCache;
use takusu_util::parse_timezone;

fn parse_hhmm(s: &str) -> Result<(u8, u8), AppError> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(AppError::BadRequest(format!("invalid time: {s}")));
    }
    let h: u8 = parts[0]
        .parse()
        .map_err(|_| AppError::BadRequest(format!("invalid time: {s}")))?;
    let m: u8 = parts[1]
        .parse()
        .map_err(|_| AppError::BadRequest(format!("invalid time: {s}")))?;
    if h > 23 || m > 59 {
        return Err(AppError::BadRequest(format!("invalid time: {s}")));
    }
    Ok((h, m))
}

/// Reject negative or unrealistically large `avg_minutes` / `sigma_minutes`,
/// which would wrap to a huge `u64` slot count in the planner and break the
/// schedule (#269, #604).
fn validate_minutes(avg: i64, sigma: Option<i64>) -> Result<(), AppError> {
    // Roughly one year in minutes.  This keeps the converted slot count well
    // within the range where `duration_score`, `total_avg`, and timestamp
    // arithmetic cannot overflow, while still allowing long-running tasks.
    const MAX_MINUTES: i64 = 60 * 24 * 365;

    if avg < 0 {
        return Err(AppError::BadRequest(format!(
            "avg_minutes must be >= 0 (got {avg})"
        )));
    }
    if avg > MAX_MINUTES {
        return Err(AppError::BadRequest(format!(
            "avg_minutes must be at most {MAX_MINUTES} (got {avg})"
        )));
    }
    if let Some(s) = sigma
        && s < 0
    {
        return Err(AppError::BadRequest(format!(
            "sigma_minutes must be >= 0 (got {s})"
        )));
    }
    if let Some(s) = sigma
        && s > MAX_MINUTES
    {
        return Err(AppError::BadRequest(format!(
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
fn validate_title(title: &str) -> Result<(), AppError> {
    takusu_util::memory::normalize_text(title, Some(takusu_util::memory::MAX_CONTENT_SCALARS))
        .map_err(|e| AppError::BadRequest(format!("invalid title: {e}")))?;
    Ok(())
}

/// Parse a recurrence JSON string into a `RecurrenceRule`.
/// This is the single point inside `takusu-local-lib` where storage/client strings
/// become `takusu_habit` types (see `doc/type-safety-issues.md` §3.4 / §8.6).
/// `takusu-worker` validates recurrences separately without converting to `takusu_habit`.
fn parse_recurrence(recurrence: &str) -> Result<takusu_habit::RecurrenceRule, AppError> {
    serde_json::from_str::<takusu_habit::RecurrenceRule>(recurrence)
        .map_err(|e| AppError::BadRequest(format!("invalid recurrence: {e}")))
}

/// Verify the recurrence string parses as a `RecurrenceRule` so that bad JSON
/// is rejected at the API boundary instead of crashing later (#285).
fn validate_recurrence(recurrence: &str) -> Result<(), AppError> {
    parse_recurrence(recurrence).map(|_| ())
}

/// Validate a skill slug, name, description, and body (#WI-6).
fn validate_skill(create: &CreateSkill) -> Result<(), AppError> {
    const MAX_SLUG_LEN: usize = 64;
    const MAX_NAME_LEN: usize = 100;
    const MAX_DESC_LEN: usize = 500;
    const MAX_BODY_LEN: usize = 64 * 1024;

    if create.slug.is_empty() || create.slug.len() > MAX_SLUG_LEN {
        return Err(AppError::BadRequest(format!(
            "slug must be 1..{MAX_SLUG_LEN} characters"
        )));
    }
    if create.slug.starts_with('.') || create.slug.contains('/') || create.slug.contains("..") {
        return Err(AppError::BadRequest(
            "slug must not contain path components".into(),
        ));
    }
    if !create
        .slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AppError::BadRequest(
            "slug must contain only ASCII letters, digits, '-', '_'".into(),
        ));
    }
    if create.name.is_empty() || create.name.len() > MAX_NAME_LEN {
        return Err(AppError::BadRequest(format!(
            "name must be 1..{MAX_NAME_LEN} characters"
        )));
    }
    if create.description.len() > MAX_DESC_LEN {
        return Err(AppError::BadRequest(format!(
            "description must be at most {MAX_DESC_LEN} characters"
        )));
    }
    if create.body.is_empty() || create.body.len() > MAX_BODY_LEN {
        return Err(AppError::BadRequest(format!(
            "body must be 1..{MAX_BODY_LEN} characters"
        )));
    }
    Ok(())
}

/// Validate a memory create request (#WI-7).
fn validate_memory(create: &CreateMemory) -> Result<(), AppError> {
    if !matches!(
        create.kind,
        takusu_util::MemoryKind::ProperNoun
            | takusu_util::MemoryKind::Fact
            | takusu_util::MemoryKind::TaskNote
    ) {
        return Err(AppError::BadRequest(
            "kind must be 'proper_noun', 'fact', or 'task_note'".into(),
        ));
    }
    if takusu_util::memory::normalize_key(&create.key).is_err() {
        return Err(AppError::BadRequest("invalid key".into()));
    }
    if takusu_util::memory::normalize_content(&create.content).is_err() {
        return Err(AppError::BadRequest("invalid content".into()));
    }
    if create
        .subject_type
        .as_ref()
        .is_some_and(|s| s.as_str().len() > 64)
    {
        return Err(AppError::BadRequest("subject_type too long".into()));
    }
    if create.subject_id.as_ref().is_some_and(|s| s.len() > 64) {
        return Err(AppError::BadRequest("subject_id too long".into()));
    }
    if create.kind == takusu_util::MemoryKind::TaskNote {
        if create.subject_type != Some(takusu_util::SubjectType::Task) {
            return Err(AppError::BadRequest(
                "task_note requires subject_type='task'".into(),
            ));
        }
        if create.subject_id.as_ref().is_none_or(|s| s.is_empty()) {
            return Err(AppError::BadRequest("task_note requires subject_id".into()));
        }
    }
    Ok(())
}

/// Validate a `HH:MM` time string (#95).
#[allow(dead_code)]
fn validate_hhmm(s: &str) -> Result<(), AppError> {
    parse_hhmm(s).map(|_| ())
}

/// Validate a bulk-replace step array (#95): per-field sanity + DAG integrity
/// (intra-habit references, cycle detection). Mirrors the worker-side
/// `validate_steps`.
fn validate_steps(steps: &[HabitStepInput]) -> Result<(), AppError> {
    use std::collections::HashMap;

    for s in steps {
        validate_minutes(s.avg_minutes, s.sigma_minutes)?;
        // start_time/end_time are TimeOfDay, already validated by deserialization.
    }

    // Build id → index map for steps that carry an id. A depends_on reference
    // must point at a sibling step with a known id.
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, s) in steps.iter().enumerate() {
        if let Some(ref id) = s.id {
            id_to_idx.insert(id.clone(), i);
        }
    }

    let mut adj = vec![Vec::new(); steps.len()];
    for (i, s) in steps.iter().enumerate() {
        for dep in &s.depends_on {
            let Some(&dep_idx) = id_to_idx.get(dep) else {
                return Err(AppError::BadRequest(format!(
                    "step depends_on references unknown step id: {dep}"
                )));
            };
            adj[i].push(dep_idx);
        }
    }

    crate::graph::detect_cycle(&adj)
        .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;
    Ok(())
}

/// Topologically sort habit steps by their `depends_on` DAG (#95). Steps with
/// no dependencies come first. Returns indices into `steps`. Cycles are
/// rejected (defensive — validation already caught them at replace time).
fn topo_sort_steps(steps: &[HabitStepRow]) -> Result<Vec<usize>, AppError> {
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, s) in steps.iter().enumerate() {
        id_to_idx.insert(s.id.clone(), i);
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for (i, s) in steps.iter().enumerate() {
        let deps: Vec<String> = serde_json::from_str(&s.depends_on).unwrap_or_default();
        for dep in &deps {
            if let Some(&dep_idx) = id_to_idx.get(dep) {
                // edge dep_idx → i (dep must come before i)
                adj[dep_idx].push(i);
            }
        }
    }
    crate::graph::topo_sort(&adj)
        .map_err(|_| AppError::BadRequest("habit steps に循環依存が検出されました".into()))
}

/// Verify the timezone string resolves to a real `jiff::tz::TimeZone` so that
/// typos don't silently fall back to UTC (#277). User-supplied timezones are
/// reported as BadRequest.
fn validate_timezone(tz: &str) -> Result<(), AppError> {
    parse_timezone(tz).map_err(AppError::BadRequest).map(|_| ())
}

/// Parse the timezone stored in settings. A corrupt stored timezone is a
/// server-side data error, so it is reported as Internal.
fn parse_settings_timezone(tz: &str) -> Result<jiff::tz::TimeZone, AppError> {
    parse_timezone(tz).map_err(AppError::Internal)
}

/// Validate `start_at` / `end_at` datetime strings and that the effective
/// start is not after the effective end. Missing fields are filled from the
/// existing row for comparison when one side is being updated (#934). If an
/// existing value is needed for comparison but cannot be parsed, it is treated
/// as a data-corruption error rather than silently ignored.
fn validate_task_datetimes(
    start_at: Option<Option<&takusu_util::Timestamp>>,
    end_at: Option<&takusu_util::Timestamp>,
    _tz: &jiff::tz::TimeZone,
    existing_start: Option<&takusu_util::Timestamp>,
    existing_end: Option<&takusu_util::Timestamp>,
) -> Result<(), AppError> {
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
        return Err(AppError::BadRequest(format!(
            "start_at must be <= end_at ({s} > {e})"
        )));
    }
    Ok(())
}

/// Build a `CoreTask` for a single step occurrence (#95). The step's window is
/// derived from the occurrence date (taken from `occ_start`) combined with the
/// step's `start_time`/`end_time`. For fixed steps the deadline is the window
/// length (end_time - start_time); otherwise it is `avg_minutes`.
fn step_to_core_task(
    step: &HabitStepRow,
    occ_start: Point,
    tz: &jiff::tz::TimeZone,
) -> Result<CoreTask, AppError> {
    let date = takusu_habit::point_to_date(occ_start, tz)
        .ok_or_else(|| AppError::Internal("occurrence date out of range".into()))?;
    let (sh, sm) = (step.start_time.hour(), step.start_time.minute());
    let start_time = takusu_habit::TimeOfDay::new(sh, sm).ok_or_else(|| {
        AppError::BadRequest(format!("invalid step start_time: {}", step.start_time))
    })?;
    let start_pt = takusu_habit::date_time_to_point(date, &start_time, tz)
        .ok_or_else(|| AppError::Internal("step start point out of range".into()))?;
    let (eh, em) = (step.end_time.hour(), step.end_time.minute());
    let start_minutes = sh as i64 * 60 + sm as i64;
    let end_minutes = eh as i64 * 60 + em as i64;
    let end_pt = if step.fixed {
        let diff = end_minutes - start_minutes;
        if diff > 0 {
            start_pt + Minutes(diff).to_slots().0
        } else {
            // overnight fixed step — fall back to avg-based deadline
            start_pt + Minutes(step.avg_minutes).to_slots().0
        }
    } else {
        start_pt + Minutes(step.avg_minutes).to_slots().0
    };
    Ok(CoreTask {
        id: 0,
        start: Some(start_pt),
        end: end_pt,
        cost_estimate: NormalDist::from_minutes(
            Minutes(step.avg_minutes),
            Minutes(step.sigma_minutes),
        ),
        depends: vec![],
        parallel_mode: ParallelMode::from_bools(step.parallelizable, step.allows_parallel),
        abandonability: step.abandonability,
        fixed: step.fixed,
        habit_group: None,
    })
}

/// Build a `CoreTask` for a step occurrence in `period` window mode
/// (#window_mode). All steps of a period-mode habit share the same window
/// (`window_start`..`deadline`), so the step's own `start_time`/`end_time`
/// are ignored. The step's avg/sigma/flags still apply.
fn step_to_core_task_period(step: &HabitStepRow, window_start: Point, deadline: Point) -> CoreTask {
    CoreTask {
        id: 0,
        start: Some(window_start),
        end: deadline,
        cost_estimate: NormalDist::from_minutes(
            Minutes(step.avg_minutes),
            Minutes(step.sigma_minutes),
        ),
        depends: vec![],
        parallel_mode: ParallelMode::from_bools(step.parallelizable, step.allows_parallel),
        abandonability: step.abandonability,
        fixed: step.fixed,
        habit_group: None,
    }
}

fn step_input_to_preview_row(input: &HabitStepInput) -> HabitStepRow {
    HabitStepRow {
        id: input
            .id
            .clone()
            .unwrap_or_else(|| input.position.to_string()),
        habit_id: String::new(),
        position: input.position,
        title: input.title.clone(),
        description: input.description.clone(),
        start_time: input.start_time,
        end_time: input.end_time,
        avg_minutes: input.avg_minutes,
        sigma_minutes: input.sigma_minutes.unwrap_or(0),
        parallelizable: input.parallelizable.unwrap_or(false),
        allows_parallel: input.allows_parallel.unwrap_or(false),
        abandonability: input.abandonability.unwrap_or(0.5.into()),
        fixed: input.fixed.unwrap_or(false),
        depends_on: serde_json::to_string(&input.depends_on).unwrap_or_default(),
        created_at: takusu_util::Timestamp::default(),
    }
}

fn core_task_to_preview(core: &CoreTask, title: &str) -> HabitPreviewTask {
    HabitPreviewTask {
        title: title.to_string(),
        start_at: point_to_iso(core.start.unwrap_or(Point(0)).0).unwrap_or_default(),
        end_at: point_to_iso(core.end.0).unwrap_or_default(),
    }
}

/// Fallback deadline (in slots) for the last occurrence of a period-mode
/// habit when there is no next occurrence to derive the deadline from
/// (e.g. count-limited rules). Returns an approximate interval duration
/// based on the recurrence frequency and interval (#window_mode).
fn freq_fallback_slots(rule: &takusu_habit::RecurrenceRule) -> i64 {
    let interval = rule.interval.max(1) as i64;
    let days = match rule.freq {
        takusu_habit::Frequency::Daily => interval,
        takusu_habit::Frequency::Weekly => interval * 7,
        takusu_habit::Frequency::Monthly => interval * 30,
        takusu_habit::Frequency::Yearly => interval * 365,
    };
    days * 288 // 288 slots per day (5-min slots)
}

/// Validate that `start` and `end` are real `YYYY-MM-DD` calendar dates and
/// that `start <= end` (#303).
fn validate_scheduled_span_dates(
    start: &takusu_util::Date,
    end: &takusu_util::Date,
) -> Result<(), AppError> {
    if start > end {
        return Err(AppError::BadRequest(format!(
            "start_date ({start}) must be <= end_date ({end})"
        )));
    }
    Ok(())
}

/// Parse a `YYYY-MM-DD` string into a `(year, month, day)` tuple if it is a
/// real calendar date, else `None`.
///
/// Enforces zero-padded fields (4-digit year, 2-digit month/day) so that
/// lexicographic comparison against `jiff`'s zero-padded `Date::to_string()`
/// works correctly during pause matching (#303).
#[allow(dead_code)]
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

fn parse_sleep(
    s: &str,
    settings: &SettingsRow,
    tz: &jiff::tz::TimeZone,
) -> Result<SleepConfig, AppError> {
    match s {
        "recommended" => {
            let (sh, sm) = (settings.sleep_start.hour(), settings.sleep_start.minute());
            let (eh, em) = (settings.sleep_end.hour(), settings.sleep_end.minute());
            Ok(SleepConfig::from_local(5, tz, sh, sm, eh, em))
        }
        "disabled" => Ok(SleepConfig::disabled()),
        custom => {
            let parts: Vec<&str> = custom.splitn(2, '-').collect();
            if parts.len() == 2 {
                let (sh, sm) = parse_hhmm(parts[0])?;
                let (eh, em) = parse_hhmm(parts[1])?;
                Ok(SleepConfig::from_local(5, tz, sh, sm, eh, em))
            } else {
                Ok(SleepConfig::disabled())
            }
        }
    }
}

/// #459: 設定から WorkloadConfig を構築する。`None` または `0` の場合はデフォルトを使う。
/// 1 スロット = 5 分なので、`Minutes` からスロット数に変換する。
fn parse_workload(settings: &SettingsRow) -> WorkloadConfig {
    let comfortable = settings.comfortable_minutes.filter(|&m| m > 0);
    let maximum = settings.maximum_minutes.filter(|&m| m > 0);
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

/// #772: `Planner` に settings の solver / time budget / seed / warm start を反映する。
fn apply_planner_settings(planner: &mut Planner, settings: &SettingsRow) {
    planner.set_workload(parse_workload(settings));
    planner.set_solver(settings.solver.into());
    planner.set_time_budget(
        settings
            .time_budget_ms
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms as u64)),
    );
    planner.set_seed(settings.seed.filter(|&s| s >= 0).map(|s| s as u64));
    planner.set_warm_start(settings.warm_start);
}

/// ISO文字列 → Point スロット値。`now` は現在時刻。
/// ハードコードされた 5 (分/スロット) は Planner の per と揃っている必要がある。
/// `.devin/docs/code-style.md` の「point_to_iso hardcoded 5-minute slots」参照。
/// 変更時は takusu-core, takusu-local-lib, google-cal など全 crate の
/// 5分前提コードを同時に更新すること。
///
/// `tz` はオフセット無しの naive な日時文字列を解釈する際のフォールバック
/// タイムゾーン。過去にモバイルアプリがオフセットを削除した文字列を保存して
/// しまった場合などに救済する。
fn iso_to_point(iso: &str, tz: &jiff::tz::TimeZone) -> Result<Point, AppError> {
    let ts = takusu_util::parse_datetime_to_timestamp(iso, tz)
        .map_err(|e| AppError::BadRequest(format!("invalid datetime: {e}")))?;
    Ok(Point::from_timestamp(ts, 5))
}

/// ISO 8601 日時文字列を絶対 UTC 表記に正規化する。
/// 明示的なオフセット/Z が付いていればそのまま UTC に変換し、
/// naive な文字列は設定タイムゾーンで解釈して絶対時刻にする。
#[allow(dead_code)]
fn normalize_iso_datetime(iso: &str, tz: &jiff::tz::TimeZone) -> Result<String, AppError> {
    let ts = takusu_util::parse_datetime_to_timestamp(iso, tz)
        .map_err(|e| AppError::BadRequest(format!("invalid datetime: {e}")))?;
    Ok(ts.to_string())
}

fn point_to_iso(slot: i64) -> Result<takusu_util::Timestamp, AppError> {
    let secs = slot
        .checked_mul(5 * 60)
        .ok_or_else(|| AppError::Internal("timestamp overflow".into()))?;
    takusu_util::Timestamp::from_second(secs)
        .ok_or_else(|| AppError::Internal("invalid timestamp".into()))
}

/// Point スロット値 → ローカルタイムゾーンの日付文字列 (YYYY-MM-DD)。
/// `point_to_iso` は UTC タイムスタンプを返すため、JST など UTC より東の
/// タイムゾーンで午前 0 時〜 9 时のタスクが前日として扱われてしまう。
/// `sync_habit_tasks` の日付キーはローカル日付で一貫させる必要がある。
fn point_to_local_date(slot: i64, tz: &jiff::tz::TimeZone) -> Result<String, AppError> {
    let secs = slot
        .checked_mul(5 * 60)
        .ok_or_else(|| AppError::Internal("timestamp overflow".into()))?;
    let ts = Timestamp::from_second(secs)
        .map_err(|e| AppError::Internal(format!("invalid timestamp: {e}")))?;
    Ok(ts.to_zoned(tz.clone()).date().to_string())
}

/// ISO 文字列 → ローカルタイムゾーンの日付文字列 (YYYY-MM-DD)。
/// `task.start_at` (UTC ISO 文字列) からローカル日付を得るために使う。
fn iso_to_local_date(iso: &str, tz: &jiff::tz::TimeZone) -> String {
    if let Ok(ts) = Timestamp::from_str(iso) {
        ts.to_zoned(tz.clone()).date().to_string()
    } else {
        // フォールバック: naive 日時は設定 tz で解釈してローカル日付を得る。
        // iso_to_point と同じアプローチ。純粋な日付文字列 (YYYY-MM-DD) など
        // DateTime::from_str でも失敗する場合は先頭 10 文字を返す。
        match jiff::civil::DateTime::from_str(iso) {
            Ok(dt) => dt
                .to_zoned(tz.clone())
                .map(|zdt| zdt.date().to_string())
                .unwrap_or_else(|_| iso.chars().take(10).collect()),
            Err(_) => iso.chars().take(10).collect(),
        }
    }
}

#[allow(clippy::type_complexity)]
fn build_dep_graph(
    tasks: &[TaskRow],
) -> Result<(Vec<Vec<usize>>, HashMap<String, usize>), AppError> {
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, t) in tasks.iter().enumerate() {
        id_to_idx.insert(t.id.clone(), i);
    }
    let mut adj = vec![Vec::new(); tasks.len()];
    for t in tasks {
        let idx = id_to_idx[&t.id];
        let deps: Vec<String> = serde_json::from_str(&t.depends).unwrap_or_default();
        for dep_id in &deps {
            if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                adj[idx].push(dep_idx);
            }
        }
    }
    Ok((adj, id_to_idx))
}

fn habit_row_to_config(
    row: &HabitRow,
    tz: &jiff::tz::TimeZone,
) -> Result<takusu_habit::Habit, AppError> {
    build_habit_core(
        &row.recurrence,
        row.start_time,
        row.end_time,
        row.avg_minutes,
        row.sigma_minutes,
        row.parallelizable,
        row.allows_parallel,
        row.abandonability,
        row.fixed,
        tz,
    )
}

fn build_habit_from_preview(
    request: &HabitPreviewRequest,
    tz: &jiff::tz::TimeZone,
) -> Result<takusu_habit::Habit, AppError> {
    build_habit_core(
        &request.recurrence,
        request.start_time,
        request.end_time,
        request.avg_minutes,
        request.sigma_minutes.unwrap_or(0),
        request.parallelizable.unwrap_or(false),
        request.allows_parallel.unwrap_or(false),
        request.abandonability.unwrap_or(0.5.into()),
        request.fixed.unwrap_or(false),
        tz,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_habit_core(
    recurrence: &str,
    start_time: takusu_util::TimeOfDay,
    end_time: takusu_util::TimeOfDay,
    avg_minutes: i64,
    sigma_minutes: i64,
    parallelizable: bool,
    allows_parallel: bool,
    abandonability: Abandonability,
    fixed: bool,
    tz: &jiff::tz::TimeZone,
) -> Result<takusu_habit::Habit, AppError> {
    let recurrence = parse_recurrence(recurrence)?;
    let (sh, sm) = (start_time.hour(), start_time.minute());
    let start_time = takusu_habit::TimeOfDay::new(sh, sm)
        .ok_or_else(|| AppError::BadRequest(format!("invalid start_time: {start_time}")))?;
    let duration = NormalDist::from_minutes(Minutes(avg_minutes), Minutes(sigma_minutes));
    let (eh, em) = (end_time.hour(), end_time.minute());
    let deadline_slots = if fixed {
        let start_minutes = sh as i64 * 60 + sm as i64;
        let end_minutes = eh as i64 * 60 + em as i64;
        let diff = end_minutes - start_minutes;
        if diff > 0 {
            Some(Minutes(diff).to_slots().0 as u64)
        } else {
            None
        }
    } else {
        None
    };
    Ok(takusu_habit::Habit {
        recurrence,
        start_time,
        tz: tz.clone(),
        duration,
        deadline_slots,
        parallelizable,
        allows_parallel,
        abandonability,
        fixed,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcalImportResult {
    pub imported: usize,
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GenerateScheduleInput {
    pub task_ids: Option<Vec<String>>,
    pub sleep: String,
}

#[derive(Debug, Clone)]
pub struct RescheduleInput {
    pub mode: String,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    pub pinned: Vec<String>,
    pub sleep: String,
}

#[derive(Debug, Clone)]
pub struct SchedulePreviewInput {
    pub mode: String,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    pub pinned: Vec<String>,
    pub sleep: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchedulePreviewOutput {
    pub entries: Vec<ScheduleEntry>,
    pub unscheduled_task_ids: Vec<String>,
    pub displaced_task_ids: Vec<String>,
    pub sleep_minutes_before: i64,
    pub sleep_minutes_after: i64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MoveEntryOutput {
    pub task_id: String,
    pub start_at: String,
    pub end_at: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoogleCalSettingsOutput {
    pub enabled: bool,
    pub calendar_id: String,
    pub client_id: String,
    pub has_client_secret: bool,
    pub has_refresh_token: bool,
}

/// A node on a dependency witness path (task or habit step) (#355).
#[derive(Debug, Clone, Serialize)]
pub struct DependencyNode {
    pub id: String,
    pub title: String,
}

/// A redundant (composite / transitively implied) dependency edge with a
/// witness path proving the direct edge is unnecessary (#355).
#[derive(Debug, Clone, Serialize)]
pub struct RedundantDependency {
    pub from: String,
    pub from_title: String,
    pub to: String,
    pub to_title: String,
    /// Witness path `from → … → to` (endpoints included, length >= 3).
    pub via: Vec<DependencyNode>,
}

fn default_settings_row() -> SettingsRow {
    SettingsRow {
        id: "active".to_string(),
        tz: "UTC".to_string(),
        sleep_start: takusu_util::TimeOfDay::new(22, 0).unwrap(),
        sleep_end: takusu_util::TimeOfDay::new(6, 0).unwrap(),
        comfortable_minutes: None,
        maximum_minutes: None,
        solver: takusu_util::Solver::Sa,
        time_budget_ms: None,
        seed: None,
        warm_start: false,
        created_at: takusu_util::Timestamp::default(),
        updated_at: takusu_util::Timestamp::default(),
    }
}

pub struct TakusuApp {
    pub storage: Arc<dyn Storage>,
    pub token_cache: Arc<TokenCache>,
}

/// Result of explicitly deleting every mapped Google Calendar event.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteAllGcalResult {
    pub deleted: usize,
    pub failed: Vec<DeleteAllGcalFailure>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteAllGcalFailure {
    pub task_id: String,
    pub error: String,
}

impl TakusuApp {
    pub fn new(storage: Arc<dyn Storage>, token_cache: Arc<TokenCache>) -> Self {
        Self {
            storage,
            token_cache,
        }
    }

    pub async fn update_workers_credentials(&self, url: &str, token: &str) -> Result<(), AppError> {
        self.storage
            .update_workers_credentials(url, token)
            .await
            .map_err(storage_to_app)
    }

    // ── Settings ──────────────────────────────────────────

    async fn get_settings_or_default(&self) -> Result<SettingsRow, AppError> {
        self.storage
            .get_settings()
            .await
            .map_err(storage_to_app)
            .or_else(|e| {
                if matches!(e, AppError::NotFound(_)) {
                    Ok(default_settings_row())
                } else {
                    Err(e)
                }
            })
    }

    pub async fn get_settings(&self) -> Result<SettingsRow, AppError> {
        self.storage.get_settings().await.map_err(storage_to_app)
    }

    pub async fn update_settings(&self, body: &UpdateSettings) -> Result<SettingsRow, AppError> {
        if let Some(tz) = &body.tz {
            validate_timezone(tz)?;
        }
        // sleep_start/sleep_end are Option<TimeOfDay>, already validated by deserialization.
        self.storage
            .update_settings(body)
            .await
            .map_err(storage_to_app)
    }

    // ── Skills ────────────────────────────────────────────

    pub async fn create_skill(&self, body: &CreateSkill) -> Result<SkillRow, AppError> {
        validate_skill(body)?;
        if let Ok(existing) = self.storage.get_skill(&body.slug).await {
            if existing.built_in {
                return Err(AppError::Conflict {
                    message: format!("built-in skill {} cannot be overwritten", body.slug),
                });
            }
            return Err(AppError::Conflict {
                message: format!("skill {} already exists", body.slug),
            });
        }
        self.storage
            .create_skill(body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn list_skills(&self) -> Result<Vec<SkillRow>, AppError> {
        self.storage.list_skills().await.map_err(storage_to_app)
    }

    pub async fn get_skill(&self, slug: &str) -> Result<SkillRow, AppError> {
        self.storage.get_skill(slug).await.map_err(storage_to_app)
    }

    pub async fn update_skill(&self, slug: &str, body: &UpdateSkill) -> Result<SkillRow, AppError> {
        let existing = self.storage.get_skill(slug).await.map_err(storage_to_app)?;
        if existing.built_in {
            return Err(AppError::Conflict {
                message: format!("built-in skill {slug} cannot be edited"),
            });
        }
        if body
            .name
            .as_ref()
            .is_some_and(|n| n.is_empty() || n.len() > 100)
        {
            return Err(AppError::BadRequest(
                "name must be 1..100 characters".into(),
            ));
        }
        if body.description.as_ref().is_some_and(|d| d.len() > 500) {
            return Err(AppError::BadRequest(
                "description must be at most 500 characters".into(),
            ));
        }
        if body
            .body
            .as_ref()
            .is_some_and(|b| b.is_empty() || b.len() > 64 * 1024)
        {
            return Err(AppError::BadRequest("body length is invalid".into()));
        }
        self.storage
            .update_skill(slug, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_skill(&self, slug: &str) -> Result<(), AppError> {
        let existing = self.storage.get_skill(slug).await.map_err(storage_to_app)?;
        if existing.built_in {
            return Err(AppError::Conflict {
                message: format!("built-in skill {slug} cannot be deleted"),
            });
        }
        self.storage
            .delete_skill(slug)
            .await
            .map_err(storage_to_app)
    }

    // ── Memory (#WI-7) ────────────────────────────────────

    pub async fn create_memory(
        &self,
        body: &CreateMemory,
        operation_id: Option<&str>,
    ) -> Result<MemoryRow, AppError> {
        validate_memory(body)?;
        let mut body = body.clone();
        if body.kind == MemoryKind::TaskNote {
            let task_id = body.subject_id.as_deref().unwrap_or("");
            let task = self
                .storage
                .get_task(task_id)
                .await
                .map_err(storage_to_app)?;
            body.subject_id = Some(task.id);
        }
        self.storage
            .create_memory(&body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_memory(&self, id: &str) -> Result<MemoryRow, AppError> {
        self.storage.get_memory(id).await.map_err(storage_to_app)
    }

    pub async fn update_memory(
        &self,
        id: &str,
        body: &UpdateMemory,
        operation_id: Option<&str>,
    ) -> Result<MemoryRow, AppError> {
        if body.content.as_ref().is_none_or(|c| c.is_empty()) {
            return Err(AppError::BadRequest("content is required".into()));
        }
        if takusu_util::memory::normalize_content(body.content.as_deref().unwrap_or("")).is_err() {
            return Err(AppError::BadRequest("invalid content".into()));
        }
        self.storage
            .update_memory(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_memory(
        &self,
        id: &str,
        observed_revision: i64,
        operation_id: Option<&str>,
    ) -> Result<(), AppError> {
        self.storage
            .delete_memory(id, observed_revision, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn search_memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryRow>, AppError> {
        if takusu_util::memory::normalize_query(&query.q).is_err() {
            return Err(AppError::BadRequest("invalid query".into()));
        }
        self.storage
            .search_memories(query)
            .await
            .map_err(storage_to_app)
    }

    pub async fn find_similar_tasks(
        &self,
        query: &SimilarTaskQuery,
    ) -> Result<Vec<SimilarTaskRow>, AppError> {
        if takusu_util::memory::normalize_text(
            &query.title,
            Some(takusu_util::memory::MAX_QUERY_SCALARS),
        )
        .is_err()
        {
            return Err(AppError::BadRequest("invalid title".into()));
        }
        self.storage
            .find_similar_tasks(query)
            .await
            .map_err(storage_to_app)
    }

    // ── Tasks ─────────────────────────────────────────────

    pub async fn create_task(&self, body: &CreateTask) -> Result<TaskRow, AppError> {
        validate_minutes(body.avg_minutes, body.sigma_minutes)?;
        validate_title(&body.title)?;
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        validate_task_datetimes(
            Some(body.start_at.as_ref()),
            Some(&body.end_at),
            &tz,
            None,
            None,
        )?;
        let mut body = body.clone();
        // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
        if let Some(ref dep_ids) = body.depends
            && !dep_ids.is_empty()
        {
            let tasks = self
                .storage
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(storage_to_app)?;
            let (_adj, id_to_idx) = build_dep_graph(&tasks)?;
            // Resolve display_id numbers / UUID prefixes to full UUIDs before
            // validating against the dep graph (which is keyed by UUID).
            let mut resolved = Vec::with_capacity(dep_ids.len());
            for did in dep_ids {
                let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                if !id_to_idx.contains_key(&full) {
                    return Err(AppError::BadRequest(format!(
                        "depends on unknown task: {did}"
                    )));
                }
                resolved.push(full);
            }
            body.depends = Some(resolved);
        }
        self.storage
            .create_task(&body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn create_task_batch(
        &self,
        body: &CreateTaskBatch,
    ) -> Result<Vec<CreateTaskBatchResult>, AppError> {
        if body.tasks.is_empty() {
            return Ok(Vec::new());
        }

        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;

        // Validate/normalize each item and build a map from client_id to local index.
        let mut items: Vec<CreateTaskBatchItem> = Vec::with_capacity(body.tasks.len());
        let mut client_to_local: HashMap<String, usize> = HashMap::new();
        for (i, item) in body.tasks.iter().enumerate() {
            validate_minutes(item.task.avg_minutes, item.task.sigma_minutes)?;
            validate_title(&item.task.title)?;
            validate_task_datetimes(
                Some(item.task.start_at.as_ref()),
                Some(&item.task.end_at),
                &tz,
                None,
                None,
            )?;
            if let Some(ref cid) = item.client_id {
                if cid.is_empty() {
                    return Err(AppError::BadRequest("client_id must not be empty".into()));
                }
                if client_to_local.insert(cid.clone(), i).is_some() {
                    return Err(AppError::BadRequest(format!("duplicate client_id: {cid}")));
                }
            }
            let task = item.task.clone();
            // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
            items.push(CreateTaskBatchItem {
                task,
                client_id: item.client_id.clone(),
            });
        }

        // Load existing tasks to detect cycles across old and new dependencies.
        let existing_tasks = self
            .storage
            .list_tasks(&TaskQuery::default())
            .await
            .map_err(storage_to_app)?;
        let (mut adj, existing_id_to_idx) = build_dep_graph(&existing_tasks)?;
        let existing_offset = existing_tasks.len();
        adj.resize(existing_offset + items.len(), Vec::new());

        // Resolve each item's depends to graph indices.
        let mut deps_by_local: Vec<Vec<usize>> = Vec::with_capacity(items.len());
        for (local_idx, item) in items.iter().enumerate() {
            let mut dep_indices = Vec::new();
            if let Some(ref dep_ids) = item.task.depends {
                for did in dep_ids {
                    if let Some(&dep_local) = client_to_local.get(did) {
                        dep_indices.push(existing_offset + dep_local);
                    } else {
                        let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                        let &idx = existing_id_to_idx.get(&full).ok_or_else(|| {
                            AppError::BadRequest(format!("depends on unknown task: {did}"))
                        })?;
                        dep_indices.push(idx);
                    }
                }
            }
            let global_idx = existing_offset + local_idx;
            adj[global_idx].extend(dep_indices.iter().copied());
            deps_by_local.push(dep_indices);
        }

        crate::graph::detect_cycle(&adj)
            .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;

        // Topologically sort and then reverse so dependencies are created
        // before dependents (the graph edges point from dependent to dependency).
        let mut order = crate::graph::topo_sort(&adj)
            .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;
        order.reverse();
        let mut created_ids: Vec<Option<String>> = vec![None; items.len()];
        let mut results: Vec<Option<CreateTaskBatchResult>> = vec![None; items.len()];
        for global_idx in order {
            if global_idx < existing_offset {
                continue;
            }
            let local_idx = global_idx - existing_offset;
            let item = &items[local_idx];
            let mut resolved_dep_ids = Vec::with_capacity(deps_by_local[local_idx].len());
            for &dep_global in &deps_by_local[local_idx] {
                if dep_global < existing_offset {
                    resolved_dep_ids.push(existing_tasks[dep_global].id.clone());
                } else {
                    let dep_local = dep_global - existing_offset;
                    let actual = created_ids[dep_local].as_ref().ok_or_else(|| {
                        AppError::Internal("dependency not created before dependent".into())
                    })?;
                    resolved_dep_ids.push(actual.clone());
                }
            }
            let mut task = item.task.clone();
            task.depends = Some(resolved_dep_ids).filter(|v| !v.is_empty());
            let row = self
                .storage
                .create_task(&task)
                .await
                .map_err(storage_to_app)?;
            created_ids[local_idx] = Some(row.id.clone());
            results[local_idx] = Some(CreateTaskBatchResult {
                client_id: item.client_id.clone(),
                task: row,
            });
        }

        results
            .into_iter()
            .map(|opt| opt.ok_or_else(|| AppError::Internal("missing batch task result".into())))
            .collect()
    }

    pub async fn list_tasks(&self, query: &TaskQuery) -> Result<Vec<TaskRow>, AppError> {
        self.storage.list_tasks(query).await.map_err(storage_to_app)
    }

    pub async fn complete_task_query(
        &self,
        input: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Completion>, AppError> {
        const DEFAULT_COMPLETION_LIMIT: usize = 30;
        const MAX_COMPLETION_LIMIT: usize = 100;

        let limit = match limit {
            None => DEFAULT_COMPLETION_LIMIT,
            Some(0) => {
                return Err(AppError::BadRequest(
                    "completion limit must be at least 1".to_string(),
                ));
            }
            Some(n) if n > MAX_COMPLETION_LIMIT => {
                return Err(AppError::BadRequest(format!(
                    "completion limit must be at most {MAX_COMPLETION_LIMIT}"
                )));
            }
            Some(n) => n,
        };

        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let today = Timestamp::now().to_zoned(tz).date();

        let tasks = self
            .storage
            .list_tasks(&TaskQuery {
                limit: Some(200),
                ..TaskQuery::default()
            })
            .await
            .map_err(storage_to_app)?;
        let habits = self.storage.list_habits().await.map_err(storage_to_app)?;

        Ok(complete(input, today, &tasks, &habits, Some(limit)))
    }

    pub async fn get_task(&self, id: &str) -> Result<TaskRow, AppError> {
        self.storage.get_task(id).await.map_err(storage_to_app)
    }

    pub async fn update_task(&self, id: &str, body: &UpdateTask) -> Result<TaskRow, AppError> {
        // Validate minutes if provided. avg_minutes is required to be present
        // only when it is actually set in the update body.
        if let Some(avg) = body.avg_minutes {
            validate_minutes(avg, body.sigma_minutes)?;
        } else if let Some(sigma) = body.sigma_minutes {
            validate_minutes(0, Some(sigma))?;
        }
        if let Some(ref t) = body.title {
            validate_title(t)?;
        }
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let mut body = body.clone();

        // Fetch the existing task once if any downstream logic needs it.
        let needs_existing = body.start_at.is_some()
            || body.end_at.is_some()
            || body.depends.is_some()
            || body.user_edited.is_none();
        let existing = if needs_existing {
            Some(self.storage.get_task(id).await.map_err(storage_to_app)?)
        } else {
            None
        };

        // Validate datetime fields and their logical ordering (#934).
        if body.start_at.is_some() || body.end_at.is_some() {
            let existing = existing.as_ref().unwrap();
            validate_task_datetimes(
                body.start_at.as_ref().map(|o| o.as_ref()),
                body.end_at.as_ref(),
                &tz,
                existing.start_at.as_ref(),
                Some(&existing.end_at),
            )?;
        }

        // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
        // start_at/end_at use Option<Option<Timestamp>>:
        //   None = no change, Some(None) = clear to NULL, Some(Some(ts)) = set.

        if let Some(dep_ids) = &body.depends {
            let tasks = self
                .storage
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(storage_to_app)?;
            let (mut adj, id_to_idx) = build_dep_graph(&tasks)?;
            let full_id = existing.as_ref().unwrap().id.clone();
            let target_idx = id_to_idx
                .get(&full_id)
                .ok_or_else(|| AppError::NotFound(format!("task {id} not found")))?;
            // Resolve display_id numbers / UUID prefixes to full UUIDs before
            // validating against the dep graph (which is keyed by UUID).
            let mut resolved = Vec::with_capacity(dep_ids.len());
            for did in dep_ids {
                let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                if !id_to_idx.contains_key(&full) {
                    return Err(AppError::BadRequest(format!(
                        "depends on unknown task: {did}"
                    )));
                }
                resolved.push(full);
            }
            adj[*target_idx] = resolved
                .iter()
                .filter_map(|did| id_to_idx.get(did).copied())
                .collect();
            crate::graph::detect_cycle(&adj)
                .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;
            body.depends = Some(resolved);
        }

        // User-edited flag: for habit-derived tasks, mark as user-edited when
        // habit-managed fields are touched by an HTTP request, unless the
        // caller explicitly set user_edited (e.g. "revert to habit" sets false).
        if body.user_edited.is_none() {
            let existing = existing.as_ref().unwrap();
            if existing.habit_id.is_some() {
                let touched = body.title.is_some()
                    || body.description.is_some()
                    || body.start_at.is_some()
                    || body.end_at.is_some()
                    || body.avg_minutes.is_some()
                    || body.sigma_minutes.is_some()
                    || body.parallelizable.is_some()
                    || body.allows_parallel.is_some()
                    || body.abandonability.is_some()
                    || body.fixed.is_some();
                if touched {
                    body.user_edited = Some(true);
                }
            }
        }

        self.storage
            .update_task(id, &body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn replace_task(&self, id: &str, body: &CreateTask) -> Result<TaskRow, AppError> {
        validate_minutes(body.avg_minutes, body.sigma_minutes)?;
        validate_title(&body.title)?;
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        validate_task_datetimes(
            Some(body.start_at.as_ref()),
            Some(&body.end_at),
            &tz,
            None,
            None,
        )?;
        let mut body = body.clone();
        // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
        if let Some(ref dep_ids) = body.depends
            && !dep_ids.is_empty()
        {
            let tasks = self
                .storage
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(storage_to_app)?;
            let (mut adj, id_to_idx) = build_dep_graph(&tasks)?;
            let full_id = self.storage.get_task(id).await.map_err(storage_to_app)?.id;
            let target_idx = id_to_idx
                .get(&full_id)
                .ok_or_else(|| AppError::NotFound(format!("task {id} not found")))?;
            // Resolve display_id numbers / UUID prefixes to full UUIDs before
            // validating against the dep graph (which is keyed by UUID).
            let mut resolved = Vec::with_capacity(dep_ids.len());
            for did in dep_ids {
                let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                if !id_to_idx.contains_key(&full) {
                    return Err(AppError::BadRequest(format!(
                        "depends on unknown task: {did}"
                    )));
                }
                resolved.push(full);
            }
            adj[*target_idx] = resolved
                .iter()
                .filter_map(|did| id_to_idx.get(did).copied())
                .collect();
            crate::graph::detect_cycle(&adj)
                .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;
            body.depends = Some(resolved);
            return self
                .storage
                .replace_task(id, &body)
                .await
                .map_err(storage_to_app);
        }
        self.storage
            .replace_task(id, &body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_task(&self, id: &str) -> Result<(), AppError> {
        self.storage.delete_task(id).await.map_err(storage_to_app)
    }

    pub async fn start_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, AppError> {
        self.storage
            .start_task_work(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn pause_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, AppError> {
        self.storage
            .pause_task_work(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn record_progress(
        &self,
        id: &str,
        body: &RecordProgress,
        operation_id: Option<&str>,
    ) -> Result<ProgressResult, AppError> {
        self.storage
            .record_progress(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn complete_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, AppError> {
        self.storage
            .complete_task_work(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_task_progress(&self, id: &str) -> Result<TaskProgress, AppError> {
        self.storage
            .get_task_progress(id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn split_task(
        &self,
        id: &str,
        body: &SplitTask,
        operation_id: Option<&str>,
    ) -> Result<SplitResult, AppError> {
        if body.end_at.is_some() {
            let settings = self.get_settings_or_default().await?;
            let tz = parse_settings_timezone(&settings.tz)?;
            let original = self.storage.get_task(id).await.map_err(storage_to_app)?;
            validate_task_datetimes(
                None,
                body.end_at.as_ref(),
                &tz,
                original.start_at.as_ref(),
                None,
            )?;
        }
        self.storage
            .split_task(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn import_ical(&self, ical_body: &str) -> Result<IcalImportResult, AppError> {
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let events = takusu_ical::parse_ical(ical_body, &tz)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        let mut imported = 0usize;
        let mut task_ids = Vec::new();
        for event in &events {
            if let Some(ref uid) = event.uid
                && self.task_exists_by_ical_uid(uid).await?
            {
                continue;
            }
            let start_at: takusu_util::Timestamp = event.start_at.into();
            let end_at: takusu_util::Timestamp = event.end_at.into();
            let avg_minutes = takusu_util::minutes_between_ts(start_at, end_at);
            validate_minutes(avg_minutes, Some(0))?;
            let task = self
                .storage
                .create_task(&CreateTask {
                    title: event.title.clone(),
                    description: event.description.clone(),
                    start_at: Some(start_at),
                    end_at,
                    avg_minutes,
                    sigma_minutes: Some(0),
                    depends: Some(vec![]),
                    parallelizable: Some(false),
                    allows_parallel: Some(false),
                    abandonability: Some(0.5.into()),
                    ical_uid: event.uid.clone(),
                    habit_id: None,
                    fixed: Some(true),
                    habit_step_id: None,
                    quantity_total: None,
                    quantity_done: None,
                    quantity_unit: None,
                    original_quantity_total: None,
                })
                .await
                .map_err(storage_to_app)?;
            imported += 1;
            task_ids.push(task.id);
        }
        Ok(IcalImportResult { imported, task_ids })
    }

    async fn task_exists_by_ical_uid(&self, uid: &str) -> Result<bool, AppError> {
        self.storage
            .task_exists_by_ical_uid(uid)
            .await
            .map_err(storage_to_app)
    }

    // ── Habits ────────────────────────────────────────────

    pub async fn create_habit(&self, body: &CreateHabit) -> Result<HabitRow, AppError> {
        validate_minutes(body.avg_minutes, body.sigma_minutes)?;
        validate_recurrence(&body.recurrence)?;

        self.storage
            .create_habit(body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn create_habit_batch(
        &self,
        body: &CreateHabitBatch,
    ) -> Result<Vec<CreateHabitBatchResult>, AppError> {
        if body.habits.is_empty() {
            return Ok(Vec::new());
        }
        let mut seen_client_ids: HashSet<String> = HashSet::new();
        for item in &body.habits {
            if let Some(ref cid) = item.client_id {
                if cid.is_empty() {
                    return Err(AppError::BadRequest("client_id must not be empty".into()));
                }
                if !seen_client_ids.insert(cid.clone()) {
                    return Err(AppError::BadRequest(format!("duplicate client_id: {cid}")));
                }
            }
        }
        let mut results = Vec::with_capacity(body.habits.len());
        for item in &body.habits {
            let row = self.create_habit(&item.habit).await?;
            results.push(CreateHabitBatchResult {
                client_id: item.client_id.clone(),
                habit: row,
            });
        }
        Ok(results)
    }

    /// Preview the tasks that would be generated from a habit definition
    /// without persisting it. Uses `takusu_habit` directly so the preview
    /// matches the server's task-generation logic.
    pub async fn preview_habit(
        &self,
        request: &HabitPreviewRequest,
    ) -> Result<Vec<HabitPreviewTask>, AppError> {
        validate_minutes(request.avg_minutes, request.sigma_minutes)?;
        validate_recurrence(&request.recurrence)?;
        validate_steps(&request.steps)?;

        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let now_ts = Timestamp::now();
        let start_of_today = now_ts
            .to_zoned(tz.clone())
            .start_of_day()
            .map_err(|e| AppError::Internal(format!("start_of_day: {e}")))?
            .timestamp();
        let from_default = Point::from_timestamp(start_of_today, 5);
        let from = request
            .from
            .as_ref()
            .map(|s| iso_to_point(s, &tz))
            .transpose()?
            .unwrap_or(from_default);
        let until = request
            .until
            .as_ref()
            .map(|s| iso_to_point(s, &tz))
            .transpose()?
            .unwrap_or(Point(from.0 + 30 * 288));
        let max_occurrences = request.max_occurrences.unwrap_or(20).max(1) as usize;

        let habit = build_habit_from_preview(request, &tz)?;
        let is_period = request.window_mode == Some(WindowMode::Period);

        let mut store = takusu_habit::HabitStore::new();
        store.add(habit);

        let mut occurrences: Vec<(Point, Point)> = Vec::new();
        if is_period {
            let rule = parse_recurrence(&request.recurrence)?;
            let until_lookahead = Point(until.0 + 365 * 288);
            let occs: Vec<(String, Point)> = store
                .generate(from, until_lookahead)
                .into_iter()
                .map(|gt| {
                    let sp = gt.task.start.unwrap_or(Point(0));
                    let date = point_to_local_date(sp.0, &tz)?;
                    Ok((date, sp))
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            for (i, (_, occ_start)) in occs.iter().enumerate() {
                if occ_start.0 >= until.0 {
                    break;
                }
                let deadline = if let Some((_, next_start)) = occs.get(i + 1) {
                    *next_start
                } else {
                    Point(occ_start.0 + freq_fallback_slots(&rule))
                };
                occurrences.push((*occ_start, deadline));
                if occurrences.len() >= max_occurrences {
                    break;
                }
            }
        } else {
            for gt in store.generate(from, until) {
                let sp = gt.task.start.unwrap_or(Point(0));
                occurrences.push((sp, gt.task.end));
                if occurrences.len() >= max_occurrences {
                    break;
                }
            }
        }

        let step_rows: Vec<HabitStepRow> = request
            .steps
            .iter()
            .map(step_input_to_preview_row)
            .collect();

        let mut tasks: Vec<HabitPreviewTask> = Vec::new();
        for (occ_start, deadline) in occurrences {
            if !step_rows.is_empty() {
                let order = topo_sort_steps(&step_rows)?;
                for &idx in &order {
                    let step = &step_rows[idx];
                    let core = if is_period {
                        step_to_core_task_period(step, occ_start, deadline)
                    } else {
                        step_to_core_task(step, occ_start, &tz)?
                    };
                    tasks.push(core_task_to_preview(&core, &step.title));
                }
            } else {
                let sigma = request.sigma_minutes.unwrap_or(0);
                let cost = NormalDist::from_minutes(Minutes(request.avg_minutes), Minutes(sigma));
                let core = CoreTask {
                    id: 0,
                    start: Some(occ_start),
                    end: deadline,
                    cost_estimate: cost,
                    depends: vec![],
                    parallel_mode: ParallelMode::from_bools(
                        request.parallelizable.unwrap_or(false),
                        request.allows_parallel.unwrap_or(false),
                    ),
                    abandonability: request.abandonability.unwrap_or(0.5.into()),
                    fixed: request.fixed.unwrap_or(false),
                    habit_group: None,
                };
                tasks.push(core_task_to_preview(&core, &request.title));
            }
        }

        Ok(tasks)
    }

    pub async fn list_habits(&self) -> Result<Vec<HabitRow>, AppError> {
        self.storage.list_habits().await.map_err(storage_to_app)
    }

    pub async fn get_habit(&self, id: &str) -> Result<HabitDetail, AppError> {
        let habit = self.storage.get_habit(id).await.map_err(storage_to_app)?;
        let steps = self
            .storage
            .list_habit_steps(id)
            .await
            .map_err(storage_to_app)?;
        Ok(HabitDetail { habit, steps })
    }

    /// Compute a habit's `avg_minutes` / `sigma_minutes` from the actual
    /// durations of completed, non-fixed tasks. Fixed habits and fixed tasks
    /// are ignored. Outliers are optionally detected and excluded using the
    /// median absolute deviation (MAD) when `request.detect_outliers` is true.
    ///
    /// For habits with steps, an estimate is computed per non-fixed step and
    /// persisted atomically via `Storage::apply_habit_estimate`. Fixed steps
    /// are left untouched and still included in the combined total. For habits
    /// without steps, the habit's own estimate is updated directly.
    pub async fn estimate_habit(
        &self,
        id: &str,
        request: &HabitEstimateRequest,
    ) -> Result<HabitEstimateResult, AppError> {
        let habit = self.storage.get_habit(id).await.map_err(storage_to_app)?;
        if habit.fixed {
            return Err(AppError::BadRequest(
                "cannot estimate fixed habit from actuals".into(),
            ));
        }

        let completed = self
            .storage
            .list_tasks(&TaskQuery {
                status: Some("completed".to_string()),
                habit_id: Some(id.to_string()),
                ..TaskQuery::default()
            })
            .await
            .map_err(storage_to_app)?;

        // Group actual minutes by habit_step_id. None means the task was
        // generated for the habit itself rather than a specific step.
        let mut by_step: std::collections::HashMap<Option<String>, Vec<(TaskRow, i64)>> =
            std::collections::HashMap::new();
        for t in completed {
            if t.fixed {
                continue;
            }
            let actual = match t.actual_minutes {
                Some(a) if a > 0 => a,
                _ => continue,
            };
            by_step
                .entry(t.habit_step_id.clone())
                .or_default()
                .push((t, actual));
        }

        // `list_habit_steps` is already ordered by position ASC, created_at ASC,
        // so iterate it directly to keep the response deterministic.
        let step_rows = self
            .storage
            .list_habit_steps(id)
            .await
            .map_err(storage_to_app)?;

        let mut step_inputs: Vec<HabitStepEstimateInput> = Vec::new();
        let mut steps: Vec<HabitEstimateStep> = Vec::new();
        let mut has_step_samples = false;
        let mut combined_avg: i128 = 0;
        let mut combined_sigma_sq: f64 = 0.0;

        // Per-step estimates for habits with steps. Fixed steps are included
        // in the combined total and in the response, but not in the update
        // input, so they are never touched by `apply_habit_estimate`.
        for step in &step_rows {
            let (effective_avg, effective_sigma, sample_count, excluded_count) = if step.fixed {
                (step.avg_minutes, step.sigma_minutes, 0, 0)
            } else {
                let entries = by_step.remove(&Some(step.id.clone())).unwrap_or_default();
                let minutes: Vec<i64> = entries.iter().map(|(_, m)| *m).collect();
                let (avg, sigma, excluded) = takusu_util::estimate_from_samples_with_outliers(
                    &minutes,
                    request.detect_outliers,
                );

                // If a step has no samples, keep its current estimate so the
                // combined total and the persisted values remain meaningful.
                let effective_avg = if minutes.is_empty() {
                    step.avg_minutes
                } else {
                    avg
                };
                let effective_sigma = if minutes.is_empty() {
                    step.sigma_minutes
                } else {
                    sigma
                };

                if !minutes.is_empty() {
                    has_step_samples = true;
                }

                step_inputs.push(HabitStepEstimateInput {
                    step_id: step.id.clone(),
                    avg_minutes: effective_avg,
                    sigma_minutes: effective_sigma,
                });

                (
                    effective_avg,
                    effective_sigma,
                    entries.len(),
                    excluded.len(),
                )
            };

            combined_avg += effective_avg as i128;
            combined_sigma_sq += (effective_sigma as f64).powi(2);

            steps.push(HabitEstimateStep {
                step_id: step.id.clone(),
                title: step.title.clone(),
                avg_minutes: effective_avg,
                sigma_minutes: effective_sigma,
                sample_count,
                excluded_count,
                applied: request.apply && !step.fixed && sample_count > 0,
            });
        }

        let overall_entries = by_step.remove(&None).unwrap_or_default();
        let overall_minutes: Vec<i64> = overall_entries.iter().map(|(_, m)| *m).collect();
        let (overall_avg, overall_sigma, overall_excluded) =
            takusu_util::estimate_from_samples_with_outliers(
                &overall_minutes,
                request.detect_outliers,
            );

        let overall_excluded_set: std::collections::HashSet<usize> =
            overall_excluded.iter().copied().collect();
        let overall_samples: Vec<HabitEstimateSample> = overall_entries
            .into_iter()
            .enumerate()
            .map(|(i, (t, actual))| HabitEstimateSample {
                task_id: t.id,
                title: t.title,
                actual_minutes: actual,
                excluded: overall_excluded_set.contains(&i),
            })
            .collect();

        // Habits with steps use the combined step total. Habits without steps
        // use the overall task estimate.
        let (final_avg, final_sigma) = if step_rows.is_empty() {
            (overall_avg, overall_sigma)
        } else {
            let max = takusu_util::MAX_ESTIMATE_MINUTES as i128;
            let min = takusu_util::MIN_ESTIMATE_MINUTES as i128;
            (
                combined_avg.clamp(min, max) as i64,
                (combined_sigma_sq.sqrt().round() as i128).clamp(min, max) as i64,
            )
        };

        let has_samples = has_step_samples || !overall_minutes.is_empty();
        let applied = request.apply && has_samples;
        let habit_row = if applied {
            self.storage
                .apply_habit_estimate(id, final_avg, final_sigma, &step_inputs)
                .await
                .map_err(storage_to_app)?;
            Some(self.storage.get_habit(id).await.map_err(storage_to_app)?)
        } else {
            None
        };

        let total_sample_count =
            steps.iter().map(|s| s.sample_count).sum::<usize>() + overall_samples.len();
        let total_excluded_count =
            steps.iter().map(|s| s.excluded_count).sum::<usize>() + overall_excluded.len();

        Ok(HabitEstimateResult {
            avg_minutes: final_avg,
            sigma_minutes: final_sigma,
            sample_count: total_sample_count,
            excluded_count: total_excluded_count,
            samples: overall_samples,
            steps,
            applied,
            habit: habit_row,
        })
    }

    pub async fn update_habit(&self, id: &str, body: &UpdateHabit) -> Result<HabitRow, AppError> {
        if let Some(avg) = body.avg_minutes {
            validate_minutes(avg, body.sigma_minutes)?;
        } else if let Some(sigma) = body.sigma_minutes {
            validate_minutes(0, Some(sigma))?;
        }
        if let Some(recurrence) = &body.recurrence {
            validate_recurrence(recurrence)?;
        }

        self.storage
            .update_habit(id, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn replace_habit(&self, id: &str, body: &CreateHabit) -> Result<HabitRow, AppError> {
        validate_minutes(body.avg_minutes, body.sigma_minutes)?;
        validate_recurrence(&body.recurrence)?;

        self.storage
            .replace_habit(id, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_habit(&self, id: &str) -> Result<(), AppError> {
        self.storage.delete_habit(id).await.map_err(storage_to_app)
    }

    // ── Habit scheduled spans (#303 / #503) ──────────────

    pub async fn list_habit_scheduled_spans(
        &self,
        id: &str,
    ) -> Result<Vec<HabitScheduledSpanRow>, AppError> {
        self.storage
            .list_habit_scheduled_spans(id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn list_all_habit_scheduled_spans(
        &self,
    ) -> Result<Vec<HabitScheduledSpanRow>, AppError> {
        self.storage
            .list_all_habit_scheduled_spans()
            .await
            .map_err(storage_to_app)
    }

    pub async fn create_habit_scheduled_span(
        &self,
        id: &str,
        body: &CreateHabitScheduledSpan,
    ) -> Result<HabitScheduledSpanRow, AppError> {
        validate_scheduled_span_dates(&body.start_date, &body.end_date)?;
        self.storage
            .create_habit_scheduled_span(id, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_habit_scheduled_span(
        &self,
        id: &str,
        span_id: &str,
    ) -> Result<(), AppError> {
        self.storage
            .delete_habit_scheduled_span(id, span_id)
            .await
            .map_err(storage_to_app)
    }

    // ── Habit steps (#95) ───────────────────────────────

    pub async fn list_habit_steps(&self, id: &str) -> Result<Vec<HabitStepRow>, AppError> {
        self.storage
            .list_habit_steps(id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn list_all_habit_steps(&self) -> Result<Vec<HabitStepRow>, AppError> {
        self.storage
            .list_all_habit_steps()
            .await
            .map_err(storage_to_app)
    }

    pub async fn replace_habit_steps(
        &self,
        id: &str,
        steps: &[HabitStepInput],
    ) -> Result<Vec<HabitStepRow>, AppError> {
        validate_steps(steps)?;
        self.storage
            .replace_habit_steps(id, steps)
            .await
            .map_err(storage_to_app)
    }

    // ── Dependency analysis (#355) ───────────────────────

    /// Detect redundant (composite) edges in the task dependency DAG.
    /// Only non-completed tasks are considered — cleaning up dependencies
    /// on already-completed tasks is pointless. `depends` references to
    /// non-existent task ids are silently ignored (defensive against
    /// legacy data).
    pub async fn analyze_task_dependencies(&self) -> Result<Vec<RedundantDependency>, AppError> {
        let tasks = self
            .storage
            .list_tasks(&TaskQuery::default())
            .await
            .map_err(storage_to_app)?;
        let active: Vec<&TaskRow> = tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Completed && t.status != TaskStatus::Skipped)
            .collect();
        let mut id_to_idx: HashMap<String, usize> = HashMap::new();
        for (i, t) in active.iter().enumerate() {
            id_to_idx.insert(t.id.clone(), i);
        }
        let mut adj = vec![Vec::new(); active.len()];
        for (i, t) in active.iter().enumerate() {
            let deps: Vec<String> = serde_json::from_str(&t.depends).unwrap_or_default();
            for dep_id in &deps {
                if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                    adj[i].push(dep_idx);
                }
            }
        }
        let redundant = crate::graph::find_redundant_edges(&adj)
            .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;
        let node = |idx: usize| DependencyNode {
            id: active[idx].id.clone(),
            title: active[idx].title.clone(),
        };
        Ok(redundant
            .into_iter()
            .map(|e| RedundantDependency {
                from: active[e.from].id.clone(),
                from_title: active[e.from].title.clone(),
                to: active[e.to].id.clone(),
                to_title: active[e.to].title.clone(),
                via: e.via.iter().map(|&i| node(i)).collect(),
            })
            .collect())
    }

    /// Detect redundant (composite) edges in a habit's step dependency DAG.
    pub async fn analyze_habit_step_dependencies(
        &self,
        habit_id: &str,
    ) -> Result<Vec<RedundantDependency>, AppError> {
        let steps = self
            .storage
            .list_habit_steps(habit_id)
            .await
            .map_err(storage_to_app)?;
        let mut id_to_idx: HashMap<String, usize> = HashMap::new();
        for (i, s) in steps.iter().enumerate() {
            id_to_idx.insert(s.id.clone(), i);
        }
        let mut adj = vec![Vec::new(); steps.len()];
        for (i, s) in steps.iter().enumerate() {
            let deps: Vec<String> = serde_json::from_str(&s.depends_on).unwrap_or_default();
            for dep_id in &deps {
                if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                    adj[i].push(dep_idx);
                }
            }
        }
        let redundant = crate::graph::find_redundant_edges(&adj)
            .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;
        let node = |idx: usize| DependencyNode {
            id: steps[idx].id.clone(),
            title: steps[idx].title.clone(),
        };
        Ok(redundant
            .into_iter()
            .map(|e| RedundantDependency {
                from: steps[e.from].id.clone(),
                from_title: steps[e.from].title.clone(),
                to: steps[e.to].id.clone(),
                to_title: steps[e.to].title.clone(),
                via: e.via.iter().map(|&i| node(i)).collect(),
            })
            .collect())
    }

    // ── Schedule ──────────────────────────────────────────

    pub async fn get_schedule(&self) -> Result<ScheduleRow, AppError> {
        let row = self
            .storage
            .get_schedule()
            .await
            .map_err(storage_to_app)?
            .ok_or_else(|| AppError::NotFound("no active schedule".into()))?;
        Ok(row)
    }

    pub async fn generate_schedule(
        &self,
        input: &GenerateScheduleInput,
    ) -> Result<ScheduleRow, AppError> {
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let sleep = parse_sleep(&input.sleep, &settings, &tz)?;
        let from_point = Point::from_timestamp(Timestamp::now(), 5);

        let habit_rows = self.sync_habit_tasks(&tz).await?;
        // Load non-habit tasks after syncing so any tasks deleted by sync
        // (stale habit tasks) are not carried into the planner (#582).
        let task_rows = self.load_task_rows(input.task_ids.as_ref()).await?;
        let all_rows = Self::merge_active_tasks(habit_rows, task_rows);
        let (mut planner, id_map, id_to_idx) = self
            .build_planner(from_point, sleep, &settings, &all_rows, &tz)
            .await?;

        // #211: 前回スケジュールを参照として渡し、直近タスクの移動に
        // ペナルティを課す（pinではなく軟制約）。SAは必要なら動かせるが、
        // 直近のタスクは前回位置を維持する方が高スコアになる。
        let existing_schedule = self.storage.get_schedule().await.map_err(storage_to_app)?;
        // unwrap_or_default: if the schedule JSON is corrupt, fall back to
        // an empty vec which disables the stability penalty rather than
        // crashing. This is intentionally more forgiving than reschedule
        // (which returns an error on parse failure) because generate is a
        // full regenerate — the user just wants a new schedule.
        let existing_entries: Vec<ScheduleEntry> = existing_schedule
            .as_ref()
            .and_then(|row| serde_json::from_str(&row.schedule).ok())
            .unwrap_or_default();
        if !existing_entries.is_empty() {
            let prev: Vec<TaskPlacement> = existing_entries
                .iter()
                .filter_map(|entry| {
                    let idx = id_to_idx.get(&entry.task_id)?;
                    let s = iso_to_point(&entry.start_at.to_string(), &tz).ok()?;
                    let e = iso_to_point(&entry.end_at.to_string(), &tz).ok()?;
                    Some(TaskPlacement::new(s, e, *idx))
                })
                .collect();
            planner.set_previous_schedule(&prev);
        }

        let plan = planner.plan();
        let mut entries = self.plan_to_entries(&plan, &id_map)?;
        // #354: in_progress タスクは planner の対象外だが、save_schedule が
        // スケジュール全体を上書きするため、進行中タスクのスケジュール情報が
        // 消えてしまう。前回スケジュールから in_progress タスクのエントリを
        // 引き継ぐ。
        entries = self
            .preserve_active_entries(entries, &existing_entries, &["in_progress"])
            .await?;
        let mark_ids: Vec<String> = all_rows.iter().map(|t| t.id.clone()).collect();

        let result = self
            .storage
            .save_schedule(&SaveScheduleRequest {
                entries,
                mark_scheduled_task_ids: mark_ids,
            })
            .await
            .map_err(storage_to_app)?;

        if let Err(e) = self.do_sync().await {
            tracing::warn!("google calendar sync failed: {e}");
        }
        Ok(result)
    }

    pub async fn preview_schedule(
        &self,
        input: &SchedulePreviewInput,
    ) -> Result<SchedulePreviewOutput, AppError> {
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let sleep = parse_sleep(&input.sleep, &settings, &tz)?;
        let from_point = Point::from_timestamp(Timestamp::now(), 5);
        let habit_rows = self.sync_habit_tasks(&tz).await?;
        let task_rows = self.load_task_rows(input.task_ids.as_ref()).await?;
        let all_rows = Self::merge_active_tasks(habit_rows, task_rows);
        let (mut planner, id_map, id_to_idx) = self
            .build_planner(from_point, sleep, &settings, &all_rows, &tz)
            .await?;
        let existing_entries = self
            .storage
            .get_schedule()
            .await
            .map_err(storage_to_app)?
            .and_then(|row| serde_json::from_str::<Vec<ScheduleEntry>>(&row.schedule).ok())
            .unwrap_or_default();
        let current_schedule = existing_entries
            .iter()
            .filter_map(|entry| {
                Some(TaskPlacement::new(
                    iso_to_point(&entry.start_at.to_string(), &tz).ok()?,
                    iso_to_point(&entry.end_at.to_string(), &tz).ok()?,
                    *id_to_idx.get(&entry.task_id)?,
                ))
            })
            .collect::<Vec<_>>();
        let plan = match input.mode.as_str() {
            "full" => {
                if !current_schedule.is_empty() {
                    planner.set_previous_schedule(&current_schedule);
                }
                planner.plan()
            }
            "tasks" => {
                if !current_schedule.is_empty() {
                    planner.set_previous_schedule(&current_schedule);
                }
                let task_ids = input.task_ids.as_ref().ok_or_else(|| {
                    AppError::BadRequest("task_ids is required for tasks mode".into())
                })?;
                let pinned = current_schedule
                    .iter()
                    .filter(|p| {
                        !task_ids.contains(&id_map[p.task_id])
                            || input.pinned.contains(&id_map[p.task_id])
                    })
                    .copied()
                    .collect::<Vec<_>>();
                planner.plan_partial(&pinned)
            }
            "range" => {
                let from_str = input.from.as_ref().ok_or_else(|| {
                    AppError::BadRequest("from is required for range mode".into())
                })?;
                let until_str = input.until.as_ref().ok_or_else(|| {
                    AppError::BadRequest("until is required for range mode".into())
                })?;
                let range = RescheduleRange {
                    from: iso_to_point(from_str, &tz)?,
                    until: iso_to_point(until_str, &tz)?,
                };
                let extra_pinned: Vec<usize> = input
                    .pinned
                    .iter()
                    .filter_map(|pid| id_to_idx.get(pid).copied())
                    .collect();
                planner.plan_in_range(&range, &current_schedule, &extra_pinned)
            }
            _ => {
                return Err(AppError::BadRequest(format!(
                    "unknown mode: {}",
                    input.mode
                )));
            }
        };
        let entries = self.plan_to_entries(&plan, &id_map)?;
        let scheduled = entries
            .iter()
            .map(|entry| entry.task_id.clone())
            .collect::<std::collections::HashSet<_>>();
        let all_ids = all_rows
            .iter()
            .map(|task| task.id.clone())
            .collect::<std::collections::HashSet<_>>();
        let unscheduled_task_ids = all_ids.difference(&scheduled).cloned().collect();
        let displaced_task_ids = existing_entries
            .iter()
            .map(|entry| entry.task_id.clone())
            .filter(|id| !scheduled.contains(id))
            .collect();
        Ok(SchedulePreviewOutput {
            entries,
            unscheduled_task_ids,
            displaced_task_ids,
            sleep_minutes_before: 0,
            sleep_minutes_after: 0,
            warnings: Vec::new(),
        })
    }

    pub async fn replace_schedule(
        &self,
        request: &SaveScheduleRequest,
    ) -> Result<ScheduleRow, AppError> {
        let result = self
            .storage
            .save_schedule(request)
            .await
            .map_err(storage_to_app)?;
        if let Err(error) = self.do_sync().await {
            tracing::warn!("google calendar sync failed: {error}");
        }
        Ok(result)
    }

    pub async fn reschedule(&self, input: &RescheduleInput) -> Result<ScheduleRow, AppError> {
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let sleep = parse_sleep(&input.sleep, &settings, &tz)?;
        let now_point = Point::from_timestamp(Timestamp::now(), 5);

        let schedule_row = self
            .storage
            .get_schedule()
            .await
            .map_err(storage_to_app)?
            .ok_or_else(|| AppError::NotFound("no active schedule".into()))?;
        let entries: Vec<ScheduleEntry> = serde_json::from_str(&schedule_row.schedule)
            .map_err(|e| AppError::Internal(e.to_string()))?;

        let habit_rows = self.sync_habit_tasks(&tz).await?;
        // Load active tasks after sync to avoid stale rows deleted by sync.
        let task_rows = self.load_task_rows(None).await?;
        let active = Self::merge_active_tasks(habit_rows, task_rows);

        let (planner, id_map, id_to_idx) = self
            .build_planner(now_point, sleep, &settings, &active, &tz)
            .await?;

        // Note: stability penalty (#211) is intentionally NOT applied here.
        // reschedule is a user-initiated partial reconfiguration — the user
        // explicitly chose which tasks to move, so we don't want to resist
        // that movement. Stability is only for generate_schedule (full
        // regenerate) where the user hasn't expressed a preference.
        let current_schedule: Vec<TaskPlacement> = entries
            .iter()
            .filter_map(|entry| {
                let idx = *id_to_idx.get(&entry.task_id)?;
                let s = iso_to_point(&entry.start_at.to_string(), &tz).ok()?;
                let e = iso_to_point(&entry.end_at.to_string(), &tz).ok()?;
                Some(TaskPlacement::new(s, e, idx))
            })
            .collect();

        let plan = match input.mode.as_str() {
            "range" => {
                let from_str = input.from.as_ref().ok_or_else(|| {
                    AppError::BadRequest("from is required for range mode".into())
                })?;
                let until_str = input.until.as_ref().ok_or_else(|| {
                    AppError::BadRequest("until is required for range mode".into())
                })?;
                let range = RescheduleRange {
                    from: iso_to_point(from_str, &tz)?,
                    until: iso_to_point(until_str, &tz)?,
                };
                let extra_pinned: Vec<usize> = input
                    .pinned
                    .iter()
                    .filter_map(|pid| id_to_idx.get(pid).copied())
                    .collect();
                planner.plan_in_range(&range, &current_schedule, &extra_pinned)
            }
            "tasks" => {
                let task_ids = input.task_ids.as_ref().ok_or_else(|| {
                    AppError::BadRequest("task_ids is required for tasks mode".into())
                })?;
                // pinned 条件: task_ids に含まれない (再スケジュール対象外) または
                // 明示的に pinned 指定されたタスクは固定。残りが再配置される。
                // id_map[idx] で planner index → 文字列ID に変換している。
                let pinned_entries: Vec<TaskPlacement> = current_schedule
                    .iter()
                    .filter(|p| {
                        let tid = &id_map[p.task_id];
                        !task_ids.contains(tid) || input.pinned.contains(tid)
                    })
                    .copied()
                    .collect();
                planner.plan_partial(&pinned_entries)
            }
            _ => {
                return Err(AppError::BadRequest(format!(
                    "unknown mode: {}",
                    input.mode
                )));
            }
        };

        let mut final_entries = self.plan_to_entries(&plan, &id_map)?;
        // #354: in_progress タスクは planner の対象外なので、再スケジュール時も
        // 進行中タスクのエントリが消えないよう前回スケジュールから引き継ぐ。
        final_entries = self
            .preserve_active_entries(final_entries, &entries, &["in_progress"])
            .await?;
        let result = self
            .storage
            .save_schedule(&SaveScheduleRequest {
                entries: final_entries,
                mark_scheduled_task_ids: vec![],
            })
            .await
            .map_err(storage_to_app)?;

        if let Err(e) = self.do_sync().await {
            tracing::warn!("google calendar sync failed: {e}");
        }
        Ok(result)
    }

    pub async fn move_entry(
        &self,
        task_id: &str,
        new_start: &str,
        force: bool,
    ) -> Result<MoveEntryOutput, AppError> {
        let full_task_id = self
            .storage
            .get_task(task_id)
            .await
            .map(|t| t.id)
            .map_err(storage_to_app)?;

        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;

        let schedule_row = self
            .storage
            .get_schedule()
            .await
            .map_err(storage_to_app)?
            .ok_or_else(|| AppError::NotFound("no active schedule".into()))?;
        let mut entries: Vec<ScheduleEntry> = serde_json::from_str(&schedule_row.schedule)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let idx = entries
            .iter()
            .position(|e| e.task_id == full_task_id)
            .ok_or_else(|| AppError::NotFound(format!("task {task_id} not in schedule")))?;

        let new_start_point = iso_to_point(new_start, &tz)?;
        let task_row = self
            .storage
            .get_task(&full_task_id)
            .await
            .map_err(storage_to_app)?;
        let old_start = iso_to_point(&entries[idx].start_at.to_string(), &tz)?;
        let old_end = iso_to_point(&entries[idx].end_at.to_string(), &tz)?;
        let duration = Point::delta(old_end, old_start);
        let new_end = Point(new_start_point.0 + duration);
        let new_entry = ScheduleEntry {
            task_id: full_task_id.clone(),
            start_at: point_to_iso(new_start_point.0)?,
            end_at: point_to_iso(new_end.0)?,
        };

        // move_entry は deadline 超過のみチェックする。
        // 依存関係違反、睡眠侵害、並列違反はチェックしない。
        // これは意図的: 手動移動はユーザーの明示的な操作であり、
        // 自動スケジューラの制約をすべて検証すると自由度が下がるため。
        // force=true で強制上書きも可能。
        let mut warnings = Vec::new();
        let task_deadline = iso_to_point(&task_row.end_at.to_string(), &tz)?;
        if new_end.0 > task_deadline.0 {
            warnings.push("deadline_violation".to_string());
        }
        if !warnings.is_empty() && !force {
            return Err(AppError::Conflict {
                message: "schedule violations detected".into(),
            });
        }
        entries[idx] = new_entry;
        self.storage
            .save_schedule(&SaveScheduleRequest {
                entries,
                mark_scheduled_task_ids: vec![],
            })
            .await
            .map_err(storage_to_app)?;

        if let Err(e) = self.do_sync().await {
            tracing::warn!("google calendar sync failed: {e}");
        }

        Ok(MoveEntryOutput {
            task_id: task_row.id,
            start_at: point_to_iso(new_start_point.0)?.to_string(),
            end_at: point_to_iso(new_end.0)?.to_string(),
            warnings,
        })
    }

    pub async fn clear_schedule(&self) -> Result<(), AppError> {
        self.storage
            .clear_schedule()
            .await
            .map_err(storage_to_app)?;
        if let Err(e) = self.do_sync().await {
            tracing::warn!("google calendar sync failed: {e}");
        }
        Ok(())
    }

    // ── Tokens ────────────────────────────────────────────

    pub async fn create_token(&self, label: Option<&str>) -> Result<TokenCreateResponse, AppError> {
        let resp = self
            .storage
            .create_token(label)
            .await
            .map_err(storage_to_app)?;
        self.token_cache.invalidate();
        Ok(resp)
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenRow>, AppError> {
        self.storage.list_tokens().await.map_err(storage_to_app)
    }

    pub async fn revoke_token(&self, id: i64) -> Result<(), AppError> {
        self.storage
            .revoke_token(id)
            .await
            .map_err(storage_to_app)?;
        self.token_cache.invalidate();
        Ok(())
    }

    // ── Sync / Google Calendar ────────────────────────────

    pub async fn get_gcal_settings(&self) -> Result<GoogleCalSettingsOutput, AppError> {
        let row = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        Ok(GoogleCalSettingsOutput {
            enabled: row.enabled,
            calendar_id: row.calendar_id,
            client_id: row.client_id,
            has_client_secret: !row.client_secret.is_empty(),
            has_refresh_token: row.refresh_token.is_some(),
        })
    }

    pub async fn update_gcal_settings(
        &self,
        body: &UpdateGoogleCalSettings,
    ) -> Result<GoogleCalSettingsOutput, AppError> {
        let row = self
            .storage
            .update_gcal_settings(body)
            .await
            .map_err(storage_to_app)?;
        Ok(GoogleCalSettingsOutput {
            enabled: row.enabled,
            calendar_id: row.calendar_id,
            client_id: row.client_id,
            has_client_secret: !row.client_secret.is_empty(),
            has_refresh_token: row.refresh_token.is_some(),
        })
    }

    pub async fn oauth_url(&self, redirect_uri: &str) -> Result<String, AppError> {
        let row = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        if row.client_id.is_empty() {
            return Err(AppError::BadRequest(
                "google calendar settings not configured".into(),
            ));
        }
        Ok(google_cal::oauth_url(&row.client_id, redirect_uri))
    }

    pub async fn oauth_callback(
        &self,
        code: &str,
        redirect_uri: Option<&str>,
    ) -> Result<(), AppError> {
        let row = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        if row.client_id.is_empty() || row.client_secret.is_empty() {
            return Err(AppError::BadRequest(
                "google calendar settings not configured".into(),
            ));
        }
        let tokens =
            google_cal::exchange_code(&row.client_id, &row.client_secret, code, redirect_uri)
                .await
                .map_err(|e| AppError::Internal(format!("oauth exchange failed: {e}")))?;
        self.storage
            .update_gcal_settings(&UpdateGoogleCalSettings {
                enabled: None,
                calendar_id: None,
                client_id: None,
                client_secret: None,
                refresh_token: Some(tokens.refresh_token),
            })
            .await
            .map_err(storage_to_app)?;
        Ok(())
    }

    pub async fn list_gcal_mappings(&self) -> Result<Vec<GoogleCalEventRow>, AppError> {
        self.storage
            .list_gcal_mappings()
            .await
            .map_err(storage_to_app)
    }

    /// Backend health check. Returns a short status string from the storage
    /// backend (e.g. "worker ok" or "sqlite ok (v3.x)").
    pub async fn health_check(&self) -> Result<String, AppError> {
        self.storage.health_check().await.map_err(storage_to_app)
    }

    pub async fn do_sync(&self) -> Result<(), String> {
        let settings = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(|e| e.to_string())?;
        let (refresh_token, client_id, client_secret, calendar_id) = match &settings {
            s if s.enabled && s.refresh_token.is_some() => (
                s.refresh_token.clone().unwrap(),
                s.client_id.clone(),
                s.client_secret.clone(),
                s.calendar_id.clone(),
            ),
            _ => return Ok(()),
        };
        let refresh_token = if refresh_token.is_empty() {
            return Ok(());
        } else {
            refresh_token
        };

        let schedule_row = self
            .storage
            .get_schedule()
            .await
            .map_err(|e| e.to_string())?;
        let entries: Option<Vec<ScheduleEntry>> = match schedule_row {
            Some(s) => serde_json::from_str(&s.schedule).ok(),
            None => None,
        };

        let client = google_cal::Client::new(client_id, client_secret, refresh_token, calendar_id)
            .map_err(|e| e.to_string())?;

        match entries {
            Some(entries) => {
                let task_ids: Vec<String> = entries.iter().map(|e| e.task_id.clone()).collect();
                let mut titles: HashMap<String, (String, Option<String>)> = HashMap::new();
                for id in &task_ids {
                    if let Ok(t) = self.storage.get_task(id).await {
                        titles.insert(t.id.clone(), (t.title, t.description));
                    }
                }
                let db_mappings = self
                    .storage
                    .list_gcal_mappings()
                    .await
                    .map_err(|e| e.to_string())?;
                let existing: HashMap<String, String> = db_mappings
                    .iter()
                    .map(|m| (m.task_id.clone(), m.google_event_id.clone()))
                    .collect();

                let sync_entries: Vec<google_cal::SyncEntry> = entries
                    .iter()
                    .map(|e| {
                        let (summary, description) = titles
                            .get(&e.task_id)
                            .cloned()
                            .unwrap_or_else(|| (e.task_id.clone(), None));
                        google_cal::SyncEntry {
                            task_id: e.task_id.clone(),
                            summary,
                            description,
                            start: e.start_at.to_string(),
                            end: e.end_at.to_string(),
                        }
                    })
                    .collect();

                let result = client
                    .sync(&sync_entries, &existing)
                    .await
                    .map_err(|e| e.to_string())?;

                let deleted_task_ids: Vec<String> = result
                    .deleted
                    .iter()
                    .filter_map(|eid| {
                        db_mappings
                            .iter()
                            .find(|m| &m.google_event_id == eid)
                            .map(|m| m.task_id.clone())
                    })
                    .collect();
                self.storage
                    .upsert_gcal_mappings(&result.mappings)
                    .await
                    .map_err(|e| e.to_string())?;
                self.storage
                    .delete_gcal_mappings(&deleted_task_ids)
                    .await
                    .map_err(|e| e.to_string())?;
                tracing::info!(
                    "google calendar sync: created/updated {}, deleted {}",
                    result.mappings.len(),
                    deleted_task_ids.len()
                );
                if !result.failed.is_empty() {
                    let summary = result
                        .failed
                        .iter()
                        .map(|f| format!("{}({}): {}", f.task_id, f.operation, f.error))
                        .collect::<Vec<_>>()
                        .join("; ");
                    tracing::warn!(
                        "google calendar sync: {} failure(s): {summary}",
                        result.failed.len()
                    );
                    return Err(format!(
                        "google calendar sync partially failed: {} operation(s) could not complete — DB and Calendar may diverge",
                        result.failed.len()
                    ));
                }
                Ok(())
            }
            None => {
                tracing::info!("no active schedule, clearing google calendar events");
                let result = self
                    .delete_all_gcal_events_with_settings(&settings)
                    .await
                    .map_err(|e| e.to_string())?;
                tracing::info!(
                    "deleted {} google calendar event(s), {} failure(s)",
                    result.deleted,
                    result.failed.len()
                );
                if !result.failed.is_empty() {
                    let summary = result
                        .failed
                        .iter()
                        .map(|f| format!("{}: {}", f.task_id, f.error))
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(format!(
                        "google calendar delete all partially failed: {} event(s) could not be deleted: {summary}",
                        result.failed.len()
                    ));
                }
                Ok(())
            }
        }
    }

    /// Delete all events that are mapped to the local schedule on Google
    /// Calendar, then remove the local mappings. This is useful when the
    /// calendar has drifted or the user wants to clean up imported events
    /// from the Google side (#598).
    pub async fn delete_all_gcal_events(&self) -> Result<DeleteAllGcalResult, AppError> {
        let settings = self
            .storage
            .get_gcal_settings()
            .await
            .map_err(storage_to_app)?;
        self.delete_all_gcal_events_with_settings(&settings).await
    }

    /// Shared implementation used by the explicit delete command and the
    /// "no active schedule" sync cleanup path.
    async fn delete_all_gcal_events_with_settings(
        &self,
        settings: &GoogleCalSettingsRow,
    ) -> Result<DeleteAllGcalResult, AppError> {
        if settings.client_id.is_empty() {
            return Err(AppError::BadRequest(
                "google calendar client_id not configured".into(),
            ));
        }
        if settings.client_secret.is_empty() {
            return Err(AppError::BadRequest(
                "google calendar client_secret not configured".into(),
            ));
        }
        let refresh_token = settings
            .refresh_token
            .as_deref()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                AppError::BadRequest("google calendar refresh token not configured".into())
            })?;

        let mappings = self
            .storage
            .list_gcal_mappings()
            .await
            .map_err(storage_to_app)?;
        if mappings.is_empty() {
            return Ok(DeleteAllGcalResult {
                deleted: 0,
                failed: vec![],
            });
        }

        let client = google_cal::Client::new(
            settings.client_id.clone(),
            settings.client_secret.clone(),
            refresh_token.to_string(),
            settings.calendar_id.clone(),
        )
        .map_err(|e| AppError::Internal(format!("failed to create google calendar client: {e}")))?;

        let task_event_pairs: Vec<(String, String)> = mappings
            .iter()
            .map(|m| (m.task_id.clone(), m.google_event_id.clone()))
            .collect();

        let result = client.delete_all(&task_event_pairs).await.map_err(|e| {
            AppError::Internal(format!("failed to delete google calendar events: {e}"))
        })?;

        self.storage
            .delete_gcal_mappings(&result.deleted)
            .await
            .map_err(storage_to_app)?;

        Ok(DeleteAllGcalResult {
            deleted: result.deleted.len(),
            failed: result
                .failed
                .into_iter()
                .map(|f| DeleteAllGcalFailure {
                    task_id: f.task_id,
                    error: f.error,
                })
                .collect(),
        })
    }

    // ── Helpers ───────────────────────────────────────────

    /// スケジュール生成対象のタスクをロード。
    ///
    /// - task_ids 指定時: 指定された ID のタスクのみ取得。
    ///   存在しない ID は無視される (ユーザーが削除済みのタスクを指定した場合など)。
    ///   これは意図的な設計: 指定 ID の一部が消失しても生成を継続する。
    ///   ただし、ユーザーはどの ID が無視されたか通知されないため、
    ///   API レベルで警告を返す余地がある。
    /// - task_ids なし: 全タスクから pending/scheduled のみをフィルタ。
    async fn load_task_rows(
        &self,
        task_ids: Option<&Vec<String>>,
    ) -> Result<Vec<TaskRow>, AppError> {
        if let Some(ids) = task_ids {
            let mut out = Vec::new();
            for id in ids {
                match self.storage.get_task(id).await {
                    Ok(t) => out.push(t),
                    Err(takusu_storage::StorageError::NotFound(_)) => continue,
                    Err(e) => return Err(storage_to_app(e)),
                }
            }
            Ok(out)
        } else {
            let all = self
                .storage
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(storage_to_app)?;
            Ok(all
                .into_iter()
                .filter(|t| t.status == TaskStatus::Pending || t.status == TaskStatus::Scheduled)
                .collect())
        }
    }

    /// Merge habit-synced task rows with the active task list and deduplicate by
    /// task id. Both sources are read after `sync_habit_tasks`, but `habit_rows`
    /// is processed first because it is the authoritative result of the sync
    /// and may contain newly created/updated habit tasks. This also ensures habit
    /// tasks are included even when `input.task_ids` filters `task_rows` to a
    /// subset. `task_rows` then adds non-habit tasks. Only `pending` / `scheduled`
    /// tasks are kept.
    fn merge_active_tasks(habit_rows: Vec<TaskRow>, task_rows: Vec<TaskRow>) -> Vec<TaskRow> {
        let mut seen = std::collections::HashSet::new();
        habit_rows
            .into_iter()
            .chain(task_rows)
            .filter(|t| t.status == TaskStatus::Pending || t.status == TaskStatus::Scheduled)
            .filter(|t| seen.insert(t.id.clone()))
            .collect()
    }

    pub async fn sync_habit_tasks(
        &self,
        tz: &jiff::tz::TimeZone,
    ) -> Result<Vec<TaskRow>, AppError> {
        let habits = self.storage.list_habits().await.map_err(storage_to_app)?;
        if habits.is_empty() {
            return Ok(vec![]);
        }

        let now_ts = Timestamp::now();
        let now = Point::from_timestamp(now_ts, 5);
        // 過去のハビットタスクは生成しない: 過去分を残すと Planner が
        // 開始時刻を過ぎたタスクを今日以降に再配置してしまい、別の日に
        // 実行される問題 (#204/#205/#207) が起きるため、from を今日の
        // 0時 (tz ローカル) にする。今日の 0 時にすることで、今日の
        // 開始時刻を過ぎたハビットタスクも expected に残り、cleanup
        // ループで削除されないようにする。
        // now_ts を再利用して日付境界をまたぐレースを防ぐ。
        // start_of_day() は DST の spring-forward で 0 時が存在しない
        // タイムゾーンでも安全に開始時刻を返す。
        let start_of_today = now_ts
            .to_zoned(tz.clone())
            .start_of_day()
            .map_err(|e| AppError::Internal(format!("start_of_day: {e}")))?
            .timestamp();
        let from = Point::from_timestamp(start_of_today, 5);
        let until = now + 14 * 24 * 12;

        // Habit scheduled spans (#303 / #503): fetch all spans once and build a
        // habit_id → Vec<(start, end)> map.
        //
        // Their effect depends on `habits.active`:
        // - active habit:    span dates are skipped (a pause).
        // - disabled habit:  only span dates are generated (an activation window).
        // The existing cleanup loop deletes now-unexpected pending/unedited tasks.
        let all_spans = self
            .storage
            .list_all_habit_scheduled_spans()
            .await
            .map_err(storage_to_app)?;
        let mut spans_by_habit: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, p) in all_spans.iter().enumerate() {
            spans_by_habit
                .entry(p.habit_id.clone())
                .or_default()
                .push(i);
        }

        // Habit steps (#95): fetch all steps once and group by habit_id.
        // Habits with at least one step emit one task per step per occurrence
        // (each with its own window/cost/flags and step-id-keyed depends);
        // habits with no steps keep the legacy single-task-per-occurrence
        // behavior.
        let all_steps = self
            .storage
            .list_all_habit_steps()
            .await
            .map_err(storage_to_app)?;
        let mut steps_by_habit: HashMap<String, Vec<HabitStepRow>> = HashMap::new();
        for s in all_steps {
            steps_by_habit
                .entry(s.habit_id.clone())
                .or_default()
                .push(s);
        }

        // expected entry:
        //   (habit_id, step_id_opt, date, core_task, habit_desc, step_title_opt)
        #[allow(clippy::type_complexity)]
        let mut expected: Vec<(
            String,
            Option<String>,
            String,
            CoreTask,
            Option<String>,
            Option<String>,
        )> = Vec::new();
        for row in &habits {
            let config = habit_row_to_config(row, tz)?;
            let mut store = takusu_habit::HabitStore::new();
            store.add(config);
            let spans = spans_by_habit.get(&row.id);
            let steps = steps_by_habit.get(&row.id);

            // window_mode (#window_mode): 'period' widens the task window from
            // the occurrence day to the whole interval (occurrence start ..
            // next occurrence start). 'day' (default) keeps the legacy
            // per-day window. The core planner needs no change — it already
            // schedules freely within [start, end].
            let is_period = row.window_mode == WindowMode::Period;

            if is_period {
                // Lookahead past `until` so we can compute the deadline of
                // the last in-range occurrence (deadline = next occurrence
                // start). 365 days covers even yearly habits; for count-
                // limited rules the generator stops early anyway.
                let until_lookahead = Point(until.0 + 365 * 288);
                let today_str = point_to_local_date(from.0, tz)?;
                let rule = parse_recurrence(&row.recurrence)?;
                let occs: Vec<(String, Point)> = store
                    .generate(from, until_lookahead)
                    .into_iter()
                    .map(|gt| {
                        let sp = gt.task.start.unwrap_or(Point(0));
                        Ok((point_to_local_date(sp.0, tz)?, sp))
                    })
                    .collect::<Result<Vec<_>, AppError>>()?;

                for (i, (date, occ_start)) in occs.iter().enumerate() {
                    // Only generate tasks for occurrences within the sync
                    // window. Occurrences past `until` are kept in `occs`
                    // solely as lookahead for the previous deadline.
                    if occ_start.0 >= until.0 {
                        break;
                    }
                    let in_span = spans.is_some_and(|spans| {
                        spans.iter().any(|&i| {
                            let s = &all_spans[i];
                            date.as_str() >= s.start_date.to_string().as_str()
                                && date.as_str() <= s.end_date.to_string().as_str()
                        })
                    });
                    // active habit: span 内は pause してスキップ
                    // disabled habit: span 内のみ生成
                    if row.active && in_span {
                        continue;
                    }
                    if !row.active && !in_span {
                        continue;
                    }

                    // deadline = next occurrence's start (just-before semantics
                    // are satisfied since the next occurrence's task starts at
                    // that point). Fall back to occurrence + freq-interval when
                    // there is no next occurrence (e.g. count-limited rules).
                    let deadline_pt = if let Some((_, next_start)) = occs.get(i + 1) {
                        *next_start
                    } else {
                        Point(occ_start.0 + freq_fallback_slots(&rule))
                    };
                    // Clamp the window start to today's 0:00 for the in-progress
                    // period (today's occurrence) so the planner can place the
                    // task later today instead of being anchored to a start
                    // time that may already be in the past (#204/#205).
                    let window_start = if *date == today_str { from } else { *occ_start };

                    if let Some(steps) = steps
                        && !steps.is_empty()
                    {
                        // period + steps: all steps share the period window;
                        // each step's own start_time/end_time is ignored
                        // (meaningful only in 'day' mode). Step avg/sigma/
                        // flags still apply.
                        let order = topo_sort_steps(steps)?;
                        for &idx in &order {
                            let step = &steps[idx];
                            let core = step_to_core_task_period(step, window_start, deadline_pt);
                            expected.push((
                                row.id.clone(),
                                Some(step.id.clone()),
                                date.clone(),
                                core,
                                step.description.clone(),
                                Some(step.title.clone()),
                            ));
                        }
                    } else {
                        let cost = NormalDist::from_minutes(
                            Minutes(row.avg_minutes),
                            Minutes(row.sigma_minutes),
                        );
                        let core = CoreTask {
                            id: 0,
                            start: Some(window_start),
                            end: deadline_pt,
                            cost_estimate: cost,
                            depends: vec![],
                            parallel_mode: ParallelMode::from_bools(
                                row.parallelizable,
                                row.allows_parallel,
                            ),
                            abandonability: row.abandonability,
                            fixed: row.fixed,
                            // period mode: no habit_group (the consistency bonus
                            // is meaningless when the window spans days).
                            habit_group: None,
                        };
                        expected.push((
                            row.id.clone(),
                            None,
                            date.clone(),
                            core,
                            row.description.clone(),
                            None,
                        ));
                    }
                }
            } else {
                for gt in store.generate(from, until) {
                    let start_point = gt.task.start.unwrap_or(Point(0));
                    let date = point_to_local_date(start_point.0, tz)?;
                    let in_span = spans.is_some_and(|spans| {
                        spans.iter().any(|&i| {
                            let s = &all_spans[i];
                            date.as_str() >= s.start_date.to_string().as_str()
                                && date.as_str() <= s.end_date.to_string().as_str()
                        })
                    });
                    // active habit: span 内は pause してスキップ
                    // disabled habit: span 内のみ生成
                    if row.active && in_span {
                        continue;
                    }
                    if !row.active && !in_span {
                        continue;
                    }

                    if let Some(steps) = steps
                        && !steps.is_empty()
                    {
                        // Multi-step habit: emit one task per step. The habit's
                        // own window/cost is ignored; each step carries its own.
                        // Steps are emitted in topological order so dependencies
                        // are created before dependents. The actual depends
                        // wiring (step ids → task ids) happens in the post-pass
                        // below, after we know the created task ids.
                        let order = topo_sort_steps(steps)?;
                        let occ_start = start_point;
                        for &idx in &order {
                            let step = &steps[idx];
                            let core = step_to_core_task(step, occ_start, tz)?;
                            expected.push((
                                row.id.clone(),
                                Some(step.id.clone()),
                                date.clone(),
                                core,
                                step.description.clone(),
                                Some(step.title.clone()),
                            ));
                        }
                    } else {
                        // Legacy single-task habit.
                        expected.push((
                            row.id.clone(),
                            None,
                            date,
                            gt.task,
                            row.description.clone(),
                            None,
                        ));
                    }
                }
            }
        }

        let all_tasks = self
            .storage
            .list_tasks(&TaskQuery::default())
            .await
            .map_err(storage_to_app)?;

        // Key: (habit_id, step_id_opt, date). step_id_opt is None for legacy
        // single-task habits and "" is not a valid step id, so the tuple
        // distinguishes step-generated tasks from legacy ones.
        let mut existing_by_key: HashMap<(String, Option<String>, String), TaskRow> =
            HashMap::new();
        for task in &all_tasks {
            if let Some(ref hid) = task.habit_id {
                let date = task
                    .start_at
                    .as_ref()
                    .map(|ts| iso_to_local_date(&ts.to_string(), tz))
                    .unwrap_or_default();
                if !date.is_empty() {
                    existing_by_key.insert(
                        (hid.clone(), task.habit_step_id.clone(), date),
                        task.clone(),
                    );
                }
            }
        }

        let mut result: Vec<TaskRow> = Vec::new();
        // Per-occurrence map: (habit_id, date) → step_id → created/updated
        // task id, used to wire step depends after the create/update pass.
        let mut occ_task_ids: HashMap<(String, String), HashMap<String, String>> = HashMap::new();

        for (habit_id, step_id_opt, date, core_task, habit_desc, step_title_opt) in &expected {
            let key = (habit_id.clone(), step_id_opt.clone(), date.clone());
            let habit_row = habits.iter().find(|h| h.id == *habit_id);
            let title = match (habit_row, step_title_opt) {
                (Some(h), Some(st)) => format!("{} — {} ({})", h.title, st, date),
                (Some(h), None) => format!("{} ({})", h.title, date),
                (None, Some(st)) => format!("{} ({})", st, date),
                (None, None) => format!("habit:{}", date),
            };

            if let Some(existing) = existing_by_key.remove(&key) {
                if existing.status == TaskStatus::Pending && !existing.user_edited {
                    // ユーザーが habit 由来タスクを編集していない場合は、
                    // habit の現在値で全フィールドを上書きする。
                    let update = UpdateTask {
                        start_at: core_task
                            .start
                            .map(|p| point_to_iso(p.0))
                            .transpose()?
                            .map(Some),
                        end_at: Some(point_to_iso(core_task.end.0)?),
                        title: Some(title),
                        description: habit_desc.clone(),
                        avg_minutes: Some(Slots(core_task.cost_estimate.avg as i64).to_minutes().0),
                        sigma_minutes: Some(
                            Slots(core_task.cost_estimate.sigma as i64).to_minutes().0,
                        ),
                        parallelizable: Some(core_task.parallel_mode.is_guest()),
                        allows_parallel: Some(core_task.parallel_mode.is_host()),
                        abandonability: Some(core_task.abandonability),
                        fixed: Some(core_task.fixed),
                        habit_step_id: step_id_opt.clone(),
                        ..Default::default()
                    };
                    let updated = self
                        .storage
                        .update_task(&existing.id, &update)
                        .await
                        .map_err(storage_to_app)?;
                    if let Some(sid) = step_id_opt {
                        occ_task_ids
                            .entry((habit_id.clone(), date.clone()))
                            .or_default()
                            .insert(sid.clone(), updated.id.clone());
                    }
                    result.push(updated);
                } else {
                    // 非 pending またはユーザーが編集済みの場合は何も変更しない。
                    if let Some(sid) = step_id_opt {
                        occ_task_ids
                            .entry((habit_id.clone(), date.clone()))
                            .or_default()
                            .insert(sid.clone(), existing.id.clone());
                    }
                    result.push(existing.clone());
                }
            } else {
                let create = CreateTask {
                    title,
                    start_at: core_task.start.map(|p| point_to_iso(p.0)).transpose()?,
                    end_at: point_to_iso(core_task.end.0)?,
                    avg_minutes: Slots(core_task.cost_estimate.avg as i64).to_minutes().0,
                    sigma_minutes: Some(Slots(core_task.cost_estimate.sigma as i64).to_minutes().0),
                    depends: Some(vec![]),
                    parallelizable: Some(core_task.parallel_mode.is_guest()),
                    allows_parallel: Some(core_task.parallel_mode.is_host()),
                    abandonability: Some(core_task.abandonability),
                    description: habit_desc.clone(),
                    ical_uid: None,
                    habit_id: Some(habit_id.clone()),
                    fixed: Some(core_task.fixed),
                    habit_step_id: step_id_opt.clone(),
                    quantity_total: None,
                    quantity_done: None,
                    quantity_unit: None,
                    original_quantity_total: None,
                };
                let created = self
                    .storage
                    .create_task(&create)
                    .await
                    .map_err(storage_to_app)?;
                if let Some(sid) = step_id_opt {
                    occ_task_ids
                        .entry((habit_id.clone(), date.clone()))
                        .or_default()
                        .insert(sid.clone(), created.id.clone());
                }
                result.push(created);
            }
        }

        // Wire step depends (#95): for each occurrence, set each step task's
        // depends to the task ids of its step-level dependencies. Only
        // pending + unedited tasks are updated (consistent with the sync
        // overwrite policy above).
        let steps_by_habit_ref = &steps_by_habit;
        for ((habit_id, _date), step_to_task) in &occ_task_ids {
            let Some(steps) = steps_by_habit_ref.get(habit_id) else {
                continue;
            };
            for step in steps {
                let Some(task_id) = step_to_task.get(&step.id) else {
                    continue;
                };
                let deps: Vec<String> = serde_json::from_str(&step.depends_on).unwrap_or_default();
                if deps.is_empty() {
                    continue;
                }
                let mut dep_task_ids: Vec<String> = Vec::new();
                for dep_step_id in &deps {
                    if let Some(dep_task_id) = step_to_task.get(dep_step_id) {
                        dep_task_ids.push(dep_task_id.clone());
                    }
                }
                if dep_task_ids.is_empty() {
                    continue;
                }
                // Find the task row to check pending + unedited.
                let Some(task_row) = result.iter().find(|t| &t.id == task_id) else {
                    continue;
                };
                if task_row.status != TaskStatus::Pending || task_row.user_edited {
                    continue;
                }
                let update = UpdateTask {
                    depends: Some(dep_task_ids),
                    ..Default::default()
                };
                let updated = self
                    .storage
                    .update_task(task_id, &update)
                    .await
                    .map_err(storage_to_app)?;
                // Replace the entry in result.
                if let Some(slot) = result.iter_mut().find(|t| t.id == *task_id) {
                    *slot = updated;
                }
            }
        }

        // 過去の生成で作られたが、今回期待されなくなった習慣タスクを削除。
        // ユーザーが編集していない、かつ in_progress / completed / skipped ではない
        // タスクを削除対象とする。scheduled はスケジュール生成によって付与された
        // システム状態なので、削除対象に含める (generate_schedule 内で sync が呼ばれる
        // たびに schedule 自体も再構築される)。
        for (_, task) in existing_by_key {
            let deletable = !task.user_edited
                && !matches!(
                    task.status,
                    TaskStatus::InProgress | TaskStatus::Completed | TaskStatus::Skipped
                );
            if deletable {
                self.storage
                    .delete_task(&task.id)
                    .await
                    .map_err(storage_to_app)?;
            } else {
                result.push(task);
            }
        }

        Ok(result)
    }

    /// Planner を構築し、CoreTask のインデックスと Row ID の対応を返す。
    ///
    /// task_rows の順序が Planner の内部インデックスを決める。
    /// 戻り値:
    /// - planner: SA で最適化する Planner
    /// - id_map: `planner.tasks[i].id` に対応する DB の task row ID
    ///   (planner のタスクインデックス → 文字列ID の O(1) 変換テーブル)
    /// - id_to_idx: 文字列ID → planner のタスクインデックス (逆引き)
    ///   build_planner 内で依存関係解決に使われた後、
    ///   呼び出し元 (reschedule など) でもスケジュールエントリのフィルタリングに使われる。
    ///
    /// id_to_idx は最初に task_rows のインデックスで初期化された後、
    /// planner.add() 後に planner のインデックスで上書きされる。
    /// 両者は同じ順序なので一致するが、一部の add が失敗すると
    /// 不整合が生じる。その場合は関数全体がエラーを返すため問題ない。
    #[allow(clippy::type_complexity)]
    async fn build_planner(
        &self,
        start: Point,
        sleep: SleepConfig,
        settings: &SettingsRow,
        task_rows: &[TaskRow],
        tz: &jiff::tz::TimeZone,
    ) -> Result<(Planner, Vec<String>, HashMap<String, usize>), AppError> {
        let mut id_to_idx: HashMap<String, usize> = HashMap::new();
        for (i, row) in task_rows.iter().enumerate() {
            id_to_idx.insert(row.id.clone(), i);
        }

        let mut all_depends: Vec<Vec<usize>> = Vec::with_capacity(task_rows.len());
        for row in task_rows {
            let dep_ids: Vec<String> = serde_json::from_str(&row.depends).unwrap_or_default();
            let mut resolved = Vec::new();
            for dep_id in &dep_ids {
                if let Some(&idx) = id_to_idx.get(dep_id) {
                    resolved.push(idx);
                }
                // Dependencies that are not part of the active schedule set
                // (e.g. already completed, skipped, or deleted) are treated as
                // satisfied and ignored rather than breaking generation (#582).
                // Note: this also silently ignores typos or stale ids in the
                // depends column; the active-set filter makes this the intended
                // behavior, but it weakens detection of benign data drift.
            }
            all_depends.push(resolved);
        }

        crate::graph::detect_cycle(&all_depends)
            .map_err(|_| AppError::BadRequest("循環依存が検出されました".into()))?;

        // #306: Build habit_id → group index map so that tasks from the same
        // habit share a habit_group index, enabling the consistency bonus.
        // #window_mode: period-mode habits with multi-day windows (weekly,
        // monthly, yearly) get no group — the consistency bonus is
        // meaningless when the window spans days. Daily period-mode habits
        // (~24h windows) still benefit from consistency, so they keep the
        // group.
        let no_group_habits: std::collections::HashSet<String> = self
            .storage
            .list_habits()
            .await
            .map_err(storage_to_app)?
            .into_iter()
            .filter(|h| {
                if h.window_mode != WindowMode::Period {
                    return false;
                }
                // Only exclude habits whose recurrence interval is > 1 day.
                let rule: Option<takusu_habit::RecurrenceRule> =
                    parse_recurrence(&h.recurrence).ok();
                match rule {
                    Some(r) => {
                        let days = match r.freq {
                            takusu_habit::Frequency::Daily => r.interval.max(1),
                            takusu_habit::Frequency::Weekly => r.interval.max(1) * 7,
                            takusu_habit::Frequency::Monthly => r.interval.max(1) * 30,
                            takusu_habit::Frequency::Yearly => r.interval.max(1) * 365,
                        };
                        days > 1
                    }
                    None => true, // unknown recurrence → safe default: no group
                }
            })
            .map(|h| h.id)
            .collect();
        let mut habit_group_map: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut next_group = 0usize;
        for row in task_rows.iter() {
            if let Some(ref hid) = row.habit_id
                && !no_group_habits.contains(hid)
                && !habit_group_map.contains_key(hid)
            {
                habit_group_map.insert(hid.clone(), next_group);
                next_group += 1;
            }
        }

        let mut planner = Planner::new(start, sleep);
        apply_planner_settings(&mut planner, settings);
        let mut id_map: Vec<String> = Vec::with_capacity(task_rows.len());

        for (i, row) in task_rows.iter().enumerate() {
            let start_opt = row
                .start_at
                .as_ref()
                .map(|s| iso_to_point(&s.to_string(), tz))
                .transpose()?;
            let end = iso_to_point(&row.end_at.to_string(), tz)?;
            let core_task = CoreTask {
                id: planner.tasks().len(),
                start: start_opt,
                end,
                cost_estimate: NormalDist::from_minutes(
                    Minutes(row.avg_minutes),
                    Minutes(row.sigma_minutes),
                ),
                depends: all_depends[i].clone(),
                parallel_mode: ParallelMode::from_bools(row.parallelizable, row.allows_parallel),
                abandonability: row.abandonability,
                fixed: row.fixed,
                habit_group: row
                    .habit_id
                    .as_ref()
                    .and_then(|hid| habit_group_map.get(hid).copied()),
            };
            planner
                .add(core_task)
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            id_map.push(row.id.clone());
            id_to_idx.insert(row.id.clone(), planner.tasks().len() - 1);
        }

        Ok((planner, id_map, id_to_idx))
    }

    fn plan_to_entries(
        &self,
        plan: &takusu_core::Plan,
        id_map: &[String],
    ) -> Result<Vec<ScheduleEntry>, AppError> {
        plan.schedules
            .iter()
            .map(|p| {
                Ok(ScheduleEntry {
                    task_id: id_map.get(p.task_id).cloned().unwrap_or_default(),
                    start_at: point_to_iso(p.start.0)?,
                    end_at: point_to_iso(p.end.0)?,
                })
            })
            .collect()
    }

    /// Preserve schedule entries for tasks that are excluded from the planner
    /// (e.g. `in_progress`) so that regenerating or rescheduling the schedule
    /// does not wipe out their schedule info (#354).
    ///
    /// `new_entries` is the freshly computed schedule. `existing_entries` is
    /// the previous schedule. For each task whose status is in `statuses` and
    /// that is not already present in `new_entries`, its previous entry is
    /// carried over verbatim.
    async fn preserve_active_entries(
        &self,
        mut new_entries: Vec<ScheduleEntry>,
        existing_entries: &[ScheduleEntry],
        statuses: &[&str],
    ) -> Result<Vec<ScheduleEntry>, AppError> {
        if existing_entries.is_empty() {
            return Ok(new_entries);
        }
        let mut preserve_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for status in statuses {
            let rows = self
                .storage
                .list_tasks(&TaskQuery {
                    status: Some((*status).to_string()),
                    ..Default::default()
                })
                .await
                .map_err(storage_to_app)?;
            for row in rows {
                preserve_ids.insert(row.id);
            }
        }
        if preserve_ids.is_empty() {
            return Ok(new_entries);
        }
        let new_ids: std::collections::HashSet<String> =
            new_entries.iter().map(|e| e.task_id.clone()).collect();
        for entry in existing_entries {
            if preserve_ids.contains(&entry.task_id) && !new_ids.contains(&entry.task_id) {
                new_entries.push(entry.clone());
            }
        }
        Ok(new_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_to_point_with_offset() {
        let tz = jiff::tz::TimeZone::UTC;
        // オフセット付きはそのままパースできる
        let p = iso_to_point("2026-07-04T10:00:00Z", &tz).unwrap();
        let p2 = iso_to_point("2026-07-04T19:00:00+09:00", &tz).unwrap();
        assert_eq!(p.0, p2.0); // 同一時刻
    }

    #[test]
    fn iso_to_point_naive_falls_back_to_tz() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        // オフセット無しの naive 日時は tz で解釈される
        let naive = iso_to_point("2026-07-04T10:00:00", &tz).unwrap();
        let with_offset = iso_to_point("2026-07-04T10:00:00+09:00", &tz).unwrap();
        assert_eq!(naive.0, with_offset.0);
    }

    #[test]
    fn iso_to_point_now() {
        let tz = jiff::tz::TimeZone::UTC;
        let _ = iso_to_point("now", &tz).unwrap();
    }

    #[test]
    fn iso_to_point_date_only_end_of_day() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        let p = iso_to_point("2026-07-04", &tz).unwrap();
        let p2 = iso_to_point("2026-07-04T23:59:59+09:00", &tz).unwrap();
        assert_eq!(p.0, p2.0);
    }

    // ── parse_timezone accepts IANA and fixed-offset timezones (#607) ────

    #[test]
    fn parse_timezone_accepts_iana_and_fixed_offset() {
        assert!(parse_timezone("Asia/Tokyo").is_ok());
        assert!(parse_timezone("UTC").is_ok());
        assert!(parse_timezone("+09:00").is_ok());
        assert!(parse_timezone("-05:30").is_ok());
        assert!(parse_timezone("+0900").is_ok());
        assert!(parse_timezone("+09").is_ok());
        assert!(parse_timezone("not/a/tz").is_err());
    }

    #[test]
    fn parse_timezone_rejects_excessive_offset() {
        // UTC±14 is the widest real-world offset.
        assert!(parse_timezone("+14:00:00").is_ok());
        assert!(parse_timezone("-14:00:00").is_ok());
        assert!(parse_timezone("+14:00:01").is_err());
        assert!(parse_timezone("+24:00:00").is_err());
        assert!(parse_timezone("+25:59:59").is_err());
        assert!(parse_timezone("+26:00:00").is_err());
    }

    // ── iso_to_local_date naive fallback (#348) ─────────────────────────

    #[test]
    fn iso_to_local_date_with_offset() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        // 20:00 UTC = 05:00 JST next day
        let d = iso_to_local_date("2026-07-06T20:00:00Z", &tz);
        assert_eq!(d, "2026-07-07");
    }

    #[test]
    fn iso_to_local_date_naive_interprets_in_tz() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        // Naive datetime should be interpreted in the configured tz, so the
        // local date is the same date as the naive string (no offset shift).
        let d = iso_to_local_date("2026-07-06T20:00:00", &tz);
        assert_eq!(d, "2026-07-06");
    }

    #[test]
    fn iso_to_local_date_naive_matches_offset_version() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        // A naive datetime interpreted in tz should yield the same local
        // date as the same wall-clock time with the tz's offset.
        let naive = iso_to_local_date("2026-07-06T20:00:00", &tz);
        let with_offset = iso_to_local_date("2026-07-06T20:00:00+09:00", &tz);
        assert_eq!(naive, with_offset);
    }

    #[test]
    fn iso_to_local_date_date_only_fallback() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        // Pure date string (no time) → first 10 chars as before.
        let d = iso_to_local_date("2026-07-06", &tz);
        assert_eq!(d, "2026-07-06");
    }

    // ── validate_minutes bounds (#604) ────────────────────────────────

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

    // ── point_to_iso / point_to_local_date overflow (#608) ─────────────

    #[test]
    fn point_to_iso_overflow_returns_err() {
        assert!(point_to_iso(i64::MAX).is_err());
        assert!(point_to_iso(i64::MIN).is_err());
    }

    #[test]
    fn point_to_local_date_overflow_returns_err() {
        let tz = jiff::tz::TimeZone::UTC;
        assert!(point_to_local_date(i64::MAX, &tz).is_err());
        assert!(point_to_local_date(i64::MIN, &tz).is_err());
    }

    // Regression (#780): parse_sleep must reject invalid HH:MM strings.
    // parse_hhmm currently swallows parse errors and does not validate ranges,
    // so custom sleep strings like "22:70-06:00" are accepted silently.
    #[test]
    fn regression_parse_sleep_rejects_invalid_hhmm() {
        let tz = jiff::tz::TimeZone::UTC;
        let settings = default_settings_row();

        // Minutes out of range and hours out of range should both error.
        assert!(
            parse_sleep("22:70-06:00", &settings, &tz).is_err(),
            "custom sleep with invalid minutes should be rejected"
        );
        assert!(
            parse_sleep("22:00-25:00", &settings, &tz).is_err(),
            "custom sleep with invalid hours should be rejected"
        );
        assert!(
            parse_sleep("22:00-06:00", &settings, &tz).is_ok(),
            "valid custom sleep should still be accepted"
        );
    }

    // ── validate_task_datetimes (#934) ─────────────────────────────────

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
