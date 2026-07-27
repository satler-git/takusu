//! RFC 5545 RRULE parser (subset).
//!
//! Supports `DTSTART`, `RRULE`, and `EXDATE` properties. The `RRULE` value
//! may be prefixed with `RRULE:` and may be accompanied by a `DTSTART:` line.
//!
//! Supported RRULE parts: `FREQ`, `INTERVAL`, `COUNT`, `UNTIL`, `BYDAY`,
//! `BYMONTH`, `BYMONTHDAY`. All other parts are rejected.

use jiff::ToSpan;
use jiff::Zoned;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::{Error, Frequency, NWeekday, RecurrenceRule, TimeOfDay, Weekday};

/// A parsed RRULE with its anchor `DTSTART` and optional `UNTIL`.
#[derive(Debug, Clone)]
pub struct ParsedRule {
    pub rule: RecurrenceRule,
    pub start: Zoned,
    pub until: Option<Until>,
}

/// Inclusive end boundary carried by `UNTIL`.
#[derive(Debug, Clone)]
pub enum Until {
    /// Inclusive date (all times on this day are included).
    Date(Date),
    /// Inclusive instant.
    DateTime(Zoned),
}

/// Parse an RFC 5545 recurrence rule.
///
/// `default_start` is used when the input does not contain a `DTSTART`
/// property. It should be a zoned datetime in the caller's default timezone
/// (e.g. the start of today in the server timezone).
pub fn parse_rrule(input: &str, default_start: &Zoned) -> Result<ParsedRule, Error> {
    let lines = unfold_lines(input);
    let mut dtstart: Option<Zoned> = None;

    // First pass: resolve DTSTART before EXDATE so that EXDATE values
    // without an explicit TZID are interpreted in the recurrence timezone.
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value, params)) = split_property(line)
            && name.eq_ignore_ascii_case("DTSTART")
        {
            dtstart = Some(parse_zoned(value, &params, default_start.time_zone())?);
        }
    }

    let tz = dtstart
        .as_ref()
        .map_or(default_start.time_zone(), |z| z.time_zone());
    let mut rrule_value: Option<String> = None;
    let mut exdates: Vec<Date> = Vec::new();

    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value, params)) = split_property(&line) {
            match name.to_uppercase().as_str() {
                "DTSTART" => {
                    // Already parsed in the first pass.
                }
                "RRULE" => {
                    if rrule_value.is_some() {
                        return Err(Error::InvalidRule(
                            "multiple RRULE properties are not supported".into(),
                        ));
                    }
                    rrule_value = Some(value.to_string());
                }
                "EXDATE" => {
                    for token in value.split(',') {
                        exdates.push(parse_exdate(token, &params, tz)?);
                    }
                }
                _ => {}
            }
        } else if rrule_value.is_none() {
            // Bare RRULE value without a property name.
            rrule_value = Some(line);
        }
    }

    let start = dtstart.unwrap_or_else(|| default_start.clone());
    let rrule_str = rrule_value.ok_or_else(|| Error::InvalidRule("missing RRULE".into()))?;
    let (rule, until) = parse_rrule_value(&rrule_str, exdates, start.time_zone())?;

    Ok(ParsedRule { rule, start, until })
}

/// RFC 5545 line folding: lines beginning with a space or tab continue the
/// previous line.
fn unfold_lines(input: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in input.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = lines.last_mut() {
                last.push_str(&line[1..]);
            }
        } else {
            lines.push(line.to_string());
        }
    }
    lines
}

#[derive(Debug, Default)]
struct Params<'a> {
    value_type: Option<&'a str>,
    tzid: Option<&'a str>,
}

/// Split a property line into `(name, value, params)`. Returns `None` if the
/// line does not contain a `:`.
fn split_property(line: &str) -> Option<(&str, &str, Params<'_>)> {
    let (name_with_params, value) = line.split_once(':')?;
    let (name, params_str) = name_with_params
        .split_once(';')
        .unwrap_or((name_with_params, ""));

    let mut params = Params::default();
    if !params_str.is_empty() {
        for param in params_str.split(';') {
            let (k, v) = param.split_once('=')?;
            match k.to_uppercase().as_str() {
                "VALUE" => params.value_type = Some(v),
                "TZID" => params.tzid = Some(v),
                _ => {}
            }
        }
    }

    Some((name, value, params))
}

fn parse_zoned(value: &str, params: &Params, default_tz: &TimeZone) -> Result<Zoned, Error> {
    let (tz, value) = if let Some(v) = value.strip_suffix('Z') {
        (TimeZone::UTC, v)
    } else if let Some(tzid) = params.tzid {
        (
            TimeZone::get(tzid).map_err(|e| Error::InvalidRule(e.to_string()))?,
            value,
        )
    } else {
        (default_tz.clone(), value)
    };

    let is_date = params
        .value_type
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
        || value.len() == 8;

    if is_date {
        let date = parse_date_only(value)?;
        tz.to_zoned(date.at(0, 0, 0, 0))
            .map_err(|e| Error::InvalidRule(e.to_string()))
    } else {
        let (date, hour, minute, _second) = parse_ymd_hms(value)?;
        let tod = TimeOfDay::new(hour, minute).ok_or(Error::InvalidTimeOfDay { hour, minute })?;
        tz.to_zoned(date.at(tod.hour() as i8, tod.minute() as i8, 0, 0))
            .map_err(|e| Error::InvalidRule(e.to_string()))
    }
}

fn parse_exdate(value: &str, params: &Params, default_tz: &TimeZone) -> Result<Date, Error> {
    // EXDATE values are typically date-only or date-time. For our purpose we
    // only need the civil date in the recurrence timezone.
    if params
        .value_type
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
        || value.len() == 8
    {
        return parse_date_only(value);
    }

    let z = parse_zoned(value, params, default_tz)?;
    Ok(z.date())
}

fn parse_until(value: &str, tz: &TimeZone) -> Result<Until, Error> {
    // UNTIL is a bare value (no parameters) inside an RRULE string. The value
    // type mirrors DTSTART: date-only or date-time, possibly with a trailing Z.
    if value.len() == 8 {
        let date = parse_date_only(value)?;
        return Ok(Until::Date(date));
    }

    let (tz_for_value, v) = if let Some(stripped) = value.strip_suffix('Z') {
        (TimeZone::UTC, stripped)
    } else {
        (tz.clone(), value)
    };
    let (date, hour, minute, _second) = parse_ymd_hms(v)?;
    let tod = TimeOfDay::new(hour, minute).ok_or(Error::InvalidTimeOfDay { hour, minute })?;
    let z = tz_for_value
        .to_zoned(date.at(tod.hour() as i8, tod.minute() as i8, 0, 0))
        .map_err(|e| Error::InvalidRule(e.to_string()))?;

    // Convert the UNTIL instant to the recurrence timezone so comparisons are
    // consistent with the DTSTART timezone.
    Ok(Until::DateTime(z.with_time_zone(tz.clone())))
}

fn parse_date_only(value: &str) -> Result<Date, Error> {
    if value.len() != 8 {
        return Err(Error::InvalidRule(format!("invalid date: {value}")));
    }
    let year = parse_i16(&value[0..4])?;
    let month = parse_i8(&value[4..6])?;
    let day = parse_i8(&value[6..8])?;
    Date::new(year, month, day).map_err(|e| Error::InvalidRule(e.to_string()))
}

fn parse_ymd_hms(value: &str) -> Result<(Date, u8, u8, u8), Error> {
    if value.len() != 15 || value.as_bytes().get(8) != Some(&b'T') {
        return Err(Error::InvalidRule(format!("invalid date-time: {value}")));
    }
    let year = parse_i16(&value[0..4])?;
    let month = parse_i8(&value[4..6])?;
    let day = parse_i8(&value[6..8])?;
    let hour = parse_u8(&value[9..11])?;
    let minute = parse_u8(&value[11..13])?;
    let second = parse_u8(&value[13..15])?;

    if hour > 24 || minute > 59 || second > 59 {
        return Err(Error::InvalidRule(format!("invalid time: {value}")));
    }

    let mut date = Date::new(year, month, day).map_err(|e| Error::InvalidRule(e.to_string()))?;

    // Round seconds to the nearest minute, then normalise the RFC 5545
    // special case of 24:00:00 to the following midnight.
    let mut total_minutes = (hour as u16) * 60 + (minute as u16);
    if second >= 30 {
        total_minutes += 1;
    }
    let mut hour = total_minutes / 60;
    let minute = total_minutes % 60;
    if hour == 24 && minute == 0 {
        date = date
            .checked_add(1.day())
            .map_err(|e| Error::InvalidRule(e.to_string()))?;
        hour = 0;
    } else if hour > 23 {
        return Err(Error::InvalidRule(format!("invalid time: {value}")));
    }

    Ok((date, hour as u8, minute as u8, 0))
}

fn parse_rrule_value(
    value: &str,
    exdates: Vec<Date>,
    tz: &TimeZone,
) -> Result<(RecurrenceRule, Option<Until>), Error> {
    let mut freq: Option<Frequency> = None;
    let mut interval = 1u32;
    let mut count: Option<u32> = None;
    let mut until: Option<Until> = None;
    let mut by_day: Vec<NWeekday> = Vec::new();
    let mut by_month: Vec<i8> = Vec::new();
    let mut by_month_day: Vec<i8> = Vec::new();

    let mut seen = std::collections::HashSet::new();

    for token in value.split(';') {
        if token.is_empty() {
            continue;
        }
        let (k, v) = token
            .split_once('=')
            .ok_or_else(|| Error::InvalidRule(format!("missing '=': {token}")))?;
        let key = k.to_uppercase();
        if !seen.insert(key.clone()) {
            return Err(Error::InvalidRule(format!("duplicate key: {k}")));
        }
        match key.as_str() {
            "FREQ" => freq = Some(parse_freq(v)?),
            "INTERVAL" => interval = parse_interval(v)?,
            "COUNT" => count = Some(parse_count(v)?),
            "UNTIL" => until = Some(parse_until(v, tz)?),
            "BYDAY" => by_day = parse_by_day(v)?,
            "BYMONTH" => by_month = parse_by_month(v)?,
            "BYMONTHDAY" => by_month_day = parse_by_month_day(v)?,
            _ => return Err(Error::InvalidRule(format!("unsupported RRULE part: {k}"))),
        }
    }

    let freq = freq.ok_or_else(|| Error::InvalidRule("missing FREQ".into()))?;

    Ok((
        RecurrenceRule {
            freq,
            interval,
            by_day,
            by_month,
            by_month_day,
            count,
            exdates,
        },
        until,
    ))
}

fn parse_freq(value: &str) -> Result<Frequency, Error> {
    match value.to_uppercase().as_str() {
        "DAILY" => Ok(Frequency::Daily),
        "WEEKLY" => Ok(Frequency::Weekly),
        "MONTHLY" => Ok(Frequency::Monthly),
        "YEARLY" => Ok(Frequency::Yearly),
        _ => Err(Error::InvalidRule(format!("invalid FREQ: {value}"))),
    }
}

fn parse_interval(value: &str) -> Result<u32, Error> {
    let n = parse_u32(value)?;
    if n == 0 {
        return Err(Error::InvalidRule("INTERVAL must be >= 1".into()));
    }
    Ok(n)
}

fn parse_count(value: &str) -> Result<u32, Error> {
    let n = parse_u32(value)?;
    if n == 0 {
        return Err(Error::InvalidRule("COUNT must be >= 1".into()));
    }
    Ok(n)
}

fn parse_by_day(value: &str) -> Result<Vec<NWeekday>, Error> {
    let mut days = Vec::new();
    for token in value.split(',') {
        days.push(parse_weekday_token(token)?);
    }
    Ok(days)
}

fn parse_weekday_token(token: &str) -> Result<NWeekday, Error> {
    let token = token.trim();
    let (n_str, wd_str) = split_leading_digits(token);
    let n = if n_str.is_empty() {
        None
    } else {
        Some(parse_i8(n_str)?)
    };
    let weekday = parse_weekday(wd_str)?;
    Ok(NWeekday { n, weekday })
}

fn split_leading_digits(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut i = 0;
    if !bytes.is_empty() && (bytes[0] == b'+' || bytes[0] == b'-') {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    (&s[..i], &s[i..])
}

fn parse_weekday(value: &str) -> Result<Weekday, Error> {
    match value.to_uppercase().as_str() {
        "MO" => Ok(Weekday::Mon),
        "TU" => Ok(Weekday::Tue),
        "WE" => Ok(Weekday::Wed),
        "TH" => Ok(Weekday::Thu),
        "FR" => Ok(Weekday::Fri),
        "SA" => Ok(Weekday::Sat),
        "SU" => Ok(Weekday::Sun),
        _ => Err(Error::InvalidRule(format!("invalid weekday: {value}"))),
    }
}

fn parse_by_month(value: &str) -> Result<Vec<i8>, Error> {
    let mut months = Vec::new();
    for token in value.split(',') {
        let m = parse_i8(token)?;
        if !(1..=12).contains(&m) {
            return Err(Error::InvalidRule(format!(
                "invalid BYMONTH value: {token}"
            )));
        }
        months.push(m);
    }
    Ok(months)
}

fn parse_by_month_day(value: &str) -> Result<Vec<i8>, Error> {
    let mut days = Vec::new();
    for token in value.split(',') {
        let d = parse_i8(token)?;
        if d == 0 || d.unsigned_abs() > 31 {
            return Err(Error::InvalidRule(format!(
                "invalid BYMONTHDAY value: {token}"
            )));
        }
        days.push(d);
    }
    Ok(days)
}

fn parse_i8(s: &str) -> Result<i8, Error> {
    s.parse::<i8>()
        .map_err(|_| Error::InvalidRule(format!("invalid integer: {s}")))
}

fn parse_i16(s: &str) -> Result<i16, Error> {
    s.parse::<i16>()
        .map_err(|_| Error::InvalidRule(format!("invalid integer: {s}")))
}

fn parse_u8(s: &str) -> Result<u8, Error> {
    s.parse::<u8>()
        .map_err(|_| Error::InvalidRule(format!("invalid integer: {s}")))
}

fn parse_u32(s: &str) -> Result<u32, Error> {
    s.parse::<u32>()
        .map_err(|_| Error::InvalidRule(format!("invalid integer: {s}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn default_start() -> Zoned {
        date(2026, 7, 27).at(9, 0, 0, 0).in_tz("UTC").unwrap()
    }

    #[test]
    fn parse_daily_count() {
        let parsed = parse_rrule("FREQ=DAILY;COUNT=4", &default_start()).unwrap();
        assert_eq!(
            parsed.rule,
            RecurrenceRule {
                freq: Frequency::Daily,
                interval: 1,
                by_day: vec![],
                by_month: vec![],
                by_month_day: vec![],
                count: Some(4),
                exdates: vec![],
            }
        );
        assert_eq!(parsed.start.date(), date(2026, 7, 27));
        assert_eq!(parsed.start.hour(), 9);
        assert_eq!(parsed.start.minute(), 0);
        assert!(parsed.until.is_none());
    }

    #[test]
    fn parse_with_dtstart_and_byday() {
        let input = "DTSTART:20260727T090000Z\nRRULE:FREQ=DAILY;COUNT=4;BYDAY=MO,TU,WE,TH,FR";
        let parsed = parse_rrule(input, &default_start()).unwrap();
        assert_eq!(parsed.rule.freq, Frequency::Daily);
        assert_eq!(parsed.rule.count, Some(4));
        assert_eq!(
            parsed.rule.by_day,
            vec![
                NWeekday::every(Weekday::Mon),
                NWeekday::every(Weekday::Tue),
                NWeekday::every(Weekday::Wed),
                NWeekday::every(Weekday::Thu),
                NWeekday::every(Weekday::Fri),
            ]
        );
        assert_eq!(parsed.start.date(), date(2026, 7, 27));
    }

    #[test]
    fn parse_weekly_interval_nth_weekday() {
        let parsed =
            parse_rrule("FREQ=WEEKLY;INTERVAL=2;BYDAY=2MO,-1FR", &default_start()).unwrap();
        assert_eq!(parsed.rule.freq, Frequency::Weekly);
        assert_eq!(parsed.rule.interval, 2);
        assert_eq!(
            parsed.rule.by_day,
            vec![
                NWeekday::nth(2, Weekday::Mon),
                NWeekday::nth(-1, Weekday::Fri),
            ]
        );
    }

    #[test]
    fn parse_by_month_and_by_month_day() {
        let parsed =
            parse_rrule("FREQ=YEARLY;BYMONTH=1,3;BYMONTHDAY=1,-1", &default_start()).unwrap();
        assert_eq!(parsed.rule.freq, Frequency::Yearly);
        assert_eq!(parsed.rule.by_month, vec![1, 3]);
        assert_eq!(parsed.rule.by_month_day, vec![1, -1]);
    }

    #[test]
    fn parse_exdate_and_until() {
        let input =
            "DTSTART:20260727T090000Z\nRRULE:FREQ=DAILY;UNTIL=20260730T090000Z\nEXDATE:20260728";
        let parsed = parse_rrule(input, &default_start()).unwrap();
        assert_eq!(parsed.rule.exdates, vec![date(2026, 7, 28)]);
        assert!(matches!(parsed.until, Some(Until::DateTime(_))));
    }

    #[test]
    fn rejects_unsupported_part() {
        assert!(parse_rrule("FREQ=DAILY;BYSETPOS=1", &default_start()).is_err());
    }

    #[test]
    fn rejects_invalid_count() {
        assert!(parse_rrule("FREQ=DAILY;COUNT=0", &default_start()).is_err());
    }

    #[test]
    fn exdate_before_dtstart_uses_dtstart_tz() {
        // EXDATE appears before DTSTART in the input. It should be interpreted
        // in the DTSTART timezone (UTC here), not the default_start timezone.
        let input = "EXDATE:20260728T000000Z\nDTSTART:20260727T090000Z\nRRULE:FREQ=DAILY;COUNT=3";
        let parsed = parse_rrule(input, &default_start()).unwrap();
        assert_eq!(parsed.start.time_zone(), &jiff::tz::TimeZone::UTC);
        assert_eq!(parsed.rule.exdates, vec![date(2026, 7, 28)]);
    }

    #[test]
    fn bymonthday_min_i8_is_rejected_without_panic() {
        assert!(parse_rrule("FREQ=MONTHLY;BYMONTHDAY=-128", &default_start()).is_err());
    }

    #[test]
    fn parses_240000_as_following_midnight() {
        let input = "DTSTART:20260727T240000Z\nRRULE:FREQ=DAILY;COUNT=1";
        let parsed = parse_rrule(input, &default_start()).unwrap();
        assert_eq!(parsed.start.date(), date(2026, 7, 28));
        assert_eq!(parsed.start.hour(), 0);
        assert_eq!(parsed.start.minute(), 0);
    }

    #[test]
    fn rounds_seconds_to_nearest_minute() {
        let input = "DTSTART:20260727T092900Z\nRRULE:FREQ=DAILY;COUNT=1";
        let parsed = parse_rrule(input, &default_start()).unwrap();
        // 29 minutes snaps down to 25 with no seconds to round.
        assert_eq!(parsed.start.minute(), 25);

        let input = "DTSTART:20260727T092930Z\nRRULE:FREQ=DAILY;COUNT=1";
        let parsed = parse_rrule(input, &default_start()).unwrap();
        // 29 minutes + 30 seconds rounds up to 30 minutes, which snaps to 30.
        assert_eq!(parsed.start.minute(), 30);
    }
}
