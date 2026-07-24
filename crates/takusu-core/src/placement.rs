//! 配置 primitives（anneal.rs と decoder.rs で共有）。

use std::cell::RefCell;

use super::*;

pub type Placement = (Point, Point, usize);

thread_local! {
    /// `day_load_with_candidate` 用の scratch buffer。
    /// 1 日あたりの区間をマージする際の allocate を避ける。
    static DAY_INTERVALS: RefCell<Vec<(Point, Point)>> = RefCell::new(Vec::with_capacity(64));
    /// `evaluate_insertion` 用の scratch buffer。
    /// 候補スケジュールを `evaluate` に渡す際の allocate を避ける。
    pub static INSERTION_PLAN: RefCell<Vec<Placement>> = RefCell::new(Vec::with_capacity(64));
    /// `evaluate_insertion` 用の `evaluate_with_scratch` buffer。
    /// 候補評価のたびに sorted / index / habit_entries を allocate するのを避ける。
    pub static INSERTION_SORTED: RefCell<Vec<Placement>> = RefCell::new(Vec::with_capacity(64));
    pub static INSERTION_INDEX: RefCell<Vec<Option<(Point, Point)>>> = RefCell::new(Vec::with_capacity(64));
    pub static INSERTION_HABIT: RefCell<Vec<(usize, i64)>> = RefCell::new(Vec::with_capacity(64));
}

// ── placement primitives (shared with anneal.rs) ───────────────────────

pub(crate) fn compute_earliest(planner: &Planner, schedules: &[Placement], task: &Task) -> Point {
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
        if let Some((_, dep_end, _)) = schedules.iter().find(|(_, _, id)| id == dep_id) {
            earliest = earliest.max(*dep_end);
        }
    }
    earliest
}

/// `compute_earliest` の index 版。依存先の終了時刻を O(1) で参照する。
/// decode のホットパスで schedules の線形走査を避ける。
pub(crate) fn compute_earliest_indexed(
    planner: &Planner,
    index: &[Option<(Point, Point)>],
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
        if let Some(Some((_, dep_end))) = index.get(*dep_id) {
            earliest = earliest.max(*dep_end);
        }
    }
    earliest
}

/// `[start, end)` と重なる睡眠窓があれば、その窓の終端スロットを返す。
fn sleep_window_conflict(planner: &Planner, start: i64, end: i64) -> Option<i64> {
    let sleep = &planner.sleep;
    if !sleep.enabled {
        return None;
    }
    let slots_per_day: i64 = (24 * 60) / planner.per as i64;
    let mut day = sleep.day_start
        + (start - sleep.day_start).div_euclid(slots_per_day) * slots_per_day
        - slots_per_day;
    while day + sleep.start < end {
        let w_start = day + sleep.start;
        let w_end = day + sleep.end;
        if w_start < end && w_end > start {
            return Some(w_end);
        }
        day += slots_per_day;
    }
    None
}

fn slots_per_day(planner: &Planner) -> i64 {
    (24 * 60) / planner.per as i64
}

fn day_start_for(planner: &Planner, p: Point) -> Point {
    let spd = slots_per_day(planner);
    let base = planner.sleep.day_start;
    Point(base + (p.0 - base).div_euclid(spd) * spd)
}

pub(crate) fn next_day_start(planner: &Planner, p: Point) -> Point {
    day_start_for(planner, p) + slots_per_day(planner)
}

/// 指定日に candidate を追加した場合の union 負荷を計算する。
/// 並列タスクの二重加算を避けるため、interval の merge を行う。
fn day_load_with_candidate(
    schedules: &[Placement],
    candidate: (Point, Point),
    day_start: Point,
    day_end: Point,
) -> i64 {
    DAY_INTERVALS.with(|v| {
        let mut intervals = v.borrow_mut();
        intervals.clear();
        intervals.push((candidate.0.max(day_start), candidate.1.min(day_end)));
        for (s, e, _) in schedules {
            if s.0 < day_end.0 && e.0 > day_start.0 {
                intervals.push((*s.max(&day_start), *e.min(&day_end)));
            }
        }
        intervals.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut total = 0i64;
        let mut cur: Option<(Point, Point)> = None;
        for (s, e) in intervals.iter().copied() {
            if let Some((cs, ce)) = cur {
                if s.0 <= ce.0 {
                    cur = Some((cs, Point(ce.0.max(e.0))));
                } else {
                    total += ce.0 - cs.0;
                    cur = Some((s, e));
                }
            } else {
                cur = Some((s, e));
            }
        }
        if let Some((cs, ce)) = cur {
            total += ce.0 - cs.0;
        }
        total
    })
}

/// 与えられた時刻が属する日の、既存スケジュールの最大終了時刻を返す。
/// cursor 以降にタスクがなければ cursor を返す。
pub(crate) fn max_end_in_day(planner: &Planner, schedules: &[Placement], cursor: Point) -> Point {
    let spd = slots_per_day(planner);
    let day_start = day_start_for(planner, cursor);
    let day_end = day_start + spd;
    let max_end = schedules
        .iter()
        .filter(|(s, e, _)| s.0 < day_end.0 && e.0 > day_start.0 && e.0 > cursor.0)
        .map(|(_, e, _)| e.0)
        .max()
        .unwrap_or(cursor.0);
    Point(max_end)
}

pub(crate) fn capacity_exceeded_for(
    planner: &Planner,
    schedules: &[Placement],
    start: Point,
    end: Point,
) -> bool {
    let max = planner.workload.maximum_slots_per_day;
    if max == 0 {
        return false;
    }
    let spd = slots_per_day(planner);
    let mut day = day_start_for(planner, start);
    while day.0 < end.0 {
        let day_end = day + spd;
        let load = day_load_with_candidate(schedules, (start, end), day, day_end);
        if load > max {
            return true;
        }
        day = day_end;
    }
    false
}

pub(crate) fn try_place<const CHECK_CAPACITY: bool>(
    planner: &Planner,
    schedules: &[Placement],
    task: &Task,
    earliest: Point,
    dur: i64,
    latest_end: Option<Point>,
) -> Result<(Point, Point), PlacementFailure> {
    if dur <= 0 {
        return Err(PlacementFailure::NoLegalSlot);
    }
    let awake_len = if planner.sleep.enabled {
        (24 * 60) / planner.per as i64 - (planner.sleep.end - planner.sleep.start)
    } else {
        i64::MAX
    };
    let avoid_sleep = dur <= awake_len;
    let mut cursor = earliest;
    let mut guard = 0u32;

    loop {
        guard += 1;
        if guard > 10_000 {
            return Err(PlacementFailure::NoLegalSlot);
        }
        let candidate_end = Point(cursor.0 + dur);

        if candidate_end.0 > task.end.0 {
            return Err(PlacementFailure::DeadlineExceeded);
        }

        if let Some(limit) = latest_end
            && candidate_end.0 > limit.0
        {
            return Err(PlacementFailure::LatestEndExceeded);
        }

        if CHECK_CAPACITY && capacity_exceeded_for(planner, schedules, cursor, candidate_end) {
            return Err(PlacementFailure::DailyCapacityExceeded);
        }

        if avoid_sleep
            && let Some(w_end) = sleep_window_conflict(planner, cursor.0, candidate_end.0)
        {
            // sleep を避けた先が latest_end / deadline を超える場合、
            // 実際の失敗原因を SleepConflict ではなく正しく報告する。
            let next_end = w_end + dur;
            if let Some(limit) = latest_end
                && next_end > limit.0
            {
                return Err(PlacementFailure::LatestEndExceeded);
            }
            if next_end > task.end.0 {
                return Err(PlacementFailure::DeadlineExceeded);
            }
            cursor = Point(w_end);
            continue;
        }

        let can_parallel = task.parallelizable;
        let can_host = task.allows_parallel;
        let mut has_overlap = false;
        let mut all_hosting = true;
        let mut all_guesting = true;
        let mut next_start = cursor.0;

        for (s, e, oid) in schedules {
            if s.0 < candidate_end.0 && e.0 > cursor.0 {
                has_overlap = true;
                if can_parallel && !planner.tasks[*oid].allows_parallel {
                    all_hosting = false;
                }
                if can_host && !planner.tasks[*oid].parallelizable {
                    all_guesting = false;
                }
                if e.0 > next_start {
                    next_start = e.0;
                }
            }
        }

        if !has_overlap {
            return Ok((cursor, candidate_end));
        }

        if can_parallel && all_hosting {
            return Ok((cursor, candidate_end));
        }
        if can_host && all_guesting {
            return Ok((cursor, candidate_end));
        }

        // 重複区間があれば e.0 > cursor.0 なので next_start は cursor より大きい。
        debug_assert!(next_start > cursor.0);
        if let Some(limit) = latest_end
            && next_start >= limit.0
        {
            return Err(PlacementFailure::LatestEndExceeded);
        }
        if next_start + dur > task.end.0 {
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
