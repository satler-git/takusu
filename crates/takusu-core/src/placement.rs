//! 配置 primitives（anneal.rs と decoder.rs で共有）。

use std::cell::RefCell;

use super::*;
use takusu_types::Slots;

/// `Vec<Option<TimeWindow>>` 形式の task_id → TimeWindow 索引から、
/// 指定 task_id の `TimeWindow` を借用で取得する。
/// `task_id` が範囲外、または未配置なら `None`。
#[inline]
pub(crate) fn get_time_window(index: &[Option<TimeWindow>], task_id: usize) -> Option<&TimeWindow> {
    index.get(task_id).and_then(|x| x.as_ref())
}

/// #306: habit グループの anchor エントリ。
/// `group` は `Task.habit_group`、`tod` は開始スロットの日付成分を除去した時刻帯
/// (`start_slot % slots_per_day`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HabitGroupAnchor {
    pub group: usize,
    pub tod: i64,
}

thread_local! {
    /// `evaluate_insertion` 用の scratch buffer。
    /// 候補スケジュールを `evaluate` に渡す際の allocate を避ける。
    pub static INSERTION_PLAN: RefCell<Vec<TaskPlacement>> = RefCell::new(Vec::with_capacity(64));
    /// `evaluate_insertion` 用の `evaluate_with_scratch` buffer。
    /// 候補評価のたびに sorted / index / habit_entries を allocate するのを避ける。
    pub static INSERTION_SORTED: RefCell<Vec<TaskPlacement>> = RefCell::new(Vec::with_capacity(64));
    pub static INSERTION_INDEX: RefCell<Vec<Option<TimeWindow>>> = RefCell::new(Vec::with_capacity(64));
    pub static INSERTION_HABIT: RefCell<Vec<HabitGroupAnchor>> = RefCell::new(Vec::with_capacity(64));
}

// ── capacity-check mode ────────────────────────────────────────────────

/// 容量チェックモード。
///
/// - `True(scratch)` — 容量チェックあり。`scratch` は呼び出し側が所有し、
///   `day_load_with_candidate` の区間マージ用に再利用される。
/// - `False` — 容量チェックなし。バッファ不要（アロケーションなし）。
pub(crate) enum CapacityMode<'a> {
    True(&'a mut Vec<TimeWindow>),
    False,
}

// ── placement primitives (shared with anneal.rs) ───────────────────────

pub(crate) fn compute_earliest(
    planner: &Planner,
    schedules: &[TaskPlacement],
    task: &Task,
) -> Point {
    // 固定タスクは start があれば now 以前の配置も許可する (学校など)。
    // start がない固定タスクは通常タスクと同様に now から配置する。
    let mut earliest = if task.fixed && task.start.is_some() {
        Point(i64::MIN)
    } else {
        planner.now
    };
    if let Some(start) = task.start {
        earliest = earliest.max(start);
    }
    for dep_id in &task.depends {
        if let Some(p) = schedules.iter().find(|p| p.task_id == *dep_id) {
            earliest = earliest.max(p.end);
        }
    }
    earliest
}

/// `compute_earliest` の index 版。依存先の終了時刻を O(1) で参照する。
/// decode のホットパスで schedules の線形走査を避ける。
pub(crate) fn compute_earliest_indexed(
    planner: &Planner,
    index: &[Option<TimeWindow>],
    task: &Task,
) -> Point {
    let mut earliest = if task.fixed && task.start.is_some() {
        Point(i64::MIN)
    } else {
        planner.now
    };
    if let Some(start) = task.start {
        earliest = earliest.max(start);
    }
    for dep_id in &task.depends {
        if let Some(tw) = get_time_window(index, *dep_id) {
            earliest = earliest.max(tw.end);
        }
    }
    earliest
}

/// `[start, end)` と重なる睡眠窓があれば、その窓の終端スロットを返す。
fn sleep_window_conflict(planner: &Planner, start: Point, end: Point) -> Option<Point> {
    let sleep = &planner.sleep;
    if !sleep.enabled() {
        return None;
    }
    let spd: i64 = (24 * 60) / planner.per as i64;
    let base = sleep.day_start();
    let mut day = base + (start.0 - base).div_euclid(spd) * spd - spd;
    while day + sleep.start() < end.0 {
        let w_start = day + sleep.start();
        let w_end = day + sleep.end();
        if w_start < end.0 && w_end > start.0 {
            return Some(Point(w_end));
        }
        day += spd;
    }
    None
}

fn slots_per_day(planner: &Planner) -> i64 {
    (24 * 60) / planner.per as i64
}

fn day_start_for(planner: &Planner, p: Point) -> Point {
    let spd = slots_per_day(planner);
    let base = planner.sleep.day_start();
    Point(base) + Slots((p.0 - base).div_euclid(spd) * spd)
}

pub(crate) fn next_day_start(planner: &Planner, p: Point) -> Point {
    day_start_for(planner, p) + Slots(slots_per_day(planner))
}

/// 指定日に candidate を追加した場合の union 負荷を計算する。
/// 並列タスクの二重加算を避けるため、interval の merge を行う。
///
/// `scratch` は呼び出し側が管理する再利用バッファ（`evaluate_with_scratch` と同じ方針）。
/// 関数内で clear → push → sort して使う。
fn day_load_with_candidate(
    schedules: &[TaskPlacement],
    candidate: TimeWindow,
    day_start: Point,
    day_end: Point,
    scratch: &mut Vec<TimeWindow>,
) -> i64 {
    scratch.clear();
    scratch.push(TimeWindow::new(
        candidate.start.max(day_start),
        candidate.end.min(day_end),
    ));
    for p in schedules {
        if p.start.0 < day_end.0 && p.end.0 > day_start.0 {
            scratch.push(TimeWindow::new(p.start.max(day_start), p.end.min(day_end)));
        }
    }
    scratch.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    let mut total = 0i64;
    let mut cur: Option<TimeWindow> = None;
    for tw in scratch.iter().copied() {
        if let Some(c) = cur {
            if tw.start.0 <= c.end.0 {
                cur = Some(TimeWindow::new(c.start, Point(c.end.0.max(tw.end.0))));
            } else {
                total += (c.end - c.start).0;
                cur = Some(tw);
            }
        } else {
            cur = Some(tw);
        }
    }
    if let Some(c) = cur {
        total += (c.end - c.start).0;
    }
    total
}

/// 与えられた時刻が属する日の、既存スケジュールの最大終了時刻を返す。
/// cursor 以降にタスクがなければ cursor を返す。
pub(crate) fn max_end_in_day(
    planner: &Planner,
    schedules: &[TaskPlacement],
    cursor: Point,
) -> Point {
    let spd = slots_per_day(planner);
    let day_start = day_start_for(planner, cursor);
    let day_end = day_start + Slots(spd);
    let max_end = schedules
        .iter()
        .filter(|p| p.start.0 < day_end.0 && p.end.0 > day_start.0 && p.end.0 > cursor.0)
        .map(|p| p.end.0)
        .max()
        .unwrap_or(cursor.0);
    Point(max_end)
}

pub(crate) fn capacity_exceeded_for(
    planner: &Planner,
    schedules: &[TaskPlacement],
    start: Point,
    end: Point,
    scratch: &mut Vec<TimeWindow>,
) -> bool {
    let max = planner.workload.maximum_slots_per_day();
    if max == 0 {
        return false;
    }
    let spd = slots_per_day(planner);
    let mut day = day_start_for(planner, start);
    while day.0 < end.0 {
        let day_end = day + Slots(spd);
        let load = day_load_with_candidate(
            schedules,
            TimeWindow::new(start, end),
            day,
            day_end,
            scratch,
        );
        if load > max {
            return true;
        }
        day = day_end;
    }
    false
}

pub(crate) fn try_place(
    planner: &Planner,
    schedules: &[TaskPlacement],
    task: &Task,
    earliest: Point,
    dur: Slots,
    latest_end: Option<Point>,
    capacity: CapacityMode<'_>,
) -> Result<TimeWindow, PlacementFailure> {
    if dur.0 <= 0 {
        return Err(PlacementFailure::NoLegalSlot);
    }
    let awake_len = if planner.sleep.enabled() {
        (24 * 60) / planner.per as i64 - (planner.sleep.end() - planner.sleep.start())
    } else {
        i64::MAX
    };
    let avoid_sleep = dur.0 <= awake_len;
    let mut cursor = earliest;
    let mut guard = 0u32;
    let mut capacity = capacity;

    loop {
        guard += 1;
        if guard > 10_000 {
            return Err(PlacementFailure::NoLegalSlot);
        }
        let candidate_end = cursor + dur;

        if candidate_end.0 > task.end.0 {
            return Err(PlacementFailure::DeadlineExceeded);
        }

        if let Some(limit) = latest_end
            && candidate_end.0 > limit.0
        {
            return Err(PlacementFailure::LatestEndExceeded);
        }

        if let CapacityMode::True(scratch) = &mut capacity
            && capacity_exceeded_for(planner, schedules, cursor, candidate_end, scratch)
        {
            return Err(PlacementFailure::DailyCapacityExceeded);
        }

        if avoid_sleep && let Some(w_end) = sleep_window_conflict(planner, cursor, candidate_end) {
            // sleep を避けた先が latest_end / deadline を超える場合、
            // 実際の失敗原因を SleepConflict ではなく正しく報告する。
            let next_end = w_end + dur;
            if let Some(limit) = latest_end
                && next_end > limit
            {
                return Err(PlacementFailure::LatestEndExceeded);
            }
            if next_end > task.end {
                return Err(PlacementFailure::DeadlineExceeded);
            }
            cursor = w_end;
            continue;
        }

        let can_parallel = task.parallel_mode.is_guest();
        let can_host = task.parallel_mode.is_host();
        let mut has_overlap = false;
        let mut all_hosting = true;
        let mut all_guesting = true;
        let mut next_start = cursor.0;

        for p in schedules {
            if p.start.0 < candidate_end.0 && p.end.0 > cursor.0 {
                has_overlap = true;
                if can_parallel && !planner.tasks[p.task_id].parallel_mode.is_host() {
                    all_hosting = false;
                }
                if can_host && !planner.tasks[p.task_id].parallel_mode.is_guest() {
                    all_guesting = false;
                }
                if p.end.0 > next_start {
                    next_start = p.end.0;
                }
            }
        }

        if !has_overlap {
            return Ok(TimeWindow::new(cursor, candidate_end));
        }

        if can_parallel && all_hosting {
            return Ok(TimeWindow::new(cursor, candidate_end));
        }
        if can_host && all_guesting {
            return Ok(TimeWindow::new(cursor, candidate_end));
        }

        // 重複区間があれば p.end.0 > cursor.0 なので next_start は cursor より大きい。
        debug_assert!(next_start > cursor.0);
        if let Some(limit) = latest_end
            && next_start >= limit.0
        {
            return Err(PlacementFailure::LatestEndExceeded);
        }
        if next_start + dur.0 > task.end.0 {
            return Err(PlacementFailure::DeadlineExceeded);
        }
        cursor = Point(next_start);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementFailure {
    DependencyCycle,
    NoLegalSlot,
    InvalidPriority,
    InvalidDependency,
    LatestEndExceeded,
    DailyCapacityExceeded,
    DeadlineExceeded,
}
