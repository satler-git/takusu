//! # ソルバー: 設定に応じた SA / priority dispatch
//!
//! `Planner.solver` / `time_budget` / `seed` / `warm_start` をもとに、
//! SA (`sa_lns`) または priority decoder + ALNS (`alns_search`) を選択する。
//! full / partial / range は pinned 集合の違いとして統一し、同じ dispatch 経路で解く。
//!
//! 実際の分岐は [`SolverStrategy`] trait にくくり出されている。組み込みの
//! [`SaSolver`] / [`PrioritySolver`] / [`AutoSolver`] は [`Solver`] enum の
//! [`Solver::strategy`] 経由で選ばれるほか、[`Planner::set_solver_strategy`] で
//! 外から差し込んだ独自実装に切り替えることもできる。新しい solver を追加する
//! には enum や本モジュールの分岐を触らず `SolverStrategy` を impl するだけでよい。

use std::cmp::Ordering;
use std::sync::{Arc, OnceLock};
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

// ── SolverStrategy trait ───────────────────────────────────────────────

/// ソルバーの振る舞いを抽象化する trait。
///
/// `pinned` が空のとき full solve、そうでないとき partial solve として振る舞う。
/// 組み込み実装は [`SaSolver`] / [`PrioritySolver`] / [`AutoSolver`]。独自実装を
/// [`Planner::set_solver_strategy`] で渡せば enum 設定を上書きできる。
///
/// `Send + Sync` は [`Planner`] が rayon の並列イテレータ内で `&Planner` 経由で
/// 戦略オブジェクトを参照するために必要。
pub trait SolverStrategy: Send + Sync + std::fmt::Debug {
    /// `pinned` を固定した上でスケジュールを計算して返す。
    fn solve(&self, planner: &Planner, pinned: &[TaskPlacement]) -> Plan;
}

/// SA (`sa_lns`) で解くソルバー。[`Solver::Sa`] に対応する。
#[derive(Debug, Default, Clone, Copy)]
pub struct SaSolver;

impl SolverStrategy for SaSolver {
    fn solve(&self, planner: &Planner, pinned: &[TaskPlacement]) -> Plan {
        if pinned.is_empty() {
            solve_sa(planner, None)
        } else {
            solve_sa_partial(planner, pinned)
        }
    }
}

/// priority decoder + ALNS で解くソルバー。[`Solver::Priority`] に対応する。
#[derive(Debug, Default, Clone, Copy)]
pub struct PrioritySolver;

impl SolverStrategy for PrioritySolver {
    fn solve(&self, planner: &Planner, pinned: &[TaskPlacement]) -> Plan {
        solve_priority(planner, pinned, None)
    }
}

/// まず priority/ALNS を試し、実行不可能または制約緩和なら SA に fallback する
/// ソルバー。[`Solver::Auto`] に対応する。fallback ロジックはこの実装内に閉じる。
#[derive(Debug, Default, Clone, Copy)]
pub struct AutoSolver;

impl SolverStrategy for AutoSolver {
    fn solve(&self, planner: &Planner, pinned: &[TaskPlacement]) -> Plan {
        solve_auto(planner, pinned)
    }
}

// 組み込み戦略のグローバルキャッシュ。`Arc::new` は初回のみで、以降は
// `Arc::clone` (refcount bump) で済むため `plan()` / `plan_partial()` の
// 呼び出しごとのヒープ確保を回避する。solve 本体のコストに比べれば無視できるが、
// `plan_in_range` が高頻度に呼ばれるパスでも無駄な allocation を減らせる。
static SA_STRATEGY: OnceLock<Arc<dyn SolverStrategy>> = OnceLock::new();
static PRIORITY_STRATEGY: OnceLock<Arc<dyn SolverStrategy>> = OnceLock::new();
static AUTO_STRATEGY: OnceLock<Arc<dyn SolverStrategy>> = OnceLock::new();

impl Solver {
    /// この enum 値に対応する組み込み [`SolverStrategy`] を返す。
    /// 組み込み戦略は `OnceLock` でキャッシュされるため、2 回目以降は
    /// `Arc` の refcount bump のみでヒープ確保は発生しない。
    pub fn strategy(self) -> Arc<dyn SolverStrategy> {
        match self {
            Solver::Sa => SA_STRATEGY.get_or_init(|| Arc::new(SaSolver)).clone(),
            Solver::Priority => PRIORITY_STRATEGY
                .get_or_init(|| Arc::new(PrioritySolver))
                .clone(),
            Solver::Auto => AUTO_STRATEGY.get_or_init(|| Arc::new(AutoSolver)).clone(),
        }
    }
}

/// full solve: `Planner` の設定に従って solver を選択する。
/// `Planner` に独自 [`SolverStrategy`] が差し込まれていればそれを、そうでなければ
/// `Planner.solver` enum から対応する組み込み戦略を選ぶ。
pub fn solve(planner: &Planner) -> Plan {
    planner.solver_strategy().solve(planner, &[])
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
    planner.solver_strategy().solve(planner, &pinned)
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

    /// 呼び出し回数を記録する戦略。本体は `SaSolver` に委譲する。
    /// `plan()` / `plan_partial()` の両経路で戦略が実際に呼ばれることを検証するために使う。
    #[derive(Debug)]
    struct CountingSolver {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl CountingSolver {
        fn new() -> Self {
            Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl SolverStrategy for CountingSolver {
        fn solve(&self, planner: &Planner, pinned: &[TaskPlacement]) -> Plan {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            SaSolver.solve(planner, pinned)
        }
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

    #[test]
    fn solver_strategy_overrides_enum_setting() {
        // enum 設定に関わらず差し込んだ戦略が使われること。
        let mut planner = make_planner(3);
        planner.set_seed(Some(42));
        planner.set_solver(Solver::Sa);

        let sa_plan = planner.plan();
        // 同じ SA を独自戦略として差し込んでも結果は一致するはず。
        planner.set_solver_strategy(Some(std::sync::Arc::new(SaSolver)));
        let strategy_plan = planner.plan();
        assert_eq!(sa_plan, strategy_plan);
    }

    #[test]
    fn custom_solver_strategy_is_used() {
        // 独自戦略が実際に呼ばれることを検出するため、呼び出しを記録する戦略。
        let strategy = std::sync::Arc::new(CountingSolver::new());
        let mut planner = make_planner(3);
        planner.set_solver(Solver::Priority);
        planner.set_solver_strategy(Some(strategy.clone()));

        let _ = planner.plan();
        assert_eq!(strategy.calls(), 1);

        // enum 設定に戻すと独自戦略は呼ばれない。
        planner.set_solver_strategy(None);
        let _ = planner.plan();
        assert_eq!(strategy.calls(), 1);
    }

    #[test]
    fn custom_solver_strategy_used_by_plan_partial() {
        // plan_partial / plan_in_range は solve_partial 経由で戦略を呼ぶ。
        // pinned 渡しの経路も戦略ディスパッチが保証されることを確認する。
        let strategy = std::sync::Arc::new(CountingSolver::new());
        let mut planner = make_planner(5);
        planner.set_seed(Some(42));
        planner.set_solver(Solver::Priority);
        planner.set_solver_strategy(Some(strategy.clone()));

        let full_plan = planner.plan();
        assert_eq!(strategy.calls(), 1);

        // pinned を1件渡して partial。戦略がもう1回呼ばれるはず。
        let pinned: Vec<_> = full_plan.schedules.get(0..1).unwrap_or(&[]).to_vec();
        let _ = planner.plan_partial(&pinned);
        assert_eq!(strategy.calls(), 2);
    }

    #[test]
    fn solver_strategy_survives_planner_clone() {
        // Planner は Clone なので、戦略を差し込んだ状態のクローンが壊れないこと。
        let mut planner = make_planner(3);
        planner.set_seed(Some(42));
        planner.set_solver_strategy(Some(std::sync::Arc::new(PrioritySolver)));
        let cloned = planner.clone();
        let a = planner.plan();
        let b = cloned.plan();
        assert_eq!(a, b);
    }
}
