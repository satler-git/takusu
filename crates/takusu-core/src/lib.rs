//! # takusu-core — schedule planner
//!
//! ユーザーのタスク集合から自動スケジュールを構築するコアライブラリ。
//! 焼きなまし法 (SA) + 大規模近傍探索 (LNS) + Tabu Search で最適化する。
//!
//! ## 概要
//!
//! ```no_run
//! use takusu_core::{Planner, NormalDist, Point, SleepConfig, Task};
//! use jiff::Timestamp;
//!
//! let mut planner = Planner::new(Point::now(5), SleepConfig::disabled());
//!
//! // 軽量なタスク追加
//! let task_id = planner.add(Task {
//!     id: 0,
//!     start: Some(Point::from_raw(0)),
//!     end: Point::from_raw(100),
//!     cost_estimate: NormalDist::new(10, 2),
//!     depends: vec![],
//!     parallelizable: false,
//!     allows_parallel: false,
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
pub mod evaluate;
mod habit;
mod placement;
mod solver;

pub use decoder::{
    DecodeDiagnostics, DecodeInput, DecodeResult, DecodeStatus, PinnedConflict, RelaxedPlacement,
    RepairMode,
};
pub use placement::{Placement, PlacementFailure, TaskPlacement, TimeWindow};
pub use anneal::{NeighborWeights, SaConfig};
pub use evaluate::EvaluationWeights;

use jiff::Timestamp;
use std::time::Duration;
use thiserror::Error;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL_ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// ── Minutes / Slots ───────────────────────────────────────────────────

pub use takusu_util::{Abandonability, Minutes, SLOT_MINUTES, Slots};

// ── Point ────────────────────────────────────────────────────────────

/// 離散時間点。1単位 = 5分。
///
/// `Point(i64)` で、`i64` はエポックからの 5 分スロット数。
/// `Point(0)` が Timestamp(0) = UNIX エポック。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point(pub i64);

impl Point {
    /// jiff の `Timestamp` から `per` 分単位の Point に変換。
    /// 通常 `per` は 5。
    pub fn from_timestamp(ts: Timestamp, per: u16) -> Point {
        Point(ts.as_second().div_euclid(per as i64 * 60))
    }

    /// 現在時刻の Point。
    pub fn now(per: u16) -> Self {
        Self::from_timestamp(Timestamp::now(), per)
    }

    /// スロット値から Point を生成。`Point::from_raw(12)` = 60 分後。
    pub fn from_raw(n: i64) -> Self {
        Point(n)
    }

    /// エポックからの経過分。
    pub const fn minutes_since_epoch(self) -> Minutes {
        Minutes(self.0 * SLOT_MINUTES)
    }

    /// 絶対値の差 (符号なし)。
    #[inline(always)]
    pub fn diff(lhs: Point, rhs: Point) -> i64 {
        (lhs.0 - rhs.0).abs()
    }

    /// 符号付きの差。`lhs - rhs`。前後関係の判定に使う。
    #[inline(always)]
    pub fn delta(lhs: Point, rhs: Point) -> i64 {
        lhs.0 - rhs.0
    }
}

impl std::ops::Add<i64> for Point {
    type Output = Point;
    fn add(self, rhs: i64) -> Point {
        Point(self.0 + rhs)
    }
}

impl std::ops::Sub<i64> for Point {
    type Output = Point;
    fn sub(self, rhs: i64) -> Point {
        Point(self.0 - rhs)
    }
}

// ── NormalDist ────────────────────────────────────────────────────────

/// 正規分布（平均と標準偏差）。タスクの所要時間見積りに使う。
///
/// - `sigma = 0`: 確定タスク（予定など）
/// - `sigma` 大: 不安定なタスク。後ろにバッファが取られる
///
/// `avg`/`sigma` の単位は 5 分スロット数。
#[derive(Debug, Clone, Copy)]
pub struct NormalDist {
    pub avg: u64,
    pub sigma: u64,
}

impl NormalDist {
    /// `avg` スロット、`sigma` スロットの正規分布。
    pub fn new(avg: u64, sigma: u64) -> Self {
        Self { avg, sigma }
    }

    /// 分から構築する。負値は 0 クランプ。
    pub fn from_minutes(avg: Minutes, sigma: Minutes) -> Self {
        Self {
            avg: avg.to_slots().0.max(0) as u64,
            sigma: sigma.to_slots().0.max(0) as u64,
        }
    }
}

// ── SleepConfig ───────────────────────────────────────────────────────

/// 睡眠設定。
///
/// 一日の基点 (`day_start`) からの相対スロット数で睡眠時間帯を指定する。
/// 例: `day_start=0` (0:00基点), `start=264` (22:00), `end=360` (翌6:00) → 8時間睡眠。
#[derive(Debug, Clone, Copy)]
pub struct SleepConfig {
    /// 一日の基点 (エポックからのスロット)。通常 0。
    pub day_start: i64,
    /// 睡眠開始 (基点からの相対スロット)。
    pub start: i64,
    /// 睡眠終了 (基点からの相対スロット)。end > start。
    pub end: i64,
    /// 睡眠制約が有効かどうか。
    pub enabled: bool,
}

impl SleepConfig {
    /// 推奨設定: 22:00–06:00 (8時間), 一日は 0:00 基点。
    pub fn recommended() -> Self {
        Self {
            day_start: 0,
            start: 264, // 22 * 12
            end: 360,   // 30 * 12 = 6:00 next day
            enabled: true,
        }
    }

    /// 睡眠制約なし。
    pub fn disabled() -> Self {
        Self {
            day_start: 0,
            start: 0,
            end: 0,
            enabled: false,
        }
    }

    /// タイムゾーンとローカル時計時刻から SleepConfig を構築。
    ///
    /// `per` は 1 スロットの分数 (通常 5)。`tz` は jiff タイムゾーン。
    /// `start_h`/`start_m` と `end_h`/`end_m` はローカル時刻による睡眠窓。
    /// 日跨ぎ (例: 22:00–06:00) は自動で処理される。
    pub fn from_local(
        per: u16,
        tz: &jiff::tz::TimeZone,
        start_h: u8,
        start_m: u8,
        end_h: u8,
        end_m: u8,
    ) -> Self {
        let slots_per_hour: i64 = 60 / per as i64;
        let slots_per_day: i64 = 24 * slots_per_hour;

        let offset_secs: i64 = tz.to_offset(jiff::Timestamp::now()).seconds().into();
        let offset_slots = offset_secs / (per as i64 * 60);

        let day_start = (slots_per_day - offset_slots).rem_euclid(slots_per_day);

        let start = start_h as i64 * slots_per_hour + start_m as i64 / per as i64;
        let mut end = end_h as i64 * slots_per_hour + end_m as i64 / per as i64;

        if end <= start {
            end += slots_per_day;
        }

        Self {
            day_start,
            start,
            end,
            enabled: true,
        }
    }
}

impl Default for SleepConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

// ── WorkloadConfig ────────────────────────────────────────────────────

/// 1 日あたりの作業負荷設定。
///
/// ユーザーの「1 日にどれくらいのタスクを入れたいか」を表す。
/// デフォルトでは内部で決定された値を使い、詳細を指定したい場合だけ
/// `Planner::set_workload` で上書きする。
#[derive(Debug, Clone, Copy)]
pub struct WorkloadConfig {
    /// 快適な 1 日あたりの作業スロット数（5 分単位）。
    /// この値を超えると緩やかなペナルティがかかる。
    pub comfortable_slots_per_day: i64,
    /// 1 日あたりの作業スロット数の上限（5 分単位）。
    /// この値を超えると強いペナルティがかかる。
    pub maximum_slots_per_day: i64,
}

impl WorkloadConfig {
    /// 負荷評価を無効化する。
    pub fn disabled() -> Self {
        Self {
            comfortable_slots_per_day: 0,
            maximum_slots_per_day: 0,
        }
    }

    /// 任意の閾値を指定する。
    pub fn new(comfortable_slots_per_day: i64, maximum_slots_per_day: i64) -> Self {
        Self {
            comfortable_slots_per_day,
            maximum_slots_per_day,
        }
    }
}

impl Default for WorkloadConfig {
    /// デフォルト設定: 快適 8 時間（96 スロット）、上限 12 時間（144 スロット）。
    fn default() -> Self {
        Self {
            comfortable_slots_per_day: 96,
            maximum_slots_per_day: 144,
        }
    }
}

// ── Task ──────────────────────────────────────────────────────────────

/// プランナーに渡すタスク。
///
/// タスクは 5 分スロットに離散化された時間軸上に配置される。
/// `start <= task < end`。
#[derive(Debug, Clone)]
pub struct Task {
    /// タスク ID。add_task 時に自動設定されるが、外部で管理したい場合は任意の値。
    pub id: usize,

    /// 開始可能時間。None の場合は即時開始可能。
    pub start: Option<Point>,

    /// 締切。この時刻までに終了している必要がある。
    pub end: Point,

    /// 所要時間の見積り (正規分布)。
    pub cost_estimate: NormalDist,

    /// 依存タスクの ID リスト。これらのタスクがすべて終了してから開始可能。
    pub depends: Vec<usize>,

    /// 他のタスク実行中でも実行可能か (例: スマホでできるタスク)。
    pub parallelizable: bool,

    /// このタスク実行中に他のタスクの並行実行を許すか (例: 電車移動)。
    pub allows_parallel: bool,

    /// 諦めやすさ [0.0, 1.0]。大きいほど諦められやすい。
    /// 全タスクが収まらない場合、この値が大きいタスクからドロップされる。
    pub abandonability: Abandonability,

    /// 開始時刻を固定するか。true の場合、Planner は now 以前の
    /// 配置も許可し、SA の近傍操作でも移動しない。
    /// 学校など開始時刻が厳密なタスクに使う。
    pub fixed: bool,

    /// #306: Habit 由来のタスクの場合、habit グループのインデックス。
    /// 同じ habit_id のタスクは日ごとに近い時刻に配置されるとボーナス。
    /// 非 habit タスクは None。
    pub habit_group: Option<usize>,
}

// ── Plan ──────────────────────────────────────────────────────────────

/// プランナーの出力。タスクの割り当て結果。
///
/// タスクは常に全数スケジュールされる。
/// `abandonability` が高いタスクは deadline 超過が許容されるが、諦められない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// スケジュールされたタスク。各要素は `TaskPlacement { start, end, task_id }`。
    pub schedules: Vec<TaskPlacement>,
}

impl Plan {
    /// タスクの開始時刻。
    pub fn task_start(&self, task_id: usize) -> Option<Point> {
        self.schedules
            .iter()
            .find(|p| p.task_id == task_id)
            .map(|p| p.start)
    }

    /// タスクの終了時刻。
    pub fn task_end(&self, task_id: usize) -> Option<Point> {
        self.schedules
            .iter()
            .find(|p| p.task_id == task_id)
            .map(|p| p.end)
    }

    /// タスクがスケジュールされているか（常に true のはず）。
    pub fn is_scheduled(&self, task_id: usize) -> bool {
        self.schedules.iter().any(|p| p.task_id == task_id)
    }
}

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

impl From<takusu_util::Solver> for Solver {
    fn from(solver: takusu_util::Solver) -> Self {
        match solver {
            takusu_util::Solver::Sa => Self::Sa,
            takusu_util::Solver::Priority => Self::Priority,
            takusu_util::Solver::Auto => Self::Auto,
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

// ── Planner ───────────────────────────────────────────────────────────

/// スケジュールプランナー。タスクを登録して `plan()` でスケジュールを得る。
///
/// ## 使用例
///
/// ```
/// use takusu_core::{Planner, Task, NormalDist, Point, SleepConfig};
///
/// let mut p = Planner::new(Point::from_raw(0), SleepConfig::disabled());
///
/// p.add(Task {
///     id: 0,
///     start: Some(Point::from_raw(0)),
///     end: Point::from_raw(20),
///     cost_estimate: NormalDist::new(5, 0),
///     depends: vec![],
///     parallelizable: false,
///     allows_parallel: false,
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
    /// デフォルトは `WorkloadConfig::default()` で、詳細を指定したい場合は
    /// `set_workload` で上書きする。
    workload: WorkloadConfig,
    /// #211: 前回スケジュールの参照（安定性ペナルティ用）。
    /// 各タスクの (start, end) で、SAが移動を嫌うようにする。
    /// 直近のタスクほど強いペナルティ。
    previous_schedule: Vec<Option<TimeWindow>>,

    /// 使用するソルバー。デフォルトは `Solver::Sa`。
    solver: Solver,
    /// 求解時間の上限。`None` の場合は既存の反復数で完了する。
    time_budget: Option<Duration>,
    /// 乱数シード。`None` の場合は決定的なデフォルトシードを使用する。
    seed: Option<u64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start するか。
    warm_start: bool,
    /// 評価関数の重み。`EvaluationWeights::default()` から差し替え可能。
    weights: EvaluationWeights,
    /// SA（焼きなまし）のパラメータ。`SaConfig::default()` から差し替え可能。
    sa_config: SaConfig,
}

impl Planner {
    /// 新しいプランナーを作成。
    ///
    /// - `now`: 現在時刻 (これより前にタスクを配置しない)
    /// - `sleep`: 睡眠設定。`SleepConfig::recommended()` または `SleepConfig::disabled()`
    pub fn new(now: Point, sleep: SleepConfig) -> Self {
        Self {
            tasks: vec![],
            now,
            per: 5,
            sleep,
            workload: WorkloadConfig::default(),
            previous_schedule: vec![],
            solver: Solver::default(),
            time_budget: None,
            seed: None,
            warm_start: false,
            weights: EvaluationWeights::default(),
            sa_config: SaConfig::default(),
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

    /// #459: 1 日あたりの作業負荷設定を上書きする。
    ///
    /// 指定しない場合は `WorkloadConfig::default()` が使われる。
    pub fn set_workload(&mut self, workload: WorkloadConfig) {
        self.workload = workload;
    }

    /// 使用するソルバーを設定する。
    pub fn set_solver(&mut self, solver: Solver) {
        self.solver = solver;
    }

    /// 求解時間の上限を設定する。`None` で制限なし。
    pub fn set_time_budget(&mut self, budget: Option<Duration>) {
        self.time_budget = budget;
    }

    /// 乱数シードを設定する。`None` で決定的なデフォルト。
    pub fn set_seed(&mut self, seed: Option<u64>) {
        self.seed = seed;
    }

    /// 前回スケジュールからの warm start を有効/無効にする。
    pub fn set_warm_start(&mut self, warm_start: bool) {
        self.warm_start = warm_start;
    }

    /// 評価関数の重みを取得する。
    pub fn weights(&self) -> &EvaluationWeights {
        &self.weights
    }

    /// 評価関数の重みを上書きする。
    ///
    /// 指定しない場合は `EvaluationWeights::default()` が使われる。
    /// チューニング実験等で重みを差し替えたい場合に使用する。
    pub fn set_weights(&mut self, weights: EvaluationWeights) {
        self.weights = weights;
    }

    /// SA（焼きなまし）のパラメータを取得する。
    pub fn sa_config(&self) -> &SaConfig {
        &self.sa_config
    }

    /// SA（焼きなまし）のパラメータを上書きする。
    ///
    /// 指定しない場合は `SaConfig::default()` が使われる。
    /// 温度スケジュール・反復数・近傍確率を調整したい場合に使用する。
    pub fn set_sa_config(&mut self, config: SaConfig) {
        self.sa_config = config;
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
        if slack < 0 {
            return f64::NEG_INFINITY;
        }
        if slack == 0 {
            return 0.;
        }
        1. - (self.tasks[id].cost_estimate.avg as f64 / slack as f64)
    }
}

impl Default for Planner {
    fn default() -> Self {
        Self::new(Point(0), SleepConfig::disabled())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
        let mut p = Planner::new(
            Point(0),
            SleepConfig {
                day_start: 0,
                start: 0,
                end: 96,
                enabled: true,
            },
        );
        p.add(Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(200),
            cost_estimate: NormalDist::new(10, 0),
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
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
            parallelizable: false,
            allows_parallel: false,
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
    fn plan_convenience_methods() {
        let plan = Plan {
            schedules: vec![TaskPlacement::new(Point(1), Point(3), 42)],
        };
        assert_eq!(plan.task_start(42), Some(Point(1)));
        assert_eq!(plan.task_end(42), Some(Point(3)));
        assert!(plan.is_scheduled(42));
        assert!(!plan.is_scheduled(99));
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
            parallelizable: false,
            allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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

    #[test]
    fn point_arithmetic() {
        let p = Point(10);
        assert_eq!((p + 5).0, 15);
        assert_eq!((p - 3).0, 7);
        assert_eq!(Point::diff(Point(10), Point(20)), 10);
        assert_eq!(Point::delta(Point(20), Point(10)), 10);
    }

    #[test]
    fn point_from_raw() {
        let p = Point::from_raw(42);
        assert_eq!(p.0, 42);
    }

    #[test]
    fn normal_dist_new() {
        let nd = NormalDist::new(10, 3);
        assert_eq!(nd.avg, 10);
        assert_eq!(nd.sigma, 3);
    }

    #[test]
    fn normal_dist_sigma_can_exceed_avg() {
        let nd = NormalDist::new(5, 8);
        assert_eq!(nd.avg, 5);
        assert_eq!(nd.sigma, 8);
    }

    #[test]
    fn normal_dist_zero_avg() {
        let nd = NormalDist::new(0, 0);
        assert_eq!(nd.avg, 0);
        assert_eq!(nd.sigma, 0);
    }

    #[test]
    fn sleep_config_disabled() {
        let sc = SleepConfig::disabled();
        assert!(!sc.enabled);
    }

    #[test]
    fn sleep_config_recommended() {
        let sc = SleepConfig::recommended();
        assert!(sc.enabled);
    }

    #[test]
    fn task_add_assigns_id() {
        let mut planner = Planner::new(Point(0), SleepConfig::disabled());
        let id1 = planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(10, 2),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
        let mut planner = Planner::new(Point(0), SleepConfig::disabled());
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        assert_eq!(planner.tasks()[1].depends, vec![0]);
    }

    #[test]
    fn freeness_returns_valid_range() {
        let mut planner = Planner::new(Point(0), SleepConfig::disabled());
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(48),
                cost_estimate: NormalDist::new(6, 2),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
        let mut planner = Planner::new(Point(100), SleepConfig::disabled());
        let late = planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
    fn plan_is_scheduled() {
        let planner = simple_two_task_planner();
        let plan = planner.plan();
        assert!(plan.is_scheduled(0));
        assert!(plan.is_scheduled(1));
    }

    #[test]
    fn plan_task_start_end_not_scheduled() {
        let plan = Plan { schedules: vec![] };
        assert!(plan.task_start(0).is_none());
        assert!(plan.task_end(0).is_none());
        assert!(!plan.is_scheduled(0));
    }

    #[test]
    fn point_from_timestamp_and_now() {
        let ts = jiff::Timestamp::from_second(0).unwrap();
        let p = Point::from_timestamp(ts, 5);
        assert_eq!(p.0, 0);
    }

    // Regression (#780): Point::from_timestamp must use Euclidean (floor)
    // division so timestamps before the epoch map to the correct slot. The
    // current left-associative integer division truncates toward zero,
    // collapsing the slot immediately before the epoch into slot 0.
    #[test]
    fn regression_point_from_timestamp_negative_floor() {
        // -1s falls in the slot [-300, 0) -> Point(-1).
        let just_before = jiff::Timestamp::from_second(-1).unwrap();
        assert_eq!(Point::from_timestamp(just_before, 5).0, -1);

        // -599s falls in the slot [-600, -300) -> Point(-2).
        let well_before = jiff::Timestamp::from_second(-599).unwrap();
        assert_eq!(Point::from_timestamp(well_before, 5).0, -2);
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
        let mut p = Planner::new(Point(0), SleepConfig::disabled());
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
        let mut p = Planner::new(Point(0), SleepConfig::disabled());
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
        let mut p = Planner::new(Point(0), SleepConfig::disabled());
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
        let mut p = Planner::new(Point(0), SleepConfig::disabled());
        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(50),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
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
        let mut p = Planner::new(Point(100), SleepConfig::disabled());
        let a = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
        let mut planner = Planner::new(Point(0), SleepConfig::disabled());
        planner
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(10, 2),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
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
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5.into(),
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        planner
    }
}
