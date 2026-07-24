//! # 評価関数 (Evaluation Function)
//!
//! スケジュール `Plan` をスカラー値に写像する。最大化すべき値。
//!
//! ```text
//! E(plan, T) = Σ task_and_depend_scores(i, T)  // 締切 + 開始可能時間 + 所要時間 + 依存関係
//!             + Σ buffer_score(i)              // 不確実性バッファ報酬
//!             + Σ sleep_score(d)               // 日ごと睡眠評価
//!             + Σ daily_load_score(d)          // #459 日ごと作業負荷
//!             + Σ parallel_violation           // 並列違反
//!             + inclusion_bonus                // スケジュール存在ボーナス
//!             + stability_score                // #211 前回配置からの安定性
//!             + habit_consistency_score        // #306 habit時刻一貫性ボーナス
//! ```
//!
//! ## 各項の詳細
//!
//! ### task_and_depend_scores
//! 1 回のループで締切・開始時刻・所要時間・依存関係の 4 つのスコアを計算する。
//!
//! - 締切 (deadline):
//!   - slack >= 0: `min(slack * W_EARLY, 早期報酬上限)` — 早く終わるほどボーナス(上限あり)
//!   - slack < 0:  `slack * W_LATE` — 締切超過ペナルティ (|W_LATE| ≫ W_EARLY)
//! - 開始可能時刻 (start):
//!   - 開始可能時刻なし または 開始可能時刻以後 → 0
//!   - それ以外 → `(scheduled_start - start) * W_START` (負)
//! - 所要時間マッチ (duration):
//!   - `deficit = avg - scheduled_duration`
//!   - deficit > 0: `-deficit² * W_SHORT` — 見積り不足 (二次で急峻)
//!   - deficit < 0: `deficit * W_OVER` — 取りすぎ (線形で軽微)
//! - 依存関係 (constraint annealing):
//!   - 依存先タスクが終了していない場合:
//!     `-(違反スロット数) * W_DEPEND_BASE * (1.0 - T/T₀)`
//!   - 温度 T が高いうちは違反ペナルティが小さい → 探索範囲が広がる
//!   - T → 0 で最大ペナルティに収束 → 実行可能領域へ誘導
//!   - 違反の大きさに比例するため、大きな依存違反ほど強く罰せられる
//!
//! ### buffer_score
//! - `task.sigma * 連続空き時間 * W_BUFFER`
//! - sigma=0 の確定タスクはバッファ報酬なし
//! - sigmaが大きいタスクの後ろに、締切まで競合なく連続する空きがあるほど高スコア
//!
//! ### sleep_score (per day, 3h threshold)
//! - ベース: `-sleep_used * W_SLEEP_NORMAL`
//! - 睡眠残りが MIN_SLEEP (3時間) を下回った場合:
//!   `-(MIN_SLEEP - sleep_got)² * W_SLEEP_SEVERE` (追加二次ペナルティ)
//!
//! ### parallel_violation (重複スロット数比例)
//! - 時間的重複があり、かつ並列条件を満たさないペア:
//!   `-(重複スロット数) * W_PARALLEL_VIOL`
//!
//! ### daily_load_score (#459)
//! - 1日あたりの占有時間 (スロット数) のunionに対して二次ペナルティを与える。
//! - 負荷は件数ではなく区間unionで測り、合法的な並列タスクは二重加算しない。
//! - `load^2` の項で同じ総作業時間でも分散配置を選好。
//! - `comfortable` 超過と `maximum` 超過に段階的に強いペナルティを追加。
//!
//! ### inclusion_bonus
//! - スケジュールされているタスクごとに `+W_INCLUSION`
//!
//! ## 重み設計
//! |W_PARALLEL_VIOL| ≫ |W_DEPEND_BASE| ≫ |W_START| ≫ |W_LATE| > W_BUFFER > W_INCLUSION
//!
//! ## 重みの根拠
//!
//! - W_PARALLEL_VIOL=2000: 人間は並列可能タスク以外は同時に実行できないため、
//!   時間重複は実質的に硬制約。並列違反は最も強く罰する。
//! - W_DEPEND_BASE=500: 依存違反は絶対に避けたい。T→0で最大500に収束。
//!   温度比(1-T/T0)を乗じるので、実際のペナルティは温度依存。
//! - W_START=100: 開始可能時刻より前に開始するのは硬違反。W_LATEより重い。
//! - W_LATE=20: 締切超過は許容されるが重い。abandonability=1.0で0になる。
//! - W_EARLY=1, cap=50: 早期完了は緩やかに報酬。上限で過学習防止。
//! - W_BUFFER=2: sigma大→多めにバッファ。高sigmaタスクを後ろに倒す誘因。
//! - W_SHORT=3 (2次): 見積り不足は2次ペナルティ。avgに近づける効果。
//! - W_OVER=0.5 (線形): 取りすぎは軽微。最適化よりタスク詰め込み優先。
//! - W_SLEEP_NORMAL=4, W_SLEEP_SEVERE=15 (2次): 3h硬閾値の意図。
//!   睡眠3h未満は2次で急峻に。設計思想: 徹夜よりタスク削減。
//! - W_DAILY_NORMAL=0.01: 同じ総作業時間なら複数日に分散を弱く奨励。
//! - W_DAILY_OVERLOAD=0.5 (2次): 快適容量超過を緩やかに抑制。
//! - W_DAILY_MAXIMUM=2 (2次): 最大容量超過を強めに抑制。
//! - W_INCLUSION=10: タスクをスケジュールから外さない誘因十分。

use super::*;
use crate::placement::Placement;

const W_EARLY: f64 = 1.0;
const W_LATE: f64 = 20.0;
const W_START: f64 = 100.0;
const W_DEPEND_BASE: f64 = 500.0;
const W_BUFFER: f64 = 2.0;
const W_SHORT: f64 = 3.0;
const W_OVER: f64 = 0.5;
const W_SLEEP_NORMAL: f64 = 4.0;
const W_SLEEP_SEVERE: f64 = 15.0;
const W_PARALLEL_VIOL: f64 = 2000.0;
const W_INCLUSION: f64 = 10.0;
const MIN_SLEEP: i64 = 36;
/// #211: 直近タスクの移動ペナルティ。前回位置からの差分スロット × 重み。
/// now に近いほど大きく、遠いタスクはほぼ無視できる。
const W_STABILITY: f64 = 3.0;
/// 安定性ペナルティの減衰スロット数（これ以降はペナルティなし）。
const STABILITY_RANGE: i64 = 24 * 12; // 24時間
/// #306: Habitタスクの時刻一貫性ボーナスの重み。
/// 同じhabitグループのタスクが日ごとに近い時刻に配置されるとボーナス。
/// 分散が小さいほど高スコア。最大ボーナス = W_HABIT_CONSISTENCY * グループ数。
const W_HABIT_CONSISTENCY: f64 = 2.0;
/// 一貫性ボーナスの計算対象となる最大分散 (スロット²)。
/// この分散を超えるとボーナス0になる。
const HABIT_CONSISTENCY_MAX_VAR: f64 = (6.0 * 12.0) * (6.0 * 12.0); // 6時間の分散で0
/// #459: 快適容量以下の負荷に対する二次ペナルティ重み。
/// 同じ総作業時間なら複数日に分散する配置を選好させる。
const W_DAILY_NORMAL: f64 = 0.01;
/// #459: 快適容量超過部分の二次ペナルティ重み。
const W_DAILY_OVERLOAD: f64 = 0.5;
/// #459: 最大容量超過部分の二次ペナルティ重み。
const W_DAILY_MAXIMUM: f64 = 2.0;

pub fn evaluate(planner: &Planner, plan: &Plan, temperature: f64, t0: f64) -> f64 {
    let mut sorted = Vec::with_capacity(plan.schedules.len());
    let mut index = Vec::with_capacity(planner.tasks.len());
    let mut habit_entries = Vec::with_capacity(planner.tasks.len());
    evaluate_with_scratch(
        planner,
        &plan.schedules,
        temperature,
        t0,
        &mut sorted,
        &mut index,
        &mut habit_entries,
    )
}

/// `evaluate` の内部実装。sorted 区間列と index 用 scratch バッファを
/// 呼び出し側が再利用することで、ホットパス（SA ループ）での毎回の allocation を避ける。
pub(crate) fn evaluate_with_scratch(
    planner: &Planner,
    schedules: &[Placement],
    temperature: f64,
    t0: f64,
    sorted: &mut Vec<Placement>,
    index: &mut Vec<Option<(Point, Point)>>,
    habit_entries: &mut Vec<(usize, i64)>,
) -> f64 {
    let mut score = 0.0;

    // index 構築と plan_range を同時に行う。
    let (plan_start, plan_end) = build_index_into(planner, schedules, index);

    // Sort schedules once by start so per-day/window scans can break early and
    // parallel_violation can avoid its own copy+sort.
    sorted.clear();
    sorted.extend_from_slice(schedules);
    sorted.sort_unstable_by_key(|(s, _, _)| s.0);

    score += task_and_depend_scores(planner, index, temperature, t0);
    score += buffer_score(planner, index, sorted);
    score += sleep_score(planner, sorted, (plan_start, plan_end));
    score += daily_load_score(planner, sorted, (plan_start, plan_end));
    score += parallel_violation_score(planner, sorted);
    score += inclusion_bonus(planner, schedules);
    score += stability_score(planner, index);
    score += habit_consistency_score(planner, index, habit_entries);

    score
}

/// task_id → (start, end) の索引。O(n) で構築し、各スコア関数の探索を O(1) にする。
/// 同時にスケジュール全体の [plan_start, plan_end) も返す。
fn build_index_into(
    planner: &Planner,
    schedules: &[Placement],
    index: &mut Vec<Option<(Point, Point)>>,
) -> (Point, Point) {
    index.clear();
    index.resize(planner.tasks.len(), None);
    let mut plan_start = Point(0);
    let mut plan_end = Point(0);
    let mut first = true;
    for (s, e, id) in schedules {
        if *id < index.len() {
            index[*id] = Some((*s, *e));
        }
        if first {
            plan_start = *s;
            plan_end = *e;
            first = false;
        } else {
            if s.0 < plan_start.0 {
                plan_start = *s;
            }
            if e.0 > plan_end.0 {
                plan_end = *e;
            }
        }
    }
    if first {
        (Point(0), Point(0))
    } else {
        (plan_start, plan_end)
    }
}

#[cfg(test)]
fn build_index(planner: &Planner, schedules: &[Placement]) -> Vec<Option<(Point, Point)>> {
    let mut index = Vec::with_capacity(planner.tasks.len());
    build_index_into(planner, schedules, &mut index);
    index
}

fn task_and_depend_scores(
    planner: &Planner,
    index: &[Option<(Point, Point)>],
    temperature: f64,
    t0: f64,
) -> f64 {
    let depend_weight = W_DEPEND_BASE * (1.0 - temperature / t0);
    let mut score = 0.0;
    let mut depend_penalty_slots = 0i64;
    for task in &planner.tasks {
        let Some((sched_start, sched_end)) = index[task.id] else {
            continue;
        };

        // deadline_score
        let slack = Point::delta(task.end, sched_end);
        if slack >= 0 {
            score += (slack as f64 * W_EARLY).min(50.0);
        } else {
            let weight = 1.0 - task.abandonability;
            score += slack as f64 * W_LATE * weight;
        }

        // start_score
        if let Some(task_start) = task.start
            && sched_start < task_start
        {
            score += Point::delta(sched_start, task_start) as f64 * W_START;
        }

        // duration_score
        let actual = Point::delta(sched_end, sched_start);
        let deficit = task.cost_estimate.avg as i64 - actual;
        if deficit > 0 {
            score += -(deficit * deficit) as f64 * W_SHORT;
        } else if deficit < 0 {
            score += deficit as f64 * W_OVER;
        }

        // depend_score (merged into the same loop)
        for dep_id in &task.depends {
            if let Some(Some((_, dep_end))) = index.get(*dep_id)
                && *dep_end > sched_start
            {
                let violation_end = dep_end.0.min(sched_end.0);
                depend_penalty_slots += violation_end - sched_start.0;
            }
        }
    }
    score - (depend_penalty_slots as f64) * depend_weight
}

/// buffer_score: sorted schedule の上位走査で、元の O(n²) (planner.tasks 二重ループ)
/// から、スケジュール済みタスクのみの走査に削減。
/// 条件: other_start < task.end かつ other_end > sched_end のタスクがバッファを遮る。
/// sched_end より前に開始しても sched_end を超えて終了するタスクがバッファを遮るため、
/// 走査は sorted[..end_pos] (start < task.end) 全体を対象とし、end > sched_end で絞る。
fn buffer_score(planner: &Planner, index: &[Option<(Point, Point)>], sorted: &[Placement]) -> f64 {
    let mut score = 0.0;
    for task in &planner.tasks {
        let Some((_start, sched_end)) = index[task.id] else {
            continue;
        };
        if task.cost_estimate.sigma == 0 {
            continue;
        }
        let mut buffer_end = task.end;
        // start < task.end の範囲を走査 (それ以降はバッファを遮らない)
        let end_pos = sorted.partition_point(|(s, _, _)| s.0 < task.end.0);
        for (other_start, other_end, other_id) in &sorted[..end_pos] {
            if *other_id == task.id {
                continue;
            }
            // sched_end を超えて終了するタスクのみバッファを遮る
            if other_end.0 <= sched_end.0 {
                continue;
            }
            let other = &planner.tasks[*other_id];
            if (task.allows_parallel && other.parallelizable)
                || (other.allows_parallel && task.parallelizable)
            {
                continue;
            }
            if other_start.0 < buffer_end.0 {
                buffer_end = *other_start;
            }
        }
        let actual = (buffer_end.0 - sched_end.0).max(0);
        score += task.cost_estimate.sigma as f64 * actual as f64 * W_BUFFER;
    }
    score
}

/// `sorted` 内の区間を `[window_start, window_end)` に clip した上で
/// 重複を統合した占有長さを返す。`start_idx` は呼び出し側が持つカーソルで、
/// 既に通過した区間を再スキャンしない。ウィンドウが単調に進む場合、
/// 全ウィンドウ通しで O(n + windows * active) に近づける。
#[inline(always)]
fn union_length_in_window(
    sorted: &[Placement],
    window_start: Point,
    window_end: Point,
    start_idx: &mut usize,
) -> i64 {
    let n = sorted.len();
    // ウィンドウ開始以前に終わる区間はスキップ
    while *start_idx < n && sorted[*start_idx].1.0 <= window_start.0 {
        *start_idx += 1;
    }

    let mut total = 0i64;
    let mut cur_start = 0i64;
    let mut cur_end = 0i64;
    let mut in_union = false;
    for (s, e, _) in &sorted[*start_idx..n] {
        if s.0 >= window_end.0 {
            break;
        }
        let clip_start = s.0.max(window_start.0);
        let clip_end = e.0.min(window_end.0);
        if !in_union {
            cur_start = clip_start;
            cur_end = clip_end;
            in_union = true;
        } else if clip_start > cur_end {
            total += cur_end - cur_start;
            cur_start = clip_start;
            cur_end = clip_end;
        } else if clip_end > cur_end {
            cur_end = clip_end;
        }
    }
    if in_union {
        total += cur_end - cur_start;
    }
    total
}

fn sleep_score(
    planner: &Planner,
    sorted: &[Placement],
    (plan_start, plan_end): (Point, Point),
) -> f64 {
    if !planner.sleep.enabled {
        return 0.0;
    }
    let slots_per_day: i64 = (24 * 60) / planner.per as i64;
    let (day_start_epoch, sleep_start_rel, sleep_end_rel) = (
        planner.sleep.day_start,
        planner.sleep.start,
        planner.sleep.end,
    );
    let sleep_len = sleep_end_rel - sleep_start_rel;

    if plan_start >= plan_end {
        return 0.0;
    }

    let first_day = day_start_epoch
        + (plan_start.0 - day_start_epoch).div_euclid(slots_per_day) * slots_per_day;
    let mut day_start_point = Point(first_day - slots_per_day);

    let mut score = 0.0;
    let mut start_idx = 0usize;

    while day_start_point.0 + sleep_start_rel <= plan_end.0 {
        let sleep_window_start = Point(day_start_point.0 + sleep_start_rel);
        let sleep_window_end = Point(day_start_point.0 + sleep_end_rel);

        let occupied =
            union_length_in_window(sorted, sleep_window_start, sleep_window_end, &mut start_idx);

        if occupied > 0 {
            let sleep_got = (sleep_len - occupied).max(0);
            score += -(occupied as f64) * W_SLEEP_NORMAL;
            if sleep_got < MIN_SLEEP {
                let deficit = MIN_SLEEP - sleep_got;
                score += -(deficit * deficit) as f64 * W_SLEEP_SEVERE;
            }
        }

        day_start_point = Point(day_start_point.0 + slots_per_day);
    }

    score
}

/// #459: 日ごとの作業負荷に基づくペナルティ。
///
/// 1 日の占有時間（スロット数）を、スケジュール区間の union として計算する。
/// 合法的に重複する並列タスクも単純に二重加算しない。
///
/// 負荷に対しては以下の項を与える。
/// - `-W_DAILY_NORMAL * load(day)^2`  
///   同じ総作業時間でも複数日に分散した plan を選好。
/// - `-W_DAILY_OVERLOAD * max(0, load(day) - comfortable)^2`  
///   快適容量超過に対する緩やかなペナルティ。
/// - `-W_DAILY_MAXIMUM * max(0, load(day) - maximum)^2`  
///   最大容量超過に対する強いペナルティ。
fn daily_load_score(
    planner: &Planner,
    sorted: &[Placement],
    (plan_start, plan_end): (Point, Point),
) -> f64 {
    if planner.workload.comfortable_slots_per_day == 0
        && planner.workload.maximum_slots_per_day == 0
    {
        return 0.0;
    }

    let slots_per_day = (24 * 60) / planner.per as i64;
    let day_start_epoch = planner.sleep.day_start;

    if plan_start >= plan_end {
        return 0.0;
    }

    let first_day = day_start_epoch
        + (plan_start.0 - day_start_epoch).div_euclid(slots_per_day) * slots_per_day;
    let mut day_start = Point(first_day);

    let mut score = 0.0;
    let mut start_idx = 0usize;
    while day_start.0 < plan_end.0 {
        let day_end = Point(day_start.0 + slots_per_day);

        let load = union_length_in_window(sorted, day_start, day_end, &mut start_idx);

        let normal_penalty = (load * load) as f64 * W_DAILY_NORMAL;
        let comfortable_excess = (load - planner.workload.comfortable_slots_per_day).max(0);
        let overload_penalty = (comfortable_excess * comfortable_excess) as f64 * W_DAILY_OVERLOAD;
        let maximum_excess = (load - planner.workload.maximum_slots_per_day).max(0);
        let maximum_penalty = (maximum_excess * maximum_excess) as f64 * W_DAILY_MAXIMUM;
        score -= normal_penalty + overload_penalty + maximum_penalty;

        day_start = Point(day_start.0 + slots_per_day);
    }

    score
}

/// 区間列の union の長さを返す。区間は `(start, end)` で `start < end` 前提。
#[cfg(test)]
fn union_length(intervals: &mut [(Point, Point)]) -> i64 {
    if intervals.is_empty() {
        return 0;
    }
    intervals.sort_unstable_by_key(|(s, _)| s.0);
    let mut total = 0i64;
    let (mut cur_start, mut cur_end) = intervals[0];
    for (s, e) in intervals.iter().skip(1) {
        if s.0 > cur_end.0 {
            total += cur_end.0 - cur_start.0;
            cur_start = *s;
            cur_end = *e;
        } else if e.0 > cur_end.0 {
            cur_end = *e;
        }
    }
    total += cur_end.0 - cur_start.0;
    total
}

fn parallel_violation_score(planner: &Planner, sorted: &[Placement]) -> f64 {
    let mut penalty_slots = 0i64;
    let n = sorted.len();
    let tasks = &planner.tasks;
    for i in 0..n {
        let (a_start, a_end, a_id) = sorted[i];
        if a_id >= tasks.len() {
            continue;
        }
        for (b_start, b_end, b_id) in &sorted[(i + 1)..n] {
            if b_start.0 >= a_end.0 {
                break;
            }
            if b_end.0 <= a_start.0 {
                continue;
            }
            if *b_id >= tasks.len() {
                continue;
            }
            let task_a = &tasks[a_id];
            let task_b = &tasks[*b_id];
            if !((task_a.allows_parallel && task_b.parallelizable)
                || (task_b.allows_parallel && task_a.parallelizable))
            {
                let overlap = a_end.0.min(b_end.0) - a_start.0.max(b_start.0);
                penalty_slots += overlap;
            }
        }
    }
    -(penalty_slots as f64) * W_PARALLEL_VIOL
}

fn inclusion_bonus(_planner: &Planner, schedules: &[Placement]) -> f64 {
    schedules.len() as f64 * W_INCLUSION
}

/// #211: 安定性ペナルティ — 前回スケジュールからタスクが移動した場合、
/// 直近（now に近い）ほど大きなペナルティを課す。
/// 前回位置との開始時刻の差分スロット × W_STABILITY × 減衰係数。
/// 減衰係数 = max(0, 1 - distance_from_now / STABILITY_RANGE)² （二次減衰）
fn stability_score(planner: &Planner, index: &[Option<(Point, Point)>]) -> f64 {
    let prev = planner.previous_schedule();
    if prev.is_empty() {
        return 0.0;
    }
    let now = planner.now;
    let mut penalty = 0.0;
    for task in &planner.tasks {
        let Some((sched_start, _)) = index[task.id] else {
            continue;
        };
        let Some(Some((prev_start, _))) = prev.get(task.id) else {
            continue;
        };
        // 過去位置のタスクは前方に移動すべきなのでペナルティなし
        if prev_start.0 < now.0 {
            continue;
        }
        let delta = (sched_start.0 - prev_start.0).abs();
        if delta == 0 {
            continue;
        }
        // 前回位置がnowに近いほど大きなペナルティ
        let distance = (prev_start.0 - now.0) as f64;
        let decay = ((1.0 - distance / STABILITY_RANGE as f64).max(0.0)).powi(2);
        penalty -= delta as f64 * W_STABILITY * decay;
    }
    penalty
}

/// #306: Habitタスクの時刻一貫性ボーナス。
///
/// 同じ `habit_group` に属するタスク群について、開始時刻の「時刻帯」
/// (日付を無視したスロット) の分散を計算し、分散が小さいほどボーナス。
///
/// - 時刻帯 = `start_slot % slots_per_day` (日付成分を除去)
/// - 分散が0 (全タスクが同時刻) → 最大ボーナス `W_HABIT_CONSISTENCY`
/// - 分散が `HABIT_CONSISTENCY_MAX_VAR` 以上 → ボーナス0
/// - 2タスク未満のグループは評価しない (分散が意味を持たない)
fn habit_consistency_score(
    planner: &Planner,
    index: &[Option<(Point, Point)>],
    entries: &mut Vec<(usize, i64)>,
) -> f64 {
    let slots_per_day = 24 * 60 / planner.per() as i64;
    entries.clear();
    for task in &planner.tasks {
        let Some(group) = task.habit_group else {
            continue;
        };
        let Some((sched_start, _)) = index[task.id] else {
            continue;
        };
        // 日付成分を除去: 時刻帯のみのスロット値。
        // スケジュールされた時刻は非負なので通常の `%` で十分。
        let tod = sched_start.0 % slots_per_day;
        entries.push((group, tod));
    }

    if entries.len() < 2 {
        return 0.0;
    }

    // 1 つの共有バッファで habit グループを扱い、FxHashMap や各グループごとの
    // Vec 割り当てを避ける。まず group だけでソートし、グループ内は小さな
    // スライスを時刻帯でソートして隣接差分を計算する。
    entries.sort_unstable_by_key(|e| e.0);

    let mut bonus = 0.0;
    let mut i = 0;
    while i < entries.len() {
        let group = entries[i].0;
        let start = i;
        i += 1;
        while i < entries.len() && entries[i].0 == group {
            i += 1;
        }
        let count = i - start;
        if count < 2 {
            continue;
        }

        entries[start..i].sort_unstable_by_key(|e| e.1);
        let times = &entries[start..i];
        let n = count as f64;
        let mut sum_sq_diff = 0.0;
        for k in 0..times.len() {
            let next = (k + 1) % times.len();
            let raw = (times[next].1 - times[k].1).abs();
            let diff = raw.min(slots_per_day - raw);
            sum_sq_diff += diff as f64 * diff as f64;
        }
        let mean_sq_diff = sum_sq_diff / n;
        // 分散が小さいほどボーナス。線形減衰。
        let consistency = (1.0 - mean_sq_diff / HABIT_CONSISTENCY_MAX_VAR).max(0.0);
        bonus += W_HABIT_CONSISTENCY * consistency;
    }
    bonus
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::Placement;

    fn make_planner() -> Planner {
        let mut p = Planner::new(Point(0), SleepConfig::disabled());
        p.set_workload(WorkloadConfig::disabled());
        p
    }

    fn add_simple_task(p: &mut Planner, avg: u64, sigma: u64, end: i64) -> usize {
        p.add(Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(end),
            cost_estimate: NormalDist::new(avg, sigma),
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5,
            fixed: false,
            habit_group: None,
        })
        .unwrap()
    }

    fn plan_with(schedules: Vec<Placement>) -> Plan {
        Plan { schedules }
    }

    #[test]
    fn evaluate_empty_schedule() {
        let p = make_planner();
        let plan = plan_with(vec![]);
        let score = evaluate(&p, &plan, 1.0, 1.0);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn evaluate_deadline_violation() {
        let mut p = make_planner();
        let id = add_simple_task(&mut p, 3, 0, 5);
        let ok = plan_with(vec![(Point(0), Point(3), id)]);
        let late = plan_with(vec![(Point(0), Point(6), id)]);

        let score_ok = evaluate(&p, &ok, 0.0, 1.0);
        let score_late = evaluate(&p, &late, 0.0, 1.0);
        assert!(score_ok > score_late, "ok={score_ok} late={score_late}");
    }

    #[test]
    fn evaluate_start_violation() {
        let mut p = make_planner();
        let id = p
            .add(Task {
                id: 0,
                start: Some(Point(10)),
                end: Point(20),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let ok = plan_with(vec![(Point(10), Point(13), id)]);
        let early = plan_with(vec![(Point(5), Point(8), id)]);

        let score_ok = evaluate(&p, &ok, 0.0, 1.0);
        let score_early = evaluate(&p, &early, 0.0, 1.0);
        assert!(score_ok > score_early);
    }

    #[test]
    fn evaluate_depend_violation() {
        let mut p = make_planner();
        let a = add_simple_task(&mut p, 2, 0, 10);
        let b_id = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(10),
                cost_estimate: NormalDist::new(2, 0),
                depends: vec![a],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let ok = plan_with(vec![(Point(0), Point(2), a), (Point(2), Point(4), b_id)]);
        let violated = plan_with(vec![(Point(0), Point(2), b_id), (Point(2), Point(4), a)]);

        let score_ok = evaluate(&p, &ok, 0.0, 1.0);
        let score_bad = evaluate(&p, &violated, 0.0, 1.0);
        assert!(score_ok > score_bad, "ok={score_ok} bad={score_bad}");
    }

    #[test]
    fn regression_depend_penalty_capped_at_sched_end() {
        // When a dependency ends after the dependent task ends, the dependency
        // penalty should only count the slots where the dependent task is
        // actually running before the dependency finishes. The current
        // implementation adds dep_end - sched_start even when dep_end exceeds
        // sched_end, overpenalizing short dependent tasks.
        let mut p = make_planner();

        let a = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(10, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let b = p
            .add(Task {
                id: 0,
                start: None,
                end: Point(100),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![a],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        // Valid: B starts exactly when A finishes.
        let valid = plan_with(vec![(Point(0), Point(10), a), (Point(10), Point(11), b)]);
        // Invalid: B starts before A ends and finishes before A even starts.
        // B runs for 1 slot while A is unfinished, so the dependency violation
        // should be 1 slot, not A's end time minus B's start (12 slots).
        let invalid = plan_with(vec![(Point(2), Point(12), a), (Point(0), Point(1), b)]);

        let score_valid = evaluate(&p, &valid, 0.0, 1.0);
        let score_invalid = evaluate(&p, &invalid, 0.0, 1.0);

        // Both plans have the same duration and capped early-bonus terms, and
        // the invalid schedule has no parallel overlap, so the score difference
        // should be exactly the dependency violation penalty: 1 slot * W_DEPEND_BASE.
        let expected_penalty = 1.0 * W_DEPEND_BASE;
        let actual_gap = score_valid - score_invalid;
        assert!(
            (actual_gap - expected_penalty).abs() < 1e-6,
            "expected gap {expected_penalty}, got {actual_gap} (dependency penalty overcounts when dep end > sched end)"
        );
    }

    #[test]
    fn buffer_prefers_high_sigma_later() {
        let mut p = make_planner();
        let a = add_simple_task(&mut p, 1, 0, 5);
        let b = add_simple_task(&mut p, 1, 2, 5);

        let ab = plan_with(vec![(Point(0), Point(1), a), (Point(1), Point(2), b)]);
        let ba = plan_with(vec![(Point(0), Point(1), b), (Point(1), Point(2), a)]);

        let score_ab = evaluate(&p, &ab, 0.0, 1.0);
        let score_ba = evaluate(&p, &ba, 0.0, 1.0);
        assert!(
            score_ab > score_ba,
            "A→B should be better (B gets buffer after A): ab={score_ab} ba={score_ba}"
        );
    }

    #[test]
    fn buffer_prefers_longer_actual_buffer() {
        let mut p = make_planner();
        let high = add_simple_task(&mut p, 1, 2, 10);
        let low = add_simple_task(&mut p, 1, 0, 100);

        let short = plan_with(vec![(Point(0), Point(1), high), (Point(1), Point(2), low)]);
        let long = plan_with(vec![(Point(0), Point(1), high), (Point(4), Point(5), low)]);

        let score_short = evaluate(&p, &short, 0.0, 1.0);
        let score_long = evaluate(&p, &long, 0.0, 1.0);
        assert!(
            score_long > score_short,
            "longer contiguous buffer should score higher: long={score_long} short={score_short}"
        );
    }

    #[test]
    fn buffer_parallel_task_does_not_block() {
        let mut p = make_planner();
        let host = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(10),
                cost_estimate: NormalDist::new(1, 2),
                depends: vec![],
                parallelizable: false,
                allows_parallel: true,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let guest = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(10),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![],
                parallelizable: true,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let plain = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(10),
                cost_estimate: NormalDist::new(1, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let host_guest = plan_with(vec![
            (Point(0), Point(1), host),
            (Point(1), Point(2), guest),
        ]);
        let host_plain = plan_with(vec![
            (Point(0), Point(1), host),
            (Point(1), Point(2), plain),
        ]);

        let score_guest = evaluate(&p, &host_guest, 0.0, 1.0);
        let score_plain = evaluate(&p, &host_plain, 0.0, 1.0);
        assert!(
            score_guest > score_plain,
            "parallelizable guest should not block host's buffer: guest={score_guest} plain={score_plain}"
        );
    }

    #[test]
    fn duration_too_short_penalized() {
        let mut p = make_planner();
        let id = add_simple_task(&mut p, 5, 0, 10);

        let full = plan_with(vec![(Point(0), Point(5), id)]);
        let short = plan_with(vec![(Point(0), Point(2), id)]);

        let score_full = evaluate(&p, &full, 0.0, 1.0);
        let score_short = evaluate(&p, &short, 0.0, 1.0);
        assert!(
            score_full > score_short,
            "full={score_full} short={score_short}"
        );
    }

    #[test]
    fn sleep_three_hour_threshold() {
        let mut p = make_planner();

        p.sleep = SleepConfig {
            day_start: 0,
            start: 0,
            end: 96,
            enabled: true,
        };

        let task_id = add_simple_task(&mut p, 24, 0, 200);
        let plan_4h_lost = plan_with(vec![(Point(0), Point(48), task_id)]);
        let plan_6h_lost = plan_with(vec![(Point(0), Point(72), task_id)]);

        let score_4h = evaluate(&p, &plan_4h_lost, 0.0, 1.0);
        let score_6h = evaluate(&p, &plan_6h_lost, 0.0, 1.0);

        assert!(
            score_4h > score_6h,
            "4h sleep lost should be less penalized than 6h: 4h={score_4h} 6h={score_6h}"
        );
    }

    #[test]
    fn parallel_task_can_overlap() {
        let mut p = make_planner();
        let host = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(10),
                cost_estimate: NormalDist::new(5, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: true,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let guest = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(10),
                cost_estimate: NormalDist::new(2, 0),
                depends: vec![],
                parallelizable: true,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let overlapping = plan_with(vec![
            (Point(0), Point(5), host),
            (Point(0), Point(2), guest),
        ]);
        let score = evaluate(&p, &overlapping, 0.0, 1.0);
        assert!(score.is_finite());
    }

    #[test]
    fn parallel_violation_penalty_applied() {
        let mut p = make_planner();
        let a = add_simple_task(&mut p, 3, 0, 100);
        let b = add_simple_task(&mut p, 3, 0, 100);

        let overlapping = plan_with(vec![(Point(0), Point(3), a), (Point(0), Point(3), b)]);
        let separate = plan_with(vec![(Point(0), Point(3), a), (Point(3), Point(6), b)]);

        let score_overlap = evaluate(&p, &overlapping, 0.0, 1.0);
        let score_separate = evaluate(&p, &separate, 0.0, 1.0);
        assert!(
            score_separate > score_overlap,
            "separate should score higher due to no parallel penalty: sep={score_separate} overlap={score_overlap}"
        );
    }

    #[test]
    fn parallel_tasks_no_penalty() {
        let mut p = make_planner();
        let host = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: true,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let guest = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: true,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let overlapping = plan_with(vec![
            (Point(0), Point(3), host),
            (Point(0), Point(3), guest),
        ]);
        let no_overlap = plan_with(vec![
            (Point(0), Point(3), host),
            (Point(3), Point(6), guest),
        ]);

        let score_overlap = evaluate(&p, &overlapping, 0.0, 1.0);
        let score_no = evaluate(&p, &no_overlap, 0.0, 1.0);
        assert!(
            (score_overlap - score_no).abs() < 1e-6,
            "parallel tasks should have no violation penalty. overlap={score_overlap} no={score_no}"
        );
    }

    #[test]
    fn sleep_recommended_nighttime_penalized() {
        let mut p = Planner::new(Point(0), SleepConfig::recommended());

        let id = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(500),
                cost_estimate: NormalDist::new(12, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let day_plan = plan_with(vec![(Point(96), Point(108), id)]);
        let night_plan = plan_with(vec![(Point(276), Point(288), id)]);

        let day_score = evaluate(&p, &day_plan, 0.0, 1.0);
        let night_score = evaluate(&p, &night_plan, 0.0, 1.0);

        assert!(
            day_score > night_score,
            "Daytime should score higher than nighttime: day={day_score} night={night_score}"
        );
    }

    #[test]
    fn sleep_recommended_second_day() {
        let mut p = Planner::new(Point(0), SleepConfig::recommended());

        let id = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(1000),
                cost_estimate: NormalDist::new(20, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let day2_plan = plan_with(vec![(Point(400), Point(420), id)]);
        let night2_plan = plan_with(vec![(Point(552), Point(572), id)]);

        let day2_score = evaluate(&p, &day2_plan, 0.0, 1.0);
        let night2_score = evaluate(&p, &night2_plan, 0.0, 1.0);

        assert!(
            day2_score > night2_score,
            "Second day afternoon should score higher than second night: day2={day2_score} night2={night2_score}"
        );
    }

    // #462: parallel sleep-occupying tasks should not double-count sleep loss.
    #[test]
    fn sleep_parallel_tasks_not_double_counted() {
        let mut p = make_planner();
        p.sleep = SleepConfig {
            day_start: 0,
            start: 0,
            end: 96,
            enabled: true,
        };

        let host = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(48, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: true,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let guest = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(48, 0),
                depends: vec![],
                parallelizable: true,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let one = plan_with(vec![(Point(0), Point(48), host)]);
        let two = plan_with(vec![
            (Point(0), Point(48), host),
            (Point(0), Point(48), guest),
        ]);

        let score_one = evaluate(&p, &one, 0.0, 1.0);
        let score_two = evaluate(&p, &two, 0.0, 1.0);
        assert!(
            (score_two - score_one - 60.0).abs() < 1e-9,
            "two parallel tasks should occupy the same sleep time as one: one={score_one} two={score_two}"
        );
    }

    // #462: the union of overlapping sleep intervals is computed correctly.
    #[test]
    fn sleep_overlapping_intervals_union() {
        let mut p = make_planner();
        p.sleep = SleepConfig {
            day_start: 0,
            start: 0,
            end: 96,
            enabled: true,
        };

        let host = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(30, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: true,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let guest = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(30, 0),
                depends: vec![],
                parallelizable: true,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let single = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(50, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let overlapping = plan_with(vec![
            (Point(0), Point(30), host),
            (Point(20), Point(50), guest),
        ]);
        let union = plan_with(vec![(Point(0), Point(50), single)]);

        let score_overlapping = evaluate(&p, &overlapping, 0.0, 1.0);
        let score_union = evaluate(&p, &union, 0.0, 1.0);
        assert!(
            (score_overlapping - score_union - 60.0).abs() < 1e-9,
            "overlapping intervals should occupy the union length: overlapping={score_overlapping} union={score_union}"
        );
    }

    // #462: sleep_got must not be negative even when the entire window is occupied.
    #[test]
    fn sleep_got_is_not_negative() {
        let mut p = make_planner();
        p.sleep = SleepConfig {
            day_start: 0,
            start: 0,
            end: 96,
            enabled: true,
        };

        let host = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(96, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: true,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let guest = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(200),
                cost_estimate: NormalDist::new(96, 0),
                depends: vec![],
                parallelizable: true,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let one = plan_with(vec![(Point(0), Point(96), host)]);
        let two = plan_with(vec![
            (Point(0), Point(96), host),
            (Point(0), Point(96), guest),
        ]);

        let score_one = evaluate(&p, &one, 0.0, 1.0);
        let score_two = evaluate(&p, &two, 0.0, 1.0);
        assert!(
            (score_two - score_one - 60.0).abs() < 1e-9,
            "full-window overlap should not make sleep_got negative: one={score_one} two={score_two}"
        );
    }

    // abandonability=1.0 → deadline-late penalty is fully suppressed.
    #[test]
    fn deadline_late_penalty_zero_when_abandonability_one() {
        let mut p = make_planner();
        let id = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(5),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 1.0,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let on_time = plan_with(vec![(Point(0), Point(3), id)]);
        let late = plan_with(vec![(Point(0), Point(6), id)]);

        let score_on = evaluate(&p, &on_time, 0.0, 1.0);
        let score_late = evaluate(&p, &late, 0.0, 1.0);
        // With abandonability=1.0 the late penalty term vanishes; the only
        // difference is the early-bonus cap (on_time gets +2 capped, late 0)
        // and duration_score (both have deficit 0). So on_time must score
        // strictly higher, but the gap should be small (just the early bonus),
        // not the W_LATE*slack gap.
        assert!(
            score_on > score_late,
            "on_time={score_on} late={score_late}"
        );
        // The gap should be the early bonus (2.0, capped at 50), NOT 20*1.
        assert!(
            (score_on - score_late) < 10.0,
            "gap should be small (early bonus only), got {}",
            score_on - score_late
        );
    }

    // abandonability=0.0 → full late penalty applied.
    #[test]
    fn deadline_late_penalty_full_when_abandonability_zero() {
        let mut p = make_planner();
        let id = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(5),
                cost_estimate: NormalDist::new(3, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.0,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let on_time = plan_with(vec![(Point(0), Point(3), id)]);
        let late = plan_with(vec![(Point(0), Point(6), id)]);

        let score_on = evaluate(&p, &on_time, 0.0, 1.0);
        let score_late = evaluate(&p, &late, 0.0, 1.0);
        // slack = 5 - 6 = -1, penalty = -1 * 20 * 1.0 = -20
        assert!(
            score_on - score_late >= 20.0,
            "full late penalty should apply: on={score_on} late={score_late}"
        );
    }

    // duration over-assignment (deficit < 0) is a light linear penalty.
    #[test]
    fn duration_over_assignment_is_light_linear() {
        let mut p = make_planner();
        let id = add_simple_task(&mut p, 3, 0, 100);
        let exact = plan_with(vec![(Point(0), Point(3), id)]);
        let over = plan_with(vec![(Point(0), Point(5), id)]);

        let score_exact = evaluate(&p, &exact, 0.0, 1.0);
        let score_over = evaluate(&p, &over, 0.0, 1.0);
        // over by 2 slots: penalty = -2 * 0.5 = -1.0 (plus deadline slack change).
        // exact: slack = 100-3 = 97 → capped at 50. over: slack = 100-5 = 95 → capped 50.
        // So deadline term equal; only duration differs by 1.0.
        assert!(
            (score_exact - score_over - 1.0).abs() < 1e-9,
            "over-assignment penalty should be -1.0: exact={score_exact} over={score_over}"
        );
    }

    // depend_score penalty scales with temperature (constraint annealing).
    // At T=T0 the penalty is ~0; at T=0 it is the full magnitude.
    #[test]
    fn depend_score_anneals_with_temperature() {
        let mut p = make_planner();
        let a = add_simple_task(&mut p, 2, 0, 10);
        let b = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(10),
                cost_estimate: NormalDist::new(2, 0),
                depends: vec![a],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        // b starts before a ends: 2-slot violation.
        let violated = plan_with(vec![(Point(0), Point(2), a), (Point(0), Point(2), b)]);

        let score_hot = evaluate(&p, &violated, 10.0, 10.0);
        let score_cold = evaluate(&p, &violated, 0.0, 10.0);
        // At T=T0: depend_weight = W_DEPEND_BASE*(1-1) = 0 → no depend penalty.
        // At T=0:  depend_weight = W_DEPEND_BASE*(1-0) = W_DEPEND_BASE → penalty = -2*W_DEPEND_BASE.
        assert!(
            score_cold < score_hot,
            "cold should penalize violation more: hot={score_hot} cold={score_cold}"
        );
        let expected_penalty = 2.0 * W_DEPEND_BASE;
        assert!(
            (score_hot - score_cold - expected_penalty).abs() < 1e-6,
            "annealed penalty magnitude: hot={score_hot} cold={score_cold} expected={expected_penalty}"
        );
        // unused warning suppression
        let _ = b;
    }

    // inclusion_bonus is linear in scheduled count.
    #[test]
    fn inclusion_bonus_scales_with_count() {
        let mut p = make_planner();
        let a = add_simple_task(&mut p, 1, 0, 100);
        let b = add_simple_task(&mut p, 1, 0, 100);
        let one = plan_with(vec![(Point(0), Point(1), a)]);
        let two = plan_with(vec![(Point(0), Point(1), a), (Point(1), Point(2), b)]);

        let score_one = evaluate(&p, &one, 0.0, 1.0);
        let score_two = evaluate(&p, &two, 0.0, 1.0);
        // Adding a second scheduled task adds exactly W_INCLUSION (10.0)
        // plus the second task's own deadline early-bonus (capped 50) and
        // duration match (deficit 0). So the gap is >= 10.
        assert!(
            score_two - score_one >= 10.0,
            "second task should add at least inclusion bonus: one={score_one} two={score_two}"
        );
    }

    // build_index ignores out-of-range task ids (defensive).
    #[test]
    fn evaluate_ignores_unknown_task_id_in_schedule() {
        let mut p = make_planner();
        let _id = add_simple_task(&mut p, 2, 0, 10);
        // schedule references task id 99 which doesn't exist in planner.
        let plan = plan_with(vec![(Point(0), Point(2), 99)]);
        // Should not panic; score is just inclusion_bonus for the bogus entry.
        let score = evaluate(&p, &plan, 0.0, 1.0);
        assert!(score.is_finite());
    }

    #[test]
    fn regression_780_evaluate_ignores_overlapping_unknown_ids() {
        // Two bogus task ids that overlap in time should not cause evaluate()
        // to panic when computing parallel violations.
        let mut p = make_planner();
        let _id = add_simple_task(&mut p, 2, 0, 10);
        let plan = plan_with(vec![(Point(0), Point(2), 99), (Point(1), Point(3), 100)]);
        let score = evaluate(&p, &plan, 0.0, 1.0);
        assert!(
            score.is_finite(),
            "evaluate must not panic on overlapping unknown ids"
        );
    }

    // #306: habit consistency bonus
    fn add_habit_task(p: &mut Planner, avg: u64, end: i64, habit_group: usize) -> usize {
        p.add(Task {
            id: 0,
            start: Some(Point(0)),
            end: Point(end),
            cost_estimate: NormalDist::new(avg, 0),
            depends: vec![],
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5,
            fixed: false,
            habit_group: Some(habit_group),
        })
        .unwrap()
    }

    #[test]
    fn habit_consistency_rewards_same_time_of_day() {
        let mut p = make_planner();
        let slots_per_day: i64 = 24 * 12;
        let t0 = add_habit_task(&mut p, 2, slots_per_day * 3, 0);
        let t1 = add_habit_task(&mut p, 2, slots_per_day * 4, 0);

        let consistent = plan_with(vec![
            (Point(100), Point(102), t0),
            (Point(100 + slots_per_day), Point(102 + slots_per_day), t1),
        ]);
        let inconsistent = plan_with(vec![
            (Point(100), Point(102), t0),
            (Point(200 + slots_per_day), Point(202 + slots_per_day), t1),
        ]);

        let score_consistent = evaluate(&p, &consistent, 0.0, 1.0);
        let score_inconsistent = evaluate(&p, &inconsistent, 0.0, 1.0);
        assert!(
            score_consistent > score_inconsistent,
            "consistent habit timing should score higher: consistent={score_consistent} inconsistent={score_inconsistent}"
        );
    }

    #[test]
    fn habit_consistency_ignores_non_habit_tasks() {
        let mut p = make_planner();
        let slots_per_day: i64 = 24 * 12;
        let t0 = add_simple_task(&mut p, 2, 0, slots_per_day * 3);
        let t1 = add_simple_task(&mut p, 2, 0, slots_per_day * 4);

        let same_time = plan_with(vec![
            (Point(100), Point(102), t0),
            (Point(100 + slots_per_day), Point(102 + slots_per_day), t1),
        ]);
        let diff_time = plan_with(vec![
            (Point(100), Point(102), t0),
            (Point(200 + slots_per_day), Point(202 + slots_per_day), t1),
        ]);

        let mut entries = Vec::new();
        assert_eq!(
            habit_consistency_score(&p, &build_index(&p, &same_time.schedules), &mut entries),
            0.0
        );
        assert_eq!(
            habit_consistency_score(&p, &build_index(&p, &diff_time.schedules), &mut entries),
            0.0
        );
    }

    #[test]
    fn habit_consistency_single_task_no_bonus() {
        let mut p = make_planner();
        let t0 = add_habit_task(&mut p, 2, 100, 0);
        let plan = plan_with(vec![(Point(10), Point(12), t0)]);
        let mut entries = Vec::new();
        let score = habit_consistency_score(&p, &build_index(&p, &plan.schedules), &mut entries);
        assert_eq!(score, 0.0, "single-task habit group should get no bonus");
    }

    // #462: union_length is the shared utility for interval union.
    #[test]
    fn union_length_combines_intervals_correctly() {
        let mut empty: Vec<(Point, Point)> = Vec::new();
        assert_eq!(union_length(&mut empty), 0);

        // disjoint intervals are summed
        let mut intervals = vec![(Point(0), Point(10)), (Point(20), Point(30))];
        assert_eq!(union_length(&mut intervals), 20);

        // partial overlap merges into the full span
        let mut intervals = vec![(Point(0), Point(20)), (Point(15), Point(35))];
        assert_eq!(union_length(&mut intervals), 35);

        // one interval contained inside another
        let mut intervals = vec![(Point(5), Point(15)), (Point(0), Point(20))];
        assert_eq!(union_length(&mut intervals), 20);

        // touching intervals are merged
        let mut intervals = vec![(Point(0), Point(10)), (Point(10), Point(20))];
        assert_eq!(union_length(&mut intervals), 20);
    }

    // #459: daily workload penalty
    #[test]
    fn daily_load_prefers_spread_over_one_day() {
        let mut p = make_planner();
        p.set_workload(WorkloadConfig::new(48, 96)); // comfortable=4h, max=8h
        let slots_per_day = 24 * 12;
        let a = add_simple_task(&mut p, 48, 0, slots_per_day * 3);
        let b = add_simple_task(&mut p, 48, 0, slots_per_day * 3);

        let one_day = plan_with(vec![(Point(0), Point(48), a), (Point(48), Point(96), b)]);
        let two_days = plan_with(vec![
            (Point(0), Point(48), a),
            (Point(slots_per_day), Point(slots_per_day + 48), b),
        ]);

        let score_one = evaluate(&p, &one_day, 0.0, 1.0);
        let score_two = evaluate(&p, &two_days, 0.0, 1.0);
        assert!(
            score_two > score_one,
            "spread over two days should score higher: one={score_one} two={score_two}"
        );
    }

    #[test]
    fn daily_load_allows_concentration_when_deadline_tight() {
        let mut p = make_planner();
        p.set_workload(WorkloadConfig::new(48, 96)); // comfortable=4h, max=8h
        let a = add_simple_task(&mut p, 24, 0, 30);
        let b = add_simple_task(&mut p, 24, 0, 30);

        let one_day = plan_with(vec![(Point(0), Point(24), a), (Point(24), Point(48), b)]);
        let two_days = plan_with(vec![(Point(0), Point(24), a), (Point(288), Point(312), b)]);

        let score_one = evaluate(&p, &one_day, 0.0, 1.0);
        let score_two = evaluate(&p, &two_days, 0.0, 1.0);
        assert!(
            score_one > score_two,
            "tight deadline should prefer concentration: one={score_one} two={score_two}"
        );
    }

    #[test]
    fn daily_load_includes_fixed_tasks() {
        let mut p = make_planner();
        p.set_workload(WorkloadConfig::new(36, 72)); // comfortable=3h, max=6h
        let slots_per_day = 24 * 12;
        let fixed = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(slots_per_day * 2),
                cost_estimate: NormalDist::new(24, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: true,
                habit_group: None,
            })
            .unwrap();
        let free = add_simple_task(&mut p, 24, 0, slots_per_day * 2);

        let busy_day = plan_with(vec![
            (Point(0), Point(24), fixed),
            (Point(0), Point(24), free),
        ]);
        let free_day = plan_with(vec![
            (Point(0), Point(24), fixed),
            (Point(slots_per_day), Point(slots_per_day + 24), free),
        ]);

        let score_busy = evaluate(&p, &busy_day, 0.0, 1.0);
        let score_free = evaluate(&p, &free_day, 0.0, 1.0);
        assert!(
            score_free > score_busy,
            "free day should score higher when fixed load is heavy: busy={score_busy} free={score_free}"
        );
    }

    #[test]
    fn daily_load_no_double_count_for_parallel_tasks() {
        let mut p = make_planner();
        p.set_workload(WorkloadConfig::new(48, 96)); // comfortable=4h, max=8h
        let host = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(24, 0),
                depends: vec![],
                parallelizable: false,
                allows_parallel: true,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();
        let guest = p
            .add(Task {
                id: 0,
                start: Some(Point(0)),
                end: Point(100),
                cost_estimate: NormalDist::new(24, 0),
                depends: vec![],
                parallelizable: true,
                allows_parallel: false,
                abandonability: 0.5,
                fixed: false,
                habit_group: None,
            })
            .unwrap();

        let overlapping = plan_with(vec![
            (Point(0), Point(24), host),
            (Point(0), Point(24), guest),
        ]);
        let no_overlap = plan_with(vec![
            (Point(0), Point(24), host),
            (Point(24), Point(48), guest),
        ]);

        let score_overlap = evaluate(&p, &overlapping, 0.0, 1.0);
        let score_no = evaluate(&p, &no_overlap, 0.0, 1.0);
        assert!(
            score_overlap > score_no,
            "parallel overlap should not double-count load (union load should be smaller): overlap={score_overlap} no={score_no}"
        );
    }

    #[test]
    fn daily_load_light_day_not_over_penalized() {
        let mut p = make_planner();
        p.set_workload(WorkloadConfig::new(48, 96));
        let slots_per_day = 24 * 12;
        let a = add_simple_task(&mut p, 12, 0, slots_per_day * 3);
        let b = add_simple_task(&mut p, 12, 0, slots_per_day * 3);

        let one_day = plan_with(vec![(Point(0), Point(12), a), (Point(12), Point(24), b)]);
        let two_days = plan_with(vec![
            (Point(0), Point(12), a),
            (Point(slots_per_day), Point(slots_per_day + 12), b),
        ]);

        let score_one = evaluate(&p, &one_day, 0.0, 1.0);
        let score_two = evaluate(&p, &two_days, 0.0, 1.0);
        let gap = score_two - score_one;
        assert!(
            gap > 0.0 && gap < 5.0,
            "light load spread should be preferred but not dominate: gap={gap}"
        );
    }

    #[test]
    fn daily_load_respects_maximum_capacity() {
        let mut p = make_planner();
        // comfortable=4h, max=8h. 10h work exceeds maximum.
        p.set_workload(WorkloadConfig::new(48, 96));
        let a = add_simple_task(&mut p, 72, 0, 144);
        let b = add_simple_task(&mut p, 48, 0, 144);

        let over_max = plan_with(vec![(Point(0), Point(72), a), (Point(72), Point(120), b)]);
        let under_max = plan_with(vec![(Point(0), Point(72), a), (Point(288), Point(336), b)]);

        let score_over = evaluate(&p, &over_max, 0.0, 1.0);
        let score_under = evaluate(&p, &under_max, 0.0, 1.0);
        assert!(
            score_under > score_over,
            "over maximum capacity should be strongly penalized: over={score_over} under={score_under}"
        );
    }
}
