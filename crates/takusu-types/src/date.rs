use jiff::{Timestamp, civil::Date, tz::TimeZone};
use std::str::FromStr;
use web_time::{SystemTime, UNIX_EPOCH};

/// Return the current UTC timestamp as an RFC 3339 string with whole-second
/// precision (e.g. `2026-07-23T09:00:00Z`).
pub fn now_rfc3339() -> String {
    now_timestamp()
        .expect("system clock error")
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub fn now_timestamp() -> Result<Timestamp, String> {
    let nanos: i128 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .map_err(|_| "system clock error".to_string())?
        .try_into()
        .map_err(|_| "system clock out of range".to_string())?;
    jiff::Timestamp::from_nanosecond(nanos).map_err(|e| format!("invalid timestamp: {e}"))
}

fn parse_rfc3339_or_legacy(s: &str) -> Option<Timestamp> {
    if let Ok(ts) = Timestamp::from_str(s) {
        return Some(ts);
    }
    // SQLite `datetime('now')` and other naive UTC wall-clock formats.
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(dt) = jiff::civil::DateTime::strptime(fmt, s)
            && let Ok(zdt) = dt.to_zoned(TimeZone::UTC)
        {
            return Some(zdt.timestamp());
        }
    }
    None
}

/// Return the later of two RFC 3339 timestamp strings.
/// Falls back to `a` if either timestamp cannot be parsed.
pub fn later_timestamp<'a>(a: &'a str, b: &'a str) -> &'a str {
    match (parse_rfc3339_or_legacy(a), parse_rfc3339_or_legacy(b)) {
        (Some(ta), Some(tb)) => {
            if ta >= tb {
                a
            } else {
                b
            }
        }
        (Some(_), _) => a,
        (_, Some(_)) => b,
        _ => a,
    }
}

/// Minutes between two RFC 3339 timestamps.
/// Falls back to parsing legacy SQLite `datetime('now')` output.
/// Returns at least 1 to avoid degenerate speed observations.
pub fn minutes_between(start: &str, end: &str) -> i64 {
    match (parse_rfc3339_or_legacy(start), parse_rfc3339_or_legacy(end)) {
        (Some(s), Some(e)) => ((e.as_second() - s.as_second()) / 60).max(1),
        _ => 1,
    }
}

fn try_build_datetime(
    year: i16,
    month: i8,
    day: i8,
    time_part: &str,
    end_of_day: bool,
) -> Result<jiff::civil::DateTime, String> {
    let date =
        jiff::civil::Date::new(year, month, day).map_err(|e| format!("invalid date: {e}"))?;
    let dt = if time_part.is_empty() {
        if end_of_day {
            date.at(23, 59, 59, 0)
        } else {
            date.at(0, 0, 0, 0)
        }
    } else {
        let t: jiff::civil::Time = time_part
            .parse()
            .map_err(|e| format!("invalid time '{time_part}': {e}"))?;
        date.at(t.hour(), t.minute(), t.second(), t.subsec_nanosecond())
    };
    Ok(dt)
}

fn dt_to_timestamp(dt: jiff::civil::DateTime, tz: &TimeZone) -> Result<Timestamp, String> {
    let zdt = dt
        .to_zoned(tz.clone())
        .map_err(|e| format!("could not convert to timezone: {e}"))?;
    Ok(zdt.timestamp())
}

pub fn parse_datetime(s: &str) -> Result<String, String> {
    parse_datetime_to_timestamp(s, &TimeZone::UTC).map(|ts| ts.to_string())
}

pub fn parse_datetime_tz(s: &str, tz: &TimeZone) -> Result<String, String> {
    parse_datetime_to_timestamp(s, tz).map(|ts| ts.to_string())
}

pub fn parse_datetime_to_timestamp(s: &str, tz: &TimeZone) -> Result<Timestamp, String> {
    let now = now_timestamp()?;
    let today = now.to_zoned(tz.clone()).date();
    parse_datetime_to_timestamp_with(s, tz, today, true, now)
}

pub fn parse_datetime_to_timestamp_with(
    s: &str,
    tz: &TimeZone,
    today: Date,
    end_of_day: bool,
    now: Timestamp,
) -> Result<Timestamp, String> {
    let s = s.trim();

    if s.eq_ignore_ascii_case("now") {
        return Ok(now);
    }

    // Full ISO 8601 timestamp
    if let Ok(ts) = Timestamp::from_str(s) {
        return Ok(ts);
    }

    // Normalize "2025-06-05 14:00" → "2025-06-05T14:00" so the civil
    // datetime fallback below can parse it. Datetimes without an explicit
    // timezone are interpreted in the configured tz (see the fallback).
    let normalized = if s.contains(' ') && !s.contains('T') {
        s.replace(' ', "T")
    } else {
        s.to_string()
    };

    // "06-15" → this year June 15th (in configured tz)
    // "-06" → this year this month day 6 (in configured tz)
    // "06-15T14:00" → this year June 15 14:00 (in configured tz)
    // "-06T14:00" → this year this month day 6 14:00 (in configured tz)
    if s.starts_with('-') && !s.starts_with("--") {
        let rest = &s[1..];
        let (day_str, time_part) = rest
            .split_once('T')
            .or_else(|| rest.split_once(' '))
            .unwrap_or((rest, ""));
        let day: i8 = day_str
            .parse()
            .map_err(|_| format!("invalid day: {day_str}"))?;
        let dt = try_build_datetime(today.year(), today.month(), day, time_part, end_of_day)?;
        return dt_to_timestamp(dt, tz);
    }

    if let Some(idx) = s.find('-') {
        let prefix = &s[..idx];
        let rest = &s[idx + 1..];
        if prefix.len() == 2 && prefix.chars().all(|c| c.is_ascii_digit()) {
            let month: i8 = prefix
                .parse()
                .map_err(|_| format!("invalid month: {prefix}"))?;
            let (day_str, time_part) = rest
                .split_once('T')
                .or_else(|| rest.split_once(' '))
                .unwrap_or((rest, ""));
            let (day_str, time_part) = if day_str.contains('-') {
                return Err(format!("ambiguous date format: {s}"));
            } else {
                (day_str, time_part)
            };
            let day: i8 = day_str
                .parse()
                .map_err(|_| format!("invalid day: {day_str}"))?;
            let dt = try_build_datetime(today.year(), month, day, time_part, end_of_day)?;
            return dt_to_timestamp(dt, tz);
        }
    }

    // Full datetime without timezone: "2025-06-05T14:00", interpreted in
    // the configured tz. Checked before the date-only branch because
    // `civil::Date::from_str` accepts datetime strings and truncates the
    // time part.
    if normalized.contains('T') {
        if let Ok(dt) = jiff::civil::DateTime::from_str(&normalized) {
            let zdt = dt
                .to_zoned(tz.clone())
                .map_err(|e| format!("could not convert datetime: {e}"))?;
            return Ok(zdt.timestamp());
        }
    } else if let Ok(d) = jiff::civil::Date::from_str(s) {
        let dt = if end_of_day {
            d.at(23, 59, 59, 0)
        } else {
            d.at(0, 0, 0, 0)
        };
        let zdt = dt
            .to_zoned(tz.clone())
            .map_err(|e| format!("could not convert date: {e}"))?;
        return Ok(zdt.timestamp());
    }

    Err(format!(
        "could not parse datetime: {s} (e.g. 2025-06-05, 06-15, -06, 06-15T14:00)"
    ))
}

/// Parse a flexible date expression into a timestamp.
///
/// Supported inputs:
/// - `"now"` -> current time.
/// - `"today"` -> start or end of today in the configured timezone.
/// - `"7d"` or `"+7d"` or `"-7d"` -> start or end of the date N days from today.
/// - `"2026-07-20"` -> start or end of that date in the configured timezone.
/// - Full RFC 3339 / ISO 8601 timestamps or naive datetimes are passed through.
///
/// `end_of_day` controls whether date-only expressions resolve to the start
/// (`00:00:00`) or end (`23:59:59`) of the day. It is ignored for absolute
/// timestamps and `now`.
pub fn parse_date_expression(
    s: &str,
    tz: &TimeZone,
    end_of_day: bool,
) -> Result<Timestamp, String> {
    let now = now_timestamp()?;
    let today = now.to_zoned(tz.clone()).date();
    parse_date_expression_with(s, tz, today, end_of_day, now)
}

/// Context-aware version of [`parse_date_expression`].
/// `today` and `now` are supplied by the caller so search filters and tests can
/// avoid mixing system time with context-dependent date evaluation.
pub fn parse_date_expression_with(
    s: &str,
    tz: &TimeZone,
    today: Date,
    end_of_day: bool,
    now: Timestamp,
) -> Result<Timestamp, String> {
    let s = s.trim();

    if s.eq_ignore_ascii_case("now") {
        return Ok(now);
    }

    if s.eq_ignore_ascii_case("today") {
        let dt = if end_of_day {
            today.at(23, 59, 59, 0)
        } else {
            today.at(0, 0, 0, 0)
        };
        return dt_to_timestamp(dt, tz);
    }

    if s.eq_ignore_ascii_case("tomorrow") {
        let date = today
            .checked_add(jiff::Span::new().days(1))
            .map_err(|e| format!("day overflow: {e}"))?;
        let dt = if end_of_day {
            date.at(23, 59, 59, 0)
        } else {
            date.at(0, 0, 0, 0)
        };
        return dt_to_timestamp(dt, tz);
    }

    if s.eq_ignore_ascii_case("yesterday") {
        let date = today
            .checked_sub(jiff::Span::new().days(1))
            .map_err(|e| format!("day overflow: {e}"))?;
        let dt = if end_of_day {
            date.at(23, 59, 59, 0)
        } else {
            date.at(0, 0, 0, 0)
        };
        return dt_to_timestamp(dt, tz);
    }

    // Relative days: "7d", "+7d", "-7d".
    if let Some(days_str) = s.strip_suffix('d').or_else(|| s.strip_suffix('D'))
        && let Ok(days) = days_str.trim().parse::<i64>()
    {
        let date = today
            .checked_add(jiff::Span::new().days(days))
            .map_err(|e| format!("day overflow: {e}"))?;
        let dt = if end_of_day {
            date.at(23, 59, 59, 0)
        } else {
            date.at(0, 0, 0, 0)
        };
        return dt_to_timestamp(dt, tz);
    }

    // "08-09" or "8-9" -> this year 08-09
    if let Some(idx) = s.find('-') {
        let month_str = &s[..idx];
        let rest = &s[idx + 1..];
        if !month_str.is_empty()
            && month_str.chars().all(|c| c.is_ascii_digit())
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit())
            && !month_str.contains('-')
            && !rest.contains('-')
        {
            let month: i8 = month_str
                .parse()
                .map_err(|_| format!("invalid month: {month_str}"))?;
            let day: i8 = rest.parse().map_err(|_| format!("invalid day: {rest}"))?;
            let date =
                Date::new(today.year(), month, day).map_err(|e| format!("invalid date: {e}"))?;
            let dt = if end_of_day {
                date.at(23, 59, 59, 0)
            } else {
                date.at(0, 0, 0, 0)
            };
            return dt_to_timestamp(dt, tz);
        }
    }

    // "05" -> this month day 5
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
        let day: i8 = s.parse().map_err(|_| format!("invalid day: {s}"))?;
        let date = Date::new(today.year(), today.month(), day)
            .map_err(|e| format!("invalid date: {e}"))?;
        let dt = if end_of_day {
            date.at(23, 59, 59, 0)
        } else {
            date.at(0, 0, 0, 0)
        };
        return dt_to_timestamp(dt, tz);
    }

    // Let the unified datetime parser handle full timestamps and remaining
    // civil date(time) forms such as "-06", "06-15T14:00" and "2026-07-20".
    parse_datetime_to_timestamp_with(s, tz, today, end_of_day, now)
}
