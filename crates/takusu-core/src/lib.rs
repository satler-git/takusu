//! # takusu-core — schedule planner
//!
//! ユーザーのタスク集合から自動スケジュールを構築するコアライブラリ。
//! 焼きなまし法 (SA) + 大規模近傍探索 (LNS) + Tabu Search で最適化する。
//!
//! ## 概要
//!
//! ```no_run
//! use takusu_contracts::SleepConfig;
//! use takusu_core::{Planner, PlannerConfig};
//! use takusu_types::{NormalDist, ParallelMode, Point, Task};
//! use jiff::Timestamp;
//!
//! let mut planner = Planner::new(PlannerConfig::new(Point::now(5), SleepConfig::disabled()));
//!
//! // 軽量なタスク追加
//! let task_id = planner.add(Task {
//!     id: 0,
//!     start: Some(Point::from_raw(0)),
//!     end: Point::from_raw(100),
//!     cost_estimate: NormalDist::new(10, 2),
//!     depends: vec![],
//!     parallel_mode: ParallelMode::Exclusive,
//!     abandonability: 0.5.into(),
//!     fixed: false,
//!     habit_group: None,
//! }).unwrap();
//!
//! let plan = planner.plan();
//! if let Some(start) = plan.task_start(task_id) {
//!     println!("task {task_id} starts at slot {}", start.0);
//! }
//! ```
//!
//! ## 時間の単位
//!
//! すべての時間は `Point` (i64) で表現する。
//! 1 単位 = 5 分。`Point::from_timestamp(ts, 5)` で jiff の Timestamp から変換。
//! `Point::from_raw(n)` でスロット値から直接生成。
//!
//! ## 睡眠
//!
//! `SleepConfig::recommended()` で 22:00-06:00 (8時間) の標準設定が得られる。
//! `SleepConfig::disabled()` で睡眠制約なし。

mod anneal;
pub mod decoder;
pub mod estimator;
pub mod evaluate;
mod habit;
mod placement;
mod solver;

pub use anneal::{NeighborWeights, SaConfig};
pub use decoder::{
    DecodeDiagnostics, DecodeInput, DecodeResult, DecodeStatus, PinnedConflict, RelaxedPlacement,
    RepairMode,
};
pub use evaluate::EvaluationWeights;
pub use placement::PlacementFailure;
pub use solver::{AutoSolver, PrioritySolver, SaSolver, SolverStrategy};

use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL_ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// ── Re-exported primitive types (crate-internal only) ─────────────────

pub(crate) use takusu_contracts::{SleepConfig, WorkloadConfig};
pub(crate) use takusu_types::{Plan, Point, Task, TaskPlacement, TimeWindow};

// ── Solver ─────────────────────────────────────────────────────────────

/// 使用するソルバー。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Solver {
    /// 焼きなまし法 (SA) + LNS。デフォルト。
    #[default]
    Sa,
    /// priority decoder + ALNS。
    Priority,
    /// まず priority/ALNS を試し、実行不可能または制約緩和（Relaxed）の場合のみ SA に fallback する。
    Auto,
}

impl From<takusu_types::Solver> for Solver {
    fn from(solver: takusu_types::Solver) -> Self {
        match solver {
            takusu_types::Solver::Sa => Self::Sa,
            takusu_types::Solver::Priority => Self::Priority,
            takusu_types::Solver::Auto => Self::Auto,
        }
    }
}

// ── RescheduleRange ───────────────────────────────────────────────────

/// 部分再スケジュールの期間指定。
#[derive(Debug, Clone, Copy)]
pub struct RescheduleRange {
    /// 期間の開始 (このスロット以降に開始されるタスクが再スケジュール対象)。
    pub from: Point,
    /// 期間の終了 (このスロット以前に終了されるタスクが再スケジュール対象)。
    pub until: Point,
}

// ── Error ─────────────────────────────────────────────────────────────

/// プランナーのエラー。
#[derive(Debug, Error)]
pub enum Error {
    /// 開始可能時刻が締切より後。
    #[error("The start is {0:?} but the end is {1:?} which is earlier than the start")]
    LateStart(Point, Point),
}

type ResultE<T> = Result<T, Error>;

// ── PlannerConfig ─────────────────────────────────────────────────────

/// `Planner` の生成設定。`Planner::new` に一度に渡すことで、setter の呼び忘れを
/// コンパイル時に防ぐ。
///
/// 必須フィールド (`now` / `sleep`) は [`PlannerConfig::new`] で指定し、
/// 残りの任意フィールドは構造体更新構文 (`..PlannerConfig::new(..)`) で上書きする。
///
/// ## 使用例
///
/// ```
/// use takusu_contracts::SleepConfig;
/// use takusu_core::{Planner, PlannerConfig, Solver};
/// use takusu_types::Point;
/// use std::time::Duration;
///
/// let config = PlannerConfig {
///     solver: Solver::Priority,
///     time_budget: Some(Duration::from_millis(500)),
///     ..PlannerConfig::new(Point::from_raw(0), SleepConfig::disabled())
/// };
/// let mut p = Planner::new(config);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct PlannerConfig {
    /// 現在時刻 (これより前にタスクを配置しない)。
    pub now: Point,
    /// 睡眠設定。`SleepConfig::recommended()` または `SleepConfig::disabled()`。
    pub sleep: SleepConfig,
    /// 使用するソルバー。デフォルトは `Solver::Sa`。
    pub solver: Solver,
    /// 求解時間の上限。`None` の場合は既存の反復数で完了する。
    pub time_budget: Option<Duration>,
    /// 乱数シード。`None` の場合は決定的なデフォルトシードを使用する。
    pub seed: Option<u64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start するか。
    pub warm_start: bool,
    /// #459: 1 日あたりの作業負荷設定。デフォルトは `WorkloadConfig::default()`。
    pub workload: WorkloadConfig,
    /// 評価関数の重み。デフォルトは `EvaluationWeights::default()`。
    pub weights: EvaluationWeights,
    /// SA（焼きなまし）のパラメータ。デフォルトは `SaConfig::default()`。
    pub sa_config: SaConfig,
}

impl PlannerConfig {
    /// 必須フィールド (`now` / `sleep`) を指定して設定を作成。
    /// 残りのフィールドはデフォルト値で初期化される。
    pub fn new(now: Point, sleep: SleepConfig) -> Self {
        Self {
            now,
            sleep,
            solver: Solver::default(),
            time_budget: None,
            seed: None,
            warm_start: false,
            workload: WorkloadConfig::default(),
            weights: EvaluationWeights::default(),
            sa_config: SaConfig::default(),
        }
    }
}

// ── Planner ───────────────────────────────────────────────────────────

/// スケジュールプランナー。タスクを登録して `plan()` でスケジュールを得る。
///
/// ## 使用例
///
/// ```
/// use takusu_contracts::SleepConfig;
/// use takusu_core::{Planner, PlannerConfig};
/// use takusu_types::{NormalDist, ParallelMode, Point, Task};
///
/// let mut p = Planner::new(PlannerConfig::new(Point::from_raw(0), SleepConfig::disabled()));
///
/// p.add(Task {
///     id: 0,
///     start: Some(Point::from_raw(0)),
///     end: Point::from_raw(20),
///     cost_estimate: NormalDist::new(5, 0),
///     depends: vec![],
///     parallel_mode: ParallelMode::Exclusive,
///     abandonability: 0.5.into(),
///     fixed: false,
///     habit_group: None,
/// }).unwrap();
///
/// let plan = p.plan();
/// assert!(plan.is_scheduled(0));
/// ```
#[derive(Debug, Clone)]
pub struct Planner {
    tasks: Vec<Task>,
    now: Point,
    per: u16,
    sleep: SleepConfig,
    /// #459: 1 日あたりの作業負荷設定。
    /// `PlannerConfig` 経由で設定する。
    workload: WorkloadConfig,
    /// #211: 前回スケジュールの参照（安定性ペナルティ用）。
    /// 各タスクの (start, end) で、SAが移動を嫌うようにする。
    /// 直近のタスクほど強いペナルティ。
    previous_schedule: Vec<Option<TimeWindow>>,

    /// 使用するソルバー。デフォルトは `Solver::Sa`。
    solver: Solver,
    /// `solver` enum を上書きする独自 [`SolverStrategy`]。`None` のときは
    /// `solver` enum から組み込み戦略を選ぶ。外部から solver を差し込む経路。
    solver_strategy: Option<Arc<dyn SolverStrategy>>,
    /// 求解時間の上限。`None` の場合は既存の反復数で完了する。
    time_budget: Option<Duration>,
    /// 乱数シード。`None` の場合は決定的なデフォルトシードを使用する。
    seed: Option<u64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start するか。
    warm_start: bool,
    /// 評価関数の重み。`PlannerConfig` 経由で差し替え可能。
    weights: EvaluationWeights,
    /// SA（焼きなまし）のパラメータ。`PlannerConfig` 経由で差し替え可能。
    sa_config: SaConfig,
}

impl Planner {
    /// 新しいプランナーを作成。
    ///
    /// 設定は [`PlannerConfig`] にまとめて渡す。必須フィールドのみ指定する
    /// 簡易ケースでは [`PlannerConfig::new`] を使う:
    ///
    /// ```
    /// use takusu_contracts::SleepConfig;
    /// use takusu_core::{Planner, PlannerConfig};
    /// use takusu_types::Point;
    /// let mut p = Planner::new(PlannerConfig::new(Point::from_raw(0), SleepConfig::disabled()));
    /// ```
    pub fn new(config: PlannerConfig) -> Self {
        Self {
            tasks: vec![],
            now: config.now,
            per: 5,
            sleep: config.sleep,
            workload: config.workload,
            previous_schedule: vec![],
            solver: config.solver,
            solver_strategy: None,
            time_budget: config.time_budget,
            seed: config.seed,
            warm_start: config.warm_start,
            weights: config.weights,
            sa_config: config.sa_config,
        }
    }

    /// タスクを登録。戻り値は登録されたタスク ID (= `self.tasks.len() - 1`)。
    ///
    /// `task.id` は内部的に上書きされる。外部で ID を管理したい場合は
    /// `add()` の戻り値を保持すること。
    pub fn add(&mut self, task: Task) -> ResultE<usize> {
        let id = self.tasks.len();

        if let Some(start) = task.start
            && start > task.end
        {
            return Err(Error::LateStart(start, task.end));
        }

        self.tasks.push(Task { id, ..task });

        Ok(id)
    }

    /// スケジュールを計算して返す。
    ///
    /// `solver` / `time_budget` / `seed` / `warm_start` の設定に従い、
    /// SA または priority/ALNS で解を探索する。
    /// 全タスクがスケジュールされる。`abandonability` が高いタスクは
    /// deadline 超過ペナルティが軽減されるが、ドロップはされない。
    ///
    /// `previous_schedule` が設定されている場合、直近のタスクを
    /// 前回位置から動かすことにペナルティを課す (#211)。
    pub fn plan(&self) -> Plan {
        solver::solve(self)
    }

    /// 指定した seed で単一 SA chain を実行する（solver 設定に関わらず SA）。
    #[doc(hidden)]
    pub fn plan_with_seed(&self, seed: u64) -> Plan {
        solver::solve_with_seed(self, seed)
    }

    /// 指定した seed で priority/ALNS を実行する（solver 設定に関わらず priority）。
    #[doc(hidden)]
    pub fn plan_alns_with_seed(&self, seed: u64) -> Plan {
        solver::solve_alns_with_seed(self, seed)
    }

    /// #211: 前回スケジュールを設定し、安定性ペナルティを有効化する。
    /// `schedule` は `TaskPlacement` のリスト。
    /// 設定後、plan() は前回位置からの移動を嫌うようになる。
    /// 直近（now に近い）ほどペナルティが大きい。
    pub fn set_previous_schedule(&mut self, schedule: &[TaskPlacement]) {
        self.previous_schedule = vec![None; self.tasks.len()];
        for p in schedule {
            if p.task_id < self.previous_schedule.len() {
                self.previous_schedule[p.task_id] = Some(TimeWindow::new(p.start, p.end));
            }
        }
    }

    /// 前回スケジュールの参照（評価関数から使用）。
    pub fn previous_schedule(&self) -> &[Option<TimeWindow>] {
        &self.previous_schedule
    }

    #[doc(hidden)]
    pub fn workload(&self) -> WorkloadConfig {
        self.workload
    }

    #[doc(hidden)]
    pub fn sleep_config(&self) -> SleepConfig {
        self.sleep
    }

    /// 独自の [`SolverStrategy`] を差し込む。`solver` enum 設定より優先され、
    /// `plan()` / `plan_partial()` はこの戦略を使う。`None` を渡すと enum 設定に戻る。
    pub fn set_solver_strategy(&mut self, strategy: Option<Arc<dyn SolverStrategy>>) {
        self.solver_strategy = strategy;
    }

    /// `plan()` 時に実際に使う [`SolverStrategy`] を返す。
    /// 独自戦略が差し込まれていればそれを、そうでなければ `solver` enum の
    /// 組み込み戦略を返す。
    pub fn solver_strategy(&self) -> Arc<dyn SolverStrategy> {
        self.solver_strategy
            .clone()
            .unwrap_or_else(|| self.solver.strategy())
    }

    /// 評価関数の重みを取得する。
    pub fn weights(&self) -> &EvaluationWeights {
        &self.weights
    }

    /// SA（焼きなまし）のパラメータを取得する。
    pub fn sa_config(&self) -> &SaConfig {
        &self.sa_config
    }

    /// 固定タスクを保持したまま未固定タスクをスケジュール。
    ///
    /// `pinned` に含まれるタスクは指定位置に固定され、近傍操作の対象外。
    /// 未固定タスクのみが探索される。評価関数は固定・未固定両方を考慮する。
    pub fn plan_partial(&self, pinned: &[TaskPlacement]) -> Plan {
        solver::solve_partial(self, pinned)
    }

    /// 指定した seed で SA partial を実行する（solver 設定に関わらず SA）。
    #[doc(hidden)]
    pub fn plan_partial_with_seed(&self, pinned: &[TaskPlacement], seed: u64) -> Plan {
        solver::solve_partial_with_seed(self, pinned, seed)
    }

    /// 指定期間内のタスクのみ再スケジュール。
    ///
    /// `current_schedule` に含まれるタスクのうち、期間外のものを固定とみなす。
    /// `extra_pinned` に追加で固定したいタスクも指定できる。
    /// 期間内 (`range.from <= start` かつ `end <= range.until`) のタスクのみが再配置される。
    ///
    /// 元の `Planner` に対して `solve_partial` を実行するため、固定タスクと再配置タスクの
    /// 時間重複・並列条件・依存関係を同じ評価関数で扱う。 (#454)
    pub fn plan_in_range(
        &self,
        range: &RescheduleRange,
        current_schedule: &[TaskPlacement],
        extra_pinned: &[usize],
    ) -> Plan {
        let mut pinned: Vec<TaskPlacement> = Vec::new();

        for p in current_schedule {
            let in_range = p.start.0 >= range.from.0 && p.end.0 <= range.until.0;
            if !in_range || extra_pinned.contains(&p.task_id) {
                pinned.push(*p);
            }
        }

        solver::solve_partial(self, &pinned)
    }

    /// 指定した seed で SA range 再スケジュールを実行する（solver 設定に関わらず SA）。
    #[doc(hidden)]
    pub fn plan_in_range_with_seed(
        &self,
        range: &RescheduleRange,
        current_schedule: &[TaskPlacement],
        extra_pinned: &[usize],
        seed: u64,
    ) -> Plan {
        let pinned: Vec<_> = current_schedule
            .iter()
            .filter(|p| {
                !(p.start.0 >= range.from.0 && p.end.0 <= range.until.0)
                    || extra_pinned.contains(&p.task_id)
            })
            .copied()
            .collect();
        solver::solve_partial_with_seed(self, &pinned, seed)
    }

    /// 登録された全タスクを返す。
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    pub fn tasks_mut(&mut self) -> &mut [Task] {
        &mut self.tasks
    }

    /// 1スロットの分数 (通常5)。
    pub fn per(&self) -> u16 {
        self.per
    }

    /// タスクの「余裕度」を返す [0.0, 1.0]。
    /// 値が大きい = 余裕がある = 優先度が低い。
    /// 値が小さい = 切迫している = 優先度が高い。
    ///
    /// # Counterintuitive naming
    /// 名前は「free」だが、値が大きいほど deprioritized される。
    /// 低 freeness → 締切までの slack が小さい → build_initial で先に配置。
    /// `freeness()` の結果でソートし、値が小さい順に greedy 配置される。
    /// 「freeness」＝「(slack - avg) / slack」のイメージ。
    pub(crate) fn freeness(&self, id: usize) -> f64 {
        let slack = Point::delta(
            self.tasks[id].end,
            self.tasks[id].start.unwrap_or(Point(0)).max(self.now),
        );
        if slack.0 < 0 {
            return f64::NEG_INFINITY;
        }
        if slack.0 == 0 {
            return 0.;
        }
        1. - (self.tasks[id].cost_estimate.avg() as f64 / slack.0 as f64)
    }
}

impl Default for Planner {
    fn default() -> Self {
        Self::new(PlannerConfig::new(Point(0), SleepConfig::disabled()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use takusu_types::{NormalDist, ParallelMode};

    #[test]
    fn planner_simple_two_tasks() {
        let mut p = Planner::default();
        let a = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(5),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let b = p
            .add(Task {
                id: 1,
                start: Some(Point(0)),
                end: Point(5),
                cost_estimate: NormalDist::new(1, 2),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let plan = p.plan();
        assert_eq!(plan.schedules.len(), 2);
        assert!(plan.task_end(b).unwrap().0 <= 5);
        assert!(
            plan.task_start(a).unwrap().0 < plan.task_start(b).unwrap().0,
            "low-sigma A should be scheduled before high-sigma B: {:?}",
            plan.schedules
        );
    }

    #[test]
    fn planner_sleep_avoided() {
        let mut p = Planner::new(PlannerConfig::new(
            Point(0),
            SleepConfig::new(0, 0, 96, true),
        ));
        p.add(Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(200),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        })
        .unwrap();

        let plan = p.plan();
        let sleep_occupied: i64 = plan
            .schedules
            .iter()
            .filter(|p| p.start.0 < 96 && p.end.0 > 0)
            .map(|p| {
                let o_start = p.start.0.max(0);
                let o_end = p.end.0.min(96);
                (o_end - o_start).max(0)
            })
            .sum();

        assert!(sleep_occupied < 96);
    }

    #[test]
    fn planner_deadline_miss_still_scheduled() {
        let mut p = Planner::default();
        p.add(Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(0),
            cost_estimate: NormalDist::new(5, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.9.into(),
            fixed: false,
            habit_group: None,
        })
        .unwrap();

        let plan = p.plan();
        assert!(
            plan.is_scheduled(0),
            "task should be scheduled even if deadline is impossible. schedules={:?}",
            plan.schedules
        );
    }
    #[test]
    fn plan_partial_keeps_pinned() {
        let mut p = Planner::default();
        let a = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(20),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _b = p
            .add(Task {
                id: 1,
                start: Some(Point(0)),
                end: Point(20),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let pinned = vec![TaskPlacement::new(Point(0), Point(3), a)];
        let plan = p.plan_partial(&pinned);

        let pinned_start = plan.task_start(a).unwrap();
        let pinned_end = plan.task_end(a).unwrap();
        assert_eq!(
            pinned_start,
            Point(0),
            "pinned task start should be unchanged"
        );
        assert_eq!(pinned_end, Point(3), "pinned task end should be unchanged");
        assert_eq!(plan.schedules.len(), 2, "all tasks should be scheduled");
    }

    #[test]
    fn plan_partial_no_pinned_equals_plan() {
        let mut p = Planner::default();
        p.add(Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(10),
            cost_estimate: NormalDist::new(2, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.5.into(),
            fixed: false,
            habit_group: None,
        })
        .unwrap();

        let plan_full = p.plan();
        let plan_partial = p.plan_partial(&[]);
        assert_eq!(
            plan_partial.schedules.len(),
            plan_full.schedules.len(),
            "plan_partial with no pinned should schedule all tasks"
        );
    }

    #[test]
    fn plan_in_range_reschedules_within_range() {
        let mut p = Planner::default();
        let _a = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(50),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _b = p
            .add(Task {
                id: 1,
                start: Some(Point(50)),
                end: Point(100),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current = p.plan();
        let range = RescheduleRange {
            from: Point(0),
            until: Point(50),
        };
        let replanned = p.plan_in_range(&range, &current.schedules, &[]);
        assert_eq!(
            replanned.schedules.len(),
            2,
            "all tasks should be scheduled"
        );
    }

    #[test]
    fn plan_in_range_preserves_task_ids_with_pinned_middle() {
        let mut p = Planner::default();
        let _a = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _b = p
            .add(Task {
                id: 1,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _c = p
            .add(Task {
                id: 2,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current_schedule = vec![
            TaskPlacement::new(Point(0), Point(5), 0),
            TaskPlacement::new(Point(10), Point(15), 1),
            TaskPlacement::new(Point(50), Point(55), 2),
        ];
        let range = RescheduleRange {
            from: Point(5),
            until: Point(50),
        };
        let replanned = p.plan_in_range(&range, &current_schedule, &[]);
        assert_eq!(replanned.schedules.len(), 3);
        let ids: Vec<usize> = replanned.schedules.iter().map(|p| p.task_id).collect();
        assert!(ids.contains(&0), "task 0 should be preserved");
        assert!(ids.contains(&1), "task 1 should be preserved");
        assert!(ids.contains(&2), "task 2 should be preserved");
        assert_eq!(
            replanned.task_start(0).unwrap(),
            Point(0),
            "pinned task 0 start should be unchanged"
        );
        assert_eq!(
            replanned.task_end(0).unwrap(),
            Point(5),
            "pinned task 0 end should be unchanged"
        );
        assert_eq!(
            replanned.task_start(2).unwrap(),
            Point(50),
            "pinned task 2 start should be unchanged"
        );
        assert_eq!(
            replanned.task_end(2).unwrap(),
            Point(55),
            "pinned task 2 end should be unchanged"
        );
    }

    #[test]
    fn plan_in_range_remaps_depends_correctly() {
        let mut p = Planner::default();
        let _a = p
            .add(Task {
                id: 0,
                start: Some(Point(20)),
                end: Point(100),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _b = p
            .add(Task {
                id: 1,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![0],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _c = p
            .add(Task {
                id: 2,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![1],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current_schedule = vec![
            TaskPlacement::new(Point(20), Point(30), 0),
            TaskPlacement::new(Point(10), Point(20), 1),
            TaskPlacement::new(Point(30), Point(40), 2),
        ];
        let range = RescheduleRange {
            from: Point(0),
            until: Point(15),
        };
        // Task 0 is out of range (starts at 20, range ends at 15) → pinned.
        // Tasks 1 and 2 are in range → rescheduled in sub-planner.
        // Before remap: task 2.depends = [1] (original id), but in sub-planner idx 1 is task 1.
        // After remap: task 2.depends should be [1] (sub-planner idx).
        // Task 1.depends = [0] (original), but 0 is pinned → filtered out, depends becomes [].
        let replanned = p.plan_in_range(&range, &current_schedule, &[]);
        assert_eq!(replanned.schedules.len(), 3);
        let pinned_0 = replanned.schedules.iter().find(|p| p.task_id == 0).unwrap();
        assert_eq!(pinned_0.start, Point(20), "task 0 pinned start unchanged");
        assert_eq!(pinned_0.end, Point(30), "task 0 pinned end unchanged");
    }

    #[test]
    fn plan_in_range_dep_chain_remap_self_dep_prevented() {
        let mut p = Planner::default();
        let _a = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _b = p
            .add(Task {
                id: 1,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![0],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let _c = p
            .add(Task {
                id: 2,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![1],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current_schedule = vec![
            TaskPlacement::new(Point(0), Point(10), 0),
            TaskPlacement::new(Point(10), Point(20), 1),
            TaskPlacement::new(Point(50), Point(60), 2),
        ];
        let range = RescheduleRange {
            from: Point(0),
            until: Point(30),
        };
        // Tasks 0 and 1 are in range → rescheduled.
        // Task 2 is out of range (starts at 50) → pinned.
        // Sub-planner: [task 0, task 1]. Task 1.depends = [0] → remapped to [0]. Correct.
        let replanned = p.plan_in_range(&range, &current_schedule, &[]);
        assert_eq!(replanned.schedules.len(), 3);
        let pinned_2 = replanned.schedules.iter().find(|p| p.task_id == 2).unwrap();
        assert_eq!(pinned_2.start, Point(50), "task 2 pinned start unchanged");
    }
    // ── End invariant validation tests ───────────────────────────────────

    #[test]
    fn task_add_assigns_id() {
        let mut planner = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let id1 = planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(10, 2),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let id2 = planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(200),
                cost_estimate: NormalDist::new(5, 1),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
    }

    #[test]
    fn task_add_updates_depend_indices() {
        let mut planner = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(200),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![0],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        assert_eq!(planner.tasks()[1].depends, vec![0]);
    }

    #[test]
    fn freeness_returns_valid_range() {
        let mut planner = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(48),
                cost_estimate: NormalDist::new(6, 2),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.0.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let f = planner.freeness(0);
        assert!((0.0..=1.0).contains(&f));
    }

    // Regression (#780): a task whose deadline is already before `now` must
    // be treated as the most urgent. `freeness()` currently uses
    // `Point::diff` (absolute difference), which turns the negative slack
    // into a positive number and deprioritizes the task.
    #[test]
    fn regression_780_freeness_past_deadline_priority() {
        let mut planner = Planner::new(PlannerConfig::new(Point(100), SleepConfig::disabled()));
        let late = planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let tight = planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(101),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let late_freeness = planner.freeness(late);
        let tight_freeness = planner.freeness(tight);
        assert!(
            late_freeness < tight_freeness,
            "past-deadline task should be more urgent than a tight but feasible task: late={late_freeness} tight={tight_freeness}"
        );
    }
    #[test]
    fn evaluate_empty_schedule_is_inclusion_loss() {
        let planner = simple_two_task_planner();
        let plan = Plan { schedules: vec![] };
        let score = evaluate::evaluate(&planner, &plan, 0.0, 1.0);
        let full_plan = planner.plan();
        let full_score = evaluate::evaluate(&planner, &full_plan, 0.0, 1.0);
        assert!(full_score > score);
    }

    #[test]
    fn plan_in_range_avoids_pinned_overlap() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let b = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current_schedule = vec![
            TaskPlacement::new(Point(0), Point(3), a),
            TaskPlacement::new(Point(0), Point(3), b),
        ];
        let range = RescheduleRange {
            from: Point(0),
            until: Point(50),
        };
        let replanned = p.plan_in_range(&range, &current_schedule, &[a]);
        let b_start = replanned
            .schedules
            .iter()
            .find(|p| p.task_id == b)
            .unwrap()
            .start;
        assert!(
            b_start.0 >= 3,
            "rescheduled task should not overlap pinned task"
        );
    }

    #[test]
    fn plan_in_range_respects_pinned_dependency() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let b = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![a],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current_schedule = vec![
            TaskPlacement::new(Point(0), Point(3), a),
            TaskPlacement::new(Point(3), Point(6), b),
        ];
        let range = RescheduleRange {
            from: Point(0),
            until: Point(50),
        };
        let replanned = p.plan_in_range(&range, &current_schedule, &[a]);
        let b_start = replanned
            .schedules
            .iter()
            .find(|p| p.task_id == b)
            .unwrap()
            .start;
        assert!(
            b_start.0 >= 3,
            "rescheduled task should start after pinned dependency"
        );
    }

    #[test]
    fn plan_in_range_keeps_extra_pinned_position() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let b = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current_schedule = vec![
            TaskPlacement::new(Point(5), Point(8), a),
            TaskPlacement::new(Point(0), Point(3), b),
        ];
        let range = RescheduleRange {
            from: Point(0),
            until: Point(50),
        };
        let replanned = p.plan_in_range(&range, &current_schedule, &[a]);
        let a_placement = replanned.schedules.iter().find(|p| p.task_id == a).unwrap();
        assert_eq!(
            a_placement.start.0, 5,
            "extra_pinned start should be unchanged"
        );
        assert_eq!(a_placement.end.0, 8, "extra_pinned end should be unchanged");
    }

    #[test]
    fn plan_in_range_pinned_depends_on_rescheduled() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let b = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![a],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let current_schedule = vec![
            TaskPlacement::new(Point(0), Point(3), a),
            TaskPlacement::new(Point(5), Point(8), b),
        ];
        let range = RescheduleRange {
            from: Point(0),
            until: Point(5),
        };
        let replanned = p.plan_in_range(&range, &current_schedule, &[]);
        let a_placement = replanned.schedules.iter().find(|p| p.task_id == a).unwrap();
        assert!(
            a_placement.end.0 <= 5,
            "rescheduled task should finish before pinned dependent"
        );
        assert!(
            a_placement.start.0 >= 0,
            "rescheduled task should not start before now"
        );
    }

    // Regression (#780): plan_in_range must pin tasks that partially overlap
    // the requested range, not only tasks completely outside it. The current
    // condition `e <= from || s >= until` misses left-overlapping intervals
    // (start < from but end > from), causing them to be rescheduled instead of
    // preserved.
    #[test]
    fn regression_plan_in_range_pins_left_overlap() {
        let mut p = Planner::new(PlannerConfig::new(Point(100), SleepConfig::disabled()));
        let a = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        // Task a overlaps the range on the left: it starts before the range
        // and ends inside it, so it is not fully contained in [20, 80).
        let current_schedule = vec![TaskPlacement::new(Point(0), Point(30), a)];
        let range = RescheduleRange {
            from: Point(20),
            until: Point(80),
        };

        let replanned = p.plan_in_range(&range, &current_schedule, &[]);
        let a_placement = replanned.schedules.iter().find(|p| p.task_id == a).unwrap();
        assert_eq!(
            a_placement.start.0, 0,
            "left-overlapping task should keep its original start, got {:?}",
            a_placement.start
        );
        assert_eq!(
            a_placement.end.0, 30,
            "left-overlapping task should keep its original end, got {:?}",
            a_placement.end
        );
    }

    fn simple_two_task_planner() -> Planner {
        let mut planner = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(10, 2),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(200),
                cost_estimate: NormalDist::new(10, 2),
                depends: vec![],
                parallel_mode: ParallelMode::Exclusive,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        planner
    }
}
