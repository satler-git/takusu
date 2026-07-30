//! # Habit 階層探索 (internal)
//!
//! `Task.habit_group` で結ばれた occurrence 群を、公開 API を変えずに
//! solver 内部で 1 つのグループとして扱う。各 occurrence は `Task.start`
//! (= occurrence 日 + 希望時刻) に自分の「日」と「時刻帯」を保持しているため、
//! recurrence rule なしで anchor を復元できる。
//!
//! ## 性能設計
//!
//! habit のグループ所属は solve 中で不変なので、`HabitIndex` として
//! **solve ごとに 1 回だけ**事前計算する。SA の近傍生成では毎 iteration
//! グルーピングを再構築せず、静的な member リストと現在の配置から
//! anchor を求める。これによりホットループ内の HashMap allocate を避ける。
//! 各グループは member 判定用の `FxHashSet` も持ち、`contains` を O(1) にする。
//!
//! - `apply_anchor_shift`: 非 exception・非 fixed・非 pinned member を
//!   `day_base + (anchor_tod + delta)` へ一律移動し、日を保持したまま
//!   グループの一貫性を強制する。
//! - exception: anchor から大きく外れた member は自動的に exception と
//!   みなし、anchor 移動で引き戻さない。個別移動 (`apply_member_shift`)
//!   で逸脱した member は次回の anchor 計算で exception になる。

use rustc_hash::{FxHashMap, FxHashSet};

use super::*;

const TAU: f64 = std::f64::consts::TAU;

/// anchor からこの距離 (slot) を超えて外れた member は exception とみなす。
/// 1 日 = slots_per_day の 1/12 (= 24h day で 2 時間)。
const EXCEPTION_FRAC: i64 = 12;

/// 1 つの habit group。solve 中で不変。
#[derive(Debug, Clone)]
pub(crate) struct HabitGroup {
    /// 昇順の member task id。
    pub members: Vec<usize>,
    /// O(1) の member 判定用。
    pub set: FxHashSet<usize>,
}

/// solve 中に不変の habit グルーピング。`build_index` で 1 回だけ構築する。
#[derive(Debug, Clone, Default)]
pub(crate) struct HabitIndex {
    /// 2 件以上の member を持つ habit group。グループ同士は昇順。
    pub groups: Vec<HabitGroup>,
}

/// `planner` の habit_group から静的なグルーピングを構築する。
pub(crate) fn build_index(planner: &Planner) -> HabitIndex {
    let mut map: FxHashMap<usize, Vec<usize>> = FxHashMap::default();
    for task in planner.tasks() {
        if let Some(g) = task.habit_group {
            map.entry(g).or_default().push(task.id);
        }
    }
    let mut groups: Vec<HabitGroup> = map
        .into_values()
        .filter(|m| m.len() >= 2)
        .map(|members| {
            let set: FxHashSet<usize> = members.iter().copied().collect();
            HabitGroup { members, set }
        })
        .collect();
    groups.sort_by(|a, b| a.members.cmp(&b.members));
    HabitIndex { groups }
}

#[inline]
fn slots_per_day(planner: &Planner) -> i64 {
    (24 * 60) / planner.per() as i64
}

/// raw 時刻帯。evaluate.rs の habit_consistency_score と同じ分解系。
#[inline]
fn tod_of(start: Point, spd: i64) -> i64 {
    start.0.rem_euclid(spd)
}

/// 円周上の距離 (日跨ぎを考慮)。
#[inline]
fn circ_dist(a: i64, b: i64, spd: i64) -> i64 {
    let raw = (a - b).abs();
    raw.min(spd - raw)
}

fn exception_threshold(spd: i64) -> i64 {
    (spd / EXCEPTION_FRAC).max(1)
}

/// グループの代表時刻 (anchor) を O(k) で求める近似。
///
/// 円周平均 (atan2) で時刻帯の「方向」を求め、最も近い member の tod へ
/// snap する (実在の occurrence 時刻に揃える)。厳密な 1-median (距離和を
/// 最小化する点) ではなく、member がほぼ同一時刻に集まる habit の前提で
/// 成り立つ近似。全 member が同一 tod ならその値をそのまま返す。
fn circular_mean_nearest(tods: &[i64], spd: i64) -> i64 {
    debug_assert!(!tods.is_empty());
    let mut sin_sum = 0.0f64;
    let mut cos_sum = 0.0f64;
    for &t in tods {
        let angle = t as f64 * TAU / spd as f64;
        sin_sum += angle.sin();
        cos_sum += angle.cos();
    }
    let mean = sin_sum.atan2(cos_sum);
    // rem_euclid の結果は [0, spd) だが、`.round()` で spd へ繰り上がり
    // 有効範囲 0..spd を外れ得るため、整数で再度 rem_euclid する。
    let mean_tod =
        ((mean * spd as f64 / TAU).rem_euclid(spd as f64).round() as i64).rem_euclid(spd);
    tods.iter()
        .copied()
        .min_by_key(|&t| circ_dist(t, mean_tod, spd))
        .unwrap()
}

/// `group` の現在配置からグループ anchor (時刻帯の代表値) を求める。
/// 配置済み member が 2 件未満なら `None`。
fn current_anchor_tod(current: &Plan, group: &HabitGroup, spd: i64) -> Option<i64> {
    let mut tods: Vec<i64> = Vec::with_capacity(group.members.len());
    for p in &current.schedules {
        if group.set.contains(&p.task_id) {
            tods.push(tod_of(p.start, spd));
        }
    }
    (tods.len() >= 2).then(|| circular_mean_nearest(&tods, spd))
}

/// グループの anchor を `delta` だけ時刻帯方向に移動し、非 exception・非 fixed・
/// 非 pinned member を `day_base + new_tod` へ一律配置する。各 member の
/// duration は保持する。何も変わらなければ `None`。
pub(crate) fn apply_anchor_shift(
    planner: &Planner,
    current: &Plan,
    group: &HabitGroup,
    delta: i64,
    pinned: &FxHashSet<usize>,
) -> Option<Plan> {
    let spd = slots_per_day(planner);
    let anchor = current_anchor_tod(current, group, spd)?;
    let new_tod = (anchor + delta).rem_euclid(spd);
    let threshold = exception_threshold(spd);

    let mut new_scheds = current.schedules.clone();
    let mut changed = false;
    for entry in new_scheds.iter_mut() {
        let s = entry.start;
        let e = entry.end;
        let id = entry.task_id;
        if !group.set.contains(&id) {
            continue;
        }
        if planner.tasks()[id].fixed || pinned.contains(&id) {
            continue;
        }
        // exception: anchor から大きく外れた member は引き戻さない。
        if circ_dist(tod_of(s, spd), anchor, spd) > threshold {
            continue;
        }
        let dur = e - s;
        // この occurrence の「日」の基点 (raw 時刻帯を引いて得る)。
        let day_base = s.0 - tod_of(s, spd);
        // 基本は day_base + new_tod で「同じ日 + 新しい時刻帯」を狙うが、
        // now / task.start より前にはクランプする (制約優先)。このため
        // 境界付近では日付成分が意図した日からずれる場合がある。
        let mut new_start = day_base + new_tod;
        if new_start < planner.now.0 {
            new_start = planner.now.0;
        }
        if let Some(ts) = planner.tasks()[id].start
            && new_start < ts.0
        {
            new_start = ts.0;
        }
        if new_start != s.0 {
            *entry = TaskPlacement::new(Point(new_start), Point(new_start) + dur, id);
            changed = true;
        }
    }
    changed.then_some(Plan {
        schedules: new_scheds,
    })
}

/// member 1 件を `delta` (絶対スロット) だけ個別移動する。anchor から外れるため、
/// 次回の anchor 計算で exception として扱われる。
pub(crate) fn apply_member_shift(
    planner: &Planner,
    current: &Plan,
    member: usize,
    delta: i64,
    pinned: &FxHashSet<usize>,
) -> Option<Plan> {
    if planner.tasks()[member].fixed || pinned.contains(&member) {
        return None;
    }
    let mut new_scheds = current.schedules.clone();
    for entry in new_scheds.iter_mut() {
        let s = entry.start;
        let e = entry.end;
        let id = entry.task_id;
        if id != member {
            continue;
        }
        let dur = e - s;
        let mut new_start = s.0 + delta;
        if new_start < planner.now.0 {
            new_start = planner.now.0;
        }
        if let Some(ts) = planner.tasks()[member].start
            && new_start < ts.0
        {
            new_start = ts.0;
        }
        if new_start == s.0 {
            return None;
        }
        *entry = TaskPlacement::new(Point(new_start), Point(new_start) + dur, member);
        return Some(Plan {
            schedules: new_scheds,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use takusu_types::{NormalDist, ParallelMode, Slots};

    const SPD: i64 = 288; // per = 5

    fn group(members: &[usize]) -> HabitGroup {
        HabitGroup {
            set: members.iter().copied().collect(),
            members: members.to_vec(),
        }
    }

    fn habit_task(day_start: i64, tod: i64, dur: i64, fixed: bool) -> Task {
        Task {
            id: 0,
            start: Some(Point(day_start + tod)),
            end: Point(day_start + tod + dur + 50),
            cost_estimate: NormalDist::new(dur as u64, 0),
            depends: vec![],
            parallel_mode: ParallelMode::Exclusive,
            abandonability: 0.3.into(),
            fixed,
            habit_group: Some(0),
        }
    }

    /// `days` 日分の daily habit (tod, dur) を group 0 で作る。
    fn habit_planner(days: usize, tod: i64, dur: i64) -> (Planner, Vec<usize>) {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let mut ids = vec![];
        for d in 0..days {
            let day_start = (d as i64 + 1) * SPD;
            let id = p.add(habit_task(day_start, tod, dur, false)).unwrap();
            ids.push(id);
        }
        (p, ids)
    }

    fn plan_at_starts(p: &Planner, ids: &[usize], dur: i64) -> Plan {
        Plan {
            schedules: ids
                .iter()
                .map(|&id| {
                    let s = p.tasks()[id].start.unwrap();
                    TaskPlacement::new(s, s + Slots(dur), id)
                })
                .collect(),
        }
    }

    fn empty_pinned() -> FxHashSet<usize> {
        FxHashSet::default()
    }

    #[test]
    fn build_index_groups_by_habit_group() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let a0 = p.add(habit_task(SPD, 100, 6, false)).unwrap();
        let a1 = p.add(habit_task(2 * SPD, 100, 6, false)).unwrap();
        let mut t = habit_task(SPD, 200, 6, false);
        t.habit_group = Some(1);
        let b0 = p.add(t).unwrap();
        let mut t = habit_task(2 * SPD, 200, 6, false);
        t.habit_group = Some(1);
        let b1 = p.add(t).unwrap();
        let mut t = habit_task(SPD, 50, 6, false);
        t.habit_group = None;
        let _c = p.add(t).unwrap();

        let index = build_index(&p);
        assert_eq!(index.groups.len(), 2);
        assert_eq!(index.groups[0].members, vec![a0, a1]);
        assert_eq!(index.groups[1].members, vec![b0, b1]);
        assert!(index.groups[0].set.contains(&a0));
        assert!(!index.groups[0].set.contains(&b0));
    }

    #[test]
    fn build_index_single_member_skipped() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let _a = p.add(habit_task(SPD, 100, 6, false)).unwrap();
        assert!(build_index(&p).groups.is_empty());
    }

    #[test]
    fn no_habit_returns_empty() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let mut t = habit_task(SPD, 100, 6, false);
        t.habit_group = None;
        let _a = p.add(t).unwrap();
        let mut t = habit_task(2 * SPD, 100, 6, false);
        t.habit_group = None;
        let _b = p.add(t).unwrap();
        assert!(build_index(&p).groups.is_empty());
    }

    #[test]
    fn anchor_tod_is_common_time() {
        let (p, ids) = habit_planner(5, 108, 6);
        let plan = plan_at_starts(&p, &ids, 6);
        let anchor = current_anchor_tod(&plan, &group(&ids), SPD).unwrap();
        assert_eq!(anchor, 108);
    }

    #[test]
    fn apply_anchor_shift_moves_all_to_new_tod() {
        let (p, ids) = habit_planner(5, 108, 6);
        let plan = plan_at_starts(&p, &ids, 6);

        let moved = apply_anchor_shift(&p, &plan, &group(&ids), 5, &empty_pinned()).unwrap();
        for p in &moved.schedules {
            let s = p.start;
            let e = p.end;
            assert_eq!(tod_of(s, SPD), 113, "all members should sit at new tod");
            assert_eq!(e.0 - s.0, 6, "duration preserved");
        }
    }

    #[test]
    fn apply_anchor_shift_preserves_day() {
        let (p, ids) = habit_planner(5, 108, 6);
        let plan = plan_at_starts(&p, &ids, 6);

        let moved = apply_anchor_shift(&p, &plan, &group(&ids), 5, &empty_pinned()).unwrap();
        let mut before: Vec<i64> = plan
            .schedules
            .iter()
            .map(|p| p.start.0 - p.start.0 % SPD)
            .collect();
        let mut after: Vec<i64> = moved
            .schedules
            .iter()
            .map(|p| p.start.0 - p.start.0 % SPD)
            .collect();
        before.sort_unstable();
        after.sort_unstable();
        assert_eq!(before, after, "each occurrence keeps its day");
    }

    #[test]
    fn apply_anchor_shift_skips_fixed() {
        let mut p = Planner::new(PlannerConfig::new(Point(0), SleepConfig::disabled()));
        let a = p.add(habit_task(SPD, 108, 6, false)).unwrap();
        let b = p.add(habit_task(2 * SPD, 108, 6, true)).unwrap();
        let plan = plan_at_starts(&p, &[a, b], 6);

        let moved = apply_anchor_shift(&p, &plan, &group(&[a, b]), 5, &empty_pinned()).unwrap();
        let b_after = moved.schedules.iter().find(|p| p.task_id == b).unwrap();
        assert_eq!(
            b_after.start,
            Point(2 * SPD + 108),
            "fixed member unchanged"
        );
        let a_after = moved.schedules.iter().find(|p| p.task_id == a).unwrap();
        assert_eq!(tod_of(a_after.start, SPD), 113, "movable member shifted");
    }

    #[test]
    fn apply_anchor_shift_skips_pinned() {
        let (p, ids) = habit_planner(3, 108, 6);
        let plan = plan_at_starts(&p, &ids, 6);

        let mut pinned = empty_pinned();
        pinned.insert(ids[1]);
        let moved = apply_anchor_shift(&p, &plan, &group(&ids), 5, &pinned).unwrap();
        let pinned_after = moved
            .schedules
            .iter()
            .find(|p| p.task_id == ids[1])
            .unwrap();
        assert_eq!(
            pinned_after.start,
            Point(2 * SPD + 108),
            "pinned member unchanged"
        );
    }

    #[test]
    fn apply_anchor_shift_skips_exception() {
        let (p, ids) = habit_planner(4, 108, 6);
        let mut plan = plan_at_starts(&p, &ids, 6);
        // deviate one member far from the anchor -> becomes an exception
        plan.schedules[2].start = plan.schedules[2].start + Slots(100);
        plan.schedules[2].end = plan.schedules[2].end + Slots(100);

        let moved = apply_anchor_shift(&p, &plan, &group(&ids), 5, &empty_pinned()).unwrap();
        let exc_after = moved
            .schedules
            .iter()
            .find(|p| p.task_id == ids[2])
            .unwrap();
        assert_eq!(
            exc_after.start,
            Point(3 * SPD + 108 + 100),
            "exception member is not yanked back to the anchor"
        );
    }

    #[test]
    fn apply_anchor_shift_respects_task_start() {
        let mut p = Planner::new(PlannerConfig::new(
            Point(SPD + 110),
            SleepConfig::disabled(),
        ));
        let a = p.add(habit_task(SPD, 108, 6, false)).unwrap();
        let b = p.add(habit_task(2 * SPD, 108, 6, false)).unwrap();
        let plan = plan_at_starts(&p, &[a, b], 6);

        let moved = apply_anchor_shift(&p, &plan, &group(&[a, b]), -50, &empty_pinned()).unwrap();
        let a_after = moved.schedules.iter().find(|p| p.task_id == a).unwrap();
        assert!(a_after.start.0 >= p.now.0, "clamp to now respected");
    }

    #[test]
    fn apply_member_shift_creates_deviation() {
        let (p, ids) = habit_planner(4, 108, 6);
        let plan = plan_at_starts(&p, &ids, 6);

        let moved = apply_member_shift(&p, &plan, ids[1], 100, &empty_pinned()).unwrap();
        // deviated member is now far from the group anchor → exception, so a
        // subsequent anchor shift must not move it.
        let again = apply_anchor_shift(&p, &moved, &group(&ids), 5, &empty_pinned()).unwrap();
        let exc_after = again
            .schedules
            .iter()
            .find(|p| p.task_id == ids[1])
            .unwrap();
        let moved_after = moved
            .schedules
            .iter()
            .find(|p| p.task_id == ids[1])
            .unwrap();
        assert_eq!(
            exc_after.start, moved_after.start,
            "individually moved member stays an exception"
        );
    }

    #[test]
    fn circular_mean_nearest_picks_common_value() {
        assert_eq!(circular_mean_nearest(&[100, 100, 100, 250], SPD), 100);
        // 日跨ぎ: 10 と 280 は円周上で 18 しか離れていないので 280 側に寄る。
        assert_eq!(circular_mean_nearest(&[10, 280, 280, 280], SPD), 280);
    }

    #[test]
    fn circular_mean_nearest_stays_in_range() {
        // 平均方向が spd 付近 (287.x) に落ちても、結果は 0..spd に収まる。
        for spd in [288i64, 100, 7] {
            for &t in &[0i64, 1, spd / 2, spd - 2, spd - 1] {
                let r = circular_mean_nearest(&[t, t, t], spd);
                assert!((0..spd).contains(&r), "spd={spd} t={t} -> {r}");
            }
        }
    }
}
