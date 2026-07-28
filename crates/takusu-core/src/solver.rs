//! # ソルバー: 設定に応じた SA / priority dispatch
//!
//! `Planner.solver` / `time_budget` / `seed` / `warm_start` をもとに、
//! SA (`sa_lns`) または priority decoder + ALNS (`alns_search`) を選択する。
//! full / partial / range は pinned 集合の違いとして統一し、同じ dispatch 経路で解く。

use std::cmp::Ordering;
use std::time::{Duration, Instant};

use rand::SeedableRng;
use rand::rngs::StdRng;
use rayon::prelude::*;

use super::*;
use anneal::{alns_search_pinned, sa_lns, sa_lns_partial};
use evaluate::evaluate;

const MAX_CHAINS: usize = 4;
const DEFAULT_SEED: u64 = 0;
const MIN_REMAINING_TIME: Duration = Duration::from_millis(1);

fn base_seed(planner: &Planner, override_seed: Option<u64>) -> u64 {
    override_seed.or(planner.seed).unwrap_or(DEFAULT_SEED)
}

/// full solve: `Planner` の設定に従って solver を選択する。
pub fn solve(planner: &Planner) -> Plan {
    match planner.solver {
        Solver::Sa => solve_sa(planner, None),
        Solver::Priority => solve_priority(planner, &[], None),
        Solver::Auto => solve_auto(planner, &[]),
    }
}

/// 単一 seed で SA full solve を実行する（solver 設定に関わらず SA）。
pub fn solve_with_seed(planner: &Planner, seed: u64) -> Plan {
    solve_sa_with_seed(planner, seed)
}

/// 単一 seed で priority/ALNS full solve を実行する。
pub fn solve_alns_with_seed(planner: &Planner, seed: u64) -> Plan {
    solve_priority(planner, &[], Some(seed))
}

/// partial / range solve: pinned 集合を固定して再スケジュールする。
pub fn solve_partial(planner: &Planner, pinned: &[TaskPlacement]) -> Plan {
    let pinned = validate_pinned(planner, pinned);
    match planner.solver {
        Solver::Sa => solve_sa_partial(planner, &pinned),
        Solver::Priority => solve_priority(planner, &pinned, None),
        Solver::Auto => solve_auto(planner, &pinned),
    }
}

/// 単一 seed で SA partial solve を実行する（solver 設定に関わらず SA）。
pub fn solve_partial_with_seed(planner: &Planner, pinned: &[TaskPlacement], seed: u64) -> Plan {
    let pinned = validate_pinned(planner, pinned);
    solve_sa_partial_with_seed(planner, &pinned, seed)
}

fn validate_pinned(planner: &Planner, pinned: &[TaskPlacement]) -> Vec<TaskPlacement> {
    let mut seen = std::collections::HashSet::new();
    pinned
        .iter()
        .filter(|p| p.task_id < planner.tasks.len())
        .copied()
        .filter(|p| seen.insert(p.task_id))
        .collect()
}

fn solve_sa(planner: &Planner, override_seed: Option<u64>) -> Plan {
    let num_chains = rayon::current_num_threads().clamp(1, MAX_CHAINS);
    let base = base_seed(planner, override_seed);

    (0..num_chains)
        .into_par_iter()
        .map(|i| sa_lns(planner, &mut StdRng::seed_from_u64(base + i as u64)))
        .max_by(|a, b| {
            evaluate(planner, a, 0.0, 1.0)
                .partial_cmp(&evaluate(planner, b, 0.0, 1.0))
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or_else(|| Plan { schedules: vec![] })
}

fn solve_sa_with_seed(planner: &Planner, seed: u64) -> Plan {
    sa_lns(planner, &mut StdRng::seed_from_u64(seed))
}

fn solve_sa_partial(planner: &Planner, pinned: &[TaskPlacement]) -> Plan {
    if pinned.is_empty() {
        return solve_sa(planner, None);
    }

    let num_chains = rayon::current_num_threads().clamp(1, MAX_CHAINS);
    let base = base_seed(planner, None);

    (0..num_chains)
        .into_par_iter()
        .map(|i| sa_lns_partial(planner, pinned, &mut StdRng::seed_from_u64(base + i as u64)))
        .max_by(|a, b| {
            evaluate(planner, a, 0.0, 1.0)
                .partial_cmp(&evaluate(planner, b, 0.0, 1.0))
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or_else(|| Plan { schedules: vec![] })
}

fn solve_sa_partial_with_seed(planner: &Planner, pinned: &[TaskPlacement], seed: u64) -> Plan {
    if pinned.is_empty() {
        return solve_sa_with_seed(planner, seed);
    }
    sa_lns_partial(planner, pinned, &mut StdRng::seed_from_u64(seed))
}

/// 並列 ALNS チェーンを使うタスク数の閾値。小規模問題では rayon の
/// オーバーヘッドが並列化の利益を上回るため、単一チェーンで実行する。
const PARALLEL_ALNS_MIN_TASKS: usize = 50;

fn solve_priority_result(
    planner: &Planner,
    pinned: &[TaskPlacement],
    override_seed: Option<u64>,
) -> DecodeResult {
    let base = base_seed(planner, override_seed);

    if planner.tasks.len() < PARALLEL_ALNS_MIN_TASKS {
        return alns_search_pinned(planner, pinned, &mut StdRng::seed_from_u64(base));
    }

    let num_chains = rayon::current_num_threads().clamp(1, MAX_CHAINS);

    (0..num_chains)
        .into_par_iter()
        .map(|i| alns_search_pinned(planner, pinned, &mut StdRng::seed_from_u64(base + i as u64)))
        .max_by(|a, b| {
            evaluate(planner, &a.plan, 0.0, 1.0)
                .partial_cmp(&evaluate(planner, &b.plan, 0.0, 1.0))
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or_else(|| DecodeResult {
            plan: Plan { schedules: vec![] },
            diagnostics: DecodeDiagnostics::default(),
            status: DecodeStatus::Infeasible,
        })
}

fn solve_priority(planner: &Planner, pinned: &[TaskPlacement], override_seed: Option<u64>) -> Plan {
    solve_priority_result(planner, pinned, override_seed).plan
}

/// Auto: まず priority/ALNS を試し、実行不可能または制約緩和（Relaxed）なら SA に fallback する。
/// time budget を超えないよう priority 実行後の残り時間を SA に渡す。
/// priority が time budget を使い切した場合は SA fallback を実行しない。
fn solve_auto(planner: &Planner, pinned: &[TaskPlacement]) -> Plan {
    let start = Instant::now();
    let priority_result = solve_priority_result(planner, pinned, None);
    if priority_result.status == DecodeStatus::Feasible {
        return priority_result.plan;
    }

    let remaining = planner
        .time_budget
        .map(|b| b.saturating_sub(start.elapsed()).max(MIN_REMAINING_TIME));
    if remaining.is_some_and(|r| r <= MIN_REMAINING_TIME) {
        return priority_result.plan;
    }

    let mut sa_planner = planner.clone();
    sa_planner.set_time_budget(remaining);
    let sa_plan = if pinned.is_empty() {
        solve_sa(&sa_planner, None)
    } else {
        solve_sa_partial(&sa_planner, pinned)
    };

    if evaluate(planner, &sa_plan, 0.0, 1.0) > evaluate(planner, &priority_result.plan, 0.0, 1.0) {
        sa_plan
    } else {
        priority_result.plan
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{NormalDist, Planner, SleepConfig, Solver, Task};

    fn make_planner(task_count: usize) -> Planner {
        let mut planner = Planner::new(Point(0), SleepConfig::disabled());
        for i in 0..task_count {
            planner
                .add(Task {
                    id: 0,
                    start: None,
                    end: Point(288 * 7),
                    cost_estimate: NormalDist::new(12, 2),
                    depends: if i > 0 { vec![i - 1] } else { vec![] },
                    parallel_mode: ParallelMode::Exclusive,
                    abandonability: 0.5.into(),
                    fixed: false,
                    habit_group: None,
                })
                .unwrap();
        }
        planner
    }

    #[test]
    fn solve_produces_valid_plan() {
        let planner = make_planner(5);
        let plan = solve(&planner);
        assert!(!plan.schedules.is_empty());
        for p in &plan.schedules {
            assert!(p.end.0 >= p.start.0);
        }
    }

    #[test]
    fn solve_partial_preserves_pinned_order() {
        let planner = make_planner(5);
        let plan = solve(&planner);
        let pinned: Vec<_> = plan.schedules.get(0..2).unwrap_or(&[]).to_vec();
        let partial = solve_partial(&planner, &pinned);
        assert!(!partial.schedules.is_empty());
        if partial.schedules.len() >= 2 {
            assert_eq!(partial.schedules[0], pinned[0]);
            assert_eq!(partial.schedules[1], pinned[1]);
        }
    }

    #[test]
    fn solve_partial_empty_pinned_equals_solve() {
        let planner = make_planner(3);
        let plan_full = solve(&planner);
        let plan_partial = solve_partial(&planner, &[]);
        assert_eq!(plan_full.schedules.len(), plan_partial.schedules.len());
    }

    #[test]
    fn solve_partial_ignores_out_of_range_pinned_ids() {
        let planner = make_planner(3);
        let plan = solve(&planner);
        let mut pinned: Vec<_> = plan.schedules.get(0..1).unwrap_or(&[]).to_vec();
        pinned.push(TaskPlacement::new(Point(0), Point(1), 99));
        let partial = solve_partial(&planner, &pinned);
        assert!(!partial.schedules.iter().any(|p| p.task_id == 99));
    }

    #[test]
    fn solve_empty_planner() {
        let planner = Planner::new(Point(0), SleepConfig::disabled());
        let plan = solve(&planner);
        assert!(plan.schedules.is_empty());
    }

    #[test]
    fn solve_no_deadline_violation_for_easy_tasks() {
        let mut planner = Planner::new(Point(0), SleepConfig::disabled());
        for _i in 0..5 {
            planner
                .add(Task {
                    id: 0,
                    start: None,
                    end: Point(10000),
                    cost_estimate: NormalDist::new(6, 0),
                    depends: vec![],
                    parallel_mode: ParallelMode::Exclusive,
                    abandonability: 0.0.into(),
                    fixed: false,
                    habit_group: None,
                })
                .unwrap();
        }
        let plan = solve(&planner);
        for p in &plan.schedules {
            assert!(p.end.0 <= 10000);
        }
    }

    #[test]
    fn plan_with_seed_is_always_sa() {
        let mut planner = make_planner(3);
        planner.set_seed(Some(42));
        planner.set_solver(Solver::Priority);
        let priority_solver_plan = planner.plan_with_seed(7);
        planner.set_solver(Solver::Sa);
        let sa_solver_plan = planner.plan_with_seed(7);
        assert_eq!(priority_solver_plan, sa_solver_plan);
    }

    #[test]
    fn plan_alns_with_seed_is_always_priority() {
        let mut planner = make_planner(3);
        planner.set_seed(Some(42));
        planner.set_solver(Solver::Sa);
        let sa_solver_plan = planner.plan_alns_with_seed(7);
        planner.set_solver(Solver::Priority);
        let priority_solver_plan = planner.plan_alns_with_seed(7);
        assert_eq!(sa_solver_plan, priority_solver_plan);
    }

    #[test]
    fn solve_respects_solver_priority() {
        let mut planner = make_planner(3);
        planner.set_seed(Some(42));
        planner.set_solver(Solver::Priority);
        let plan = planner.plan();
        let alns_plan = planner.plan_alns_with_seed(42);
        assert_eq!(plan, alns_plan);
    }

    #[test]
    fn seed_is_deterministic() {
        let mut planner = make_planner(3);
        planner.set_seed(Some(42));
        planner.set_solver(Solver::Priority);
        let plan1 = planner.plan();
        let plan2 = planner.plan();
        assert_eq!(plan1, plan2);
    }

    #[test]
    fn time_budget_zero_returns_initial_plan() {
        let mut planner = make_planner(3);
        planner.set_time_budget(Some(Duration::ZERO));
        let plan = planner.plan();
        assert_eq!(plan.schedules.len(), planner.tasks.len());
    }
}
