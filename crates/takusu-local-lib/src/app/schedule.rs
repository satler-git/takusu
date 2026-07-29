//! Schedule generation, preview, and rescheduling (#11).
//!
//! Extracted from the `app.rs` god module. Holds the planner slot ↔ ISO
//! datetime conversion helpers, the `PlannerConfig` builder, the schedule
//! I/O structs, and the `TakusuApp::generate_schedule` / `preview_schedule` /
//! `reschedule` / `move_entry` / `clear_schedule` methods plus their private
//! planner-construction helpers.

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use jiff::Timestamp;
use serde::Serialize;
use takusu_core::{Planner, PlannerConfig, Point, RescheduleRange, SleepConfig, TaskPlacement};
use takusu_storage::{
    SaveScheduleRequest, ScheduleEntry, ScheduleRow, SettingsRow, TaskQuery, TaskRow,
};
use takusu_util::{ScheduleMode, SleepInput, TaskStatus};

use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind, ConflictKind};
use crate::validate::{SettingsPlannerExt, parse_recurrence, parse_settings_timezone};

/// ISO文字列 → Point スロット値。`now` は現在時刻。
/// ハードコードされた 5 (分/スロット) は Planner の per と揃っている必要がある。
/// `.devin/docs/code-style.md` の「point_to_iso hardcoded 5-minute slots」参照。
/// 変更時は takusu-core, takusu-local-lib, google-cal など全 crate の
/// 5分前提コードを同時に更新すること。
///
/// `tz` はオフセット無しの naive な日時文字列を解釈する際のフォールバック
/// タイムゾーン。過去にモバイルアプリがオフセットを削除した文字列を保存して
/// しまった場合などに救済する。
pub(super) fn iso_to_point(iso: &str, tz: &jiff::tz::TimeZone) -> Result<Point, AppError> {
    let ts = takusu_util::parse_datetime_to_timestamp(iso, tz).map_err(|e| {
        AppError::BadRequest(BadRequestKind::InvalidTime(format!(
            "invalid datetime: {e}"
        )))
    })?;
    Ok(Point::from_timestamp(ts, 5))
}

pub(super) fn point_to_iso(slot: i64) -> Result<takusu_util::Timestamp, AppError> {
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
pub(super) fn point_to_local_date(slot: i64, tz: &jiff::tz::TimeZone) -> Result<String, AppError> {
    let secs = slot
        .checked_mul(5 * 60)
        .ok_or_else(|| AppError::Internal("timestamp overflow".into()))?;
    let ts = Timestamp::from_second(secs)
        .map_err(|e| AppError::Internal(format!("invalid timestamp: {e}")))?;
    Ok(ts.to_zoned(tz.clone()).date().to_string())
}

/// ISO 文字列 → ローカルタイムゾーンの日付文字列 (YYYY-MM-DD)。
/// `task.start_at` (UTC ISO 文字列) からローカル日付を得るために使う。
pub(super) fn iso_to_local_date(iso: &str, tz: &jiff::tz::TimeZone) -> String {
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

/// #772: settings の solver / time budget / seed / warm start / workload を
/// `PlannerConfig` に反映する。
fn planner_config(start: Point, sleep: SleepConfig, settings: &SettingsRow) -> PlannerConfig {
    PlannerConfig {
        workload: settings.workload_config(),
        solver: settings.solver.into(),
        time_budget: settings
            .time_budget_ms
            .filter(|&ms| ms > 0)
            .map(|ms| Duration::from_millis(ms as u64)),
        seed: settings.seed.filter(|&s| s >= 0).map(|s| s as u64),
        warm_start: settings.warm_start,
        ..PlannerConfig::new(start, sleep)
    }
}

#[derive(Debug, Clone)]
pub struct GenerateScheduleInput {
    pub task_ids: Option<Vec<String>>,
    pub sleep: SleepInput,
}

#[derive(Debug, Clone)]
pub struct RescheduleInput {
    pub mode: ScheduleMode,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    pub pinned: Vec<String>,
    pub sleep: SleepInput,
}

#[derive(Debug, Clone)]
pub struct SchedulePreviewInput {
    pub mode: ScheduleMode,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    pub pinned: Vec<String>,
    pub sleep: SleepInput,
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

impl super::TakusuApp {
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
        let sleep = settings.sleep_config(&input.sleep, &tz)?;
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
        // unwrap_or_default: if the schedule is empty, fall back to
        // an empty vec which disables the stability penalty rather than
        // crashing. This is intentionally more forgiving than reschedule
        // (which returns an error on parse failure) because generate is a
        // full regenerate — the user just wants a new schedule.
        let existing_entries: Vec<ScheduleEntry> = existing_schedule
            .as_ref()
            .map(|row| row.schedule.as_inner().clone())
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
            .preserve_active_entries(entries, &existing_entries, &[TaskStatus::InProgress])
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
        let sleep = settings.sleep_config(&input.sleep, &tz)?;
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
            .map(|row| row.schedule.as_inner().clone())
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
        let plan = match input.mode {
            ScheduleMode::Full => {
                if !current_schedule.is_empty() {
                    planner.set_previous_schedule(&current_schedule);
                }
                planner.plan()
            }
            ScheduleMode::Tasks => {
                if !current_schedule.is_empty() {
                    planner.set_previous_schedule(&current_schedule);
                }
                let task_ids = input.task_ids.as_ref().ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other(
                        "task_ids is required for tasks mode".into(),
                    ))
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
            ScheduleMode::Range => {
                let from_str = input.from.as_ref().ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other(
                        "from is required for range mode".into(),
                    ))
                })?;
                let until_str = input.until.as_ref().ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other(
                        "until is required for range mode".into(),
                    ))
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
        let sleep = settings.sleep_config(&input.sleep, &tz)?;
        let now_point = Point::from_timestamp(Timestamp::now(), 5);

        let schedule_row = self
            .storage
            .get_schedule()
            .await
            .map_err(storage_to_app)?
            .ok_or_else(|| AppError::NotFound("no active schedule".into()))?;
        let entries: Vec<ScheduleEntry> = schedule_row.schedule.as_inner().clone();

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

        let plan = match input.mode {
            ScheduleMode::Full => {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "full mode is not supported for reschedule; use generate_schedule instead"
                        .into(),
                )));
            }
            ScheduleMode::Range => {
                let from_str = input.from.as_ref().ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other(
                        "from is required for range mode".into(),
                    ))
                })?;
                let until_str = input.until.as_ref().ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other(
                        "until is required for range mode".into(),
                    ))
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
            ScheduleMode::Tasks => {
                let task_ids = input.task_ids.as_ref().ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other(
                        "task_ids is required for tasks mode".into(),
                    ))
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
        };

        let mut final_entries = self.plan_to_entries(&plan, &id_map)?;
        // #354: in_progress タスクは planner の対象外なので、再スケジュール時も
        // 進行中タスクのエントリが消えないよう前回スケジュールから引き継ぐ。
        final_entries = self
            .preserve_active_entries(final_entries, &entries, &[TaskStatus::InProgress])
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
        let mut entries: Vec<ScheduleEntry> = schedule_row.schedule.as_inner().clone();
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
            return Err(AppError::Conflict(ConflictKind::ScheduleViolation));
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
            let dep_ids: Vec<String> = row.depends.to_vec();
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
            .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;

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
                if h.window_mode != takusu_util::WindowMode::Period {
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

        let mut planner = Planner::new(planner_config(start, sleep, settings));
        let mut id_map: Vec<String> = Vec::with_capacity(task_rows.len());

        for (i, row) in task_rows.iter().enumerate() {
            let start_opt = row
                .start_at
                .as_ref()
                .map(|s| iso_to_point(&s.to_string(), tz))
                .transpose()?;
            let end = iso_to_point(&row.end_at.to_string(), tz)?;
            let core_task = takusu_core::Task {
                id: planner.tasks().len(),
                start: start_opt,
                end,
                cost_estimate: takusu_core::NormalDist::from_minutes(
                    takusu_core::Minutes(row.avg_minutes),
                    takusu_core::Minutes(row.sigma_minutes),
                ),
                depends: all_depends[i].clone(),
                parallel_mode: takusu_core::ParallelMode::from_bools(
                    row.parallelizable,
                    row.allows_parallel,
                ),
                abandonability: row.abandonability,
                fixed: row.fixed,
                habit_group: row
                    .habit_id
                    .as_ref()
                    .and_then(|hid| habit_group_map.get(hid).copied()),
            };
            planner
                .add(core_task)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
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
        statuses: &[TaskStatus],
    ) -> Result<Vec<ScheduleEntry>, AppError> {
        if existing_entries.is_empty() {
            return Ok(new_entries);
        }
        let mut preserve_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for status in statuses {
            let rows = self
                .storage
                .list_tasks(&TaskQuery {
                    status: Some((*status).into()),
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
}
