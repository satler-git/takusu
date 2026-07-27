use jiff::Timestamp;
use jiff::ToSpan;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use takusu_core::{Abandonability, NormalDist, Point, Task};

use crate::GeneratedTask;
use crate::rule::{Frequency, NWeekday, RecurrenceRule, Weekday};
use crate::time::{SLOT_MINUTES, TimeOfDay, date_time_to_point, date_to_day_number, point_to_date};

pub struct RecurrenceGenerator {
    rule: RecurrenceRule,
    start_time: TimeOfDay,
    tz: TimeZone,
    duration: NormalDist,
    deadline_slots: Option<u64>,
    parallelizable: bool,
    allows_parallel: bool,
    abandonability: Abandonability,
    fixed: bool,
    start_point: Point,
    until_point: Point,
    start_date: Date,
    end_date: Date,
    current_date: Date,
    occurrence_count: u32,
    done: bool,
}

impl RecurrenceGenerator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rule: RecurrenceRule,
        start_time: TimeOfDay,
        tz: TimeZone,
        duration: NormalDist,
        deadline_slots: Option<u64>,
        parallelizable: bool,
        allows_parallel: bool,
        abandonability: Abandonability,
        fixed: bool,
        start: Point,
        until: Point,
    ) -> Self {
        // Clamp interval=0 to 1 to avoid division-by-zero panic in
        // matches_frequency / is_in_weekly_interval. interval=0 can arrive
        // via serde deserialization of untrusted recurrence JSON (#273).
        let mut rule = rule;
        if rule.interval == 0 {
            rule.interval = 1;
        }
        let start_date_opt = point_to_date(start, &tz);
        // If until is out of representable range, cap end_date to Date::MAX
        // and also cap until_point to the maximum valid point (derived from
        // Timestamp::MAX) so the start_pt >= until_point check in next()
        // terminates the loop instead of iterating through ~2.9M days to
        // Date::MAX (#276). We use Timestamp::MAX rather than
        // date_time_to_point(Date::MAX, ...) because the latter fails for
        // Date::MAX in jiff.
        let (end_date, until_point) = match point_to_date(until, &tz) {
            Some(d) => (d, until),
            None => {
                let max_point = Point::from_timestamp(Timestamp::MAX, SLOT_MINUTES as u16);
                (Date::MAX, max_point)
            }
        };
        let (start_date, done) = match start_date_opt {
            Some(d) => (d, false),
            // start point is out of representable range — no tasks to generate
            None => (Date::MAX, true),
        };
        Self {
            rule,
            start_time,
            tz,
            duration,
            deadline_slots,
            parallelizable,
            allows_parallel,
            abandonability,
            fixed,
            start_point: start,
            until_point,
            start_date,
            end_date,
            current_date: start_date,
            occurrence_count: 0,
            done,
        }
    }

    fn matches_frequency(&self, date: Date) -> bool {
        let days_from_start = date_to_day_number(date) - date_to_day_number(self.start_date);
        if days_from_start < 0 {
            return false;
        }
        match self.rule.freq {
            Frequency::Daily => days_from_start % (self.rule.interval as i64) == 0,
            Frequency::Weekly => {
                if self.rule.by_day.is_empty() {
                    let start_wd = self.start_date.weekday();
                    date.weekday() == start_wd
                        && days_from_start % (7 * self.rule.interval as i64) == 0
                } else {
                    self.matches_weekday(date) && self.is_in_weekly_interval(days_from_start)
                }
            }
            Frequency::Monthly => {
                let months = (date.year() as i64 - self.start_date.year() as i64) * 12
                    + (date.month() as i64 - self.start_date.month() as i64);
                let in_interval = months >= 0 && months % (self.rule.interval as i64) == 0;
                if self.rule.by_day.is_empty() && self.rule.by_month_day.is_empty() {
                    in_interval && date.day() == self.start_date.day()
                } else {
                    in_interval
                }
            }
            Frequency::Yearly => {
                let years = date.year() as i64 - self.start_date.year() as i64;
                let in_interval = years >= 0 && years % (self.rule.interval as i64) == 0;
                let mut matches = in_interval;
                if self.rule.by_month.is_empty() {
                    matches = matches && date.month() == self.start_date.month();
                }
                if self.rule.by_day.is_empty() && self.rule.by_month_day.is_empty() {
                    matches = matches && date.day() == self.start_date.day();
                }
                matches
            }
        }
    }

    fn is_in_weekly_interval(&self, days_from_start: i64) -> bool {
        let week_num = days_from_start / 7;
        week_num % (self.rule.interval as i64) == 0
    }

    fn matches_weekday(&self, date: Date) -> bool {
        let jiff_wd = date.weekday();
        let our_wd = Weekday::from_jiff(jiff_wd);
        self.rule.by_day.iter().any(|nw| nw.weekday == our_wd)
    }

    fn matches_by_day(&self, date: Date) -> bool {
        if self.rule.by_day.is_empty() {
            return true;
        }
        self.rule
            .by_day
            .iter()
            .any(|nw| self.is_nth_weekday(date, nw))
    }

    fn is_nth_weekday(&self, date: Date, nw: &NWeekday) -> bool {
        let jiff_wd = date.weekday();
        if Weekday::from_jiff(jiff_wd) != nw.weekday {
            return false;
        }
        match nw.n {
            None => true,
            Some(n) if n > 0 => ((date.day() - 1) / 7 + 1) == n,
            Some(n) if n < 0 => {
                let dim = days_in_month(date);
                let from_end = dim - date.day();
                (from_end / 7 + 1) == -n
            }
            _ => false,
        }
    }

    fn matches_by_month(&self, date: Date) -> bool {
        if self.rule.by_month.is_empty() {
            return true;
        }
        self.rule.by_month.contains(&date.month())
    }

    fn matches_by_month_day(&self, date: Date) -> bool {
        if self.rule.by_month_day.is_empty() {
            return true;
        }
        let dim = days_in_month(date);
        self.rule.by_month_day.iter().any(|&d| {
            if d > 0 {
                date.day() == d
            } else {
                (dim + d + 1) == date.day()
            }
        })
    }

    fn is_exdate(&self, date: Date) -> bool {
        self.rule.exdates.contains(&date)
    }

    fn effective_deadline(&self) -> u64 {
        self.deadline_slots.unwrap_or(self.duration.avg)
    }
}

pub(crate) fn days_in_month(date: Date) -> i8 {
    let m = date.month();
    let y = date.year();
    let days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut d = days[(m - 1) as usize];
    if m == 2 && (y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)) {
        d = 29;
    }
    d
}

impl Iterator for RecurrenceGenerator {
    type Item = GeneratedTask;

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_date <= self.end_date && !self.done {
            let date = self.current_date;
            self.current_date = match date.checked_add(1.day()) {
                Ok(d) => d,
                // Date::MAX + 1 day: can't advance further, stop iterating
                // to avoid infinite loop when end_date == Date::MAX (#275)
                Err(_) => {
                    self.done = true;
                    Date::MAX
                }
            };

            if !self.matches_frequency(date) {
                continue;
            }
            if !self.matches_by_day(date) {
                continue;
            }
            if !self.matches_by_month(date) {
                continue;
            }
            if !self.matches_by_month_day(date) {
                continue;
            }
            if self.is_exdate(date) {
                continue;
            }

            let start_pt = match date_time_to_point(date, &self.start_time, &self.tz) {
                Some(p) => p,
                None => continue,
            };

            if start_pt < self.start_point {
                continue;
            }
            if start_pt >= self.until_point {
                self.done = true;
                continue;
            }

            self.occurrence_count += 1;
            if let Some(count) = self.rule.count
                && self.occurrence_count > count
            {
                self.done = true;
                return None;
            }

            let end_pt = start_pt + self.effective_deadline() as i64;

            return Some(GeneratedTask {
                task: Task {
                    id: 0,
                    start: Some(start_pt),
                    end: end_pt,
                    cost_estimate: self.duration,
                    depends: vec![],
                    parallelizable: self.parallelizable,
                    allows_parallel: self.allows_parallel,
                    abandonability: self.abandonability,
                    fixed: self.fixed,
                    habit_group: None,
                },
            });
        }
        None
    }
}
