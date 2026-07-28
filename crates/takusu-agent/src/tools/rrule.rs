//! RFC 5545 RRULE expansion tool for the agent.
//!
//! `expand_rrule` takes an RFC 5545 RRULE (with optional `DTSTART`) and a
//! count `n`, and returns `n` ISO 8601 datetimes. The implementation parses
//! the rule into the existing `takusu_habit::RecurrenceRule` representation
//! and reuses `RecurrenceGenerator` to produce the dates.

use async_trait::async_trait;
use jiff::{ToSpan, civil::Date};
use schemars::JsonSchema;
use serde::Deserialize;
use takusu_core::{NormalDist, Point, SLOT_MINUTES};
use takusu_habit::{
    Frequency, NWeekday, ParsedRule, RecurrenceGenerator, TimeOfDay, Until, Weekday,
    date_time_to_point, date_to_day_number, parse_rrule, point_to_date,
};

use crate::tools::other_error;
use crate::tools::takusu::TimeZoneCache;
use crate::{
    InvalidArgsError, ToolError, ToolExposure, ToolName, ToolOutput, ToolRegistry, TypedTool,
};

pub fn register_tools(registry: &mut ToolRegistry, tz_cache: TimeZoneCache) {
    registry.register(Box::new(crate::tool::Typed(ExpandRRule { tz_cache })));
}

struct ExpandRRule {
    tz_cache: TimeZoneCache,
}

/// Arguments for [`ExpandRRule`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExpandRRuleArgs {
    /// RFC 5545 recurrence rule. May include a DTSTART line and/or EXDATE lines. Example: 'DTSTART:20260727T090000Z\nRRULE:FREQ=DAILY;COUNT=4;BYDAY=MO,TU,WE,TH,FR'
    rrule: String,
    /// Number of datetimes to return.
    #[schemars(range(min = 1, max = 1000))]
    count: u64,
}

#[async_trait]
impl TypedTool for ExpandRRule {
    type Params = ExpandRRuleArgs;

    fn name(&self) -> &'static str {
        ToolName::ExpandRrule.into()
    }

    fn description(&self) -> &'static str {
        "Expand an RFC 5545 RRULE into a list of n start datetimes. \
        Supports DTSTART, FREQ, INTERVAL, COUNT, UNTIL, BYDAY, BYMONTH, \
        BYMONTHDAY and EXDATE. Times are returned in the timezone specified \
        by DTSTART, or the server timezone when DTSTART is absent."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if args.rrule.is_empty() {
            return Err(InvalidArgsError::new("rrule", "missing or empty"));
        }
        if !(1..=1000).contains(&args.count) {
            return Err(InvalidArgsError::new(
                "count",
                "must be an integer between 1 and 1000",
            ));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let tz = self.tz_cache.get_with_fallback().await;

        let default_start = jiff::Timestamp::now()
            .to_zoned(tz.clone())
            .start_of_day()
            .map_err(|e| ToolError::Other(Box::new(e)))?;

        let parsed = parse_rrule(&args.rrule, &default_start)
            .map_err(|e| ToolError::InvalidArgs(InvalidArgsError::no_field(e.to_string())))?;

        let dates = expand_dates(&parsed, args.count as usize)?;

        Ok(ToolOutput {
            content: serde_json::to_string(&dates).unwrap_or_default(),
            ..Default::default()
        })
    }
}

/// Upper bound on how many days into the future the generator must scan to
/// collect `count` occurrences. The bound is per-frequency and accounts for
/// the sparsest valid pattern for each rule type.
const MAX_LOOKAHEAD_DAYS: i64 = 5_000_000;

fn expand_dates(parsed: &ParsedRule, count: usize) -> Result<Vec<String>, ToolError> {
    // Use the timezone carried by DTSTART. If DTSTART was missing, parse_rrule
    // falls back to the caller's default (server) timezone, so this also
    // covers that case.
    let tz = parsed.start.time_zone().clone();

    let hour = parsed.start.hour();
    let minute = parsed.start.minute();
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "start_time",
            format!("invalid: {hour}:{minute}"),
        )));
    }
    let start_time = TimeOfDay::new(hour as u8, minute as u8)
        .ok_or_else(|| ToolError::InvalidArgs(InvalidArgsError::new("start_time", "invalid")))?;
    let start_point = date_time_to_point(parsed.start.date(), &start_time, &tz)
        .ok_or_else(|| other_error("failed to convert DTSTART to an internal time point"))?;

    const SLOTS_PER_DAY: i64 = 24 * 12;

    // Honour a COUNT in the RRULE itself when it is smaller than the
    // requested count.
    let effective_count = parsed.rule.count.map_or(count, |c| count.min(c as usize));

    let max_gap = estimate_max_gap(parsed);
    let max_days = (effective_count as i64)
        .saturating_mul(max_gap)
        .saturating_add(max_gap)
        .min(MAX_LOOKAHEAD_DAYS);
    let mut until_point = start_point + (max_days * SLOTS_PER_DAY);

    if let Some(until) = &parsed.until {
        let candidate = match until {
            Until::Date(date) => {
                let next = date
                    .checked_add(1.day())
                    .map_err(|e| ToolError::Other(Box::new(e)))?;
                let midnight =
                    TimeOfDay::new(0, 0).ok_or_else(|| other_error("invalid midnight time"))?;
                date_time_to_point(next, &midnight, &tz).ok_or_else(|| {
                    other_error("failed to convert UNTIL date to an internal time point")
                })?
            }
            Until::DateTime(z) => Point::from_timestamp(z.timestamp(), 5) + 1,
        };
        if candidate < until_point {
            until_point = candidate;
        }
    }

    let generator = RecurrenceGenerator::new(
        parsed.rule.clone(),
        start_time,
        tz.clone(),
        NormalDist::new(0, 0),
        None,
        false,
        false,
        0.0.into(),
        false,
        start_point,
        until_point,
    );

    let mut results = Vec::new();
    for gt in generator {
        let point = gt
            .task
            .start
            .ok_or_else(|| other_error("generated occurrence is missing a start time"))?;

        if let Some(until) = &parsed.until {
            match until {
                Until::Date(date) => {
                    let occ_date = point_to_date(point, &tz)
                        .ok_or_else(|| other_error("failed to convert occurrence to a date"))?;
                    if occ_date > *date {
                        continue;
                    }
                }
                Until::DateTime(z) => {
                    if point > Point::from_timestamp(z.timestamp(), 5) {
                        continue;
                    }
                }
            }
        }

        let seconds = point
            .0
            .checked_mul(SLOT_MINUTES)
            .and_then(|s| s.checked_mul(60))
            .ok_or_else(|| other_error("occurrence time overflow"))?;
        let ts =
            jiff::Timestamp::from_second(seconds).map_err(|e| ToolError::Other(Box::new(e)))?;
        let z = ts.to_zoned(tz.clone());
        results.push(z.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string());

        if results.len() >= effective_count {
            break;
        }
    }

    Ok(results)
}

/// Length of the Gregorian 400-year cycle in days. This is the largest
/// period on which both leap-year and weekday patterns repeat.
const GREGORIAN_CYCLE_DAYS: i64 = 146_097;

/// Estimate the largest possible number of days between two consecutive
/// occurrences of `parsed.rule`. This is used to choose a horizon far enough
/// to contain `count` occurrences without scanning the entire Date::MAX
/// range.
fn estimate_max_gap(parsed: &ParsedRule) -> i64 {
    let interval = parsed.rule.interval.max(1) as i64;
    let base = base_max_gap(parsed);

    match parsed.rule.freq {
        // Candidates are every `interval` days. The gap between valid
        // candidates is at most the underlying gap plus one step.
        Frequency::Daily => base + interval,
        // Candidates are every `7 * interval` days, but BYDAY may select
        // multiple weekdays within a candidate week.
        Frequency::Weekly => base + 7 * interval,
        // Candidates are every `interval` months. Multiplying the base gap
        // (which already ignores interval) is a safe upper bound.
        Frequency::Monthly => base * interval,
        Frequency::Yearly => base * interval,
    }
}

/// Maximum gap in days between two days that satisfy the rule's day-level
/// constraints, ignoring FREQ/INTERVAL. Computed by scanning one full
/// Gregorian 400-year cycle, which is sufficient because leap-year and
/// weekday patterns both repeat every 400 years.
fn base_max_gap(parsed: &ParsedRule) -> i64 {
    let start = Date::new(2000, 1, 1).expect("2000-01-01 is valid");
    let end = start
        .checked_add(jiff::Span::new().days(GREGORIAN_CYCLE_DAYS))
        .unwrap_or(Date::MAX);

    let months = effective_months(parsed);
    let days = effective_days(parsed);
    let by_day = &parsed.rule.by_day;

    let mut prev: Option<Date> = None;
    let mut first: Option<Date> = None;
    let mut last: Option<Date> = None;
    let mut max_gap: i64 = 0;

    let mut date = start;
    while date < end {
        if matches_constraints(date, &months, &days, by_day) {
            if let Some(p) = prev {
                max_gap = max_gap.max(date_to_day_number(date) - date_to_day_number(p));
            }
            if first.is_none() {
                first = Some(date);
            }
            last = Some(date);
            prev = Some(date);
        }
        date = date.checked_add(1.day()).unwrap_or(Date::MAX);
        if date == Date::MAX {
            break;
        }
    }

    if let (Some(first), Some(last)) = (first, last) {
        // The Gregorian cycle repeats, so include the wrap-around gap.
        let wrap = date_to_day_number(first) + GREGORIAN_CYCLE_DAYS - date_to_day_number(last);
        max_gap = max_gap.max(wrap);
    }

    max_gap.max(1)
}

fn effective_months(parsed: &ParsedRule) -> Vec<i8> {
    if parsed.rule.freq == Frequency::Yearly && parsed.rule.by_month.is_empty() {
        vec![parsed.start.month()]
    } else if parsed.rule.by_month.is_empty() {
        (1..=12).collect()
    } else {
        parsed.rule.by_month.clone()
    }
}

fn effective_days(parsed: &ParsedRule) -> Vec<i8> {
    if (parsed.rule.freq == Frequency::Monthly || parsed.rule.freq == Frequency::Yearly)
        && parsed.rule.by_month_day.is_empty()
        && parsed.rule.by_day.is_empty()
    {
        vec![parsed.start.day()]
    } else if parsed.rule.by_month_day.is_empty() {
        vec![]
    } else {
        parsed.rule.by_month_day.clone()
    }
}

fn matches_constraints(date: Date, months: &[i8], days: &[i8], by_day: &[NWeekday]) -> bool {
    if !months.contains(&date.month()) {
        return false;
    }
    if !days.is_empty() && !days.iter().any(|&d| matches_month_day(date, d)) {
        return false;
    }
    if by_day.is_empty() {
        return true;
    }
    by_day.iter().any(|nw| is_nth_weekday(date, nw))
}

fn matches_month_day(date: Date, day: i8) -> bool {
    if day > 0 {
        date.day() == day
    } else {
        let dim = days_in_month(date) as i16;
        (dim + day as i16 + 1) == date.day() as i16
    }
}

fn is_nth_weekday(date: Date, nw: &NWeekday) -> bool {
    if Weekday::from_jiff(date.weekday()) != nw.weekday {
        return false;
    }
    match nw.n {
        None => true,
        Some(n) if n > 0 => (date.day() - 1) / 7 + 1 == n,
        Some(n) => {
            let dim = days_in_month(date);
            let from_end = dim - date.day();
            from_end / 7 + 1 == -n
        }
    }
}

fn days_in_month(date: Date) -> i8 {
    let m = date.month();
    let y = date.year();
    let days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if m == 2 && is_leap_year(y) {
        29
    } else {
        days[(m as usize).saturating_sub(1)]
    }
}

fn is_leap_year(y: i16) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;
    use takusu_habit::{NWeekday, RecurrenceRule, Weekday};

    fn parsed_daily() -> ParsedRule {
        ParsedRule {
            rule: RecurrenceRule::daily().count(4),
            start: date(2026, 7, 27).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
            until: None,
        }
    }

    #[test]
    fn expand_daily_count() {
        let dates = expand_dates(&parsed_daily(), 4).unwrap();
        assert_eq!(dates.len(), 4);
        assert!(dates[0].starts_with("2026-07-27T09:00:00"));
        assert!(dates[1].starts_with("2026-07-28T09:00:00"));
        assert!(dates[2].starts_with("2026-07-29T09:00:00"));
        assert!(dates[3].starts_with("2026-07-30T09:00:00"));
    }

    #[test]
    fn expand_weekdays_from_sunday() {
        let parsed = ParsedRule {
            rule: RecurrenceRule::daily().by_day(vec![
                NWeekday::every(Weekday::Mon),
                NWeekday::every(Weekday::Tue),
                NWeekday::every(Weekday::Wed),
                NWeekday::every(Weekday::Thu),
                NWeekday::every(Weekday::Fri),
            ]),
            start: date(2026, 7, 26).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
            until: None,
        };
        let dates = expand_dates(&parsed, 4).unwrap();
        assert_eq!(dates.len(), 4);
        assert!(dates[0].starts_with("2026-07-27T09:00:00")); // Monday
        assert!(dates[1].starts_with("2026-07-28T09:00:00")); // Tuesday
        assert!(dates[2].starts_with("2026-07-29T09:00:00")); // Wednesday
        assert!(dates[3].starts_with("2026-07-30T09:00:00")); // Thursday
    }

    #[test]
    fn expand_until_limits_results() {
        let parsed = ParsedRule {
            rule: RecurrenceRule::daily(),
            start: date(2026, 7, 27).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
            until: Some(Until::Date(date(2026, 7, 29))),
        };
        let dates = expand_dates(&parsed, 10).unwrap();
        assert_eq!(dates.len(), 3);
        assert!(dates[0].starts_with("2026-07-27"));
        assert!(dates[1].starts_with("2026-07-28"));
        assert!(dates[2].starts_with("2026-07-29"));
    }

    #[test]
    fn expand_limits_to_requested_count() {
        // No COUNT in the rule; the tool should return only the requested 2.
        let mut parsed = parsed_daily();
        parsed.rule.count = None;
        let dates = expand_dates(&parsed, 2).unwrap();
        assert_eq!(dates.len(), 2);
    }

    #[test]
    fn expand_dtstart_timezone_is_preserved() {
        // DTSTART in Asia/Tokyo (UTC+9). Without an INTERVAL the generator
        // advances one civil day at a time, so the wall-clock time stays
        // 09:00 Asia/Tokyo.
        let parsed = parse_rrule(
            "DTSTART;TZID=Asia/Tokyo:20260727T090000\nRRULE:FREQ=DAILY;COUNT=3",
            &date(2026, 7, 27).at(0, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        let dates = expand_dates(&parsed, 3).unwrap();
        assert_eq!(dates.len(), 3);
        assert!(dates[0].contains("+09:00"));
        assert!(dates[0].starts_with("2026-07-27T09:00:00"));
        assert!(dates[1].starts_with("2026-07-28T09:00:00"));
    }

    #[test]
    fn expand_yearly_feb29_sparse_recurrence() {
        // February 29th occurs roughly every 4 years. 10 occurrences should
        // span ~40 years, well within the lookahead.
        let parsed = parse_rrule(
            "DTSTART:20240229T090000Z\nRRULE:FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29;COUNT=10",
            &date(2024, 2, 29).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        let dates = expand_dates(&parsed, 10).unwrap();
        assert_eq!(dates.len(), 10);
        // 2024 is a leap year; 2028 is next.
        assert!(dates[1].starts_with("2028-02-29T09:00:00"));
    }

    #[test]
    fn expand_monthly_start_day_31_count_1000() {
        let parsed = parse_rrule(
            "DTSTART:20260131T090000Z\nRRULE:FREQ=MONTHLY;COUNT=1000",
            &date(2026, 1, 31).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 1000).unwrap().len(), 1000);
    }

    #[test]
    fn expand_daily_bymonthday_31_count_1000() {
        let parsed = parse_rrule(
            "DTSTART:20260131T090000Z\nRRULE:FREQ=DAILY;BYMONTHDAY=31;COUNT=1000",
            &date(2026, 1, 31).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 1000).unwrap().len(), 1000);
    }

    #[test]
    fn expand_yearly_feb29_count_1000() {
        let parsed = parse_rrule(
            "DTSTART:20240229T090000Z\nRRULE:FREQ=YEARLY;COUNT=1000",
            &date(2024, 2, 29).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 1000).unwrap().len(), 1000);
    }

    #[test]
    fn expand_daily_bymonth_bymonthday_feb29_count_1000() {
        let parsed = parse_rrule(
            "DTSTART:20240229T090000Z\nRRULE:FREQ=DAILY;BYMONTH=2;BYMONTHDAY=29;COUNT=1000",
            &date(2024, 2, 29).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 1000).unwrap().len(), 1000);
    }

    #[test]
    fn expand_monthly_start_day_30_count_1000() {
        let parsed = parse_rrule(
            "DTSTART:20260130T090000Z\nRRULE:FREQ=MONTHLY;COUNT=1000",
            &date(2026, 1, 30).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 1000).unwrap().len(), 1000);
    }

    #[test]
    fn expand_daily_bymonthday_30_count_1000() {
        let parsed = parse_rrule(
            "DTSTART:20260130T090000Z\nRRULE:FREQ=DAILY;BYMONTHDAY=30;COUNT=1000",
            &date(2026, 1, 30).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 1000).unwrap().len(), 1000);
    }

    #[test]
    fn expand_monthly_byday_second_monday_count_100() {
        let parsed = parse_rrule(
            "DTSTART:20260112T090000Z\nRRULE:FREQ=MONTHLY;BYDAY=2MO;COUNT=100",
            &date(2026, 1, 12).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 100).unwrap().len(), 100);
    }

    #[test]
    fn expand_daily_byday_and_bymonthday_composite_count_100() {
        // Only Mondays that are also the 31st day of a month.
        let parsed = parse_rrule(
            "DTSTART:20260331T090000Z\nRRULE:FREQ=DAILY;BYDAY=MO;BYMONTHDAY=31;COUNT=100",
            &date(2026, 3, 31).at(9, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        assert_eq!(expand_dates(&parsed, 100).unwrap().len(), 100);
    }

    #[test]
    fn expand_monthly_negative_month_day_count_10() {
        // BYMONTHDAY=-1 means the last day of each month.
        let parsed = parse_rrule(
            "DTSTART:20260731T230000Z\nRRULE:FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=10",
            &date(2026, 7, 31).at(23, 0, 0, 0).in_tz("UTC").unwrap(),
        )
        .unwrap();
        let dates = expand_dates(&parsed, 10).unwrap();
        assert_eq!(dates.len(), 10);
        assert!(dates[0].starts_with("2026-07-31T23:00:00"));
        assert!(dates[1].starts_with("2026-08-31T23:00:00"));
        assert!(dates[2].starts_with("2026-09-30T23:00:00"));
    }
}
