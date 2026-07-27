//! # takusu-habit — habitual task generator
//!
//! 習慣的なタスクを生成し、takusu-core の Planner に渡せる `Task` に変換する。
//!
//! ## 設計
//!
//! - `HabitConfig` trait: 習慣定義 → ジェネレータ変換
//! - `HabitGenerator` trait: `Iterator<Item = GeneratedTask>` を満たすマーカーtrait
//! - `Habit`: 組み込み実装 (RecurrenceRule ベース)
//! - `HabitStore`: 複数習慣の集約, `generate(start, until)` で一括生成
//!
//! ## 例
//!
//! ```no_run
//! use takusu_habit::*;
//! use takusu_core::{NormalDist, Point};
//! use jiff::tz::TimeZone;
//!
//! let mut store = HabitStore::new();
//!
//! let habit = Habit {
//!     recurrence: RecurrenceRule::weekly()
//!         .by_day(vec![NWeekday::every(Weekday::Mon)]),
//!     start_time: TimeOfDay::new(9, 0).unwrap(),
//!     tz: TimeZone::get("Asia/Tokyo").unwrap(),
//!     duration: NormalDist::new(6, 1),
//!     deadline_slots: None,
//!     parallelizable: false,
//!     allows_parallel: false,
//!     abandonability: 0.3.into(),
//!     fixed: false,
//! };
//! store.add(habit);
//!
//! let start = Point::now(5);
//! let until = start + 7 * 288;
//! let tasks = store.generate(start, until);
//! ```

mod error;
mod generator;
mod parse;
mod rule;
mod summarize;
mod time;

pub use error::Error;
pub use generator::RecurrenceGenerator;
pub use parse::{ParsedRule, Until, parse_rrule};
pub use rule::{Frequency, NWeekday, RecurrenceRule, Weekday};
pub use summarize::summarize;
pub use time::{TimeOfDay, date_time_to_point, date_to_day_number, point_to_date};

use jiff::tz::TimeZone;
use takusu_core::{Abandonability, NormalDist, Point, Task};

/// 習慣から生成されたタスク。task.id は Planner.add() で上書きされる
pub struct GeneratedTask {
    pub task: Task,
}

/// 習慣ジェネレータ。start以降のタスクを順次生成し、untilを超えたらNoneを返す
pub trait HabitGenerator: Iterator<Item = GeneratedTask> {}

impl HabitGenerator for RecurrenceGenerator {}

/// 習慣定義 → ジェネレータ変換trait。
/// カスタムジェネレータを実装する場合はこのtraitを実装する。
pub trait HabitConfig {
    fn timezone(&self) -> &TimeZone;
    fn create_generator(&self, start: Point, until: Point) -> Box<dyn HabitGenerator>;
}

/// 繰り返し習慣のテンプレート
pub struct Habit {
    pub recurrence: RecurrenceRule,
    pub start_time: TimeOfDay,
    pub tz: TimeZone,
    pub duration: NormalDist,
    pub deadline_slots: Option<u64>,
    pub parallelizable: bool,
    pub allows_parallel: bool,
    pub abandonability: Abandonability,
    /// 開始時刻を固定するか。true の場合、生成される Task の fixed が true になる。
    pub fixed: bool,
}

impl HabitConfig for Habit {
    fn timezone(&self) -> &TimeZone {
        &self.tz
    }

    fn create_generator(&self, start: Point, until: Point) -> Box<dyn HabitGenerator> {
        Box::new(RecurrenceGenerator::new(
            self.recurrence.clone(),
            self.start_time,
            self.tz.clone(),
            self.duration,
            self.deadline_slots,
            self.parallelizable,
            self.allows_parallel,
            self.abandonability,
            self.fixed,
            start,
            until,
        ))
    }
}

/// 習慣の集合。generate() で全習慣からタスクを一括生成
pub struct HabitStore {
    configs: Vec<Box<dyn HabitConfig>>,
}

impl HabitStore {
    pub fn new() -> Self {
        Self { configs: vec![] }
    }

    pub fn add<C: HabitConfig + 'static>(&mut self, config: C) {
        self.configs.push(Box::new(config));
    }

    pub fn generate(&self, start: Point, until: Point) -> Vec<GeneratedTask> {
        self.configs
            .iter()
            .flat_map(|config| config.create_generator(start, until))
            .collect()
    }
}

impl Default for HabitStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn utc() -> TimeZone {
        TimeZone::get("Etc/UTC").unwrap()
    }

    fn tokyo() -> TimeZone {
        TimeZone::get("Asia/Tokyo").unwrap()
    }

    fn point_at(date: jiff::civil::Date, time: &TimeOfDay, tz: &TimeZone) -> Point {
        time::date_time_to_point(date, time, tz).unwrap()
    }

    fn date_at(point: Point, tz: &TimeZone) -> jiff::civil::Date {
        time::point_to_date(point, tz).unwrap()
    }

    #[test]
    fn time_of_day_new_valid() {
        let t = TimeOfDay::new(9, 30).unwrap();
        assert_eq!(t.hour, 9);
        assert_eq!(t.minute, 30);
    }

    #[test]
    fn time_of_day_snaps_to_5min() {
        let t = TimeOfDay::new(9, 33).unwrap();
        assert_eq!(t.minute, 30);
        let t2 = TimeOfDay::new(9, 37).unwrap();
        assert_eq!(t2.minute, 35);
    }

    #[test]
    fn time_of_day_rejects_invalid() {
        assert!(TimeOfDay::new(24, 0).is_none());
        assert!(TimeOfDay::new(0, 60).is_none());
    }

    #[test]
    fn weekday_roundtrip() {
        for wd in [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ] {
            let jiff_wd = wd.to_jiff();
            let back = Weekday::from_jiff(jiff_wd);
            assert_eq!(wd, back);
        }
    }

    #[test]
    fn date_to_day_number_known() {
        assert_eq!(time::date_to_day_number(date(2000, 1, 1)), 2451545);
        assert_eq!(time::date_to_day_number(date(2025, 1, 1)), 2460677);
    }

    #[test]
    fn point_date_roundtrip_utc() {
        let tz = utc();
        let t = TimeOfDay::new(14, 30).unwrap();
        let d = date(2025, 6, 15);
        let pt = point_at(d, &t, &tz);
        let back = date_at(pt, &tz);
        assert_eq!(back, d);
    }

    #[test]
    fn point_date_roundtrip_fixed_offset() {
        let tz = jiff::tz::TimeZone::fixed(jiff::tz::offset(9));
        let t = TimeOfDay::new(14, 30).unwrap();
        let d = date(2025, 6, 15);
        let pt = point_at(d, &t, &tz);
        let back = date_at(pt, &tz);
        assert_eq!(back, d);
    }

    #[test]
    fn days_in_month_feb_leap() {
        assert_eq!(generator::days_in_month(date(2024, 2, 1)), 29);
        assert_eq!(generator::days_in_month(date(2023, 2, 1)), 28);
    }

    #[test]
    fn days_in_month_regular() {
        assert_eq!(generator::days_in_month(date(2025, 1, 1)), 31);
        assert_eq!(generator::days_in_month(date(2025, 4, 1)), 30);
    }

    #[test]
    fn daily_generates_each_day() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 7), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily(),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.5.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 6);
        for (i, gt) in tasks.iter().enumerate() {
            let expected_date = date(2025, 3, 1 + i as i8);
            let expected_start = point_at(expected_date, &TimeOfDay::new(9, 0).unwrap(), &tz);
            assert_eq!(gt.task.start, Some(expected_start));
        }
    }

    #[test]
    fn daily_with_interval() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(10, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 10), &TimeOfDay::new(10, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().interval(3),
            TimeOfDay::new(10, 0).unwrap(),
            tz.clone(),
            NormalDist::new(12, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(dates.len(), 3);
        assert_eq!(dates[0], date(2025, 3, 1));
        assert_eq!(dates[1], date(2025, 3, 4));
        assert_eq!(dates[2], date(2025, 3, 7));
    }

    #[test]
    fn weekly_by_day_mon_wed_fri() {
        let tz = utc();
        let start = point_at(date(2025, 3, 3), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 10), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::weekly().by_day(vec![
                NWeekday::every(Weekday::Mon),
                NWeekday::every(Weekday::Wed),
                NWeekday::every(Weekday::Fri),
            ]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.3.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(dates.len(), 3);
        assert!(dates.contains(&date(2025, 3, 3)));
        assert!(dates.contains(&date(2025, 3, 5)));
        assert!(dates.contains(&date(2025, 3, 7)));
    }

    #[test]
    fn count_limits_occurrences() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().count(3),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn exdate_skips_dates() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 6), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().exdates(vec![date(2025, 3, 3)]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(tasks.len(), 4);
        assert!(!dates.contains(&date(2025, 3, 3)));
    }

    #[test]
    fn by_month_filters() {
        let tz = utc();
        let start = point_at(date(2025, 1, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::monthly().by_month(vec![1, 3]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 2);
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert!(dates.contains(&date(2025, 1, 1)));
        assert!(dates.contains(&date(2025, 3, 1)));
    }

    #[test]
    fn by_month_day_filters() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().by_month_day(vec![1, 15]),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn nth_weekday_of_month() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::monthly().by_day(vec![NWeekday::nth(2, Weekday::Fri)]),
            TimeOfDay::new(10, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(dates.len(), 1);
        // 2nd Friday of March 2025 = March 14
        assert_eq!(dates[0], date(2025, 3, 14));
    }

    #[test]
    fn yearly_same_date() {
        let tz = utc();
        let start = point_at(date(2023, 6, 15), &TimeOfDay::new(8, 0).unwrap(), &tz);
        let until = point_at(date(2026, 6, 15), &TimeOfDay::new(8, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::yearly()
                .by_month_day(vec![15])
                .by_month(vec![6]),
            TimeOfDay::new(8, 0).unwrap(),
            tz,
            NormalDist::new(12, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 3);
    }

    // Regression (#780): YEARLY with BYMONTH (but no BYMONTHDAY/BYDAY)
    // should default to the start date's month-day, not match every day
    // in the specified months.
    #[test]
    fn regression_yearly_by_month_defaults_to_start_day() {
        let tz = utc();
        let start = point_at(date(2025, 6, 15), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2028, 6, 15), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::yearly().by_month(vec![6]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(
            dates,
            vec![date(2025, 6, 15), date(2026, 6, 15), date(2027, 6, 15)]
        );
    }

    // Regression (#780): YEARLY with BYDAY (but no BYMONTH/BYMONTHDAY)
    // should default to the start date's month, not match the nth weekday
    // in every month of each year.
    #[test]
    fn regression_yearly_by_day_defaults_to_start_month() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2027, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::yearly().by_day(vec![NWeekday::nth(2, Weekday::Fri)]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        // 2nd Friday of March in 2025-2027.
        assert_eq!(
            dates,
            vec![date(2025, 3, 14), date(2026, 3, 13), date(2027, 3, 12)]
        );
    }

    #[test]
    fn deadline_slots_overrides_duration() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 2), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().count(1),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            Some(24),
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0].task;
        let duration_slots = task.end.0 - task.start.unwrap().0;
        assert_eq!(duration_slots, 24);
    }

    #[test]
    fn task_properties_preserved() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 8), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().count(1),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 1),
            None,
            true,
            true,
            0.7.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let t = &tasks[0].task;
        assert!(t.parallelizable);
        assert!(t.allows_parallel);
        assert!((t.abandonability.get() - 0.7).abs() < 1e-10);
        assert_eq!(t.cost_estimate.avg, 6);
        assert_eq!(t.cost_estimate.sigma, 1);
        assert!(t.depends.is_empty());
    }

    #[test]
    fn habit_store_generate_multiple() {
        let tz = utc();
        let mut store = HabitStore::new();

        store.add(Habit {
            recurrence: RecurrenceRule::daily().count(2),
            start_time: TimeOfDay::new(9, 0).unwrap(),
            tz: tz.clone(),
            duration: NormalDist::new(6, 0),
            deadline_slots: None,
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
        });

        store.add(Habit {
            recurrence: RecurrenceRule::daily().count(2),
            start_time: TimeOfDay::new(14, 0).unwrap(),
            tz: tz.clone(),
            duration: NormalDist::new(12, 0),
            deadline_slots: None,
            parallelizable: true,
            allows_parallel: false,
            abandonability: 0.2.into(),
            fixed: false,
        });

        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(0, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 8), &TimeOfDay::new(0, 0).unwrap(), &tz);

        let tasks = store.generate(start, until);
        assert_eq!(tasks.len(), 4);
    }

    #[test]
    fn empty_store_generates_nothing() {
        let store = HabitStore::new();
        let start = Point::from_raw(0);
        let until = Point::from_raw(10000);
        let tasks = store.generate(start, until);
        assert!(tasks.is_empty());
    }

    #[test]
    fn timezone_affects_point_calculation() {
        let utc_tz = utc();
        let tokyo_tz = tokyo();
        let time = TimeOfDay::new(9, 0).unwrap();
        let d = date(2025, 6, 15);

        let utc_point = point_at(d, &time, &utc_tz);
        let tokyo_point = point_at(d, &time, &tokyo_tz);

        // Asia/Tokyo = UTC+9, so 9:00 JST = 0:00 UTC
        let diff = utc_point.0 - tokyo_point.0;
        assert_eq!(diff, 9 * 12);
    }

    #[test]
    fn start_before_range_is_skipped() {
        let tz = utc();
        let range_start = point_at(date(2025, 3, 3), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 6), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily(),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            range_start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn negative_by_month_day() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        // -1 = last day of month
        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().by_month_day(vec![-1]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0], date(2025, 3, 31));
    }

    // ── RRULE edge cases ────────────────────────────────────────────────

    #[test]
    fn negative_nth_weekday_last_friday() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        // -1 = last Friday of the month. March 2025: last Friday = March 28.
        let iter = RecurrenceGenerator::new(
            RecurrenceRule::monthly().by_day(vec![NWeekday::nth(-1, Weekday::Fri)]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0], date(2025, 3, 28));
    }

    #[test]
    fn negative_nth_weekday_second_to_last_monday() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        // -2 = second-to-last Monday of March 2025.
        // March 2025 Mondays: 3, 10, 17, 24, 31. Last = 31, 2nd-to-last = 24.
        let iter = RecurrenceGenerator::new(
            RecurrenceRule::monthly().by_day(vec![NWeekday::nth(-2, Weekday::Mon)]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(dates.len(), 1);
        assert_eq!(dates[0], date(2025, 3, 24));
    }

    #[test]
    fn weekly_interval_skips_weeks() {
        let tz = utc();
        // Start on Monday March 3 2025.
        let start = point_at(date(2025, 3, 3), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 7), &TimeOfDay::new(9, 0).unwrap(), &tz);

        // Every other Monday (interval=2). Mondays in range:
        // Mar 3 (week 0), Mar 17 (week 2), Mar 31 (week 4).
        // Mar 10 / Mar 24 / Apr 7 are in odd weeks → skipped.
        let iter = RecurrenceGenerator::new(
            RecurrenceRule::weekly()
                .interval(2)
                .by_day(vec![NWeekday::every(Weekday::Mon)]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(
            dates,
            vec![date(2025, 3, 3), date(2025, 3, 17), date(2025, 3, 31)]
        );
    }

    #[test]
    fn count_does_not_count_exdated_occurrences() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 31), &TimeOfDay::new(9, 0).unwrap(), &tz);

        // count=3 but exdate removes the 2nd and 4th days. The 3 counted
        // occurrences should be Mar 1, Mar 3, Mar 4 (Mar 2 exdated).
        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily()
                .count(3)
                .exdates(vec![date(2025, 3, 2)]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(
            dates,
            vec![date(2025, 3, 1), date(2025, 3, 3), date(2025, 3, 4)]
        );
    }

    #[test]
    fn until_boundary_excludes_at_until() {
        let tz = utc();
        // until is exclusive: a task starting exactly at until is not emitted.
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 3), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily(),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &utc()))
            .collect();
        // Mar 1 and Mar 2 only; Mar 3 == until → excluded.
        assert_eq!(dates, vec![date(2025, 3, 1), date(2025, 3, 2)]);
    }

    #[test]
    fn deadline_slots_sets_task_end() {
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 2), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().count(1),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            Some(48),
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let t = &tasks[0].task;
        let dur = t.end.0 - t.start.unwrap().0;
        assert_eq!(dur, 48, "deadline_slots should set end = start + 48 slots");
    }

    #[test]
    fn recurrence_rule_serde_roundtrip() {
        let rule = RecurrenceRule::weekly()
            .interval(2)
            .by_day(vec![
                NWeekday::every(Weekday::Mon),
                NWeekday::nth(2, Weekday::Fri),
            ])
            .by_month(vec![1, 7])
            .by_month_day(vec![15, -1])
            .count(10)
            .exdates(vec![date(2025, 3, 3)]);
        let json = serde_json::to_string(&rule).unwrap();
        let back: RecurrenceRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back.freq, Frequency::Weekly);
        assert_eq!(back.interval, 2);
        assert_eq!(back.by_day.len(), 2);
        assert_eq!(back.by_month, vec![1, 7]);
        assert_eq!(back.by_month_day, vec![15, -1]);
        assert_eq!(back.count, Some(10));
        assert_eq!(back.exdates, vec![date(2025, 3, 3)]);
    }

    #[test]
    fn monthly_without_by_day_uses_start_day() {
        let tz = utc();
        // Start on Jan 15. Monthly with no by_day/by_month_day → 15th of each month.
        let start = point_at(date(2025, 1, 15), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 4, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::monthly(),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(
            dates,
            vec![date(2025, 1, 15), date(2025, 2, 15), date(2025, 3, 15)]
        );
    }

    #[test]
    fn daily_by_day_weekdays_from_sunday() {
        let tz = tokyo();
        let start_time = TimeOfDay::new(8, 40).unwrap();
        let start = point_at(date(2026, 7, 5), &start_time, &tz); // Sunday
        let until = point_at(date(2026, 7, 12), &start_time, &tz); // next Sunday

        let rule_json = r#"{"freq":"daily","interval":1,"by_day":[{"n":null,"weekday":"mon"},{"n":null,"weekday":"tue"},{"n":null,"weekday":"wed"},{"n":null,"weekday":"thu"},{"n":null,"weekday":"fri"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#;
        let rule: RecurrenceRule = serde_json::from_str(rule_json).unwrap();
        println!(
            "Parsed rule: freq={:?}, by_day={:?}",
            rule.freq, rule.by_day
        );

        let iter = RecurrenceGenerator::new(
            rule,
            start_time,
            tz.clone(),
            NormalDist::new(94, 3),
            None,
            false,
            false,
            0.2.into(),
            false,
            start,
            until,
        );

        let dates: Vec<_> = iter
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        for d in &dates {
            println!("Generated: {} ({:?})", d, d.weekday());
        }
        assert!(
            !dates.contains(&date(2026, 7, 5)),
            "Sunday 7/5 should NOT be generated"
        );
        assert!(
            dates.contains(&date(2026, 7, 6)),
            "Monday 7/6 should be generated"
        );
        assert!(
            dates.contains(&date(2026, 7, 10)),
            "Friday 7/10 should be generated"
        );
        assert!(
            !dates.contains(&date(2026, 7, 11)),
            "Saturday 7/11 should NOT be generated"
        );
    }

    // ── Bug fix tests ───────────────────────────────────────────────────

    #[test]
    fn interval_zero_does_not_panic_daily() {
        // #273: interval=0 arriving via deserialized JSON should be clamped
        // to 1 instead of panicking on division-by-zero.
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 4), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let rule_json = r#"{"freq":"daily","interval":0,"by_day":[],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#;
        let rule: RecurrenceRule = serde_json::from_str(rule_json).unwrap();

        let iter = RecurrenceGenerator::new(
            rule,
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        // Should behave like interval=1 (daily): Mar 1, 2, 3
        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 3);
    }

    #[test]
    fn interval_zero_does_not_panic_weekly() {
        // #273: weekly with interval=0 and by_day should also not panic.
        let tz = utc();
        let start = point_at(date(2025, 3, 3), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2025, 3, 10), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let rule_json = r#"{"freq":"weekly","interval":0,"by_day":[{"n":null,"weekday":"mon"}],"by_month":[],"by_month_day":[],"count":null,"exdates":[]}"#;
        let rule: RecurrenceRule = serde_json::from_str(rule_json).unwrap();

        let iter = RecurrenceGenerator::new(
            rule,
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        // interval=0 clamped to 1: Mar 3 (Mon) and Mar 10 (Mon) — but until is
        // exclusive at Mar 10 09:00, so only Mar 3.
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn interval_zero_builder_clamps_to_one() {
        // #273: the builder method should also clamp interval=0 to 1.
        let rule = RecurrenceRule::daily().interval(0);
        assert_eq!(rule.interval, 1);
    }

    #[test]
    fn date_max_until_terminates() {
        // #275: when end_date saturates to Date::MAX, the generator must
        // terminate instead of looping forever.
        let tz = utc();
        let start = point_at(date(2025, 3, 1), &TimeOfDay::new(9, 0).unwrap(), &tz);
        // Use count=1 so we only generate one task, but set until to a point
        // that maps to Date::MAX (i64::MAX slots → overflow → None → Date::MAX).
        let until = Point::from_raw(i64::MAX);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily().count(1),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        // This should terminate (not hang) and produce 1 task.
        let tasks: Vec<_> = iter.collect();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn point_to_date_overflow_returns_none() {
        // #276: point_to_date should return None for out-of-range points
        // instead of silently mapping to 1970-01-01.
        let tz = utc();
        // i64::MAX slots × 5 min × 60 sec overflows i64
        let overflow_point = Point::from_raw(i64::MAX);
        assert!(time::point_to_date(overflow_point, &tz).is_none());

        // Negative point also overflows when multiplied
        let neg_point = Point::from_raw(i64::MIN);
        assert!(time::point_to_date(neg_point, &tz).is_none());
    }

    #[test]
    fn point_to_date_valid_point_returns_some() {
        // #276: valid points should still work correctly.
        let tz = utc();
        let pt = point_at(date(2025, 6, 15), &TimeOfDay::new(14, 30).unwrap(), &tz);
        assert_eq!(time::point_to_date(pt, &tz), Some(date(2025, 6, 15)));
    }

    #[test]
    fn overflow_until_without_count_terminates() {
        // #276 review follow-up: when until overflows but count is None,
        // the generator must still terminate (until_point is capped to
        // Date::MAX's point so the start_pt >= until_point check fires).
        // Use a start date near Date::MAX so the loop is only ~2 iterations.
        let tz = utc();
        let start = point_at(date(9999, 12, 30), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = Point::from_raw(i64::MAX);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::daily(),
            TimeOfDay::new(9, 0).unwrap(),
            tz,
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        // Should terminate quickly (not hang) and produce at most 1 task
        // (Dec 30; Dec 31 == Date::MAX == capped until_point → excluded).
        let tasks: Vec<_> = iter.collect();
        assert!(tasks.len() <= 1);
    }

    // Regression (#780): YEARLY with BYMONTHDAY but no BYMONTH should
    // default to the start date's month, not match the 15th of every month.
    #[test]
    fn regression_yearly_by_month_day_defaults_to_start_month() {
        let tz = utc();
        let start = point_at(date(2025, 6, 15), &TimeOfDay::new(9, 0).unwrap(), &tz);
        let until = point_at(date(2028, 6, 15), &TimeOfDay::new(9, 0).unwrap(), &tz);

        let iter = RecurrenceGenerator::new(
            RecurrenceRule::yearly().by_month_day(vec![15]),
            TimeOfDay::new(9, 0).unwrap(),
            tz.clone(),
            NormalDist::new(6, 0),
            None,
            false,
            false,
            0.0.into(),
            false,
            start,
            until,
        );

        let tasks: Vec<_> = iter.collect();
        let dates: Vec<_> = tasks
            .iter()
            .map(|gt| date_at(gt.task.start.unwrap(), &tz))
            .collect();
        assert_eq!(
            dates,
            vec![date(2025, 6, 15), date(2026, 6, 15), date(2027, 6, 15)]
        );
    }
}
