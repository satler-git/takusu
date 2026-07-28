//! # SA + LNS + Tabu Search
//!
//! ```text
//! 1. greedy_initial(active_tasks) → 初期解
//! 2. T = T₀ → ... → T_min:
//!    各温度でN反復:
//!      neighbor = generate (8種, 確率重み付き)
//!      tabuチェック (aspirationあり)
//!      ΔEで受理判定 (Metropolis)
//!      tabu更新
//! 3. best を返却
//! ```
//!
//! | prob | neighbor      |
//! |------|---------------|
//! | 17%  | shift         |
//! | 17%  | swap          |
//! | 14%  | duration ±1   |
//! | 14%  | reorder       |
//! | 14%  | repair_depend |
//! | 10%  | habit_anchor (グループの時刻帯を一括移動。なければ shift) |
//! |  6%  | habit_exception (habit member 1 件を逸脱。なければ shift) |
//! |  8%  | lns (destroy+rebuild) |
//!
//! ## Design rationale
//!
//! ### Tabu list key = (task_id, start, duration)
//! 同一タスクの同一配置への再訪を防ぐ。完全なハッシュ (全taskの配置) だと容量爆発するため
//! 最後に動かした一つのタスクのみを記録。容量 = task_count*2。
//! aspiration: tabu でも best より良ければ受理 (改善解は tabu を無視)。
//!
//! ### LNS window size
//! pivot タスクの duration*2、最低4スロット。総タスク時間の1/3以上にはしない。
//! これにより小さな window の局所改善と大きな再配置のバランスを取る。
//!
//! ### greedy_rebuild の freeness 順
//! destroy で除去したタスクを freeness 昇順に再配置。freeness の低い(切迫した)タスクから
//! 空きスロットに詰めることで、高 freeness タスクが柔軟に後回しにされる。
//!
//! ### partial モードで pinned_ids を毎回渡す理由
//! generate_neighbor_partial は pinned タスクの位置を一切変更しないため、
//! unpinned の task_id 一覧を毎回抽出する。これは計算量O(n)だが、n<100で支配的でない。
//! 代わりに pinned 固定のインデックス集合をキャッシュすることもできるが、簡潔さを優先。

use std::collections::VecDeque;
use std::time::Instant;

use rand::{Rng, RngExt};
use rustc_hash::FxHashSet;

use super::*;
use crate::decoder::{DecodeInput, RepairMode, decode, decode_status, fallback_for};
use crate::habit;
use crate::placement::{
    Placement, capacity_exceeded_for, compute_earliest, compute_earliest_indexed, try_place,
};
#[cfg(test)]
use evaluate::evaluate;
use evaluate::{
    evaluate_presorted, evaluate_with_scratch, sorted_incremental_apply, sorted_revert,
};

struct TabuList {
    entries: VecDeque<(usize, i64, i64)>,
    set: FxHashSet<(usize, i64, i64)>,
    capacity: usize,
}

impl TabuList {
    fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            set: FxHashSet::default(),
            capacity,
        }
    }

    fn push(&mut self, task_id: usize, start: Point, duration: i64) {
        let key = (task_id, start.0, duration);
        if self.entries.len() >= self.capacity
            && let Some(old) = self.entries.pop_front()
        {
            self.set.remove(&old);
        }
        self.entries.push_back(key);
        self.set.insert(key);
    }

    fn contains(&self, task_id: usize, start: Point, duration: i64) -> bool {
        self.set.contains(&(task_id, start.0, duration))
    }
}

fn rand_range(rng: &mut impl Rng, low: i64, high: i64) -> i64 {
    if high <= low {
        return low;
    }
    rng.random_range(low..high)
}

/// トポロジカル順序を計算。依存関係のないタスクは自由順序。
/// 注意: この順序は freeness ソートの入力に使われるだけで、配置順自体ではない。
/// build_initial 内でさらに freeness 昇順に並び替えられる。
fn topological_order(planner: &Planner, active: &FxHashSet<usize>) -> Vec<usize> {
    let n = planner.tasks.len();
    let mut in_degree = vec![0usize; n];
    let mut adj = vec![Vec::new(); n];

    for task in &planner.tasks {
        if !active.contains(&task.id) {
            continue;
        }
        for dep in &task.depends {
            if active.contains(dep) {
                adj[*dep].push(task.id);
                in_degree[task.id] += 1;
            }
        }
    }

    let mut queue: Vec<usize> = (0..n)
        .filter(|i| active.contains(i) && in_degree[*i] == 0)
        .collect();
    let mut result = Vec::with_capacity(n);

    while let Some(u) = queue.pop() {
        result.push(u);
        for &v in &adj[u] {
            in_degree[v] -= 1;
            if in_degree[v] == 0 {
                queue.push(v);
            }
        }
    }

    result
}

/// トポロジカル順序を満たしつつ、配置可能になったタスクの中で freeness が
/// 最も低い (最も切迫している) タスクを優先して選ぶ順序を返す。
///
/// `active` に含まれるタスクだけを対象とし、active 外の依存は既に配置済みとして
/// 無視する。これにより、未ピンのタスク同士の依存関係を保ちながら freeness 順に
/// 並べることができる。
fn topological_order_by_freeness(planner: &Planner, active: &FxHashSet<usize>) -> Vec<usize> {
    let n = planner.tasks.len();
    let mut in_degree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];

    for task in &planner.tasks {
        if !active.contains(&task.id) {
            continue;
        }
        for dep in &task.depends {
            if active.contains(dep) {
                dependents[*dep].push(task.id);
                in_degree[task.id] += 1;
            }
        }
    }

    let mut ready: Vec<usize> = (0..n)
        .filter(|i| active.contains(i) && in_degree[*i] == 0)
        .collect();
    let mut result = Vec::with_capacity(active.len());
    let mut in_result = vec![false; n];

    while !ready.is_empty() {
        let idx = ready
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| planner.freeness(**a).total_cmp(&planner.freeness(**b)))
            .map(|(i, _)| i)
            .unwrap();
        let u = ready.swap_remove(idx);
        result.push(u);
        in_result[u] = true;
        for &v in &dependents[u] {
            in_degree[v] -= 1;
            if in_degree[v] == 0 {
                ready.push(v);
            }
        }
    }

    // 環があった場合やその他の理由で配置できなかったタスクも、freeness 順に末尾に追加して
    // すべての active タスクが結果に含まれるようにする。
    let mut remaining: Vec<usize> = active.iter().copied().filter(|&i| !in_result[i]).collect();
    remaining.sort_by(|a, b| planner.freeness(*a).total_cmp(&planner.freeness(*b)));
    result.extend(remaining);

    result
}

/// 貪欲法で初期解を構築。
///
/// 方針: 切迫したタスク (freeness 低い) から順に、依存を満たす最も早い位置に配置。
/// SA 初期解構築用の fallback 配置。
///
/// 末尾に配置し、必ず earliest / now を尊重する。容量チェックは行わない。
/// SA はその後、評価関数の勾配に従って改善する。
fn push_fallback(
    planner: &Planner,
    schedules: &mut Vec<Placement>,
    earliest: Point,
    dur: i64,
    task_id: usize,
    last_end: Point,
) -> Point {
    let start = last_end.max(planner.now).max(earliest);
    let end = Point(start.0 + dur);
    schedules.push(TaskPlacement::new(start, end, task_id));
    end
}

fn build_initial(planner: &Planner) -> Plan {
    let all: FxHashSet<usize> = planner.tasks.iter().map(|t| t.id).collect();
    let order = topological_order(planner, &all);

    let mut by_freeness: Vec<usize> = order.into_iter().collect();
    by_freeness.sort_by(|a, b| {
        planner
            .freeness(*a)
            .partial_cmp(&planner.freeness(*b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut schedules: Vec<Placement> = Vec::new();
    let mut last_end = planner.now;

    // 固定タスクを先に配置して、通常タスクの try_place が重複を避けられるようにする
    // (#391)。固定タスクは移動できないため、通常タスク側が重複を回避する必要がある。
    for &task_id in &by_freeness {
        let task = &planner.tasks[task_id];
        let dur = (task.cost_estimate.avg as i64).max(1);
        if task.fixed
            && let Some(start) = task.start
        {
            let end = Point(start.0 + dur);
            schedules.push(TaskPlacement::new(start, end, task_id));
            last_end = last_end.max(end);
        }
    }

    for task_id in by_freeness {
        let task = &planner.tasks[task_id];
        let dur = (task.cost_estimate.avg as i64).max(1);

        // 固定タスクは先に配置済み
        if task.fixed && task.start.is_some() {
            continue;
        }

        let earliest = compute_earliest(planner, &schedules, task);
        if let Ok(tw) = try_place::<false>(planner, &schedules, task, earliest, dur, None) {
            let start = tw.start;
            let end = tw.end;
            schedules.push(TaskPlacement::new(start, end, task_id));
            last_end = last_end.max(end);
        } else {
            last_end = push_fallback(planner, &mut schedules, earliest, dur, task_id, last_end);
        }
    }

    Plan { schedules }
}

#[cfg(test)]
pub(crate) fn priority_order_search(planner: &Planner, rng: &mut impl Rng) -> Plan {
    let mut priority: Vec<_> = planner.tasks.iter().map(|task| task.id).collect();
    priority.sort_by(|a, b| planner.freeness(*a).total_cmp(&planner.freeness(*b)));

    let mut sorted = Vec::with_capacity(planner.tasks.len());
    let mut index = Vec::with_capacity(planner.tasks.len());
    let mut habit_entries = Vec::with_capacity(planner.tasks.len());
    let mut current = decode(
        planner,
        DecodeInput {
            priority: &priority,
            duration_choices: &[],
            pinned: &[],
            repair_mode: RepairMode::Earliest,
        },
    )
    .plan;
    let mut current_score = evaluate_with_scratch(
        planner,
        &current.schedules,
        0.0,
        1.0,
        &mut sorted,
        &mut index,
        &mut habit_entries,
    );
    let mut best = current.clone();
    let mut best_score = current_score;
    let movable: Vec<_> = priority
        .iter()
        .enumerate()
        .filter(|(_, id)| !planner.tasks[**id].fixed)
        .map(|(position, _)| position)
        .collect();
    if movable.len() < 2 {
        return best;
    }

    let iterations = planner.tasks.len().max(1) * 100;
    let initial_temperature = planner.tasks.len().max(1) as f64;
    for iteration in 0..iterations {
        let a_index = rng.random_range(0..movable.len());
        let mut b_index = rng.random_range(0..movable.len());
        if a_index == b_index {
            b_index = (b_index + 1) % movable.len();
        }
        let a = movable[a_index];
        let b = movable[b_index];
        priority.swap(a, b);
        let candidate = decode(
            planner,
            DecodeInput {
                priority: &priority,
                duration_choices: &[],
                pinned: &[],
                repair_mode: RepairMode::Earliest,
            },
        )
        .plan;
        let candidate_score = evaluate_with_scratch(
            planner,
            &candidate.schedules,
            0.0,
            1.0,
            &mut sorted,
            &mut index,
            &mut habit_entries,
        );
        let temperature = initial_temperature * (1.0 - iteration as f64 / iterations as f64);
        let delta = candidate_score - current_score;
        if delta > 0.0 || rng.random::<f64>() < (delta / temperature.max(0.01)).exp() {
            current = candidate;
            current_score = candidate_score;
            if current_score > best_score {
                best = current.clone();
                best_score = current_score;
            }
        } else {
            priority.swap(a, b);
        }
    }

    best
}

// ── ALNS (Adaptive Large Neighborhood Search) for priority decoder ─────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestroyOperator {
    Random,
    Worst,
    Related,
    /// 1 つの habit group の全 movable member をまとめて除去する。
    /// グループ単位の再配置が目的のため、`count` 引数は意図的に無視する。
    /// habit がない場合は Random にフォールバックする。
    HabitGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RepairOperator {
    Earliest,
    Deadline,
    Regret2,
    LowestDelta,
    Random,
    /// habit group を希望時刻 (task.start) の anchor へ再配置する。
    HabitAnchor,
}

struct AlnsConfig {
    iterations: usize,
    initial_temperature: f64,
    segment_size: usize,
    reaction_factor: f64,
    destroy_min_frac: f64,
    destroy_max_frac: f64,
    destroy_operators: Vec<DestroyOperator>,
    repair_operators: Vec<RepairOperator>,
}

impl Default for AlnsConfig {
    fn default() -> Self {
        Self {
            // 0 は "task 数に応じて自動決定" を意味する。
            iterations: 0,
            initial_temperature: 10.0,
            // 0 は iterations/10 で自動決定。
            segment_size: 0,
            reaction_factor: 0.1,
            destroy_min_frac: 0.05,
            destroy_max_frac: 0.2,
            destroy_operators: vec![
                DestroyOperator::Random,
                DestroyOperator::Worst,
                DestroyOperator::Related,
                DestroyOperator::HabitGroup,
            ],
            // 初期設定は軽量な repair のみ。Regret2/LowestDelta は decode が高コスト
            // (O(n²) per placement) のためデフォルトでは無効。
            repair_operators: vec![
                RepairOperator::Earliest,
                RepairOperator::Deadline,
                RepairOperator::Random,
                RepairOperator::HabitAnchor,
            ],
        }
    }
}

const MAX_TIME_BUDGET: Duration = Duration::from_secs(60 * 60 * 24 * 365 * 10);

fn deadline_from(budget: Option<Duration>) -> Option<Instant> {
    budget.map(|b| Instant::now() + b.min(MAX_TIME_BUDGET))
}

/// ALNS で得た解を SA で磨く。priority 順列空間では届かない
/// shift / duration / reorder の微調整を行い、軟制約のスコアを改善する。
/// 残り time budget の範囲で動作し、改善しなければ元を返す。
fn sa_polish(
    planner: &Planner,
    plan: Plan,
    pinned_ids: &FxHashSet<usize>,
    rng: &mut impl Rng,
    deadline: Option<Instant>,
) -> Plan {
    sa_polish_inner(planner, plan, pinned_ids, rng, deadline, 5)
}

fn sa_polish_inner(
    planner: &Planner,
    plan: Plan,
    pinned_ids: &FxHashSet<usize>,
    rng: &mut impl Rng,
    deadline: Option<Instant>,
    iter_multiplier: usize,
) -> Plan {
    let n = planner.tasks.len();
    if n <= 1 {
        return plan;
    }

    let mut index = Vec::with_capacity(n);
    let mut habit_entries = Vec::with_capacity(n);
    let habit_index = habit::build_index(planner);

    let total_avg: i64 = planner
        .tasks
        .iter()
        .filter(|t| !pinned_ids.contains(&t.id))
        .map(|t| t.cost_estimate.avg as i64)
        .sum();
    let t0 = (total_avg as f64 * 0.02).max(1.0);
    let alpha = 0.85;
    let t_min = t0 * 1e-3;
    let has_pinned = !pinned_ids.is_empty();
    let movable_count = if has_pinned {
        planner
            .tasks
            .iter()
            .filter(|t| !pinned_ids.contains(&t.id))
            .count()
    } else {
        n
    };
    let iter_per_temp = movable_count.max(1) * iter_multiplier;

    let mut current = plan;
    let mut best = current.clone();
    let mut neighbor_scheds: Vec<Placement> = Vec::with_capacity(n);

    let mut sorted: Vec<Placement> = current.schedules.clone();
    sorted.sort_unstable_by_key(|p| p.start.0);

    let unpinned_positions: Vec<usize> = if has_pinned {
        current
            .schedules
            .iter()
            .enumerate()
            .filter(|(_, p)| !pinned_ids.contains(&p.task_id))
            .map(|(i, _)| i)
            .collect()
    } else {
        Vec::new()
    };

    let mut eval_current = evaluate_presorted(
        planner,
        &current.schedules,
        0.0,
        1.0,
        &sorted,
        &mut index,
        &mut habit_entries,
    );
    let mut eval_best = eval_current;

    let mut temperature = t0;
    let span = plan_span(&current);

    while temperature > t_min {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }

        for iter in 0..iter_per_temp {
            if iter & 63 == 0 && deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }

            let ok = if has_pinned {
                generate_neighbor_partial_into(
                    planner,
                    &current.schedules,
                    &mut neighbor_scheds,
                    rng,
                    pinned_ids,
                    &habit_index,
                    &unpinned_positions,
                    span,
                )
            } else {
                generate_neighbor_into(
                    planner,
                    &current.schedules,
                    &mut neighbor_scheds,
                    rng,
                    &habit_index,
                    span,
                )
            };
            if !ok {
                continue;
            }

            let undo = sorted_incremental_apply(&mut sorted, &current.schedules, &neighbor_scheds);

            let eval_neighbor = evaluate_presorted(
                planner,
                &neighbor_scheds,
                0.0,
                1.0,
                &sorted,
                &mut index,
                &mut habit_entries,
            );

            let delta = eval_neighbor - eval_current;
            if delta > 0.0 || rng.random::<f64>() < (delta / temperature).exp() {
                std::mem::swap(&mut current.schedules, &mut neighbor_scheds);
                eval_current = eval_neighbor;
                if eval_current > eval_best {
                    best.schedules.clone_from(&current.schedules);
                    eval_best = eval_current;
                }
            } else {
                sorted_revert(&mut sorted, &undo);
            }
        }

        temperature *= alpha;
    }

    best
}

/// priority decoder + ALNS。`pinned` を固定配置として扱う。
pub(crate) fn alns_search_pinned(
    planner: &Planner,
    pinned: &[Placement],
    rng: &mut impl Rng,
) -> DecodeResult {
    let config = AlnsConfig::default();
    let n = planner.tasks.len();

    let pinned_ids: FxHashSet<usize> = pinned.iter().map(|p| p.task_id).collect();

    // 初期 priority: warm start 時は前回スケジュールの開始時刻順、そうでなければ freeness 昇順
    let mut priority: Vec<_> = (0..n).collect();
    if planner.warm_start && !planner.previous_schedule.is_empty() {
        priority.sort_by(|a, b| {
            let anchor = |id: &usize| {
                planner
                    .previous_schedule
                    .get(*id)
                    .and_then(|x| *x)
                    .map(|tw| tw.start.0)
                    .unwrap_or(i64::MAX)
            };
            anchor(a).cmp(&anchor(b))
        });
    } else {
        priority.sort_by(|a, b| planner.freeness(*a).total_cmp(&planner.freeness(*b)));
    }

    let initial_mode = if planner.warm_start {
        RepairMode::Stability
    } else {
        RepairMode::Earliest
    };

    let mut sorted = Vec::with_capacity(n);
    let mut index = Vec::with_capacity(n);
    let mut habit_entries = Vec::with_capacity(n);

    let decode_result = |priority: &[usize], mode: RepairMode| {
        decode(
            planner,
            DecodeInput {
                priority,
                duration_choices: &[],
                pinned,
                repair_mode: mode,
            },
        )
    };

    let deadline = deadline_from(planner.time_budget);

    let mut current_result = decode_result(&priority, initial_mode);
    let mut current_score = evaluate_with_scratch(
        planner,
        &current_result.plan.schedules,
        0.0,
        1.0,
        &mut sorted,
        &mut index,
        &mut habit_entries,
    );
    let mut best_result = current_result.clone();
    let mut best_score = current_score;

    if n <= 1 {
        return current_result;
    }

    let d_ops = config.destroy_operators;
    let r_ops = config.repair_operators;
    let mut d_weights = vec![1.0; d_ops.len()];
    let mut r_weights = vec![1.0; r_ops.len()];
    let mut d_scores = vec![0.0; d_ops.len()];
    let mut r_scores = vec![0.0; r_ops.len()];
    let mut d_usages = vec![0usize; d_ops.len()];
    let mut r_usages = vec![0usize; r_ops.len()];
    let habit_index = habit::build_index(planner);

    // 後続タスクの開始時刻 upper bound 計算用に依存グラフを事前構築
    let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];
    for task in &planner.tasks {
        for &dep in &task.depends {
            if dep < n {
                dependents[dep].push(task.id);
            }
        }
    }

    let iterations = if config.iterations == 0 {
        n.max(1) * 50
    } else {
        config.iterations
    };
    let segment_size = if config.segment_size == 0 {
        (iterations / 10).max(1)
    } else {
        config.segment_size
    };

    for iteration in 0..iterations {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }

        let d_i = select_operator_index(&d_weights, rng);
        let r_i = select_operator_index(&r_weights, rng);
        let destroy_op = d_ops[d_i];
        let repair_op = r_ops[r_i];

        let destroy_count = destroy_count(n, config.destroy_min_frac, config.destroy_max_frac, rng);
        let removed = destroy_priority(
            planner,
            &priority,
            &current_result.plan,
            &pinned_ids,
            rng,
            destroy_op,
            destroy_count,
            &habit_index,
        );

        let removed_set: FxHashSet<usize> = removed.iter().copied().collect();
        let mut partial = priority.clone();
        partial.retain(|id| !removed_set.contains(id));

        let new_priority = repair_priority(planner, &partial, &removed, repair_op, rng);

        // incremental decode: 生存タスクの配置を維持し、除去タスクのみ再配置。
        // full decode O(n²) の代わりに O(removed × n)。
        let removed_in_order: Vec<usize> = new_priority
            .iter()
            .copied()
            .filter(|id| removed_set.contains(id))
            .collect();
        let candidate_scheds = incremental_decode(
            planner,
            &current_result.plan.schedules,
            &removed_in_order,
            &dependents,
        );
        let candidate_score = evaluate_with_scratch(
            planner,
            &candidate_scheds,
            0.0,
            1.0,
            &mut sorted,
            &mut index,
            &mut habit_entries,
        );

        let temperature = config.initial_temperature * (1.0 - iteration as f64 / iterations as f64);
        let delta = candidate_score - current_score;

        let old_current_score = current_score;
        let mut accepted = false;
        let mut new_best = false;

        if delta > 0.0 || rng.random::<f64>() < (delta / temperature.max(0.01)).exp() {
            current_result.plan.schedules = candidate_scheds;
            current_score = candidate_score;
            priority = new_priority;
            accepted = true;

            if current_score > best_score {
                best_result
                    .plan
                    .schedules
                    .clone_from(&current_result.plan.schedules);
                best_score = current_score;
                new_best = true;
            }
        }

        let reward = if new_best {
            33.0
        } else if candidate_score > old_current_score {
            9.0
        } else if accepted {
            3.0
        } else {
            0.0
        };

        d_scores[d_i] += reward;
        r_scores[r_i] += reward;
        d_usages[d_i] += 1;
        r_usages[r_i] += 1;

        if iteration > 0 && iteration % segment_size == 0 {
            update_operator_weights(&mut d_weights, &d_scores, &d_usages, config.reaction_factor);
            update_operator_weights(&mut r_weights, &r_scores, &r_usages, config.reaction_factor);
            d_scores.fill(0.0);
            r_scores.fill(0.0);
            d_usages.fill(0);
            r_usages.fill(0);
        }
    }

    best_result.plan = sa_polish(planner, best_result.plan, &pinned_ids, rng, deadline);
    if has_dependency_violation(planner, &best_result.plan, &pinned_ids) {
        best_result.plan = force_fix_dependencies(planner, best_result.plan, &pinned_ids);
        best_result.plan =
            sa_polish_inner(planner, best_result.plan, &pinned_ids, rng, deadline, 3);
    }

    // 最終 plan に対して status/diagnostics を再計算。
    // decode() を pinned として検証し、capacity は別途チェックする。
    let mut sorted_scheds = best_result.plan.schedules.clone();
    sorted_scheds.sort_unstable_by_key(|p| p.start.0);
    let final_priority: Vec<usize> = sorted_scheds.iter().map(|p| p.task_id).collect();
    let final_input = DecodeInput {
        priority: &final_priority,
        duration_choices: &[],
        pinned: &best_result.plan.schedules,
        repair_mode: RepairMode::Earliest,
    };
    let mut final_result = decode(planner, final_input);
    if best_result
        .plan
        .schedules
        .iter()
        .any(|p| capacity_exceeded_for(planner, &best_result.plan.schedules, p.start, p.end))
    {
        final_result
            .diagnostics
            .failures
            .push(PlacementFailure::DailyCapacityExceeded);
        final_result.diagnostics.relaxed.push(RelaxedPlacement {
            reason: PlacementFailure::DailyCapacityExceeded,
        });
        final_result.status = decode_status(&final_result.diagnostics);
    }
    best_result.diagnostics = final_result.diagnostics;
    best_result.status = final_result.status;

    best_result
}

fn has_dependency_violation(planner: &Planner, plan: &Plan, pinned_ids: &FxHashSet<usize>) -> bool {
    let n = planner.tasks.len();
    let mut pos_index: Vec<Option<TimeWindow>> = vec![None; n];
    for p in &plan.schedules {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        if id < n {
            pos_index[id] = Some(TimeWindow::new(s, e));
        }
    }
    for task in &planner.tasks {
        if pinned_ids.contains(&task.id) || task.fixed {
            continue;
        }
        let Some(tw) = pos_index[task.id] else {
            continue;
        };
        let start = tw.start;
        for dep_id in &task.depends {
            if let Some(Some(dep_tw)) = pos_index.get(*dep_id)
                && dep_tw.end > start
            {
                return true;
            }
        }
    }
    false
}

/// 依存違反をスコアに関係なく強制修正する。違反タスクを依存先の終了直後へ移動し、
/// 連鎖的に後続タスクも押し出す。その後の sa_polish で他のcomponentを回復させる。
fn force_fix_dependencies(planner: &Planner, plan: Plan, pinned_ids: &FxHashSet<usize>) -> Plan {
    let n = planner.tasks.len();
    let mut schedules = plan.schedules;

    let mut pos_index: Vec<Option<TimeWindow>> = vec![None; n];
    for p in &schedules {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        if id < n {
            pos_index[id] = Some(TimeWindow::new(s, e));
        }
    }

    // 依存違反がある限り繰り返す（連鎖修正のため）
    for _ in 0..n {
        let mut any_fixed = false;
        for task in &planner.tasks {
            if pinned_ids.contains(&task.id) || task.fixed {
                continue;
            }
            let Some(tw) = pos_index[task.id] else {
                continue;
            };
            let start = tw.start;
            let mut latest_dep_end: Option<Point> = None;
            for dep_id in &task.depends {
                if let Some(Some(dep_tw)) = pos_index.get(*dep_id)
                    && dep_tw.end > start
                {
                    latest_dep_end = Some(latest_dep_end.map_or(dep_tw.end, |m| m.max(dep_tw.end)));
                }
            }
            let Some(dep_end) = latest_dep_end else {
                continue;
            };

            if let Some(pos) = schedules.iter().position(|p| p.task_id == task.id) {
                let dur = schedules[pos].end.0 - schedules[pos].start.0;
                schedules[pos] = TaskPlacement::new(dep_end, Point(dep_end.0 + dur), task.id);
                pos_index[task.id] = Some(TimeWindow::new(dep_end, Point(dep_end.0 + dur)));
                any_fixed = true;
            }
        }
        if !any_fixed {
            break;
        }
    }

    Plan { schedules }
}

fn select_operator_index(weights: &[f64], rng: &mut impl Rng) -> usize {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return rng.random_range(0..weights.len());
    }
    let mut r = rng.random::<f64>() * total;
    for (i, w) in weights.iter().enumerate() {
        r -= *w;
        if r <= 0.0 {
            return i;
        }
    }
    weights.len() - 1
}

pub(crate) fn update_operator_weights(
    weights: &mut [f64],
    scores: &[f64],
    usages: &[usize],
    reaction_factor: f64,
) {
    for i in 0..weights.len() {
        let usage = usages[i].max(1);
        let avg = scores[i] / usage as f64;
        weights[i] = (1.0 - reaction_factor) * weights[i] + reaction_factor * avg;
    }
    normalize_weights(weights, 0.1);
}

fn normalize_weights(weights: &mut [f64], min: f64) {
    let n = weights.len();
    if n == 0 {
        return;
    }
    for _ in 0..n {
        let sum: f64 = weights.iter().sum();
        if sum <= 0.0 {
            return;
        }
        let mut clamped = false;
        for w in weights.iter_mut() {
            let normalized = *w / sum * n as f64;
            *w = normalized.max(min);
            if normalized < min {
                clamped = true;
            }
        }
        if !clamped {
            break;
        }
    }
}

fn destroy_count(n: usize, min_frac: f64, max_frac: f64, rng: &mut impl Rng) -> usize {
    if n <= 1 {
        return 0;
    }
    let min = (n as f64 * min_frac).ceil() as usize;
    let max = (n as f64 * max_frac).ceil() as usize;
    let min = min.clamp(1, n - 1);
    let max = max.clamp(min, n - 1);
    rng.random_range(min..=max)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn destroy_priority(
    planner: &Planner,
    priority: &[usize],
    plan: &Plan,
    pinned_ids: &FxHashSet<usize>,
    rng: &mut impl Rng,
    op: DestroyOperator,
    count: usize,
    habit: &habit::HabitIndex,
) -> Vec<usize> {
    let movable: Vec<_> = priority
        .iter()
        .copied()
        .filter(|id| !planner.tasks[*id].fixed && !pinned_ids.contains(id))
        .collect();
    if movable.is_empty() || count == 0 {
        return vec![];
    }
    let count = count.min(movable.len());

    let n = planner.tasks.len();
    let mut pos_index: Vec<Option<TimeWindow>> = vec![None; n];
    for p in &plan.schedules {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        if id < n {
            pos_index[id] = Some(TimeWindow::new(s, e));
        }
    }
    let scheduled =
        |id: usize| -> TimeWindow { pos_index[id].unwrap_or(TimeWindow::new(Point(0), Point(0))) };

    match op {
        DestroyOperator::Random => {
            let mut chosen = FxHashSet::default();
            while chosen.len() < count {
                let idx = rng.random_range(0..movable.len());
                chosen.insert(movable[idx]);
            }
            chosen.into_iter().collect()
        }
        DestroyOperator::Worst => {
            let mut sorted_scheds: Vec<&Placement> = plan.schedules.iter().collect();
            sorted_scheds.sort_unstable_by_key(|p| p.start.0);

            let mut badness: Vec<(usize, i64)> = movable
                .iter()
                .map(|&id| {
                    let tw = scheduled(id);
                    let s = tw.start;
                    let e = tw.end;
                    let task = &planner.tasks[id];
                    let mut bad = 0i64;
                    if e.0 > task.end.0 {
                        bad += e.0 - task.end.0;
                    }
                    if let Some(min_start) = task.start
                        && s.0 < min_start.0
                    {
                        bad += min_start.0 - s.0;
                    }
                    let end_pos = sorted_scheds.partition_point(|p| p.start.0 < e.0);
                    for other_p in &sorted_scheds[..end_pos] {
                        if other_p.task_id == id {
                            continue;
                        }
                        if other_p.end.0 <= s.0 {
                            continue;
                        }
                        let other = &planner.tasks[other_p.task_id];
                        if !(task.parallelizable && other.allows_parallel
                            || task.allows_parallel && other.parallelizable)
                        {
                            bad += other_p.end.0.min(e.0) - other_p.start.0.max(s.0);
                        }
                    }
                    (id, bad)
                })
                .collect();
            badness.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            badness.into_iter().map(|(id, _)| id).take(count).collect()
        }
        DestroyOperator::Related => {
            if movable.is_empty() {
                return vec![];
            }
            let seed_idx = rng.random_range(0..movable.len());
            let seed = movable[seed_idx];
            let seed_tw = scheduled(seed);
            let seed_s = seed_tw.start;
            let seed_e = seed_tw.end;
            let window = (planner.tasks[seed].cost_estimate.avg as i64).max(5);

            let mut scored: Vec<(usize, i64)> = movable
                .iter()
                .map(|&id| {
                    if id == seed {
                        return (id, 1);
                    }
                    let task = &planner.tasks[id];
                    let tw = scheduled(id);
                    let s = tw.start;
                    let e = tw.end;
                    let time_dist = if e.0 <= seed_s.0 {
                        seed_s.0 - e.0
                    } else if s.0 >= seed_e.0 {
                        s.0 - seed_e.0
                    } else {
                        0
                    };
                    let mut tie = 0;
                    if task.habit_group.is_some()
                        && task.habit_group == planner.tasks[seed].habit_group
                    {
                        tie -= 1000;
                    }
                    if task.depends.contains(&seed) || planner.tasks[seed].depends.contains(&id) {
                        tie -= 500;
                    }
                    (id, time_dist + tie)
                })
                .collect();
            scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

            let mut removed = vec![seed];
            for (id, dist) in scored {
                if removed.len() >= count {
                    break;
                }
                if id == seed {
                    continue;
                }
                if dist <= window || dist < 0 {
                    removed.push(id);
                }
            }
            // 足りなければランダムで補完
            while removed.len() < count {
                let candidate = movable[rng.random_range(0..movable.len())];
                if !removed.contains(&candidate) {
                    removed.push(candidate);
                }
            }
            removed.truncate(count);
            removed
        }
        DestroyOperator::HabitGroup => {
            // count は意図的に無視し、選ばれたグループの全 movable member を
            // まとめて除去する。グルーピングは静的な HabitIndex を再利用し、
            // 毎 iteration の再構築を避ける。
            if habit.groups.is_empty() {
                let mut chosen = FxHashSet::default();
                while chosen.len() < count {
                    let idx = rng.random_range(0..movable.len());
                    chosen.insert(movable[idx]);
                }
                return chosen.into_iter().collect();
            }
            let group = &habit.groups[rng.random_range(0..habit.groups.len())];
            let removed: Vec<usize> = group
                .members
                .iter()
                .copied()
                .filter(|id| !planner.tasks[*id].fixed && !pinned_ids.contains(id))
                .collect();
            if removed.is_empty() {
                let mut chosen = FxHashSet::default();
                while chosen.len() < count {
                    let idx = rng.random_range(0..movable.len());
                    chosen.insert(movable[idx]);
                }
                return chosen.into_iter().collect();
            }
            removed
        }
    }
}

pub(crate) fn repair_priority(
    planner: &Planner,
    partial: &[usize],
    removed: &[usize],
    op: RepairOperator,
    rng: &mut impl Rng,
) -> Vec<usize> {
    let mut result = partial.to_vec();
    let remaining: Vec<usize> = match op {
        RepairOperator::Deadline => {
            let mut v = removed.to_vec();
            v.sort_by_key(|&id| planner.tasks[id].end);
            v
        }
        RepairOperator::Random => {
            let mut v = removed.to_vec();
            for i in (1..v.len()).rev() {
                let j = rng.random_range(0..=i);
                v.swap(i, j);
            }
            v
        }
        RepairOperator::Earliest | RepairOperator::Regret2 | RepairOperator::LowestDelta => {
            removed.to_vec()
        }
        RepairOperator::HabitAnchor => {
            // habit task を先に配置して anchor スロットを確保させる。
            let mut v = removed.to_vec();
            v.sort_by(|&a, &b| {
                let a_habit = planner.tasks[a].habit_group.is_some();
                let b_habit = planner.tasks[b].habit_group.is_some();
                b_habit.cmp(&a_habit).then_with(|| {
                    let a_s = planner.tasks[a].start.map(|p| p.0).unwrap_or(i64::MAX);
                    let b_s = planner.tasks[b].start.map(|p| p.0).unwrap_or(i64::MAX);
                    a_s.cmp(&b_s)
                })
            });
            v
        }
    };

    if matches!(op, RepairOperator::Regret2 | RepairOperator::LowestDelta) {
        result.extend(remaining);
        return result;
    }

    let n = planner.tasks.len();
    let mut pos_index: Vec<usize> = vec![usize::MAX; n];
    for (i, &id) in result.iter().enumerate() {
        pos_index[id] = i;
    }

    for id in remaining {
        let mut max_dep_pos: Option<usize> = None;
        for &dep in &planner.tasks[id].depends {
            if dep < n {
                let pos = pos_index[dep];
                if pos == usize::MAX {
                    max_dep_pos = Some(result.len());
                    break;
                }
                max_dep_pos = Some(max_dep_pos.map_or(pos, |m| m.max(pos)));
            }
        }
        let pos = max_dep_pos.map_or(0, |p| p + 1).min(result.len());

        for i in pos..result.len() {
            pos_index[result[i]] += 1;
        }
        result.insert(pos, id);
        pos_index[id] = pos;
    }
    result
}

/// 改善なしの温度レベルがこの回数続いたら current を best に戻す (intensification)。
const STAGNATION_LIMIT: u32 = 3;

/// 長距離 shift を選ぶ確率の分母 (1/5 = 20%)。
const LONG_SHIFT_ONE_IN: u32 = 5;

/// プラン全体の時間スパン。長距離 shift の移動幅に使う。
fn plan_span(plan: &Plan) -> i64 {
    let min_s = plan.schedules.iter().map(|p| p.start.0).min();
    let max_e = plan.schedules.iter().map(|p| p.end.0).max();
    match (min_s, max_e) {
        (Some(a), Some(b)) => (b - a).max(1),
        _ => 1,
    }
}
pub fn sa_lns(planner: &Planner, rng: &mut impl Rng) -> Plan {
    let task_count = planner.tasks.len().max(1);

    let mut current = build_initial(planner);
    let mut best = current.clone();

    let mut sorted = Vec::with_capacity(task_count);
    let mut index = Vec::with_capacity(task_count);
    let mut habit_entries = Vec::with_capacity(task_count);
    let habit_index = habit::build_index(planner);

    let mut neighbor_scheds: Vec<Placement> = Vec::with_capacity(task_count);

    let total_avg: i64 = planner
        .tasks
        .iter()
        .map(|t| t.cost_estimate.avg as i64)
        .sum();
    let t0 = (total_avg as f64 * 0.1).max(1.0);
    let alpha = 0.93;
    let t_min = t0 * 1e-4;
    let iter_per_temp = task_count * 30;

    let mut tabu = TabuList::new(task_count * 2);
    let mut tabu_scratch: Vec<Option<(i64, i64)>> = Vec::with_capacity(task_count);
    let mut temperature = t0;

    let mut eval_current = evaluate_with_scratch(
        planner,
        &current.schedules,
        temperature,
        t0,
        &mut sorted,
        &mut index,
        &mut habit_entries,
    );
    let mut eval_best = eval_current;

    let mut stagnant_levels = 0u32;
    let deadline = deadline_from(planner.time_budget);
    let span = plan_span(&current);

    while temperature > t_min {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }

        let mut improved = false;
        for iter in 0..iter_per_temp {
            if iter & 63 == 0 && deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }

            if !generate_neighbor_into(
                planner,
                &current.schedules,
                &mut neighbor_scheds,
                rng,
                &habit_index,
                span,
            ) {
                continue;
            }

            let eval_neighbor = evaluate_with_scratch(
                planner,
                &neighbor_scheds,
                temperature,
                t0,
                &mut sorted,
                &mut index,
                &mut habit_entries,
            );

            if is_tabu_scheds(&tabu, &neighbor_scheds) && eval_neighbor <= eval_best {
                continue;
            }

            let delta = eval_neighbor - eval_current;

            if delta > 0.0 || rng.random::<f64>() < (delta / temperature).exp() {
                mark_tabu_scheds(
                    &mut tabu,
                    &current.schedules,
                    &neighbor_scheds,
                    &mut tabu_scratch,
                );
                std::mem::swap(&mut current.schedules, &mut neighbor_scheds);
                eval_current = eval_neighbor;

                if eval_current > eval_best {
                    if evaluate_with_scratch(
                        planner,
                        &current.schedules,
                        0.0,
                        t0,
                        &mut sorted,
                        &mut index,
                        &mut habit_entries,
                    ) > evaluate_with_scratch(
                        planner,
                        &best.schedules,
                        0.0,
                        t0,
                        &mut sorted,
                        &mut index,
                        &mut habit_entries,
                    ) {
                        best.schedules.clone_from(&current.schedules);
                        eval_best = eval_current;
                        improved = true;
                    } else {
                        eval_best = eval_current;
                    }
                }
            }
        }

        if improved {
            stagnant_levels = 0;
        } else {
            stagnant_levels += 1;
            if stagnant_levels >= STAGNATION_LIMIT {
                current.schedules.clone_from(&best.schedules);
                stagnant_levels = 0;
            }
        }

        temperature *= alpha;
        eval_current = evaluate_with_scratch(
            planner,
            &current.schedules,
            temperature,
            t0,
            &mut sorted,
            &mut index,
            &mut habit_entries,
        );
        eval_best = evaluate_with_scratch(
            planner,
            &best.schedules,
            temperature,
            t0,
            &mut sorted,
            &mut index,
            &mut habit_entries,
        );
    }

    repair_polish(planner, best, None)
}
pub fn sa_lns_partial(planner: &Planner, pinned: &[Placement], rng: &mut impl Rng) -> Plan {
    if pinned.is_empty() {
        return sa_lns(planner, rng);
    }

    let pinned_ids: FxHashSet<usize> = pinned.iter().map(|p| p.task_id).collect();

    let unpinned_count = planner
        .tasks
        .iter()
        .filter(|t| !pinned_ids.contains(&t.id))
        .count();
    let task_count = planner.tasks.len().max(1);

    let mut current = build_initial_partial(planner, pinned);
    let mut best = current.clone();

    let mut sorted = Vec::with_capacity(task_count);
    let mut index = Vec::with_capacity(task_count);
    let mut habit_entries = Vec::with_capacity(task_count);
    let habit_index = habit::build_index(planner);

    let mut neighbor_scheds: Vec<Placement> = Vec::with_capacity(task_count);

    let unpinned_positions: Vec<usize> = current
        .schedules
        .iter()
        .enumerate()
        .filter(|(_, p)| !pinned_ids.contains(&p.task_id))
        .map(|(i, _)| i)
        .collect();

    let total_avg: i64 = planner
        .tasks
        .iter()
        .filter(|t| !pinned_ids.contains(&t.id))
        .map(|t| t.cost_estimate.avg as i64)
        .sum();
    let t0 = (total_avg as f64 * 0.1).max(1.0);
    let alpha = 0.93;
    let t_min = t0 * 1e-4;
    let iter_per_temp = unpinned_count.max(1) * 30;

    let mut tabu = TabuList::new(task_count * 2);
    let mut tabu_scratch: Vec<Option<(i64, i64)>> = Vec::with_capacity(task_count);
    let mut temperature = t0;

    let mut eval_current = evaluate_with_scratch(
        planner,
        &current.schedules,
        temperature,
        t0,
        &mut sorted,
        &mut index,
        &mut habit_entries,
    );
    let mut eval_best = eval_current;

    let mut stagnant_levels = 0u32;
    let deadline = deadline_from(planner.time_budget);
    let span = plan_span(&current);

    while temperature > t_min {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }

        let mut improved = false;
        for iter in 0..iter_per_temp {
            if iter & 63 == 0 && deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }

            if !generate_neighbor_partial_into(
                planner,
                &current.schedules,
                &mut neighbor_scheds,
                rng,
                &pinned_ids,
                &habit_index,
                &unpinned_positions,
                span,
            ) {
                continue;
            }

            let eval_neighbor = evaluate_with_scratch(
                planner,
                &neighbor_scheds,
                temperature,
                t0,
                &mut sorted,
                &mut index,
                &mut habit_entries,
            );

            if is_tabu_scheds(&tabu, &neighbor_scheds) && eval_neighbor <= eval_best {
                continue;
            }

            let delta = eval_neighbor - eval_current;

            if delta > 0.0 || rng.random::<f64>() < (delta / temperature).exp() {
                mark_tabu_scheds(
                    &mut tabu,
                    &current.schedules,
                    &neighbor_scheds,
                    &mut tabu_scratch,
                );
                std::mem::swap(&mut current.schedules, &mut neighbor_scheds);
                eval_current = eval_neighbor;

                if eval_current > eval_best {
                    if evaluate_with_scratch(
                        planner,
                        &current.schedules,
                        0.0,
                        t0,
                        &mut sorted,
                        &mut index,
                        &mut habit_entries,
                    ) > evaluate_with_scratch(
                        planner,
                        &best.schedules,
                        0.0,
                        t0,
                        &mut sorted,
                        &mut index,
                        &mut habit_entries,
                    ) {
                        best.schedules.clone_from(&current.schedules);
                        eval_best = eval_current;
                        improved = true;
                    } else {
                        eval_best = eval_current;
                    }
                }
            }
        }

        if improved {
            stagnant_levels = 0;
        } else {
            stagnant_levels += 1;
            if stagnant_levels >= STAGNATION_LIMIT {
                current.schedules.clone_from(&best.schedules);
                stagnant_levels = 0;
            }
        }

        temperature *= alpha;
        eval_current = evaluate_with_scratch(
            planner,
            &current.schedules,
            temperature,
            t0,
            &mut sorted,
            &mut index,
            &mut habit_entries,
        );
        eval_best = evaluate_with_scratch(
            planner,
            &best.schedules,
            temperature,
            t0,
            &mut sorted,
            &mut index,
            &mut habit_entries,
        );
    }

    repair_polish(planner, best, Some(&pinned_ids))
}

/// SA 後の仕上げ: 依存違反中のタスクを取り除いて貪欲に再配置し、
/// T=0 の評価が改善する場合のみ採用する。
fn repair_polish(planner: &Planner, best: Plan, pinned_ids: Option<&FxHashSet<usize>>) -> Plan {
    let mut index: Vec<Option<TimeWindow>> = vec![None; planner.tasks.len()];
    for p in &best.schedules {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        if id < index.len() {
            index[id] = Some(TimeWindow::new(s, e));
        }
    }

    let mut violators: FxHashSet<usize> = FxHashSet::default();
    for task in &planner.tasks {
        if let Some(p) = pinned_ids
            && p.contains(&task.id)
        {
            continue;
        }
        let Some(tw) = index[task.id] else {
            continue;
        };
        let start = tw.start;
        for dep_id in &task.depends {
            if let Some(Some(dep_tw)) = index.get(*dep_id)
                && dep_tw.end > start
            {
                violators.insert(task.id);
            }
        }
    }

    if violators.is_empty() {
        return best;
    }

    // 違反タスクを後ろへ動かすとその依存元も違反し得るため、推移的な依存元も再配置対象にする。
    loop {
        let mut grew = false;
        for task in &planner.tasks {
            if violators.contains(&task.id) {
                continue;
            }
            if let Some(p) = pinned_ids
                && p.contains(&task.id)
            {
                continue;
            }
            if task.depends.iter().any(|d| violators.contains(d)) {
                violators.insert(task.id);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    let mut remaining = Vec::new();
    let mut destroyed: Vec<usize> = Vec::new();
    for sched in &best.schedules {
        if violators.contains(&sched.task_id) {
            destroyed.push(sched.task_id);
        } else {
            remaining.push(*sched);
        }
    }

    let rebuilt = Plan {
        schedules: greedy_rebuild(planner, &remaining, &destroyed),
    };

    let mut eval_sorted = Vec::with_capacity(planner.tasks.len());
    let mut eval_index = Vec::with_capacity(planner.tasks.len());
    let mut eval_habit = Vec::with_capacity(planner.tasks.len());
    if evaluate_with_scratch(
        planner,
        &rebuilt.schedules,
        0.0,
        1.0,
        &mut eval_sorted,
        &mut eval_index,
        &mut eval_habit,
    ) > evaluate_with_scratch(
        planner,
        &best.schedules,
        0.0,
        1.0,
        &mut eval_sorted,
        &mut eval_index,
        &mut eval_habit,
    ) {
        rebuilt
    } else {
        best
    }
}

fn build_initial_partial(planner: &Planner, pinned: &[Placement]) -> Plan {
    let pinned_ids: FxHashSet<usize> = pinned.iter().map(|p| p.task_id).collect();

    let unpinned_ids: FxHashSet<usize> = planner
        .tasks
        .iter()
        .filter(|t| !pinned_ids.contains(&t.id))
        .map(|t| t.id)
        .collect();

    let unpinned = topological_order_by_freeness(planner, &unpinned_ids);

    let mut schedules: Vec<Placement> = pinned.to_vec();

    // 固定タスクを先に配置して、通常タスクの try_place が重複を避けられるようにする
    // (#391)。pinned に含まれない固定タスクを先に処理する。
    for &task_id in &unpinned {
        let task = &planner.tasks[task_id];
        let dur = (task.cost_estimate.avg as i64).max(1);
        if task.fixed
            && let Some(start) = task.start
        {
            let end = Point(start.0 + dur);
            schedules.push(TaskPlacement::new(start, end, task_id));
        }
    }

    for task_id in unpinned {
        let task = &planner.tasks[task_id];
        let dur = (task.cost_estimate.avg as i64).max(1);

        // 固定タスクは先に配置済み
        if task.fixed && task.start.is_some() {
            continue;
        }

        let earliest = compute_earliest(planner, &schedules, task);
        if let Ok(tw) = try_place::<false>(planner, &schedules, task, earliest, dur, None) {
            let start = tw.start;
            let end = tw.end;
            schedules.push(TaskPlacement::new(start, end, task_id));
        } else {
            let last_end = schedules
                .iter()
                .map(|p| p.end.0)
                .max()
                .unwrap_or(planner.now.0);
            let _ = push_fallback(
                planner,
                &mut schedules,
                earliest,
                dur,
                task_id,
                Point(last_end),
            );
        }
    }

    Plan { schedules }
}

fn is_tabu_scheds(tabu: &TabuList, schedules: &[Placement]) -> bool {
    schedules
        .iter()
        .any(|p| tabu.contains(p.task_id, p.start, p.end.0 - p.start.0))
}

/// O(n) tabu marking: scratch buffer を再利用して allocation を避ける。
fn mark_tabu_scheds(
    tabu: &mut TabuList,
    current: &[Placement],
    neighbor: &[Placement],
    scratch: &mut Vec<Option<(i64, i64)>>,
) {
    let max_id = current
        .iter()
        .chain(neighbor.iter())
        .map(|p| p.task_id)
        .max()
        .unwrap_or(0);
    scratch.clear();
    scratch.resize(max_id + 1, None);
    for p in current {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        scratch[id] = Some((s.0, e.0));
    }
    for p in neighbor {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        let changed = match scratch[id] {
            Some((cs, ce)) => cs != s.0 || ce != e.0,
            None => true,
        };
        if changed {
            tabu.push(id, s, e.0 - s.0);
        }
    }
}

/// In-place neighbor generation: copies `current` into `buf` and modifies
/// `buf`. Returns true on success. Avoids heap allocation per SA iteration
/// by reusing the pre-allocated buffer.
fn generate_neighbor_into(
    planner: &Planner,
    current: &[Placement],
    buf: &mut Vec<Placement>,
    rng: &mut impl Rng,
    habit: &habit::HabitIndex,
    span: i64,
) -> bool {
    buf.clear();
    buf.extend_from_slice(current);

    let r = rng.random_range(0..100u32) as i32;

    match r {
        0..=16 => ShiftOp.apply_full(planner, buf, rng, span),
        17..=33 => SwapOp.apply_full(planner, buf, rng, span),
        34..=47 => DurationOp.apply_full(planner, buf, rng, span),
        48..=61 => ReorderOp.apply_full(planner, buf, rng, span),
        62..=75 => neighbor_repair_depend_into(planner, buf, rng, None),
        76..=85 => {
            if !neighbor_habit_anchor_into(planner, buf, rng, habit, &FxHashSet::default()) {
                ShiftOp.apply_full(planner, buf, rng, span)
            } else {
                true
            }
        }
        86..=91 => {
            if !neighbor_habit_exception_into(planner, buf, rng, habit, &FxHashSet::default()) {
                ShiftOp.apply_full(planner, buf, rng, span)
            } else {
                true
            }
        }
        _ => neighbor_lns_into(planner, buf, rng),
    }
}

/// Partial-mode in-place neighbor generation with pre-computed unpinned positions.
#[allow(clippy::too_many_arguments)]
fn generate_neighbor_partial_into(
    planner: &Planner,
    current: &[Placement],
    buf: &mut Vec<Placement>,
    rng: &mut impl Rng,
    pinned_ids: &FxHashSet<usize>,
    habit: &habit::HabitIndex,
    unpinned_positions: &[usize],
    span: i64,
) -> bool {
    if unpinned_positions.is_empty() {
        return false;
    }

    buf.clear();
    buf.extend_from_slice(current);

    let r = rng.random_range(0..100u32) as i32;

    match r {
        0..=16 => ShiftOp.apply_partial(planner, buf, rng, span, unpinned_positions),
        17..=33 => SwapOp.apply_partial(planner, buf, rng, span, unpinned_positions),
        34..=47 => DurationOp.apply_partial(planner, buf, rng, span, unpinned_positions),
        48..=61 => ReorderOp.apply_partial(planner, buf, rng, span, unpinned_positions),
        62..=75 => neighbor_repair_depend_into(planner, buf, rng, Some(pinned_ids)),
        76..=85 => {
            if !neighbor_habit_anchor_into(planner, buf, rng, habit, pinned_ids) {
                ShiftOp.apply_partial(planner, buf, rng, span, unpinned_positions)
            } else {
                true
            }
        }
        86..=91 => {
            if !neighbor_habit_exception_into(planner, buf, rng, habit, pinned_ids) {
                ShiftOp.apply_partial(planner, buf, rng, span, unpinned_positions)
            } else {
                true
            }
        }
        _ => neighbor_lns_partial_into(planner, buf, rng, pinned_ids),
    }
}

// ── Neighbor operators ──────────────────────────────────────────────
//
// The four simple neighbor types (shift, swap, duration, reorder) share a
// common skeleton: pick position(s), skip fixed tasks, compute new placement.
// `NeighborOp` abstracts the full/partial position picking so that adding a
// new simple neighbor only requires implementing `apply_at`.

/// Neighbor operator that mutates one or two schedule positions.
trait NeighborOp {
    /// Number of schedule positions this operator reads/mutates (1 or 2).
    const ARITY: usize;

    /// Core mutation at the given positions. Positions are distinct and in
    /// range. Returns false if the move is rejected (e.g. fixed task).
    fn apply_at(
        &self,
        planner: &Planner,
        scheds: &mut [Placement],
        positions: &[usize],
        rng: &mut impl Rng,
        span: i64,
    ) -> bool;

    /// Full mode: pick positions from the entire schedule.
    fn apply_full(
        &self,
        planner: &Planner,
        scheds: &mut [Placement],
        rng: &mut impl Rng,
        span: i64,
    ) -> bool {
        let n = scheds.len();
        if n < Self::ARITY {
            return false;
        }
        let positions = pick_positions_full(Self::ARITY, n, rng);
        self.apply_at(planner, scheds, &positions[..Self::ARITY], rng, span)
    }

    /// Partial mode: pick positions from the unpinned set.
    fn apply_partial(
        &self,
        planner: &Planner,
        scheds: &mut [Placement],
        rng: &mut impl Rng,
        span: i64,
        unpinned: &[usize],
    ) -> bool {
        if unpinned.len() < Self::ARITY {
            return false;
        }
        let positions = pick_positions_from(Self::ARITY, unpinned, rng);
        self.apply_at(planner, scheds, &positions[..Self::ARITY], rng, span)
    }
}

/// Pick `arity` distinct positions from `0..n`. Returns a stack-allocated
/// `[usize; 2]`; only the first `arity` elements are meaningful.
fn pick_positions_full(arity: usize, n: usize, rng: &mut impl Rng) -> [usize; 2] {
    match arity {
        1 => [rng.random_range(0..n), 0],
        2 => {
            let a = rng.random_range(0..n);
            let mut b = rng.random_range(0..n);
            if b == a {
                b = (a + 1) % n;
            }
            [a, b]
        }
        _ => unreachable!(),
    }
}

/// Pick `arity` distinct positions from a pool. Returns a stack-allocated
/// `[usize; 2]`; only the first `arity` elements are meaningful.
fn pick_positions_from(arity: usize, pool: &[usize], rng: &mut impl Rng) -> [usize; 2] {
    match arity {
        1 => [pool[rng.random_range(0..pool.len())], 0],
        2 => {
            let a_idx = rng.random_range(0..pool.len());
            let mut b_idx = rng.random_range(0..pool.len());
            if b_idx == a_idx {
                b_idx = (a_idx + 1) % pool.len();
            }
            [pool[a_idx], pool[b_idx]]
        }
        _ => unreachable!(),
    }
}

// --- Shift: move a single task to a nearby start time ---

struct ShiftOp;

impl NeighborOp for ShiftOp {
    const ARITY: usize = 1;

    fn apply_at(
        &self,
        planner: &Planner,
        scheds: &mut [Placement],
        positions: &[usize],
        rng: &mut impl Rng,
        span: i64,
    ) -> bool {
        let idx = positions[0];
        let p = scheds[idx];
        let task_id = p.task_id;
        if planner.tasks[task_id].fixed {
            return false;
        }
        let dur = p.end.0 - p.start.0;
        let range = shift_range_from_span(dur, rng, span);
        let k = rand_range(rng, -range, range + 1);
        let new_start_0 = (p.start.0 + k).max(planner.now.0);
        scheds[idx] = TaskPlacement::new(Point(new_start_0), Point(new_start_0 + dur), task_id);
        true
    }
}

// --- Swap: exchange start times of two tasks ---

struct SwapOp;

impl NeighborOp for SwapOp {
    const ARITY: usize = 2;

    fn apply_at(
        &self,
        planner: &Planner,
        scheds: &mut [Placement],
        positions: &[usize],
        _rng: &mut impl Rng,
        _span: i64,
    ) -> bool {
        let (a, b) = (positions[0], positions[1]);
        if a == b {
            return false;
        }
        let a_p = scheds[a];
        let b_p = scheds[b];
        if planner.tasks[a_p.task_id].fixed || planner.tasks[b_p.task_id].fixed {
            return false;
        }
        let a_dur = a_p.end.0 - a_p.start.0;
        let b_dur = b_p.end.0 - b_p.start.0;
        scheds[a] = TaskPlacement::new(b_p.start, Point(b_p.start.0 + a_dur), a_p.task_id);
        scheds[b] = TaskPlacement::new(a_p.start, Point(a_p.start.0 + b_dur), b_p.task_id);
        true
    }
}

// --- Duration: shrink or grow a task by one slot ---

struct DurationOp;

impl NeighborOp for DurationOp {
    const ARITY: usize = 1;

    fn apply_at(
        &self,
        planner: &Planner,
        scheds: &mut [Placement],
        positions: &[usize],
        rng: &mut impl Rng,
        _span: i64,
    ) -> bool {
        let idx = positions[0];
        let p = scheds[idx];
        let task_id = p.task_id;
        if planner.tasks[task_id].fixed {
            return false;
        }
        let dur = p.end.0 - p.start.0;
        if dur <= 1 {
            return false;
        }
        let delta: i64 = if rng.random::<bool>() { 1 } else { -1 };
        let new_dur = dur + delta;
        if new_dur < 1 {
            return false;
        }
        scheds[idx] = TaskPlacement::new(p.start, Point(p.start.0 + new_dur), task_id);
        true
    }
}

// --- Reorder: swap start times of two tasks, keeping durations ---

struct ReorderOp;

impl NeighborOp for ReorderOp {
    const ARITY: usize = 2;

    fn apply_at(
        &self,
        planner: &Planner,
        scheds: &mut [Placement],
        positions: &[usize],
        _rng: &mut impl Rng,
        _span: i64,
    ) -> bool {
        let (a, b) = (positions[0], positions[1]);
        let a_p = scheds[a];
        let b_p = scheds[b];
        if planner.tasks[a_p.task_id].fixed || planner.tasks[b_p.task_id].fixed {
            return false;
        }
        let (first, second) = if a_p.start.0 <= b_p.start.0 {
            (a, b)
        } else {
            (b, a)
        };
        let f_s = scheds[first].start;
        let f_dur = scheds[first].end.0 - f_s.0;
        let s_s = scheds[second].start;
        let s_dur = scheds[second].end.0 - s_s.0;
        scheds[first] = TaskPlacement::new(s_s, Point(s_s.0 + f_dur), scheds[first].task_id);
        scheds[second] = TaskPlacement::new(f_s, Point(f_s.0 + s_dur), scheds[second].task_id);
        true
    }
}

fn neighbor_repair_depend_into(
    planner: &Planner,
    scheds: &mut [Placement],
    rng: &mut impl Rng,
    pinned_ids: Option<&FxHashSet<usize>>,
) -> bool {
    let n = planner.tasks.len();
    let mut index: Vec<Option<TimeWindow>> = vec![None; n];
    for p in scheds.iter() {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        if id < n {
            index[id] = Some(TimeWindow::new(s, e));
        }
    }

    let mut violations: Vec<(usize, Point)> = Vec::new();
    for task in &planner.tasks {
        if let Some(p) = pinned_ids
            && p.contains(&task.id)
        {
            continue;
        }
        if task.fixed {
            continue;
        }
        let Some(tw) = index[task.id] else {
            continue;
        };
        let start = tw.start;
        let mut latest_dep_end: Option<Point> = None;
        for dep_id in &task.depends {
            if let Some(Some(dep_tw)) = index.get(*dep_id)
                && dep_tw.end > start
            {
                latest_dep_end = Some(latest_dep_end.map_or(dep_tw.end, |m| m.max(dep_tw.end)));
            }
        }
        if let Some(dep_end) = latest_dep_end {
            violations.push((task.id, dep_end));
        }
    }

    if violations.is_empty() {
        return false;
    }

    let (task_id, new_start) = violations[rng.random_range(0..violations.len())];
    let Some(pos) = scheds.iter().position(|p| p.task_id == task_id) else {
        return false;
    };
    let p = scheds[pos];
    let start = p.start;
    let end = p.end;
    let dur = end.0 - start.0;
    scheds[pos] = TaskPlacement::new(new_start, Point(new_start.0 + dur), task_id);
    true
}

/// anchor 移動の時刻帯 delta。通常は ±1 時間程度、まれに大きく探索する。
fn habit_anchor_delta(planner: &Planner, rng: &mut impl Rng) -> i64 {
    let spd = (24 * 60) / planner.per() as i64;
    let range = if rng.random_range(0..LONG_SHIFT_ONE_IN) == 0 {
        (spd / 4).max(1)
    } else {
        (spd / 24).max(1)
    };
    rand_range(rng, -range, range + 1)
}

fn neighbor_habit_anchor_into(
    planner: &Planner,
    scheds: &mut Vec<Placement>,
    rng: &mut impl Rng,
    habit: &habit::HabitIndex,
    pinned_ids: &FxHashSet<usize>,
) -> bool {
    if habit.groups.is_empty() {
        return false;
    }
    let group = &habit.groups[rng.random_range(0..habit.groups.len())];
    let delta = habit_anchor_delta(planner, rng);
    if delta == 0 {
        return false;
    }
    let plan = Plan {
        schedules: std::mem::take(scheds),
    };
    let result = habit::apply_anchor_shift(planner, &plan, group, delta, pinned_ids);
    match result {
        Some(new_plan) => {
            *scheds = new_plan.schedules;
            true
        }
        None => {
            *scheds = plan.schedules;
            false
        }
    }
}

fn neighbor_habit_exception_into(
    planner: &Planner,
    scheds: &mut Vec<Placement>,
    rng: &mut impl Rng,
    habit: &habit::HabitIndex,
    pinned_ids: &FxHashSet<usize>,
) -> bool {
    if habit.groups.is_empty() {
        return false;
    }
    let group = &habit.groups[rng.random_range(0..habit.groups.len())];
    let movable: Vec<usize> = group
        .members
        .iter()
        .copied()
        .filter(|&id| !planner.tasks()[id].fixed && !pinned_ids.contains(&id))
        .collect();
    if movable.is_empty() {
        return false;
    }
    let member = movable[rng.random_range(0..movable.len())];
    let dur = scheds
        .iter()
        .find(|p| p.task_id == member)
        .map(|p| p.end.0 - p.start.0)
        .unwrap_or(1);
    let span = plan_span_scheds(scheds);
    let range = shift_range_from_span(dur, rng, span);
    let delta = rand_range(rng, -range, range + 1);
    if delta == 0 {
        return false;
    }
    let plan = Plan {
        schedules: std::mem::take(scheds),
    };
    let result = habit::apply_member_shift(planner, &plan, member, delta, pinned_ids);
    match result {
        Some(new_plan) => {
            *scheds = new_plan.schedules;
            true
        }
        None => {
            *scheds = plan.schedules;
            false
        }
    }
}

fn neighbor_lns_into(planner: &Planner, scheds: &mut Vec<Placement>, rng: &mut impl Rng) -> bool {
    if scheds.is_empty() {
        return false;
    }

    let pivot_idx = rng.random_range(0..scheds.len());
    let pivot_p = scheds[pivot_idx];
    let pivot_start = pivot_p.start;
    let pivot_end = pivot_p.end;

    let total_dur: i64 = scheds.iter().map(|p| p.end.0 - p.start.0).sum();
    let window_size = ((pivot_end.0 - pivot_start.0) * 2)
        .max(4)
        .min(total_dur / 3 + 1);

    let window_start = pivot_start.0 - rand_range(rng, 0, window_size / 2 + 1);
    let window_end = window_start + window_size;

    let mut destroyed_ids = Vec::new();
    let mut remaining = Vec::new();
    for sched in scheds.iter() {
        if planner.tasks[sched.task_id].fixed {
            remaining.push(*sched);
        } else if sched.start.0 >= window_start && sched.start.0 < window_end {
            destroyed_ids.push(sched.task_id);
        } else {
            remaining.push(*sched);
        }
    }

    let rebuilt = greedy_rebuild(planner, &remaining, &destroyed_ids);
    *scheds = rebuilt;
    true
}

fn neighbor_lns_partial_into(
    planner: &Planner,
    scheds: &mut Vec<Placement>,
    rng: &mut impl Rng,
    pinned_ids: &FxHashSet<usize>,
) -> bool {
    if scheds.is_empty() {
        return false;
    }

    let pivot_idx = rng.random_range(0..scheds.len());
    let pivot_p = scheds[pivot_idx];
    let pivot_start = pivot_p.start;
    let pivot_end = pivot_p.end;

    let total_dur: i64 = scheds.iter().map(|p| p.end.0 - p.start.0).sum();
    let window_size = ((pivot_end.0 - pivot_start.0) * 2)
        .max(4)
        .min(total_dur / 3 + 1);

    let window_start = pivot_start.0 - rand_range(rng, 0, window_size / 2 + 1);
    let window_end = window_start + window_size;

    let mut destroyed_ids = Vec::new();
    let mut remaining = Vec::new();
    for sched in scheds.iter() {
        if planner.tasks[sched.task_id].fixed || pinned_ids.contains(&sched.task_id) {
            remaining.push(*sched);
        } else if sched.start.0 >= window_start && sched.start.0 < window_end {
            destroyed_ids.push(sched.task_id);
        } else {
            remaining.push(*sched);
        }
    }

    let rebuilt = greedy_rebuild(planner, &remaining, &destroyed_ids);
    *scheds = rebuilt;
    true
}

/// Cached plan_span that works on a slice.
fn plan_span_scheds(schedules: &[Placement]) -> i64 {
    let min_s = schedules.iter().map(|p| p.start.0).min();
    let max_e = schedules.iter().map(|p| p.end.0).max();
    match (min_s, max_e) {
        (Some(a), Some(b)) => (b - a).max(1),
        _ => 1,
    }
}

/// shift_range using a pre-computed span to avoid re-scanning the schedule.
fn shift_range_from_span(dur: i64, rng: &mut impl Rng, span: i64) -> i64 {
    if rng.random_range(0..LONG_SHIFT_ONE_IN) == 0 {
        span
    } else {
        (dur / 2).max(1)
    }
}

/// Incremental decode: 生存タスクの配置を維持し、除去タスクのみ再配置する。
/// 全タスク再配置の decode() O(n²) に対し、O(removed × n) で済む。
/// removed は priority 順 (repair 済み) で渡される。
///
/// 注意: repair operator の配置戦略 (Regret2/LowestDelta/HabitAnchor 等) は
/// 使用せず、全て Earliest 配置する。repair operator の多様性は
/// repair_priority の priority ソート順序を通じて間接的に反映される。
/// これは速度と探索品質のトレードオフ: full decode の配置戦略を維持すると
/// O(n²) に戻るため、incremental decode では簡略化している。
fn incremental_decode(
    planner: &Planner,
    current_schedules: &[Placement],
    removed: &[usize],
    dependents: &[Vec<usize>],
) -> Vec<Placement> {
    let n = planner.tasks.len();
    let removed_set: FxHashSet<usize> = removed.iter().copied().collect();

    // 生存タスクの配置を維持
    let mut scheds: Vec<Placement> = current_schedules
        .iter()
        .filter(|p| !removed_set.contains(&p.task_id))
        .copied()
        .collect();

    // 生存タスクの index (latest_end 計算用)
    let mut pos_index: Vec<Option<TimeWindow>> = vec![None; n];
    for p in &scheds {
        let s = p.start;
        let e = p.end;
        let id = p.task_id;
        if id < n {
            pos_index[id] = Some(TimeWindow::new(s, e));
        }
    }

    let mut placed: FxHashSet<usize> = scheds.iter().map(|p| p.task_id).collect();
    let mut pending: Vec<usize> = removed.to_vec();

    while !pending.is_empty() {
        let mut next_pending = Vec::new();
        let mut progressed = false;

        for &task_id in &pending {
            let task = &planner.tasks[task_id];
            let deps_ready = task
                .depends
                .iter()
                .all(|d| *d >= n || placed.contains(d) || !removed_set.contains(d));
            if !deps_ready {
                next_pending.push(task_id);
                continue;
            }

            let dur = (task.cost_estimate.avg as i64).max(1);
            let earliest = compute_earliest_indexed(planner, &pos_index, task);
            let latest_end = dependents[task_id]
                .iter()
                .filter_map(|&d| pos_index[d].map(|tw| tw.start))
                .min();
            if let Ok(tw) = try_place::<false>(planner, &scheds, task, earliest, dur, latest_end) {
                scheds.push(TaskPlacement::new(tw.start, tw.end, task_id));
                pos_index[task_id] = Some(tw);
            } else {
                let (start, end, _err) =
                    fallback_for::<false>(planner, &scheds, earliest, dur, latest_end, task);
                scheds.push(TaskPlacement::new(start, end, task_id));
                pos_index[task_id] = Some(TimeWindow::new(start, end));
            }
            placed.insert(task_id);
            progressed = true;
        }

        if !progressed {
            for &task_id in &next_pending {
                let task = &planner.tasks[task_id];
                let dur = (task.cost_estimate.avg as i64).max(1);
                let earliest = compute_earliest_indexed(planner, &pos_index, task);
                let latest_end = dependents[task_id]
                    .iter()
                    .filter_map(|&d| pos_index[d].map(|tw| tw.start))
                    .min();
                let (start, end, _err) =
                    fallback_for::<false>(planner, &scheds, earliest, dur, latest_end, task);
                scheds.push(TaskPlacement::new(start, end, task_id));
                pos_index[task_id] = Some(TimeWindow::new(start, end));
            }
            break;
        }
        pending = next_pending;
    }

    scheds
}

fn greedy_rebuild(planner: &Planner, existing: &[Placement], task_ids: &[usize]) -> Vec<Placement> {
    let mut scheds = existing.to_vec();

    let mut pending: Vec<usize> = task_ids.to_vec();
    pending.sort_by(|a, b| {
        planner
            .freeness(*a)
            .partial_cmp(&planner.freeness(*b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let destroyed: FxHashSet<usize> = task_ids.iter().copied().collect();
    let mut placed: FxHashSet<usize> = FxHashSet::default();

    // 固定タスクを先に配置して、通常タスクの try_place が重複を避けられるようにする
    // (#391)。start = None の固定タスクは依存解決ループに残して、依存順序を守る。
    for &task_id in &pending {
        let task = &planner.tasks[task_id];
        if task.fixed && task.start.is_some() {
            place_one(planner, &mut scheds, task_id);
            placed.insert(task_id);
        }
    }
    pending.retain(|id| !placed.contains(id));

    // 依存先が先に配置されるよう、配置可能なタスクから複数パスで配置する。
    while !pending.is_empty() {
        let mut progressed = false;
        let mut next_pending = Vec::new();

        for task_id in pending {
            let task = &planner.tasks[task_id];
            let deps_ready = task
                .depends
                .iter()
                .all(|d| !destroyed.contains(d) || placed.contains(d));
            if !deps_ready {
                next_pending.push(task_id);
                continue;
            }

            place_one(planner, &mut scheds, task_id);
            placed.insert(task_id);
            progressed = true;
        }

        if !progressed {
            for task_id in next_pending {
                place_one(planner, &mut scheds, task_id);
            }
            break;
        }
        pending = next_pending;
    }

    scheds
}

fn place_one(planner: &Planner, scheds: &mut Vec<Placement>, task_id: usize) {
    let task = &planner.tasks[task_id];
    // build_initial と同様、avg=0 のタスクは dur=1 として配置する。
    // さもないと iCal 由来の avg=0 タスクが LNS/repair_polish の再構築で
    // サイレントにドロップされてしまう (inclusion_bonus の不整合)。
    let dur = (task.cost_estimate.avg as i64).max(1);
    // 固定タスクは start に直接配置
    if task.fixed
        && let Some(start) = task.start
    {
        let end = Point(start.0 + dur);
        scheds.push(TaskPlacement::new(start, end, task_id));
        return;
    }
    let earliest = compute_earliest(planner, scheds, task);
    if let Ok(tw) = try_place::<false>(planner, scheds, task, earliest, dur, None) {
        let start = tw.start;
        let end = tw.end;
        scheds.push(TaskPlacement::new(start, end, task_id));
    } else {
        // build_initial と同様、配置できない場合は末尾に fallback してタスクを落とさない。
        let last_end = scheds
            .iter()
            .map(|p| p.end.0)
            .max()
            .unwrap_or(planner.now.0);
        let _ = push_fallback(planner, scheds, earliest, dur, task_id, Point(last_end));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rng;

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

    fn simple_task(id: usize, start: i64, dur: u64, fixed: bool) -> Task {
        Task {
            id,
            start: Some(Point(start)),
            end: Point(start + 100),
            cost_estimate: NormalDist::new(dur, 0),
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed,
            habit_group: None,
        }
    }

    #[test]
    fn shift_op_preserves_duration_and_rejects_fixed() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;

        let planner = test_planner(vec![simple_task(0, 10, 4, false)]);
        let mut buf = vec![TaskPlacement::new(Point(10), Point(14), 0)];
        let mut rng = StdRng::seed_from_u64(0);
        let span = 100;

        // Succeeds and preserves duration
        assert!(ShiftOp.apply_full(&planner, &mut buf, &mut rng, span));
        assert_eq!(buf[0].end.0 - buf[0].start.0, 4);
        assert!(buf[0].start.0 >= planner.now.0);

        // Fixed task is rejected
        let planner_fixed = test_planner(vec![simple_task(0, 10, 4, true)]);
        let mut buf2 = vec![TaskPlacement::new(Point(10), Point(14), 0)];
        let positions = [0usize, 0];
        assert!(!ShiftOp.apply_at(&planner_fixed, &mut buf2, &positions[..1], &mut rng, span));
    }

    #[test]
    fn swap_op_exchanges_start_times() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;

        let planner = test_planner(vec![
            simple_task(0, 10, 4, false),
            simple_task(1, 20, 6, false),
        ]);
        let mut buf = vec![
            TaskPlacement::new(Point(10), Point(14), 0),
            TaskPlacement::new(Point(20), Point(26), 1),
        ];
        let mut rng = StdRng::seed_from_u64(0);

        // Apply swap at explicit positions
        let positions = [0usize, 1];
        assert!(SwapOp.apply_at(&planner, &mut buf, &positions, &mut rng, 0));
        // Task 0 should now start at 20 (task 1's old start), task 1 at 10
        assert_eq!(buf[0].start.0, 20);
        assert_eq!(buf[1].start.0, 10);
        // Durations preserved
        assert_eq!(buf[0].end.0 - buf[0].start.0, 4);
        assert_eq!(buf[1].end.0 - buf[1].start.0, 6);
    }

    #[test]
    fn duration_op_changes_by_one() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;

        let planner = test_planner(vec![simple_task(0, 10, 4, false)]);
        let mut buf = vec![TaskPlacement::new(Point(10), Point(14), 0)];
        let orig_dur = buf[0].end.0 - buf[0].start.0;
        let mut rng = StdRng::seed_from_u64(0);

        let positions = [0usize, 0];
        assert!(DurationOp.apply_at(&planner, &mut buf, &positions[..1], &mut rng, 0));
        let new_dur = buf[0].end.0 - buf[0].start.0;
        assert!(
            (new_dur - orig_dur).abs() == 1,
            "duration should change by ±1, got {new_dur}"
        );
        assert_eq!(buf[0].start.0, 10, "start should not change");
    }

    #[test]
    fn reorder_op_swaps_starts_keeping_durations() {
        let planner = test_planner(vec![
            simple_task(0, 10, 4, false),
            simple_task(1, 20, 6, false),
        ]);
        let mut buf = vec![
            TaskPlacement::new(Point(10), Point(14), 0),
            TaskPlacement::new(Point(20), Point(26), 1),
        ];
        let mut rng = rand::rng();

        let positions = [0usize, 1];
        assert!(ReorderOp.apply_at(&planner, &mut buf, &positions, &mut rng, 0));
        // First (earlier) position gets second's start, second gets first's start
        assert_eq!(buf[0].start.0, 20);
        assert_eq!(buf[1].start.0, 10);
        // Durations preserved
        assert_eq!(buf[0].end.0 - buf[0].start.0, 4);
        assert_eq!(buf[1].end.0 - buf[1].start.0, 6);
    }

    #[test]
    fn arity_guard_rejects_small_schedules() {
        let planner = test_planner(vec![simple_task(0, 10, 4, false)]);
        let mut buf = vec![TaskPlacement::new(Point(10), Point(14), 0)];
        let mut rng = rand::rng();

        // Swap needs 2 positions, only 1 available
        assert!(!SwapOp.apply_full(&planner, &mut buf, &mut rng, 0));
        // Reorder needs 2 positions, only 1 available
        assert!(!ReorderOp.apply_full(&planner, &mut buf, &mut rng, 0));
    }

    #[test]
    fn partial_mode_uses_unpinned_positions() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;

        let planner = test_planner(vec![
            simple_task(0, 10, 4, false),
            simple_task(1, 20, 6, false),
            simple_task(2, 30, 4, true),
        ]);
        let mut buf = vec![
            TaskPlacement::new(Point(10), Point(14), 0),
            TaskPlacement::new(Point(20), Point(26), 1),
            TaskPlacement::new(Point(30), Point(34), 2),
        ];
        let mut rng = StdRng::seed_from_u64(0);
        // Only positions 0 and 1 are unpinned
        let unpinned = [0usize, 1];

        // Shift should only touch unpinned positions
        assert!(ShiftOp.apply_partial(&planner, &mut buf, &mut rng, 100, &unpinned));
        // Position 2 (fixed) should be unchanged
        assert_eq!(buf[2].start.0, 30);
        assert_eq!(buf[2].end.0, 34);
    }

    #[test]
    fn habit_anchor_neighbor_snaps_group_to_common_tod() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;

        let spd = 288;
        // 4 日の daily habit。task.start (最早) は day+100 で統一し、
        // 初期配置だけ day+100+off にずらして anchor move の収束を確認する。
        let tasks: Vec<Task> = (0..4)
            .map(|id| {
                let day_start = (id as i64 + 1) * spd;
                Task {
                    id,
                    start: Some(Point(day_start + 100)),
                    end: Point(day_start + 100 + 60),
                    cost_estimate: NormalDist::new(6, 0),
                    depends: vec![],
                    parallelizable: false,
                    allows_parallel: false,
                    abandonability: 0.3.into(),
                    fixed: false,
                    habit_group: Some(0),
                }
            })
            .collect();
        let planner = test_planner(tasks);

        let offsets = [0i64, 4, 8, 12];
        let current = Plan {
            schedules: planner
                .tasks()
                .iter()
                .zip(offsets.iter())
                .map(|(t, off)| {
                    let s = Point(t.start.unwrap().0 + off);
                    TaskPlacement::new(s, Point(s.0 + 6), t.id)
                })
                .collect(),
        };

        let mut saw_some = false;
        let habit_index = habit::build_index(&planner);
        let pinned = FxHashSet::default();
        for seed in 0..32u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let mut buf = current.schedules.clone();
            if neighbor_habit_anchor_into(&planner, &mut buf, &mut rng, &habit_index, &pinned) {
                saw_some = true;
                let tods_after: Vec<i64> = buf.iter().map(|p| p.start.0.rem_euclid(spd)).collect();
                let first = tods_after[0];
                assert!(
                    tods_after.iter().all(|t| *t == first),
                    "seed {seed}: all habit members should share one tod: {tods_after:?}"
                );
            }
        }
        assert!(
            saw_some,
            "anchor neighbor should succeed for at least one seed"
        );
    }

    fn habit_task_at(id: usize, day_start: i64, tod: i64, group: usize) -> Task {
        Task {
            id,
            start: Some(Point(day_start + tod)),
            end: Point(day_start + tod + 60),
            cost_estimate: NormalDist::new(6, 0),
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.3.into(),
            fixed: false,
            habit_group: Some(group),
        }
    }

    #[test]
    fn destroy_habit_group_removes_whole_group() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;

        let spd = 288;
        let mut tasks = vec![];
        for d in 0..3 {
            tasks.push(habit_task_at(tasks.len(), (d as i64 + 1) * spd, 108, 0));
        }
        for d in 0..2 {
            tasks.push(habit_task_at(tasks.len(), (d as i64 + 1) * spd, 200, 1));
        }
        let mut non_habit = habit_task_at(tasks.len(), spd, 50, 0);
        non_habit.habit_group = None;
        tasks.push(non_habit);

        let planner = test_planner(tasks);
        let plan = Plan {
            schedules: planner
                .tasks()
                .iter()
                .map(|t| {
                    let s = t.start.unwrap();
                    TaskPlacement::new(s, Point(s.0 + 6), t.id)
                })
                .collect(),
        };
        let priority: Vec<usize> = (0..planner.tasks.len()).collect();
        let pinned = FxHashSet::default();
        let mut rng = StdRng::seed_from_u64(0);
        let habit_index = habit::build_index(&planner);

        let removed = destroy_priority(
            &planner,
            &priority,
            &plan,
            &pinned,
            &mut rng,
            DestroyOperator::HabitGroup,
            2,
            &habit_index,
        );
        let removed_set: FxHashSet<usize> = removed.iter().copied().collect();
        let group0: FxHashSet<usize> = [0, 1, 2].into_iter().collect();
        let group1: FxHashSet<usize> = [3, 4].into_iter().collect();
        assert!(
            removed_set == group0 || removed_set == group1,
            "HabitGroup destroy should remove exactly one whole group, got {removed:?}"
        );
    }

    #[test]
    fn habit_anchor_repair_keeps_group_consistent() {
        let spd = 288;
        let tasks: Vec<Task> = (0..4)
            .map(|id| habit_task_at(id, (id as i64 + 1) * spd, 108, 0))
            .collect();
        let planner = test_planner(tasks);
        let priority: Vec<usize> = (0..4).collect();

        let result = decode(
            &planner,
            DecodeInput {
                priority: &priority,
                duration_choices: &[],
                pinned: &[],
                repair_mode: RepairMode::HabitAnchor,
            },
        );
        let tods: Vec<i64> = result
            .plan
            .schedules
            .iter()
            .map(|p| p.start.0.rem_euclid(spd))
            .collect();
        let first = tods[0];
        assert!(
            tods.iter().all(|t| *t == first),
            "HabitAnchor repair should keep the group at one time-of-day: {tods:?}"
        );
    }

    #[test]
    fn priority_decoder_respects_dependency_order() {
        let first = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 4, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
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
        let result = crate::decoder::decode(
            &planner,
            crate::decoder::DecodeInput {
                priority: &[1, 0],
                duration_choices: &[],
                pinned: &[],
                repair_mode: crate::decoder::RepairMode::Earliest,
            },
        );
        let plan = result.plan;
        assert_eq!(plan.schedules.len(), 2);
        assert!(plan.task_end(0).unwrap() <= plan.task_start(1).unwrap());
    }

    #[test]
    fn build_initial_dependency_order() {
        let t0 = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let t1 = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![0],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![t0, t1]);
        let plan = build_initial(&p);
        assert_eq!(plan.schedules.len(), 2, "both tasks should be scheduled");
        let t0_entry = plan.schedules.iter().find(|p| p.task_id == 0).unwrap();
        let t1_entry = plan.schedules.iter().find(|p| p.task_id == 1).unwrap();
        assert!(
            t0_entry.end.0 <= t1_entry.start.0,
            "task 0 must end before task 1 starts"
        );
    }

    #[test]
    fn build_initial_schedules_all() {
        let t0 = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let t1 = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![t0, t1]);
        let plan = build_initial(&p);
        assert_eq!(plan.schedules.len(), 2, "all tasks should be scheduled");
    }

    #[test]
    fn sa_lns_finds_buffer_ordering() {
        let t0 = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(5),
            cost_estimate: NormalDist { avg: 1, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let t1 = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(5),
            cost_estimate: NormalDist { avg: 1, sigma: 2 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![t0, t1]);
        let mut rng = rng();
        let plan = sa_lns(&p, &mut rng);

        assert_eq!(plan.schedules.len(), 2, "both tasks should be scheduled");

        let b_entry = plan.schedules.iter().find(|p| p.task_id == 1).unwrap();
        let a_entry = plan.schedules.iter().find(|p| p.task_id == 0).unwrap();

        let b_score = evaluate(&p, &plan, 0.0, 1.0);
        let swapped = Plan {
            schedules: vec![
                TaskPlacement::new(a_entry.start, a_entry.end, b_entry.task_id),
                TaskPlacement::new(b_entry.start, b_entry.end, a_entry.task_id),
            ],
        };
        let swapped_score = evaluate(&p, &swapped, 0.0, 1.0);

        assert!(
            b_score >= swapped_score,
            "A→B should score at least as well as B→A: b_score={b_score} swapped={swapped_score}"
        );
    }

    #[test]
    fn sa_lns_respects_dependencies() {
        let t0 = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let t1 = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![0],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![t0, t1]);
        let mut rng = rng();
        let plan = sa_lns(&p, &mut rng);

        let t0_entry = plan.schedules.iter().find(|p| p.task_id == 0).unwrap();
        let t1_entry = plan.schedules.iter().find(|p| p.task_id == 1).unwrap();
        assert!(
            t0_entry.end.0 <= t1_entry.start.0,
            "SA must respect dependencies"
        );
    }

    // Regression: zero-avg tasks (e.g. iCal imports with avg_minutes=0) must
    // not be silently dropped by greedy_rebuild/place_one. build_initial
    // places them with dur=1, so the rebuild path must be consistent.
    #[test]
    fn greedy_rebuild_keeps_zero_avg_task() {
        let zero = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 0, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let other = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 3, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![zero, other]);

        // Destroy both and rebuild from empty — both must come back.
        let rebuilt = greedy_rebuild(&p, &[], &[0, 1]);
        assert_eq!(
            rebuilt.len(),
            2,
            "zero-avg task must not be dropped by greedy_rebuild: {rebuilt:?}"
        );
        assert!(rebuilt.iter().any(|p| p.task_id == 0));
    }

    // Regression (#391): fixed-time tasks must not overlap with normal tasks.
    // Fixed tasks are placed first so that normal tasks' try_place avoids
    // the fixed task's time slot.
    #[test]
    fn build_initial_fixed_task_no_overlap() {
        // Fixed task at slot 2..4, normal task with tight deadline that
        // would naturally be placed at now=0..4 if overlap weren't checked.
        let fixed = Task {
            id: 0,
            start: Some(Point(2)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: true,
            habit_group: None,
        };
        let normal = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![fixed, normal]);
        let plan = build_initial(&p);
        assert_eq!(plan.schedules.len(), 2);

        let f = plan.schedules.iter().find(|p| p.task_id == 0).unwrap();
        let n = plan.schedules.iter().find(|p| p.task_id == 1).unwrap();

        // Fixed task must be at its start time.
        assert_eq!(f.start.0, 2, "fixed task must be at its start time");
        assert_eq!(f.end.0, 4, "fixed task end");

        // Normal task must not overlap with the fixed task.
        assert!(
            n.end.0 <= f.start.0 || n.start.0 >= f.end.0,
            "normal task [{}, {}) must not overlap fixed task [{}, {})",
            n.start.0,
            n.end.0,
            f.start.0,
            f.end.0
        );
    }

    // Regression (#391): same overlap check for build_initial_partial.
    #[test]
    fn build_initial_partial_fixed_task_no_overlap() {
        let fixed = Task {
            id: 0,
            start: Some(Point(2)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: true,
            habit_group: None,
        };
        let normal = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![fixed, normal]);
        let plan = build_initial_partial(&p, &[]);

        let f = plan.schedules.iter().find(|p| p.task_id == 0).unwrap();
        let n = plan.schedules.iter().find(|p| p.task_id == 1).unwrap();

        assert_eq!(f.start.0, 2, "fixed task must be at its start time");
        assert!(
            n.end.0 <= f.start.0 || n.start.0 >= f.end.0,
            "normal task [{}, {}) must not overlap fixed task [{}, {})",
            n.start.0,
            n.end.0,
            f.start.0,
            f.end.0
        );
    }

    // Regression (#780): build_initial_partial must respect dependency order
    // even when the dependent task has a lower freeness (more urgent) than its
    // dependency. Currently unpinned tasks are sorted only by freeness, so a
    // dependent can be placed before the task it depends on.
    #[test]
    fn regression_780_build_initial_partial_dependency_order() {
        let dep = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let dependent = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![0],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![dep, dependent]);
        let plan = build_initial_partial(&p, &[]);

        let dep_entry = plan.schedules.iter().find(|p| p.task_id == 0).unwrap();
        let dependent_entry = plan.schedules.iter().find(|p| p.task_id == 1).unwrap();

        assert!(
            dependent_entry.start.0 >= dep_entry.end.0,
            "dependent task [{}, {}) must start after dependency [{}, {}) ends",
            dependent_entry.start.0,
            dependent_entry.end.0,
            dep_entry.start.0,
            dep_entry.end.0
        );
    }

    // Regression (#391): greedy_rebuild must also place fixed tasks first.
    #[test]
    fn greedy_rebuild_fixed_task_no_overlap() {
        let fixed = Task {
            id: 0,
            start: Some(Point(2)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: true,
            habit_group: None,
        };
        let normal = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 2, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![fixed, normal]);
        let rebuilt = greedy_rebuild(&p, &[], &[0, 1]);

        let f = rebuilt.iter().find(|p| p.task_id == 0).unwrap();
        let n = rebuilt.iter().find(|p| p.task_id == 1).unwrap();

        assert_eq!(f.start.0, 2, "fixed task must be at its start time");
        assert!(
            n.end.0 <= f.start.0 || n.start.0 >= f.end.0,
            "normal task [{}, {}) must not overlap fixed task [{}, {})",
            n.start.0,
            n.end.0,
            f.start.0,
            f.end.0
        );
    }

    // Regression: repair_polish must not drop a zero-avg violator. Even when
    // the violator has avg=0, it should be re-placed rather than removed.
    #[test]
    fn repair_polish_keeps_zero_avg_violator() {
        let dep = Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 5, sigma: 0 },
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        // zero-avg task that depends on dep, but is scheduled before dep ends.
        let violator = Task {
            id: 1,
            start: Some(Point(0)),
            end: Point(100),
            cost_estimate: NormalDist { avg: 0, sigma: 0 },
            depends: vec![0],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        };
        let p = test_planner(vec![dep, violator]);
        // Force a dependency violation: dep ends at 5, violator starts at 0.
        let bad = Plan {
            schedules: vec![
                TaskPlacement::new(Point(0), Point(5), 0),
                TaskPlacement::new(Point(0), Point(1), 1),
            ],
        };
        let polished = repair_polish(&p, bad, None);
        assert_eq!(
            polished.schedules.len(),
            2,
            "zero-avg violator must not be dropped by repair_polish: {:?}",
            polished.schedules
        );
        assert!(polished.schedules.iter().any(|p| p.task_id == 1));
    }

    mod alns_tests {
        use super::*;
        use rand::SeedableRng;
        use rand::rngs::StdRng;

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

        #[test]
        fn alns_search_schedules_all_tasks() {
            let a = Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist { avg: 5, sigma: 0 },
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            };
            let b = Task {
                id: 1,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist { avg: 5, sigma: 0 },
                depends: vec![0],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            };
            let planner = test_planner(vec![a, b]);
            let mut rng = StdRng::seed_from_u64(42);
            let result = alns_search_pinned(&planner, &[], &mut rng);
            assert_eq!(result.plan.schedules.len(), planner.tasks.len());
        }

        #[test]
        fn alns_search_finds_better_plan_than_priority_order() {
            let a = Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist { avg: 10, sigma: 0 },
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            };
            let b = Task {
                id: 1,
                start: Some(Point(0)),
                end: Point(30),
                cost_estimate: NormalDist { avg: 10, sigma: 0 },
                depends: vec![0],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            };
            let planner = test_planner(vec![a, b]);
            let mut rng = StdRng::seed_from_u64(42);
            let alns_result = alns_search_pinned(&planner, &[], &mut rng);
            let priority_plan = priority_order_search(&planner, &mut StdRng::seed_from_u64(42));
            let alns_score = evaluate(&planner, &alns_result.plan, 0.0, 1.0);
            let priority_score = evaluate(&planner, &priority_plan, 0.0, 1.0);
            assert!(
                alns_score >= priority_score,
                "ALNS should at least match priority decoder: alns={alns_score}, priority={priority_score}"
            );
        }

        #[test]
        fn alns_warm_start_uses_previous_schedule() {
            let task = Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist { avg: 1, sigma: 0 },
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            };
            let mut planner = test_planner(vec![task]);
            planner.set_warm_start(true);
            planner.set_previous_schedule(&[TaskPlacement::new(Point(42), Point(43), 0)]);
            let mut rng = StdRng::seed_from_u64(1);
            let result = alns_search_pinned(&planner, &[], &mut rng);
            assert_eq!(result.plan.task_start(0), Some(Point(42)));
        }

        #[test]
        fn alns_destroy_random_returns_expected_count() {
            let tasks: Vec<_> = (0..10)
                .map(|id| Task {
                    id,
                    start: Some(Point(0)),
                    end: Point(100),
                    cost_estimate: NormalDist { avg: 2, sigma: 0 },
                    depends: vec![],
                    parallelizable: false,
                    allows_parallel: false,
                    abandonability: 0.5.into(),
                    fixed: false,
                    habit_group: None,
                })
                .collect();
            let planner = test_planner(tasks);
            let priority: Vec<_> = planner.tasks.iter().map(|t| t.id).collect();
            let plan = decode(
                &planner,
                DecodeInput {
                    priority: &priority,
                    duration_choices: &[],
                    pinned: &[],
                    repair_mode: RepairMode::Earliest,
                },
            )
            .plan;
            let mut rng = StdRng::seed_from_u64(7);
            let removed = destroy_priority(
                &planner,
                &priority,
                &plan,
                &FxHashSet::default(),
                &mut rng,
                DestroyOperator::Random,
                4,
                &habit::HabitIndex::default(),
            );
            assert_eq!(removed.len(), 4);
            assert!(removed.iter().all(|id| *id < planner.tasks.len()));
        }

        #[test]
        fn alns_repair_reinserts_all_removed() {
            let tasks: Vec<_> = (0..5)
                .map(|id| Task {
                    id,
                    start: Some(Point(0)),
                    end: Point(100),
                    cost_estimate: NormalDist { avg: 2, sigma: 0 },
                    depends: vec![],
                    parallelizable: false,
                    allows_parallel: false,
                    abandonability: 0.5.into(),
                    fixed: false,
                    habit_group: None,
                })
                .collect();
            let planner = test_planner(tasks);
            let partial = vec![0, 1];
            let removed = vec![2, 3, 4];
            let repaired = repair_priority(
                &planner,
                &partial,
                &removed,
                RepairOperator::Earliest,
                &mut StdRng::seed_from_u64(0),
            );
            assert_eq!(repaired.len(), planner.tasks.len());
            let mut sorted = repaired.clone();
            sorted.sort();
            assert_eq!(sorted, (0..planner.tasks.len()).collect::<Vec<_>>());
        }

        #[test]
        fn alns_weights_update_without_panic() {
            let mut weights = vec![1.0; 3];
            let mut scores = vec![0.0; 3];
            let usages = vec![1, 2, 0];
            scores[0] = 33.0;
            scores[1] = 9.0;
            update_operator_weights(&mut weights, &scores, &usages, 0.1);
            assert!(weights.iter().all(|w| *w > 0.0));
            assert!((weights.iter().sum::<f64>() - 3.0).abs() < 1e-6);
        }
    }
}
