//! Core planner primitive types.
//!
//! These types represent the time model, task model, and schedule output used
//! by `takusu-core`. They live here so other crates (habit generation, local
//! server, agent tools, etc.) can build and inspect schedules without depending
//! on the planner algorithm crate.

use crate::{Abandonability, Minutes, SLOT_MINUTES, Slots};
use jiff::Timestamp;

// ── Point ────────────────────────────────────────────────────────────

/// 離散時間点。1単位 = 5分。
///
/// `Point(i64)` で、`i64` はエポックからの 5 分スロット数。
/// `Point(0)` が Timestamp(0) = UNIX エポック。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

    /// `per` 分単位の Point から jiff の `Timestamp` に変換。
    ///
    /// 秒数が `i64` の範囲を超えるか、jiff が扱えない範囲の場合は `None`。
    pub fn to_timestamp(self, per: u16) -> Option<Timestamp> {
        let seconds = self.0.checked_mul(per as i64)?.checked_mul(60)?;
        Timestamp::from_second(seconds).ok()
    }

    /// エポックからの経過分。
    pub const fn minutes_since_epoch(self) -> Minutes {
        Minutes(self.0 * SLOT_MINUTES)
    }

    /// 絶対値の差 (符号なし Slots)。
    ///
    /// `Point - Point` と同じ意味だが常に非負。`lhs` と `rhs` の前後関係を
    /// 気にせず距離だけが欲しい場合に使う。
    #[inline(always)]
    pub fn diff(lhs: Point, rhs: Point) -> Slots {
        Slots((lhs.0 - rhs.0).abs())
    }

    /// 符号付きの差 (`lhs - rhs`)。前後関係の判定に使う。
    ///
    /// `(lhs - rhs).0` と等価。`Point - Point -> Slots` 演算子と同じ意味だが、
    /// 戻り値を `Slots` として受け取る明示的な名前付き API。
    #[inline(always)]
    pub fn delta(lhs: Point, rhs: Point) -> Slots {
        Slots(lhs.0 - rhs.0)
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

impl std::ops::Add<Slots> for Point {
    type Output = Point;
    #[inline(always)]
    fn add(self, rhs: Slots) -> Point {
        Point(self.0 + rhs.0)
    }
}

impl std::ops::Sub<Slots> for Point {
    type Output = Point;
    #[inline(always)]
    fn sub(self, rhs: Slots) -> Point {
        Point(self.0 - rhs.0)
    }
}

impl std::ops::Sub for Point {
    type Output = Slots;
    #[inline(always)]
    fn sub(self, rhs: Point) -> Slots {
        Slots(self.0 - rhs.0)
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
    avg: u64,
    sigma: u64,
}

impl NormalDist {
    /// `avg` スロット、`sigma` スロットの正規分布。
    pub fn new(avg: u64, sigma: u64) -> Self {
        Self { avg, sigma }
    }

    /// 分から構築する。負値は 0 クランプ。
    pub fn from_minutes(avg: Minutes, sigma: Minutes) -> Self {
        Self::new(
            avg.to_slots().0.max(0) as u64,
            sigma.to_slots().0.max(0) as u64,
        )
    }

    /// 平均（スロット数）。
    pub fn avg(&self) -> u64 {
        self.avg
    }

    /// 標準偏差（スロット数）。
    pub fn sigma(&self) -> u64 {
        self.sigma
    }
}

// ── ParallelMode ──────────────────────────────────────────────────────

/// タスクの並行実行モード。
///
/// `parallelizable`（他タスク実行中に動ける）と `allows_parallel`（自タスク
/// 実行中に他を許す）の 2 つの bool を意味のある 4 状態にまとめたもの。
/// 無意味な組み合わせを型レベルで排除できる。
///
/// 二つのタスクが同時に実行されてよい（オーバーラップ可能）のは、
/// どちらかが `Host`/`Bidirectional`（許す側）で、かつもう一方が
/// `Guest`/`Bidirectional`（動ける側）のときだけ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParallelMode {
    /// 他タスクと並行実行できない。`parallelizable=false, allows_parallel=false`。
    #[default]
    Exclusive,
    /// 他タスク実行中に動ける（ゲスト）。`parallelizable=true, allows_parallel=false`。
    /// 例: スマホでできるタスク。
    Guest,
    /// 自タスク実行中に他のタスクの並行実行を許す（ホスト）。`parallelizable=false, allows_parallel=true`。
    /// 例: 電車移動。
    Host,
    /// ゲストかつホスト。`parallelizable=true, allows_parallel=true`。
    Bidirectional,
}

impl ParallelMode {
    /// 2 つの bool から `ParallelMode` を構築する。
    /// 境界層（DB や API が bool 2 つで保持している場合）の変換用。
    #[inline]
    pub fn from_bools(parallelizable: bool, allows_parallel: bool) -> Self {
        match (parallelizable, allows_parallel) {
            (false, false) => ParallelMode::Exclusive,
            (true, false) => ParallelMode::Guest,
            (false, true) => ParallelMode::Host,
            (true, true) => ParallelMode::Bidirectional,
        }
    }

    /// 他タスク実行中に動ける（`parallelizable`）か。
    #[inline]
    pub fn is_guest(self) -> bool {
        matches!(self, ParallelMode::Guest | ParallelMode::Bidirectional)
    }

    /// 自タスク実行中に他のタスクの並行実行を許す（`allows_parallel`）か。
    #[inline]
    pub fn is_host(self) -> bool {
        matches!(self, ParallelMode::Host | ParallelMode::Bidirectional)
    }

    /// 二つのタスクがオーバーラップ実行可能か。
    /// どちらかがホストで、かつもう一方がゲストなら許可される。
    #[inline]
    pub fn can_overlap(a: Self, b: Self) -> bool {
        (a.is_host() && b.is_guest()) || (b.is_host() && a.is_guest())
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

    /// 並行実行モード。他タスクとのオーバーラップ可否を表す。
    /// 詳細は [`ParallelMode`] を参照。
    pub parallel_mode: ParallelMode,

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

// ── TaskPlacement ─────────────────────────────────────────────────────

/// タスクの配置。`(start, end, task_id)` を意味付きで表現する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskPlacement {
    pub start: Point,
    pub end: Point,
    pub task_id: usize,
}

impl TaskPlacement {
    #[inline]
    pub const fn new(start: Point, end: Point, task_id: usize) -> Self {
        Self {
            start,
            end,
            task_id,
        }
    }
}

// ── TimeWindow ────────────────────────────────────────────────────────

/// 時間窓 `(start, end)`。`previous_schedule` や index の要素。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeWindow {
    pub start: Point,
    pub end: Point,
}

impl TimeWindow {
    #[inline]
    pub const fn new(start: Point, end: Point) -> Self {
        Self { start, end }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_arithmetic() {
        let p = Point(10);
        assert_eq!((p + 5).0, 15);
        assert_eq!((p - 3).0, 7);
        assert_eq!(Point::diff(Point(10), Point(20)), Slots(10));
        assert_eq!(Point::delta(Point(20), Point(10)), Slots(10));
    }

    #[test]
    fn point_slots_arithmetic() {
        let p = Point(10);
        assert_eq!((p + Slots(5)).0, 15);
        assert_eq!((p - Slots(3)).0, 7);
        // Point - Point -> Slots (duration)
        assert_eq!((Point(20) - Point(5)).0, 15);
        // Chained: point + duration - duration
        assert_eq!((Point(10) + Slots(5) - Slots(3)).0, 12);
    }

    #[test]
    fn point_from_raw() {
        let p = Point::from_raw(42);
        assert_eq!(p.0, 42);
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
    fn normal_dist_new() {
        let nd = NormalDist::new(10, 3);
        assert_eq!(nd.avg(), 10);
        assert_eq!(nd.sigma(), 3);
    }

    #[test]
    fn normal_dist_sigma_can_exceed_avg() {
        let nd = NormalDist::new(5, 8);
        assert_eq!(nd.avg(), 5);
        assert_eq!(nd.sigma(), 8);
    }

    #[test]
    fn normal_dist_zero_avg() {
        let nd = NormalDist::new(0, 0);
        assert_eq!(nd.avg(), 0);
        assert_eq!(nd.sigma(), 0);
    }

    #[test]
    fn normal_dist_from_minutes_clamps_negative() {
        let nd = NormalDist::from_minutes(Minutes(-5), Minutes(-3));
        assert_eq!(nd.avg(), 0);
        assert_eq!(nd.sigma(), 0);
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
    fn plan_task_start_end_not_scheduled() {
        let plan = Plan { schedules: vec![] };
        assert!(plan.task_start(0).is_none());
        assert!(plan.task_end(0).is_none());
        assert!(!plan.is_scheduled(0));
    }
}
