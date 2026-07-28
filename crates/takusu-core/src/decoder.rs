//! # Priority decoder
//!
//! `priority` 順序と固定配置 `pinned` から `Plan` を貪欲に構築する。
//! 探索方法（SA/ALNS）を知らず、decoder は入力をデコードするだけ。
//!
//! Stage 1 では full / partial / range を `pinned` 集合の違いとして統一する。
//! fixed タスクは自動的に `pinned` へ追加される。

use super::evaluate::evaluate_with_scratch;
use super::*;
use crate::placement::*;

// ── decode API ───

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RepairMode {
    #[default]
    Earliest,
    LowestDelta,
    Regret2,
    Deadline,
    Habit,
    Stability,
    /// habit task を自分の `task.start` (希望時刻) へ配置する。同じ habit の
    /// member は同じ time-of-day を持つため、グループが一貫した anchor に揃う。
    /// 非 habit task は Earliest 同様。
    HabitAnchor,
}

pub struct DecodeInput<'a> {
    pub priority: &'a [usize],
    pub duration_choices: &'a [i64],
    pub pinned: &'a [Placement],
    pub repair_mode: RepairMode,
}

#[derive(Clone)]
pub struct DecodeResult {
    pub plan: Plan,
    pub diagnostics: DecodeDiagnostics,
    pub status: DecodeStatus,
}

#[derive(Debug, Default, Clone)]
pub struct DecodeDiagnostics {
    pub failures: Vec<PlacementFailure>,
    pub relaxed: Vec<RelaxedPlacement>,
    pub pinned_conflicts: Vec<PinnedConflict>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeStatus {
    Feasible,
    Relaxed,
    Infeasible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinnedConflict {
    InvalidInterval {
        task_id: usize,
        start: Point,
        end: Point,
    },
    Overlap {
        a: usize,
        b: usize,
    },
    StartViolation {
        task_id: usize,
        pinned_start: Point,
        task_start: Point,
    },
    DeadlineViolation {
        task_id: usize,
        pinned_end: Point,
        deadline: Point,
    },
    DuplicateId {
        task_id: usize,
    },
    DependencyViolation {
        task_id: usize,
        dep_id: usize,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct RelaxedPlacement {
    pub reason: PlacementFailure,
}

fn validate_and_collect_fixed(
    planner: &Planner,
    input: &DecodeInput<'_>,
    diagnostics: &mut DecodeDiagnostics,
) -> Vec<Placement> {
    let n = planner.tasks.len();
    let mut placements: Vec<Placement> = Vec::new();

    for p in input.pinned {
        let s = &p.start;
        let e = &p.end;
        let id = &p.task_id;
        if *id >= n {
            continue;
        }
        let task = &planner.tasks[*id];

        if s.0 >= e.0 {
            diagnostics
                .pinned_conflicts
                .push(PinnedConflict::InvalidInterval {
                    task_id: *id,
                    start: *s,
                    end: *e,
                });
            continue;
        }

        if let Some(task_start) = task.start
            && s.0 < task_start.0
        {
            diagnostics
                .pinned_conflicts
                .push(PinnedConflict::StartViolation {
                    task_id: *id,
                    pinned_start: *s,
                    task_start,
                });
        }

        if e.0 > task.end.0 {
            diagnostics
                .pinned_conflicts
                .push(PinnedConflict::DeadlineViolation {
                    task_id: *id,
                    pinned_end: *e,
                    deadline: task.end,
                });
        }

        if placements.iter().any(|p| &p.task_id == id) {
            diagnostics
                .pinned_conflicts
                .push(PinnedConflict::DuplicateId { task_id: *id });
            continue;
        }

        placements.push(TaskPlacement::new(*s, *e, *id));
    }

    for task in &planner.tasks {
        if !task.fixed {
            continue;
        }
        if let Some(start) = task.start {
            let dur = duration_for(task, input.duration_choices.get(task.id));
            let end = Point(start.0 + dur);

            if placements.iter().any(|p| p.task_id == task.id) {
                diagnostics
                    .pinned_conflicts
                    .push(PinnedConflict::DuplicateId { task_id: task.id });
                continue;
            }

            if end.0 > task.end.0 {
                diagnostics
                    .pinned_conflicts
                    .push(PinnedConflict::DeadlineViolation {
                        task_id: task.id,
                        pinned_end: end,
                        deadline: task.end,
                    });
            }

            placements.push(TaskPlacement::new(start, end, task.id));
        }
    }

    // pinned / fixed 同士の overlap
    for i in 0..placements.len() {
        let p1 = placements[i];
        for p2 in placements.iter().skip(i + 1).copied() {
            if p1.end.0 > p2.start.0 && p2.end.0 > p1.start.0 {
                let t1 = &planner.tasks[p1.task_id];
                let t2 = &planner.tasks[p2.task_id];
                let can_parallel = ParallelMode::can_overlap(t1.parallel_mode, t2.parallel_mode);
                if !can_parallel {
                    let a = p1.task_id.min(p2.task_id);
                    let b = p1.task_id.max(p2.task_id);
                    diagnostics
                        .pinned_conflicts
                        .push(PinnedConflict::Overlap { a, b });
                }
            }
        }
    }

    // pinned / fixed 間の dependency 違反
    for p in &placements {
        for dep in &planner.tasks[p.task_id].depends {
            if let Some(dep_p) = placements.iter().find(|p| &p.task_id == dep)
                && p.start.0 < dep_p.end.0
            {
                diagnostics
                    .pinned_conflicts
                    .push(PinnedConflict::DependencyViolation {
                        task_id: p.task_id,
                        dep_id: *dep,
                    });
            }
        }
    }

    placements
}

pub(crate) fn decode_status(diagnostics: &DecodeDiagnostics) -> DecodeStatus {
    if !diagnostics.pinned_conflicts.is_empty() {
        return DecodeStatus::Infeasible;
    }
    let has_cycle = diagnostics
        .failures
        .contains(&PlacementFailure::DependencyCycle)
        || diagnostics
            .relaxed
            .iter()
            .any(|r| r.reason == PlacementFailure::DependencyCycle);
    if has_cycle {
        return DecodeStatus::Infeasible;
    }
    if !diagnostics.failures.is_empty() || !diagnostics.relaxed.is_empty() {
        return DecodeStatus::Relaxed;
    }
    DecodeStatus::Feasible
}

/// 既に配置済みの依存先（後続タスク）の開始時刻の最小値を upper bound として返す。
/// 未配置の後続タスクがあれば `None` となる。したがってこの関数は固定/ピン済みの
/// 後続タスクがある場合にのみ効果を持ち、全タスクを配置する通常の貪欲ループでは
/// 多くの場合 `None` を返す（それは意図通り）。
fn latest_end_for(
    task_id: usize,
    index: &[Option<TimeWindow>],
    dependents: &[Vec<usize>],
) -> Option<Point> {
    let latest_start = dependents[task_id]
        .iter()
        .filter_map(|&d| index[d].map(|tw| tw.start))
        .min()
        .unwrap_or(Point(i64::MAX));
    if latest_start.0 == i64::MAX {
        None
    } else {
        Some(latest_start)
    }
}

pub(crate) fn fallback_for<const CHECK_CAPACITY: bool>(
    planner: &Planner,
    schedules: &[Placement],
    earliest: Point,
    dur: i64,
    latest_end: Option<Point>,
    task: &Task,
) -> (Point, Point, Option<PlacementFailure>) {
    let max_end = schedules
        .iter()
        .map(|p| p.end.0)
        .max()
        .unwrap_or(planner.now.0);
    let start = Point(max_end).max(planner.now).max(earliest);

    // try_place を使い、latest_end / 容量 / deadline を尊重できるスロットを探す。
    // 見つからなければ最後尾にフォールバックし、失敗理由を返す。
    match try_place::<CHECK_CAPACITY>(planner, schedules, task, start, dur, latest_end) {
        Ok(tw) => (tw.start, tw.end, None),
        Err(err) => {
            let end = Point(start.0 + dur);
            (start, end, Some(err))
        }
    }
}

fn place_task_earliest(
    planner: &Planner,
    schedules: &[Placement],
    input: &DecodeInput<'_>,
    task_id: usize,
    index: &[Option<TimeWindow>],
    dependents: &[Vec<usize>],
) -> (Point, Point, Option<PlacementFailure>) {
    let task = &planner.tasks[task_id];
    let earliest = compute_earliest_indexed(planner, index, task);
    let latest_end = latest_end_for(task_id, index, dependents);
    let mut first_err = None;

    for dur in duration_candidates(task, input.duration_choices.get(task_id)) {
        let (slots, err) = feasible_slots(planner, schedules, task, earliest, dur, latest_end, 1);
        if let Some(tw) = slots.into_iter().next() {
            return (tw.start, tw.end, None);
        }
        if first_err.is_none() {
            first_err = err;
        }
    }

    let dur = duration_for(task, input.duration_choices.get(task_id));
    let (start, end, fallback_err) =
        fallback_for::<true>(planner, schedules, earliest, dur, latest_end, task);
    (
        start,
        end,
        first_err
            .or(fallback_err)
            .or(Some(PlacementFailure::NoLegalSlot)),
    )
}

fn place_task_near_anchor(
    planner: &Planner,
    schedules: &[Placement],
    input: &DecodeInput<'_>,
    task_id: usize,
    index: &[Option<TimeWindow>],
    dependents: &[Vec<usize>],
    anchor: i64,
) -> (Point, Point, Option<PlacementFailure>) {
    let task = &planner.tasks[task_id];
    let earliest = compute_earliest_indexed(planner, index, task);
    let latest_end = latest_end_for(task_id, index, dependents);
    let mut best: Option<(Point, Point, i64)> = None;

    for dur in duration_candidates(task, input.duration_choices.get(task_id)) {
        let (slots, _) = feasible_slots(
            planner,
            schedules,
            task,
            earliest,
            dur,
            latest_end,
            usize::MAX,
        );
        for tw in slots {
            let dist = (tw.start.0 - anchor).abs();
            if best.is_none_or(|(_, _, d)| dist < d) {
                best = Some((tw.start, tw.end, dist));
            }
        }
    }

    if let Some((s, e, _)) = best {
        return (s, e, None);
    }

    let dur = duration_for(task, input.duration_choices.get(task_id));
    let (start, end, fallback_err) =
        fallback_for::<true>(planner, schedules, earliest, dur, latest_end, task);
    (
        start,
        end,
        fallback_err.or(Some(PlacementFailure::NoLegalSlot)),
    )
}

fn place_task_lowest_delta(
    planner: &Planner,
    schedules: &[Placement],
    input: &DecodeInput<'_>,
    task_id: usize,
    index: &[Option<TimeWindow>],
    dependents: &[Vec<usize>],
) -> (Point, Point, Option<PlacementFailure>) {
    let task = &planner.tasks[task_id];
    let earliest = compute_earliest_indexed(planner, index, task);
    let latest_end = latest_end_for(task_id, index, dependents);
    let mut best: Option<(Point, Point, f64)> = None;
    let mut first_err = None;

    for dur in duration_candidates(task, input.duration_choices.get(task_id)) {
        let (slots, err) = feasible_slots(
            planner,
            schedules,
            task,
            earliest,
            dur,
            latest_end,
            usize::MAX,
        );
        if slots.is_empty() {
            if first_err.is_none() {
                first_err = err;
            }
            continue;
        }
        for tw in slots {
            let score = evaluate_insertion(planner, schedules, task_id, tw.start, tw.end);
            if best.is_none_or(|(_, _, b)| score > b) {
                best = Some((tw.start, tw.end, score));
            }
        }
    }

    if let Some((s, e, _)) = best {
        return (s, e, None);
    }

    let dur = duration_for(task, input.duration_choices.get(task_id));
    let (s, e, fallback_err) =
        fallback_for::<true>(planner, schedules, earliest, dur, latest_end, task);
    (
        s,
        e,
        first_err
            .or(fallback_err)
            .or(Some(PlacementFailure::NoLegalSlot)),
    )
}

fn place_regret2(
    planner: &Planner,
    schedules: &[Placement],
    input: &DecodeInput<'_>,
    ready: &[usize],
    index: &[Option<TimeWindow>],
    dependents: &[Vec<usize>],
) -> Option<(usize, Point, Point, Option<PlacementFailure>)> {
    #[derive(Clone, Copy)]
    struct Candidate {
        task_id: usize,
        start: Point,
        end: Point,
        score: f64,
        error: Option<PlacementFailure>,
    }

    let mut best_choice: Option<Candidate> = None;
    let mut best_regret = f64::NEG_INFINITY;
    let mut fallback: Option<Candidate> = None;

    for &task_id in ready {
        let task = &planner.tasks[task_id];
        let earliest = compute_earliest_indexed(planner, index, task);
        let latest_end = latest_end_for(task_id, index, dependents);
        let mut scores: Vec<(Point, Point, f64)> = Vec::new();

        for dur in duration_candidates(task, input.duration_choices.get(task_id)) {
            let (slots, _) = feasible_slots(
                planner,
                schedules,
                task,
                earliest,
                dur,
                latest_end,
                usize::MAX,
            );
            for tw in slots {
                let score = evaluate_insertion(planner, schedules, task_id, tw.start, tw.end);
                scores.push((tw.start, tw.end, score));
            }
        }

        if scores.is_empty() {
            let dur = (task.cost_estimate.avg() as i64).max(1);
            let (s, e, fallback_err) =
                fallback_for::<true>(planner, schedules, earliest, dur, latest_end, task);
            let score = evaluate_insertion(planner, schedules, task_id, s, e);
            if fallback.is_none_or(|c| score > c.score) {
                fallback = Some(Candidate {
                    task_id,
                    start: s,
                    end: e,
                    score,
                    error: fallback_err,
                });
            }
            continue;
        }

        scores.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let best_score = scores[0].2;
        let second_score = scores.get(1).map_or(f64::NEG_INFINITY, |c| c.2);
        let regret = best_score - second_score;

        if regret > best_regret
            || (regret == best_regret
                && best_score > best_choice.map_or(f64::NEG_INFINITY, |c| c.score))
        {
            best_regret = regret;
            best_choice = Some(Candidate {
                task_id,
                start: scores[0].0,
                end: scores[0].1,
                score: best_score,
                error: None,
            });
        }
    }

    if let Some(c) = best_choice {
        return Some((c.task_id, c.start, c.end, None));
    }
    if let Some(c) = fallback {
        return Some((c.task_id, c.start, c.end, c.error));
    }
    None
}

/// 配置結果を `schedules` / `index` / `placed` / `in_degree` に反映し、
/// エラーと `forced` フラグに応じて diagnostics を更新する。
#[allow(clippy::too_many_arguments)]
fn record_placement(
    chosen_id: usize,
    start: Point,
    end: Point,
    placement_err: Option<PlacementFailure>,
    forced: bool,
    schedules: &mut Vec<Placement>,
    index: &mut [Option<TimeWindow>],
    placed: &mut [bool],
    in_degree: &mut [usize],
    remaining: &mut usize,
    dependents: &[Vec<usize>],
    diagnostics: &mut DecodeDiagnostics,
) {
    if let Some(err) = placement_err {
        if forced {
            diagnostics.failures.push(PlacementFailure::DependencyCycle);
            if err != PlacementFailure::DependencyCycle {
                diagnostics.failures.push(err);
                diagnostics.relaxed.push(RelaxedPlacement { reason: err });
            }
            diagnostics.relaxed.push(RelaxedPlacement {
                reason: PlacementFailure::DependencyCycle,
            });
        } else {
            diagnostics.failures.push(err);
            diagnostics.relaxed.push(RelaxedPlacement { reason: err });
        }
    } else if forced {
        diagnostics.failures.push(PlacementFailure::DependencyCycle);
        diagnostics.relaxed.push(RelaxedPlacement {
            reason: PlacementFailure::DependencyCycle,
        });
    }

    schedules.push(TaskPlacement::new(start, end, chosen_id));
    index[chosen_id] = Some(TimeWindow::new(start, end));
    placed[chosen_id] = true;
    *remaining -= 1;

    for &dep_id in &dependents[chosen_id] {
        if !placed[dep_id] {
            in_degree[dep_id] -= 1;
        }
    }
}

pub fn decode(planner: &Planner, input: DecodeInput<'_>) -> DecodeResult {
    let n = planner.tasks.len();
    let mut schedules: Vec<Placement> = Vec::with_capacity(n);
    let mut placed = vec![false; n];
    let mut diagnostics = DecodeDiagnostics::default();

    // priority は完全な順列であることを期待する。欠落・重複・範囲外があれば
    // 診断して、不足 task は末尾に追加する。
    let (priority, priority_invalid) = normalize_priority(input.priority, n);
    if priority_invalid {
        diagnostics.failures.push(PlacementFailure::InvalidPriority);
    }

    // 依存先 ID が存在するか検証。存在しない場合は診断するが、
    // そのタスクは依存なしとして後続の配置を続ける。
    for task in &planner.tasks {
        for &dep in &task.depends {
            if dep >= n {
                diagnostics
                    .failures
                    .push(PlacementFailure::InvalidDependency);
            }
        }
    }

    // 逆向き依存 index: 各タスクを依存先とするタスク一覧
    let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];
    for task in &planner.tasks {
        for &dep in &task.depends {
            if dep < n {
                dependents[dep].push(task.id);
            }
        }
    }

    // task_id -> (start, end) 配置済みタスクの index
    let mut index: Vec<Option<TimeWindow>> = vec![None; n];

    // 1. pinned / fixed 配置を投入し、入力の矛盾を検証
    let fixed_placements = validate_and_collect_fixed(planner, &input, &mut diagnostics);
    let mut remaining = n - fixed_placements.len();
    for p in &fixed_placements {
        schedules.push(*p);
        index[p.task_id] = Some(TimeWindow::new(p.start, p.end));
        placed[p.task_id] = true;
    }

    // in-degree 管理: 未配置の依存先数。0 = ready。
    // 毎 iteration の O(n * deps) ready スキャンを O(n) の初期化 + O(1) の判定に削減。
    let mut in_degree: Vec<usize> = vec![0; n];
    for task in &planner.tasks {
        if placed[task.id] {
            continue;
        }
        for dep in &task.depends {
            if *dep < n && !placed[*dep] {
                in_degree[task.id] += 1;
            }
        }
    }

    // 3. priority 順に依存が解決済みのタスクを配置
    while remaining > 0 {
        // priority 順で最初の ready (in_degree == 0) タスクを探す
        let mut task_id_opt = None;
        for &id in priority.iter() {
            if !placed[id] && in_degree[id] == 0 {
                task_id_opt = Some(id);
                break;
            }
        }

        let mut forced = false;
        let task_id = match task_id_opt {
            Some(id) => id,
            None => {
                // ready タスクがない = dependency cycle または入力不整合
                forced = true;
                match priority.iter().copied().find(|&id| !placed[id]) {
                    Some(id) => id,
                    None => break,
                }
            }
        };

        let (start, end, placement_err) = match input.repair_mode {
            RepairMode::Earliest => {
                let (s, e, err) =
                    place_task_earliest(planner, &schedules, &input, task_id, &index, &dependents);
                (s, e, err)
            }
            RepairMode::LowestDelta => {
                let (s, e, err) = place_task_lowest_delta(
                    planner,
                    &schedules,
                    &input,
                    task_id,
                    &index,
                    &dependents,
                );
                (s, e, err)
            }
            RepairMode::Regret2 => {
                let ready: Vec<usize> = priority
                    .iter()
                    .copied()
                    .filter(|&id| !placed[id] && in_degree[id] == 0)
                    .collect();
                if let Some((id, s, e, err)) =
                    place_regret2(planner, &schedules, &input, &ready, &index, &dependents)
                {
                    record_placement(
                        id,
                        s,
                        e,
                        err,
                        forced,
                        &mut schedules,
                        &mut index,
                        &mut placed,
                        &mut in_degree,
                        &mut remaining,
                        &dependents,
                        &mut diagnostics,
                    );
                    continue;
                } else {
                    let (s, e, err) = place_task_earliest(
                        planner,
                        &schedules,
                        &input,
                        task_id,
                        &index,
                        &dependents,
                    );
                    (s, e, err)
                }
            }
            RepairMode::Deadline => {
                // 締切が最も厳しい ready タスクを優先する
                let mut best_id = task_id;
                for &id in priority.iter() {
                    if !placed[id]
                        && in_degree[id] == 0
                        && planner.tasks[id].end < planner.tasks[best_id].end
                    {
                        best_id = id;
                    }
                }
                let (s, e, err) =
                    place_task_earliest(planner, &schedules, &input, best_id, &index, &dependents);
                record_placement(
                    best_id,
                    s,
                    e,
                    err,
                    forced,
                    &mut schedules,
                    &mut index,
                    &mut placed,
                    &mut in_degree,
                    &mut remaining,
                    &dependents,
                    &mut diagnostics,
                );
                continue;
            }
            RepairMode::Habit => {
                // 習慣タスク優先 → 前回 anchor 昇順で ready タスクを選択
                let previous = planner.previous_schedule();
                let mut best_id = task_id;
                let mut best_key = (
                    planner.tasks[task_id].habit_group.is_none(),
                    previous
                        .get(task_id)
                        .and_then(|x| x.map(|tw| tw.start.0))
                        .unwrap_or(i64::MAX),
                );
                for &id in priority.iter() {
                    if !placed[id] && in_degree[id] == 0 {
                        let key = (
                            planner.tasks[id].habit_group.is_none(),
                            previous
                                .get(id)
                                .and_then(|x| x.map(|tw| tw.start.0))
                                .unwrap_or(i64::MAX),
                        );
                        if key < best_key {
                            best_key = key;
                            best_id = id;
                        }
                    }
                }
                let anchor = previous
                    .get(best_id)
                    .and_then(|x| x.map(|tw| tw.start.0))
                    .unwrap_or(planner.now.0);
                let (s, e, err) = place_task_near_anchor(
                    planner,
                    &schedules,
                    &input,
                    best_id,
                    &index,
                    &dependents,
                    anchor,
                );
                record_placement(
                    best_id,
                    s,
                    e,
                    err,
                    forced,
                    &mut schedules,
                    &mut index,
                    &mut placed,
                    &mut in_degree,
                    &mut remaining,
                    &dependents,
                    &mut diagnostics,
                );
                continue;
            }
            RepairMode::Stability => {
                // 前回 anchor 昇順で ready タスクを選択
                let previous = planner.previous_schedule();
                let mut best_id = task_id;
                let mut best_anchor = previous
                    .get(task_id)
                    .and_then(|x| x.map(|tw| tw.start.0))
                    .unwrap_or(i64::MAX);
                for &id in priority.iter() {
                    if !placed[id] && in_degree[id] == 0 {
                        let anchor = previous
                            .get(id)
                            .and_then(|x| x.map(|tw| tw.start.0))
                            .unwrap_or(i64::MAX);
                        if anchor < best_anchor {
                            best_anchor = anchor;
                            best_id = id;
                        }
                    }
                }
                let anchor = previous
                    .get(best_id)
                    .and_then(|x| x.map(|tw| tw.start.0))
                    .unwrap_or(planner.now.0);
                let (s, e, err) = place_task_near_anchor(
                    planner,
                    &schedules,
                    &input,
                    best_id,
                    &index,
                    &dependents,
                    anchor,
                );
                record_placement(
                    best_id,
                    s,
                    e,
                    err,
                    forced,
                    &mut schedules,
                    &mut index,
                    &mut placed,
                    &mut in_degree,
                    &mut remaining,
                    &dependents,
                    &mut diagnostics,
                );
                continue;
            }
            RepairMode::HabitAnchor => {
                // 習慣タスク優先で ready タスクを選択
                let mut best_id = task_id;
                let mut best_is_habit = planner.tasks[task_id].habit_group.is_some();
                for &id in priority.iter() {
                    if !placed[id] && in_degree[id] == 0 {
                        let is_habit = planner.tasks[id].habit_group.is_some();
                        if is_habit && !best_is_habit {
                            best_is_habit = true;
                            best_id = id;
                        }
                    }
                }
                if planner.tasks[best_id].habit_group.is_some()
                    && let Some(pref) = planner.tasks[best_id].start
                {
                    let (s, e, err) = place_task_near_anchor(
                        planner,
                        &schedules,
                        &input,
                        best_id,
                        &index,
                        &dependents,
                        pref.0,
                    );
                    record_placement(
                        best_id,
                        s,
                        e,
                        err,
                        forced,
                        &mut schedules,
                        &mut index,
                        &mut placed,
                        &mut in_degree,
                        &mut remaining,
                        &dependents,
                        &mut diagnostics,
                    );
                    continue;
                } else {
                    let (s, e, err) = place_task_earliest(
                        planner,
                        &schedules,
                        &input,
                        best_id,
                        &index,
                        &dependents,
                    );
                    record_placement(
                        best_id,
                        s,
                        e,
                        err,
                        forced,
                        &mut schedules,
                        &mut index,
                        &mut placed,
                        &mut in_degree,
                        &mut remaining,
                        &dependents,
                        &mut diagnostics,
                    );
                    continue;
                }
            }
        };

        record_placement(
            task_id,
            start,
            end,
            placement_err,
            forced,
            &mut schedules,
            &mut index,
            &mut placed,
            &mut in_degree,
            &mut remaining,
            &dependents,
            &mut diagnostics,
        );
    }

    let status = decode_status(&diagnostics);
    DecodeResult {
        plan: Plan { schedules },
        diagnostics,
        status,
    }
}

fn duration_for(task: &Task, choice: Option<&i64>) -> i64 {
    match choice {
        Some(&d) if d > 0 => d,
        _ => (task.cost_estimate.avg() as i64).max(1),
    }
}

fn duration_candidates(task: &Task, choice: Option<&i64>) -> Vec<i64> {
    if let Some(&d) = choice {
        return vec![d.max(1)];
    }
    let avg = task.cost_estimate.avg() as i64;
    let sigma = task.cost_estimate.sigma() as i64;
    let expected = avg.max(1);
    let conservative = (avg + sigma).max(1);
    let short = (avg - sigma).max(1);

    let mut candidates = vec![expected];
    if conservative != expected {
        candidates.push(conservative);
    }
    if short != expected && short != conservative {
        candidates.push(short);
    }
    candidates
}

fn feasible_slots(
    planner: &Planner,
    schedules: &[Placement],
    task: &Task,
    earliest: Point,
    dur: i64,
    latest_end: Option<Point>,
    max_count: usize,
) -> (Vec<TimeWindow>, Option<PlacementFailure>) {
    let mut slots = Vec::new();
    let mut cursor = earliest;
    let mut last_err = None;
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 1000 {
            break;
        }
        if slots.len() >= max_count {
            break;
        }
        match try_place::<true>(planner, schedules, task, cursor, dur, latest_end) {
            Ok(tw) => {
                if slots.last() == Some(&tw) {
                    break;
                }
                slots.push(tw);
                cursor = tw.end;
            }
            Err(PlacementFailure::DailyCapacityExceeded) => {
                // 同日の後続スロットを先に探し、なければ翌日へ進む。
                let mut next = max_end_in_day(planner, schedules, cursor);
                if next.0 <= cursor.0 {
                    next = next_day_start(planner, cursor);
                }
                if next.0 <= cursor.0 {
                    break;
                }
                cursor = next;
            }
            Err(err) => {
                last_err = Some(err);
                break;
            }
        }
    }
    (slots, last_err)
}

fn evaluate_insertion(
    planner: &Planner,
    schedules: &[Placement],
    task_id: usize,
    start: Point,
    end: Point,
) -> f64 {
    INSERTION_PLAN.with(|plan_v| {
        INSERTION_SORTED.with(|sorted_v| {
            INSERTION_INDEX.with(|index_v| {
                INSERTION_HABIT.with(|habit_v| {
                    let mut scratch = plan_v.borrow_mut();
                    let mut sorted = sorted_v.borrow_mut();
                    let mut index = index_v.borrow_mut();
                    let mut habit = habit_v.borrow_mut();
                    scratch.clear();
                    scratch.extend_from_slice(schedules);
                    scratch.push(TaskPlacement::new(start, end, task_id));

                    let mut plan = Plan {
                        schedules: Vec::with_capacity(0),
                    };
                    std::mem::swap(&mut plan.schedules, &mut *scratch);
                    sorted.clear();
                    index.clear();
                    habit.clear();
                    let score = evaluate_with_scratch(
                        planner,
                        &plan.schedules,
                        0.0,
                        1.0,
                        &mut sorted,
                        &mut index,
                        &mut habit,
                    );
                    std::mem::swap(&mut plan.schedules, &mut *scratch);
                    score
                })
            })
        })
    })
}

fn normalize_priority(priority: &[usize], n: usize) -> (Vec<usize>, bool) {
    let mut seen = vec![false; n];
    let mut normalized = Vec::with_capacity(n);
    let mut invalid = false;

    for &id in priority {
        if id >= n {
            invalid = true;
            continue;
        }
        if seen[id] {
            invalid = true;
            continue;
        }
        seen[id] = true;
        normalized.push(id);
    }

    for (id, &was_seen) in seen.iter().enumerate() {
        if !was_seen {
            invalid = true;
            normalized.push(id);
        }
    }

    (normalized, invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_planner(tasks: Vec<Task>) -> Planner {
        Planner {
            tasks,
            now: Point(0),
            per: 5,
            sleep: SleepConfig::disabled(),
            workload: WorkloadConfig::default(),
            previous_schedule: vec![],
            ..Planner::default()
        }
    }

    fn input_with<'a>(priority: &'a [usize], pinned: &'a [Placement]) -> DecodeInput<'a> {
        DecodeInput {
            priority,
            duration_choices: &[],
            pinned,
            repair_mode: RepairMode::Earliest,
        }
    }

    fn input_with_mode<'a>(
        priority: &'a [usize],
        pinned: &'a [Placement],
        repair_mode: RepairMode,
    ) -> DecodeInput<'a> {
        DecodeInput {
            priority,
            duration_choices: &[],
            pinned,
            repair_mode,
        }
    }

    #[test]
    fn decoder_respects_dependency_order() {
        let first = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(4, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let second = Task {
            id: 1,
            depends: vec![0],
            ..first.clone()
        };
        let planner = test_planner(vec![first, second]);
        let result = decode(&planner, input_with(&[1, 0], &[]));
        assert_eq!(result.plan.schedules.len(), 2);
        assert!(result.plan.task_end(0).unwrap().0 <= result.plan.task_start(1).unwrap().0);
    }

    #[test]
    fn decoder_preserves_pinned_and_fixed() {
        let pinned = Task {
            id: 0,
            start: Some(Point(10)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let fixed = Task {
            id: 1,
            start: Some(Point(20)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: true,
            habit_group: None,
        };
        let normal = Task {
            id: 2,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![pinned, fixed, normal]);
        let pinned_placement = TaskPlacement::new(Point(5), Point(7), 0);
        let result = decode(&planner, input_with(&[2, 0, 1], &[pinned_placement]));

        assert_eq!(result.plan.task_start(1), Some(Point(20)));
        assert_eq!(result.plan.task_start(0), Some(Point(5)));

        let normal_entry = result
            .plan
            .schedules
            .iter()
            .find(|p| p.task_id == 2)
            .unwrap();
        assert!(
            normal_entry.end.0 <= pinned_placement.start.0
                || normal_entry.start.0 >= pinned_placement.end.0,
            "normal task must not overlap pinned task"
        );
        assert!(
            normal_entry.end.0 <= 20 || normal_entry.start.0 >= 22,
            "normal task must not overlap fixed task"
        );
    }

    #[test]
    fn decoder_schedules_all_task_ids() {
        let mut planner = test_planner(vec![]);
        for i in 0..5 {
            planner.tasks.push(Task {
                id: i,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(2, 0),
                depends: if i > 0 { vec![i - 1] } else { vec![] },
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            });
        }
        let priority: Vec<_> = (0..5).collect();
        let result = decode(&planner, input_with(&priority, &[]));
        assert_eq!(result.plan.schedules.len(), 5);
        let ids: Vec<_> = result.plan.schedules.iter().map(|p| p.task_id).collect();
        assert_eq!(ids.len(), 5);
        for i in 0..5 {
            assert!(ids.contains(&i));
        }
    }

    #[test]
    fn decoder_reports_dependency_cycle() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![1],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            depends: vec![0],
            ..a.clone()
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0, 1], &[]));
        assert_eq!(result.plan.schedules.len(), 2);
        assert!(
            result
                .diagnostics
                .failures
                .contains(&PlacementFailure::DependencyCycle),
            "cycle should be reported"
        );
    }

    #[test]
    fn decoder_keeps_zero_avg_task() {
        let zero = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(0, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![zero]);
        let result = decode(&planner, input_with(&[0], &[]));
        assert_eq!(result.plan.schedules.len(), 1);
        assert_eq!(result.plan.task_start(0), Some(Point(0)));
    }

    // Regression: pinned/fixed tasks must respect dependencies on unpinned tasks.
    // If a pinned/fixed task depends on a not-yet-placed task, that dependency
    // must finish before the pinned/fixed task starts.

    #[test]
    fn pinned_dependent_fits_before_pinned() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(4, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(5)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![0],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(
            &planner,
            input_with(&[0, 1], &[TaskPlacement::new(Point(5), Point(7), 1)]),
        );

        let a_end = result.plan.task_end(0).unwrap().0;
        let b_start = result.plan.task_start(1).unwrap().0;
        assert!(
            a_end <= b_start,
            "A must end before pinned B starts: a_end={a_end}, b_start={b_start}"
        );
    }

    #[test]
    fn fixed_dependent_enforces_dependency_before_fixed() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(20)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![0],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: true,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0, 1], &[]));

        let a_end = result.plan.task_end(0).unwrap().0;
        let b_start = result.plan.task_start(1).unwrap().0;
        assert!(
            a_end <= b_start,
            "A must end before fixed B starts: a_end={a_end}, b_start={b_start}"
        );
    }

    #[test]
    fn fixed_dependency_infeasible_reports_no_legal_slot() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(25, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(20)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![0],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: true,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0, 1], &[]));

        assert!(
            result
                .diagnostics
                .failures
                .contains(&PlacementFailure::LatestEndExceeded),
            "infeasible dependency on fixed task should report LatestEndExceeded"
        );
    }

    #[test]
    fn pinned_dependency_infeasible_reports_no_legal_slot() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(5)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![0],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(
            &planner,
            input_with(&[0, 1], &[TaskPlacement::new(Point(5), Point(7), 1)]),
        );

        assert!(
            result
                .diagnostics
                .failures
                .contains(&PlacementFailure::LatestEndExceeded),
            "infeasible dependency on pinned task should report LatestEndExceeded"
        );
    }

    // Regression: invalid priority lists must not panic and must be diagnosed.

    #[test]
    fn invalid_priority_out_of_range_does_not_panic() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0, 5], &[]));

        assert_eq!(result.plan.schedules.len(), 2);
        assert!(
            result
                .diagnostics
                .failures
                .contains(&PlacementFailure::InvalidPriority)
        );
    }

    #[test]
    fn invalid_priority_missing_does_not_panic() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0], &[]));

        assert_eq!(result.plan.schedules.len(), 2);
        assert!(
            result
                .diagnostics
                .failures
                .contains(&PlacementFailure::InvalidPriority)
        );
    }

    #[test]
    fn invalid_priority_duplicate_does_not_panic() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0, 0], &[]));

        assert_eq!(result.plan.schedules.len(), 2);
        assert!(
            result
                .diagnostics
                .failures
                .contains(&PlacementFailure::InvalidPriority)
        );
    }

    // DecodeResult status

    #[test]
    fn decode_status_is_feasible_for_clean_input() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0, 1], &[]));
        assert_eq!(result.status, DecodeStatus::Feasible);
    }

    #[test]
    fn decode_status_is_relaxed_on_latest_end_exceeded() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(5)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![0],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(
            &planner,
            input_with(&[0, 1], &[TaskPlacement::new(Point(5), Point(7), 1)]),
        );

        assert_eq!(result.status, DecodeStatus::Relaxed);
    }

    #[test]
    fn decode_status_is_infeasible_on_dependency_cycle() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![1],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            depends: vec![0],
            ..a.clone()
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(&planner, input_with(&[0, 1], &[]));

        assert_eq!(result.status, DecodeStatus::Infeasible);
    }

    #[test]
    fn decode_status_is_infeasible_on_pinned_conflict() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(
            &planner,
            input_with(
                &[0, 1],
                &[
                    TaskPlacement::new(Point(0), Point(5), 0),
                    TaskPlacement::new(Point(3), Point(8), 1),
                ],
            ),
        );

        assert_eq!(result.status, DecodeStatus::Infeasible);
    }

    // try_place failure reasons

    #[test]
    fn try_place_reports_deadline_exceeded() {
        let task = Task {
            id: 0,
            start: Some(Point(1)),
            end: Point(5),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![task.clone()]);
        let err = try_place::<true>(&planner, &[], &task, Point(1), 10, None).unwrap_err();
        assert_eq!(err, PlacementFailure::DeadlineExceeded);
    }

    #[test]
    fn try_place_reports_latest_end_exceeded() {
        let task = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![task.clone()]);
        let err =
            try_place::<true>(&planner, &[], &task, Point(0), 10, Some(Point(5))).unwrap_err();
        assert_eq!(err, PlacementFailure::LatestEndExceeded);
    }

    #[test]
    fn try_place_reports_sleep_conflict() {
        let task = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(30),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let mut planner = test_planner(vec![task.clone()]);
        // 20..25 を sleep window とする。start=20 から duration=10 だと
        // sleep を避けて 25 から始めるが、25+10=35 > deadline 30 となる。
        // この場合、sleep そのものではなく deadline 超過が根本原因なので
        // DeadlineExceeded を優先して報告する。
        planner.sleep = SleepConfig::new(0, 20, 25, true);
        let err = try_place::<true>(&planner, &[], &task, Point(20), 10, None).unwrap_err();
        assert_eq!(err, PlacementFailure::DeadlineExceeded);
    }

    #[test]
    fn try_place_parallel_conflict_leads_to_deadline_exceeded() {
        let host = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        // parallelizable だが、host が allows_parallel=false なので並行不可
        let guest = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(7),
            cost_estimate: NormalDist::new(3, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Guest,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![host.clone(), guest.clone()]);
        let schedules = vec![TaskPlacement::new(Point(0), Point(5), 0)];
        // guest は [0,3) で host と重なり、並行不可かつ [5,8) では deadline=7 を超える
        let err = try_place::<true>(&planner, &schedules, &guest, Point(0), 3, None).unwrap_err();
        assert_eq!(err, PlacementFailure::DeadlineExceeded);
    }

    // pinned / fixed input validation

    #[test]
    fn pinned_overlap_is_reported() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        let result = decode(
            &planner,
            input_with(
                &[0, 1],
                &[
                    TaskPlacement::new(Point(0), Point(5), 0),
                    TaskPlacement::new(Point(3), Point(8), 1),
                ],
            ),
        );

        assert!(
            result
                .diagnostics
                .pinned_conflicts
                .contains(&PinnedConflict::Overlap { a: 0, b: 1 })
        );
    }

    #[test]
    fn pinned_invalid_interval_is_reported() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a]);
        let result = decode(
            &planner,
            input_with(&[0], &[TaskPlacement::new(Point(10), Point(5), 0)]),
        );

        assert!(result.diagnostics.pinned_conflicts.iter().any(|c| matches!(
            c,
            PinnedConflict::InvalidInterval { task_id: 0, start: s, end: e }
            if s.0 == 10 && e.0 == 5
        )));
    }

    #[test]
    fn pinned_start_violation_is_reported() {
        let a = Task {
            id: 0,
            start: Some(Point(10)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a]);
        let result = decode(
            &planner,
            input_with(&[0], &[TaskPlacement::new(Point(5), Point(15), 0)]),
        );

        assert!(result.diagnostics.pinned_conflicts.iter().any(|c| matches!(
            c,
            PinnedConflict::StartViolation {
                task_id: 0,
                pinned_start,
                task_start,
            } if pinned_start.0 == 5 && task_start.0 == 10
        )));
    }

    #[test]
    fn pinned_deadline_violation_is_reported() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(20),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a]);
        let result = decode(
            &planner,
            input_with(&[0], &[TaskPlacement::new(Point(15), Point(25), 0)]),
        );

        assert!(result.diagnostics.pinned_conflicts.iter().any(|c| matches!(
            c,
            PinnedConflict::DeadlineViolation {
                task_id: 0,
                pinned_end,
                deadline,
            } if pinned_end.0 == 25 && deadline.0 == 20
        )));
    }

    #[test]
    fn duplicate_pinned_id_is_reported() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a]);
        let result = decode(
            &planner,
            input_with(
                &[0],
                &[
                    TaskPlacement::new(Point(0), Point(5), 0),
                    TaskPlacement::new(Point(10), Point(15), 0),
                ],
            ),
        );

        assert!(
            result
                .diagnostics
                .pinned_conflicts
                .contains(&PinnedConflict::DuplicateId { task_id: 0 })
        );
    }

    #[test]
    fn pinned_dependency_violation_is_reported() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![0],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a, b]);
        // b depends on a, but pinned b starts before pinned a ends.
        let result = decode(
            &planner,
            input_with(
                &[0, 1],
                &[
                    TaskPlacement::new(Point(5), Point(10), 0),
                    TaskPlacement::new(Point(7), Point(12), 1),
                ],
            ),
        );

        assert!(result.diagnostics.pinned_conflicts.contains(
            &PinnedConflict::DependencyViolation {
                task_id: 1,
                dep_id: 0,
            }
        ));
    }

    // Stage 2: duration choice

    #[test]
    fn lowest_delta_uses_shorter_duration_to_meet_deadline() {
        let task = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(9),
            cost_estimate: NormalDist::new(10, 2),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![task]);
        let result = decode(
            &planner,
            input_with_mode(&[0], &[], RepairMode::LowestDelta),
        );

        // avg=10 では deadline=9 に収まらないが、short=avg-sigma=8 なら収まる。
        assert_eq!(result.status, DecodeStatus::Feasible);
        assert_eq!(result.plan.task_end(0), Some(Point(8)));
    }

    // Stage 2: lowest-delta insertion

    #[test]
    fn lowest_delta_places_tight_deadline_first() {
        let loose = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let tight = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist::new(8, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![loose, tight]);
        // LowestDelta は priority 順の最初の task に対して最良スロットを選ぶ。
        // tight -> loose の順なら tight を [0,8) に置いて feasible となる。
        let result = decode(
            &planner,
            input_with_mode(&[1, 0], &[], RepairMode::LowestDelta),
        );

        assert_eq!(result.status, DecodeStatus::Feasible);
        assert_eq!(result.plan.task_start(1), Some(Point(0)));
    }

    // Stage 2: regret-2 insertion

    #[test]
    fn regret_2_prioritizes_limited_options() {
        // fixed タスク [6,11) があり、task A は deadline=12 なので [0,6) のみ可能。
        // task B は deadline=100 で多数の選択肢がある。regret-2 なら A を先に置く。
        let fixed = Task {
            id: 0,
            start: Some(Point(6)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: true,
            habit_group: None,
        };
        let a = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(12),
            cost_estimate: NormalDist::new(6, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 2,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(3, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![fixed, a, b]);
        let result = decode(
            &planner,
            input_with_mode(&[0, 2, 1], &[], RepairMode::Regret2),
        );

        assert_eq!(result.status, DecodeStatus::Feasible);
        assert_eq!(result.plan.task_start(1), Some(Point(0)));
    }

    // Stage 2: deadline repair

    #[test]
    fn deadline_repair_schedules_tight_first() {
        let loose = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let tight = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![loose, tight]);
        // priority loose -> tight を無視して deadline の厳しい tight を先に置く
        let result = decode(
            &planner,
            input_with_mode(&[0, 1], &[], RepairMode::Deadline),
        );

        assert_eq!(result.plan.task_start(1), Some(Point(0)));
    }

    // Stage 2: stability repair

    #[test]
    fn stability_repair_keeps_previous_schedule() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let b = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let mut planner = test_planner(vec![a, b]);
        planner.set_previous_schedule(&[TaskPlacement::new(Point(20), Point(25), 1)]);

        let result = decode(
            &planner,
            input_with_mode(&[0, 1], &[], RepairMode::Stability),
        );

        // Stability モードでは task 1 を前回の開始時刻 20 に近づけるよう先に置く
        assert_eq!(result.plan.task_start(1), Some(Point(20)));
    }

    // Review fixes: TDD

    #[test]
    fn invalid_dependency_id_is_reported() {
        let a = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![99], // 存在しない依存 ID
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let planner = test_planner(vec![a]);
        let result = decode(&planner, input_with(&[0], &[]));

        assert!(
            result
                .diagnostics
                .failures
                .contains(&PlacementFailure::InvalidDependency)
        );
    }

    #[test]
    fn habit_repair_keeps_habit_anchor() {
        let non_habit = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let habit = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: Some(1),
        };
        let mut planner = test_planner(vec![non_habit, habit]);
        planner.set_previous_schedule(&[TaskPlacement::new(Point(20), Point(25), 1)]);

        let result = decode(&planner, input_with_mode(&[0, 1], &[], RepairMode::Habit));

        // Habit モードでは habit task を前回の anchor 20 に近づけて先に置く
        assert_eq!(result.plan.task_start(1), Some(Point(20)));
    }

    #[test]
    fn place_regret2_returns_none_for_empty_ready() {
        let planner = test_planner(vec![]);
        let result = place_regret2(
            &planner,
            &[],
            &DecodeInput {
                priority: &[],
                duration_choices: &[],
                pinned: &[],
                repair_mode: RepairMode::Regret2,
            },
            &[],
            &[],
            &[],
        );

        assert!(result.is_none());
    }

    #[test]
    fn try_place_reports_daily_capacity_exceeded() {
        let task = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let mut planner = test_planner(vec![]);
        planner.workload = WorkloadConfig::new(8, 10);
        // 1 日の最大容量 10 に対し、既存 [0,8) + 候補 [7,12) は union で 12 を超える。
        let schedules = vec![TaskPlacement::new(Point(0), Point(8), 0)];
        let err = try_place::<true>(&planner, &schedules, &task, Point(7), 5, None).unwrap_err();
        assert_eq!(err, PlacementFailure::DailyCapacityExceeded);
    }
}
