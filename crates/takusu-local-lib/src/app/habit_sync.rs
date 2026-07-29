//! Habit → task synchronisation (#11).
//!
//! Extracted from the `app.rs` god module. Holds the habit-to-`CoreTask`
//! builders (`step_to_core_task`, `build_habit_core`, …), the preview/estimate
//! helpers shared with `TakusuApp::preview_habit`, and the
//! `sync_habit_tasks` method that materialises habit occurrences into task
//! rows.

use std::collections::HashMap;

use jiff::Timestamp;
use takusu_core::{Minutes, NormalDist, ParallelMode, Point, Slots, Task as CoreTask};
use takusu_storage::{
    CreateTask, HabitPreviewRequest, HabitPreviewTask, HabitRow, HabitStepInput, HabitStepRow,
    TaskQuery, TaskRow, UpdateTask,
};
use takusu_util::{Abandonability, TaskStatus, WindowMode};

use super::dependency::topo_sort_steps;
use super::schedule::{iso_to_local_date, point_to_iso, point_to_local_date};
use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind};
use crate::validate::parse_recurrence;

/// Build a `CoreTask` for a single step occurrence (#95). The step's window is
/// derived from the occurrence date (taken from `occ_start`) combined with the
/// step's `start_time`/`end_time`. For fixed steps the deadline is the window
/// length (end_time - start_time); otherwise it is `avg_minutes`.
pub(super) fn step_to_core_task(
    step: &HabitStepRow,
    occ_start: Point,
    tz: &jiff::tz::TimeZone,
) -> Result<CoreTask, AppError> {
    let date = takusu_habit::point_to_date(occ_start, tz)
        .ok_or_else(|| AppError::Internal("occurrence date out of range".into()))?;
    let (sh, sm) = (step.start_time.hour(), step.start_time.minute());
    let start_time = takusu_habit::TimeOfDay::new(sh, sm).ok_or_else(|| {
        AppError::BadRequest(BadRequestKind::InvalidTime(format!(
            "invalid step start_time: {}",
            step.start_time
        )))
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
pub(super) fn step_to_core_task_period(
    step: &HabitStepRow,
    window_start: Point,
    deadline: Point,
) -> CoreTask {
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

pub(super) fn step_input_to_preview_row(input: &HabitStepInput) -> HabitStepRow {
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
        depends_on: takusu_util::DependencyList::new(input.depends_on.clone()),
        created_at: takusu_util::Timestamp::default(),
    }
}

pub(super) fn core_task_to_preview(core: &CoreTask, title: &str) -> HabitPreviewTask {
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
pub(super) fn freq_fallback_slots(rule: &takusu_habit::RecurrenceRule) -> i64 {
    let interval = rule.interval.max(1) as i64;
    let days = match rule.freq {
        takusu_habit::Frequency::Daily => interval,
        takusu_habit::Frequency::Weekly => interval * 7,
        takusu_habit::Frequency::Monthly => interval * 30,
        takusu_habit::Frequency::Yearly => interval * 365,
    };
    days * 288 // 288 slots per day (5-min slots)
}

pub(super) fn habit_row_to_config(
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

pub(super) fn build_habit_from_preview(
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
    let start_time = takusu_habit::TimeOfDay::new(sh, sm).ok_or_else(|| {
        AppError::BadRequest(BadRequestKind::InvalidTime(format!(
            "invalid start_time: {start_time}"
        )))
    })?;
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

impl super::TakusuApp {
    /// スケジュール生成対象のタスクをロード。
    ///
    /// - task_ids 指定時: 指定された ID のタスクのみ取得。
    ///   存在しない ID は無視される (ユーザーが削除済みのタスクを指定した場合など)。
    ///   これは意図的な設計: 指定 ID の一部が消失しても生成を継続する。
    ///   ただし、ユーザーはどの ID が無視されたか通知されないため、
    ///   API レベルで警告を返す余地がある。
    /// - task_ids なし: 全タスクから pending/scheduled のみをフィルタ。
    pub(super) async fn load_task_rows(
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
    pub(super) fn merge_active_tasks(
        habit_rows: Vec<TaskRow>,
        task_rows: Vec<TaskRow>,
    ) -> Vec<TaskRow> {
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
                        avg_minutes: Some(
                            Slots(core_task.cost_estimate.avg() as i64).to_minutes().0,
                        ),
                        sigma_minutes: Some(
                            Slots(core_task.cost_estimate.sigma() as i64).to_minutes().0,
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
                    avg_minutes: Slots(core_task.cost_estimate.avg() as i64).to_minutes().0,
                    sigma_minutes: Some(
                        Slots(core_task.cost_estimate.sigma() as i64).to_minutes().0,
                    ),
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
                let deps: Vec<String> = step.depends_on.to_vec();
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
}
