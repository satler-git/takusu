pub mod abandonability;
pub mod date;
pub mod duration_seconds;
pub mod enum_label;
pub mod id_ref;
pub mod json_string;
pub mod jwt;
pub mod planner;
pub mod quantity;
pub mod similarity;
#[cfg(feature = "sqlx")]
pub mod sqlx_impl;
pub mod time_types;

pub use abandonability::Abandonability;
pub use date::{
    later_timestamp, minutes_between, now_rfc3339, now_timestamp, parse_date_expression,
    parse_datetime, parse_datetime_to_timestamp, parse_datetime_tz,
};
pub use enum_label::enum_serde::option as enum_option_serde;
pub use enum_label::{
    CommentAuthor, EnumLabel, MemoryKind, MemorySource, ScheduleMode, SimilarityMetric, Solver,
    SubjectType, TaskStatus, TaskStatusFilter, TokenScope, UnknownLabel, WindowMode, enum_serde,
};
pub use id_ref::{HabitRef, IdRefError, TaskRef};
pub use json_string::{DependencyList, JsonString};
pub use planner::{NormalDist, ParallelMode, Plan, Point, Task, TaskPlacement, TimeWindow};
pub use quantity::{Quantity, QuantityError};
pub use similarity::Similarity;
pub use time_types::{Date, TimeOfDay, TimeParseError, Timestamp, minutes_between_ts};

/// 1 スロットあたりの分数。
pub const SLOT_MINUTES: i64 = 5;

/// 分単位の長さ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Minutes(pub i64);

/// 5 分スロット数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slots(pub i64);

impl Minutes {
    /// 整数除算（0 方向への切り捨て）。
    pub const fn to_slots(self) -> Slots {
        Slots(self.0 / SLOT_MINUTES)
    }

    /// 切り上げ。
    pub const fn to_slots_ceil(self) -> Slots {
        let div = self.0.div_euclid(SLOT_MINUTES);
        let rem = self.0.rem_euclid(SLOT_MINUTES);
        Slots(div + if rem == 0 { 0 } else { 1 })
    }
}

impl Slots {
    pub const fn to_minutes(self) -> Minutes {
        Minutes(self.0 * SLOT_MINUTES)
    }
}

pub use jwt::{
    Claims as TokenClaims, DEFAULT_AUD, DEFAULT_ISS, JwtError, SCOPE_READ_WRITE, SCOPE_ROOT,
};

/// Parse a fixed-offset timezone string such as `+09:00`, `+0900`, `+09`,
/// or `-05:30:15`. Returns `None` for invalid formats or offsets outside
/// the real-world UTC±14 range.
pub fn parse_fixed_offset_timezone(s: &str) -> Option<jiff::tz::TimeZone> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    let (hours, minutes, seconds) = if rest.contains(':') {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.is_empty() || parts.len() > 3 {
            return None;
        }
        let h: i32 = parts[0].parse().ok()?;
        let m: i32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let sec: i32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        (h, m, sec)
    } else {
        match rest.len() {
            0 => return None,
            1 | 2 => {
                let h: i32 = rest.parse().ok()?;
                (h, 0, 0)
            }
            4 => {
                let h: i32 = rest[..2].parse().ok()?;
                let m: i32 = rest[2..].parse().ok()?;
                (h, m, 0)
            }
            6 => {
                let h: i32 = rest[..2].parse().ok()?;
                let m: i32 = rest[2..4].parse().ok()?;
                let sec: i32 = rest[4..].parse().ok()?;
                (h, m, sec)
            }
            _ => return None,
        }
    };
    if !(0..=23).contains(&hours) || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
        return None;
    }
    let total_seconds_i64 = sign * (hours as i64 * 3600 + minutes as i64 * 60 + seconds as i64);
    // Real-world timezones are within UTC±14 (e.g. Line Islands, Baker/Howland).
    if total_seconds_i64.abs() > 14 * 3600 {
        return None;
    }
    let total_seconds = i32::try_from(total_seconds_i64).ok()?;
    let offset = jiff::tz::Offset::from_seconds(total_seconds).ok()?;
    Some(jiff::tz::TimeZone::fixed(offset))
}

/// Parse an IANA or fixed-offset timezone string. Returns a descriptive error
/// string on failure.
pub fn parse_timezone(tz: &str) -> Result<jiff::tz::TimeZone, String> {
    if let Ok(tz) = jiff::tz::TimeZone::get(tz) {
        return Ok(tz);
    }
    parse_fixed_offset_timezone(tz).ok_or_else(|| format!("invalid timezone: {tz}"))
}

pub const MIN_ESTIMATE_MINUTES: f64 = 5.0;
pub const MAX_ESTIMATE_MINUTES: f64 = 24.0 * 60.0;
/// Compute a delta-weighted `(avg_minutes, sigma_minutes)` estimate from a set
/// of progress observations and a target quantity.
///
/// Each observation is an `(active_minutes, delta_quantity)` pair; pairs with
/// non-positive values are ignored. An observation's projection to the full
/// quantity is weighted by how much it accomplished (`delta_quantity`), so
/// observations that did more work dominate the estimate (#1419). The weighted
/// mean is equivalent to the aggregate pace (`sum(active) / sum(delta)`)
/// projected to the full quantity before clamping.
///
/// Returns `None` when there is no usable `quantity_total` or no positive
/// observation. With a single usable observation the sigma is `0`.
pub fn weighted_estimate(
    observations: &[(i64, i64)],
    quantity_total: Option<i64>,
) -> Option<(i64, i64)> {
    let total = match quantity_total {
        Some(t) if t > 0 => t as f64,
        _ => return None,
    };
    let projections: Vec<(f64, f64)> = observations
        .iter()
        .filter(|(a, d)| *a > 0 && *d > 0)
        .map(|(a, d)| {
            let projection =
                ((*a as f64 / *d as f64) * total).clamp(MIN_ESTIMATE_MINUTES, MAX_ESTIMATE_MINUTES);
            (projection, *d as f64)
        })
        .collect();
    if projections.is_empty() {
        return None;
    }

    let weight_sum: f64 = projections.iter().map(|(_, w)| w).sum();
    let mean = projections.iter().map(|(x, w)| x * w).sum::<f64>() / weight_sum;
    let avg = mean
        .clamp(MIN_ESTIMATE_MINUTES, MAX_ESTIMATE_MINUTES)
        .round() as i64;

    if projections.len() < 2 {
        return Some((avg, 0));
    }

    let weighted_variance = projections
        .iter()
        .map(|(x, w)| w * (x - mean).powi(2))
        .sum::<f64>()
        / weight_sum;
    let sigma = weighted_variance
        .sqrt()
        .clamp(MIN_ESTIMATE_MINUTES, MAX_ESTIMATE_MINUTES)
        .round() as i64;
    Some((avg, sigma.max(1)))
}

/// Compute an updated `(avg_minutes, sigma_minutes)` estimate from a new
/// progress observation and a history of prior observations.
///
/// `events` is a slice of `(active_minutes, delta_quantity)` pairs. Pairs
/// with non-positive values are ignored. The estimate is the delta-weighted
/// aggregate of every observation (history plus the new one); see
/// [`weighted_estimate`]. The supplied `avg_minutes` / `sigma_minutes` are used
/// only as a fallback when no usable observation exists.
///
/// Returns the original estimate when there is no usable `quantity_total` or
/// no positive progress in this observation.
pub fn estimate_progress(
    avg_minutes: i64,
    sigma_minutes: i64,
    quantity_total: Option<i64>,
    active_minutes: i64,
    delta_quantity: i64,
    events: &[(i64, i64)],
) -> (i64, i64) {
    if delta_quantity <= 0 || active_minutes <= 0 {
        return (avg_minutes, sigma_minutes);
    }
    let mut observations: Vec<(i64, i64)> = events.to_vec();
    observations.push((active_minutes, delta_quantity));
    weighted_estimate(&observations, quantity_total).unwrap_or((avg_minutes, sigma_minutes))
}

/// Median of a sorted slice of `f64` values.
fn median_sorted(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Detect outlier indices in `samples` using the median absolute deviation
/// (MAD). Values whose distance from the median exceeds `3 * MAD` are
/// considered outliers. When the MAD is zero (many identical values), the
/// median itself is used as a scale to avoid missing a clear outlier.
pub fn detect_outlier_indices(samples: &[i64]) -> Vec<usize> {
    if samples.len() < 3 {
        return Vec::new();
    }
    let mut values: Vec<f64> = samples.iter().map(|&x| x as f64).collect();
    values.sort_by(|a, b| a.total_cmp(b));
    let median = median_sorted(&values);

    let mut deviations: Vec<f64> = values.iter().map(|v| (v - median).abs()).collect();
    deviations.sort_by(|a, b| a.total_cmp(b));
    let mad = median_sorted(&deviations);

    // If every value has the same deviation (typically because most values are
    // identical), fall back to the median as a scale so that a value several
    // times larger is still flagged.
    let scale = if mad > 0.0 { mad } else { median };
    let threshold = 3.0 * scale;

    samples
        .iter()
        .enumerate()
        .filter(|&(_, x)| (*x as f64 - median).abs() > threshold)
        .map(|(i, _)| i)
        .collect()
}

/// Estimate an `(avg_minutes, sigma_minutes)` pair from a collection of
/// observed durations in minutes, optionally excluding outliers detected by
/// `detect_outlier_indices`.
///
/// Returns `(0, 0)` for an empty slice. With a single sample the sigma is
/// `0`. Otherwise the sample standard deviation is computed and clamped to
/// the same `[MIN_ESTIMATE_MINUTES, MAX_ESTIMATE_MINUTES]` range as the
/// average. Sigma is therefore at least `MIN_ESTIMATE_MINUTES` (5 minutes)
/// when two or more samples exist.
///
/// Also returns the indices of any excluded outliers.
pub fn estimate_from_samples_with_outliers(
    samples: &[i64],
    exclude_outliers: bool,
) -> (i64, i64, Vec<usize>) {
    let excluded = if exclude_outliers {
        detect_outlier_indices(samples)
    } else {
        Vec::new()
    };
    let excluded_set: std::collections::HashSet<usize> = excluded.iter().copied().collect();
    let used: Vec<i64> = samples
        .iter()
        .enumerate()
        .filter(|(i, _)| !excluded_set.contains(i))
        .map(|(_, &x)| x)
        .collect();

    let (avg, sigma) = estimate_from_samples_internal(&used);
    (avg, sigma, excluded)
}

fn estimate_from_samples_internal(samples: &[i64]) -> (i64, i64) {
    if samples.is_empty() {
        return (0, 0);
    }
    if samples.len() == 1 {
        let avg = (samples[0] as f64)
            .clamp(MIN_ESTIMATE_MINUTES, MAX_ESTIMATE_MINUTES)
            .round() as i64;
        return (avg, 0);
    }

    // Use f64 accumulation to avoid i64 overflow with very large samples.
    let mean = samples.iter().map(|&x| x as f64).sum::<f64>() / samples.len() as f64;
    let variance = samples
        .iter()
        .map(|&x| {
            let diff = x as f64 - mean;
            diff * diff
        })
        .sum::<f64>()
        / (samples.len() - 1) as f64;
    let stddev = variance.sqrt();

    let avg = mean
        .clamp(MIN_ESTIMATE_MINUTES, MAX_ESTIMATE_MINUTES)
        .round() as i64;
    let sigma = stddev
        .clamp(MIN_ESTIMATE_MINUTES, MAX_ESTIMATE_MINUTES)
        .round() as i64;
    (avg, sigma)
}

/// Estimate an `(avg_minutes, sigma_minutes)` pair from a collection of
/// observed durations in minutes.
///
/// Equivalent to `estimate_from_samples_with_outliers(samples, false)`
/// without returning outlier indices.
pub fn estimate_from_samples(samples: &[i64]) -> (i64, i64) {
    let (avg, sigma, _) = estimate_from_samples_with_outliers(samples, false);
    (avg, sigma)
}

pub fn parse_duration(s: &str) -> Result<i64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".to_string());
    }

    if s.chars().all(|c| c.is_ascii_digit()) {
        let mins: i64 = s.parse().map_err(|_| format!("invalid number: {s}"))?;
        return Ok(mins);
    }

    let mut total_minutes: i64 = 0;
    let mut num_start = 0;
    let mut chars = s.char_indices().peekable();
    let mut parsed_something = false;
    let mut pending_number = false;

    while let Some(&(i, c)) = chars.peek() {
        if c.is_ascii_digit() {
            while let Some(&(.., c)) = chars.peek() {
                if c.is_ascii_digit() {
                    chars.next();
                } else {
                    break;
                }
            }
            num_start = i;
            pending_number = true;
        } else {
            let unit = c;
            chars.next();
            let num_str = &s[num_start..i];
            let num: i64 = num_str
                .parse()
                .map_err(|_| format!("invalid number in duration: {num_str}"))?;
            let value = match unit {
                'h' => num.checked_mul(60),
                'm' => Some(num),
                's' => num.checked_mul(SLOT_MINUTES),
                _ => {
                    return Err(format!(
                        "unknown unit '{unit}' in duration (use h, m, s for slots)"
                    ));
                }
            }
            .ok_or_else(|| format!("duration overflow in {num}{unit}"))?;
            total_minutes = total_minutes
                .checked_add(value)
                .ok_or_else(|| "duration overflow".to_string())?;
            parsed_something = true;
            pending_number = false;
        }
    }

    if !parsed_something {
        return Err(format!("could not parse duration: {s}"));
    }
    if pending_number {
        return Err(format!(
            "trailing number without unit in duration: {s} (use h, m, s for slots)"
        ));
    }
    Ok(total_minutes)
}

/// Percent-encode a URL path segment. Encodes every byte that is not an
/// unreserved character per RFC 3986 (`A-Z a-z 0-9 - . _ ~`).
pub fn url_encode(s: &str) -> String {
    s.bytes()
        .flat_map(|b| match b {
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date::now_timestamp;
    use std::str::FromStr;

    #[test]
    fn test_pure_number() {
        assert_eq!(parse_duration("30").unwrap(), 30);
    }

    #[test]
    fn test_hours_and_minutes() {
        assert_eq!(parse_duration("1h30m").unwrap(), 90);
    }

    #[test]
    fn test_minutes() {
        assert_eq!(parse_duration("30m").unwrap(), 30);
    }

    #[test]
    fn test_slots() {
        assert_eq!(parse_duration("30s").unwrap(), 150);
    }

    #[test]
    fn test_hours_only() {
        assert_eq!(parse_duration("2h").unwrap(), 120);
    }

    #[test]
    fn test_combined() {
        assert_eq!(parse_duration("1h15m").unwrap(), 75);
    }

    #[test]
    fn test_slots_and_minutes() {
        assert_eq!(parse_duration("6s").unwrap(), 30);
    }

    #[test]
    fn test_parse_datetime_iso() {
        let result = parse_datetime("2025-06-05T14:00:00Z").unwrap();
        assert!(result.starts_with("2025-06-05T14:00:00"));
    }

    #[test]
    fn test_parse_datetime_space() {
        let result = parse_datetime("2025-06-05 14:00").unwrap();
        assert!(result.starts_with("2025-06-05T14:00"));
    }

    #[test]
    fn test_parse_datetime_date_only() {
        let result = parse_datetime("2025-06-05").unwrap();
        assert!(result.starts_with("2025-06-05"));
    }

    #[test]
    fn test_parse_datetime_day_only() {
        // parse_datetime resolves relative dates in UTC, so the expected
        // year/month must come from UTC "now", not local time (#1419).
        let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
        let result = parse_datetime("-06").unwrap();
        let ts = jiff::Timestamp::from_str(&result).unwrap();
        let expected = jiff::civil::Date::new(now.year(), now.month(), 6)
            .unwrap()
            .at(23, 59, 59, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp();
        assert_eq!(ts, expected);
    }

    #[test]
    fn test_parse_datetime_month_day() {
        let result = parse_datetime("06-15").unwrap();
        let ts = jiff::Timestamp::from_str(&result).unwrap();
        let year = jiff::Timestamp::now()
            .to_zoned(jiff::tz::TimeZone::UTC)
            .year();
        let expected = jiff::civil::Date::new(year, 6, 15)
            .unwrap()
            .at(23, 59, 59, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp();
        assert_eq!(ts, expected);
    }

    #[test]
    fn test_parse_datetime_month_day_with_time() {
        let result = parse_datetime("06-15T14:00").unwrap();
        assert!(result.contains("T14:00"));
    }

    #[test]
    fn test_parse_datetime_day_with_time() {
        let result = parse_datetime("-06T14:30").unwrap();
        assert!(result.contains("T14:30"));
    }

    #[test]
    fn test_trailing_number_without_unit_errors() {
        assert!(parse_duration("1h30").is_err());
    }

    #[test]
    fn test_parse_datetime_naive_uses_configured_tz() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        let result = parse_datetime_tz("2025-06-05T14:00", &tz).unwrap();
        // 14:00 JST == 05:00 UTC
        let ts = jiff::Timestamp::from_str(&result).unwrap();
        let expected = jiff::civil::date(2025, 6, 5)
            .at(14, 0, 0, 0)
            .to_zoned(tz)
            .unwrap()
            .timestamp();
        assert_eq!(ts, expected);
    }

    #[test]
    fn test_parse_datetime_explicit_offset_preserved() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        let result = parse_datetime_tz("2025-06-05T14:00:00Z", &tz).unwrap();
        assert!(result.starts_with("2025-06-05T14:00:00"));
    }

    // ── parse_duration edge cases ───────────────────────────────────────

    #[test]
    fn parse_duration_empty_errors() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("   ").is_err());
    }

    #[test]
    fn parse_duration_overflow_errors() {
        let max = i64::MAX.to_string();
        assert!(parse_duration(&format!("{max}h")).is_err());
        assert!(parse_duration(&format!("{max}m1m")).is_err());
    }

    #[test]
    fn parse_duration_unknown_unit_errors() {
        assert!(parse_duration("5x").is_err());
        assert!(
            parse_duration("1d").is_err(),
            "'d' is not a duration unit here"
        );
    }

    #[test]
    fn parse_duration_unit_without_number_errors() {
        assert!(parse_duration("h").is_err());
        assert!(parse_duration("m").is_err());
    }

    #[test]
    fn parse_duration_zero_pure_number() {
        assert_eq!(parse_duration("0").unwrap(), 0);
    }

    #[test]
    fn parse_duration_trims_whitespace() {
        assert_eq!(parse_duration("  30m  ").unwrap(), 30);
    }

    #[test]
    fn parse_duration_s_is_slots_not_seconds() {
        // Documented footgun: 's' means 5-min slots, not seconds.
        // 1s = 1 slot = 5 minutes.
        assert_eq!(parse_duration("1s").unwrap(), 5);
        assert_eq!(parse_duration("12s").unwrap(), 60);
    }

    #[test]
    fn parse_duration_multiple_units() {
        assert_eq!(parse_duration("1h30m15s").unwrap(), 60 + 30 + 75);
    }

    // ── parse_datetime edge cases ───────────────────────────────────────

    #[test]
    fn parse_datetime_now_keyword() {
        let now = now_timestamp().unwrap();
        let result = parse_datetime("now").unwrap();
        let ts = jiff::Timestamp::from_str(&result).unwrap();
        assert!((ts.as_second() - now.as_second()).abs() <= 2);
    }

    #[test]
    fn parse_datetime_ambiguous_dash_format_errors() {
        // "06-15-2025" looks like month-day-year but is ambiguous → error
        assert!(parse_datetime("06-15-2025").is_err());
    }

    #[test]
    fn parse_datetime_garbage_errors() {
        assert!(parse_datetime("hello world").is_err());
        assert!(parse_datetime("2025-13-45").is_err());
    }

    // ── parse_date_expression ───────────────────────────────────────────

    #[test]
    fn parse_date_expression_now() {
        let now = now_timestamp().unwrap();
        let tz = jiff::tz::TimeZone::UTC;
        let result = parse_date_expression("now", &tz, false).unwrap();
        assert!((result.as_second() - now.as_second()).abs() <= 2);
    }

    #[test]
    fn parse_date_expression_today_start_and_end() {
        let tz = jiff::tz::TimeZone::UTC;
        let today = now_timestamp().unwrap().to_zoned(tz.clone()).date();
        let start = parse_date_expression("today", &tz, false).unwrap();
        let end = parse_date_expression("today", &tz, true).unwrap();
        assert_eq!(
            start.to_zoned(tz.clone()).date().to_string(),
            today.to_string()
        );
        assert_eq!(start.to_zoned(tz.clone()).time().to_string(), "00:00:00");
        assert_eq!(
            end.to_zoned(tz.clone()).date().to_string(),
            today.to_string()
        );
        assert_eq!(end.to_zoned(tz.clone()).time().to_string(), "23:59:59");
    }

    #[test]
    fn parse_date_expression_relative_days() {
        let tz = jiff::tz::TimeZone::UTC;
        let today = now_timestamp().unwrap().to_zoned(tz.clone()).date();
        let expected = today.checked_add(jiff::Span::new().days(7)).unwrap();
        let start = parse_date_expression("7d", &tz, false).unwrap();
        let end = parse_date_expression("7d", &tz, true).unwrap();
        assert_eq!(
            start.to_zoned(tz.clone()).date().to_string(),
            expected.to_string()
        );
        assert_eq!(start.to_zoned(tz.clone()).time().to_string(), "00:00:00");
        assert_eq!(
            end.to_zoned(tz.clone()).date().to_string(),
            expected.to_string()
        );
        assert_eq!(end.to_zoned(tz.clone()).time().to_string(), "23:59:59");

        // "+7d" must produce the same timestamp as "7d".
        assert_eq!(parse_date_expression("+7d", &tz, false).unwrap(), start);
        assert_eq!(parse_date_expression("+7d", &tz, true).unwrap(), end);
    }

    #[test]
    fn parse_date_expression_today_and_relative_in_non_utc_timezone() {
        let tz = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        let today = now_timestamp().unwrap().to_zoned(tz.clone()).date();

        let start = parse_date_expression("today", &tz, false).unwrap();
        let end = parse_date_expression("today", &tz, true).unwrap();
        assert_eq!(
            start.to_zoned(tz.clone()).date().to_string(),
            today.to_string()
        );
        assert_eq!(start.to_zoned(tz.clone()).time().to_string(), "00:00:00");
        assert_eq!(
            end.to_zoned(tz.clone()).date().to_string(),
            today.to_string()
        );
        assert_eq!(end.to_zoned(tz.clone()).time().to_string(), "23:59:59");

        let expected = today.checked_add(jiff::Span::new().days(7)).unwrap();
        let start = parse_date_expression("7d", &tz, false).unwrap();
        let end = parse_date_expression("7d", &tz, true).unwrap();
        assert_eq!(
            start.to_zoned(tz.clone()).date().to_string(),
            expected.to_string()
        );
        assert_eq!(start.to_zoned(tz.clone()).time().to_string(), "00:00:00");
        assert_eq!(
            end.to_zoned(tz.clone()).date().to_string(),
            expected.to_string()
        );
        assert_eq!(end.to_zoned(tz.clone()).time().to_string(), "23:59:59");
    }

    #[test]
    fn parse_date_expression_negative_days() {
        let tz = jiff::tz::TimeZone::UTC;
        let today = now_timestamp().unwrap().to_zoned(tz.clone()).date();
        let expected = today.checked_add(jiff::Span::new().days(-3)).unwrap();
        let start = parse_date_expression("-3d", &tz, false).unwrap();
        assert_eq!(
            start.to_zoned(tz.clone()).date().to_string(),
            expected.to_string()
        );
        assert_eq!(start.to_zoned(tz.clone()).time().to_string(), "00:00:00");
    }

    #[test]
    fn parse_date_expression_absolute_date() {
        let tz = jiff::tz::TimeZone::UTC;
        let start = parse_date_expression("2026-07-20", &tz, false).unwrap();
        let end = parse_date_expression("2026-07-20", &tz, true).unwrap();
        assert_eq!(start.to_zoned(tz.clone()).date().to_string(), "2026-07-20");
        assert_eq!(start.to_zoned(tz.clone()).time().to_string(), "00:00:00");
        assert_eq!(end.to_zoned(tz.clone()).date().to_string(), "2026-07-20");
        assert_eq!(end.to_zoned(tz.clone()).time().to_string(), "23:59:59");
    }

    #[test]
    fn parse_date_expression_full_datetime_passthrough() {
        let tz = jiff::tz::TimeZone::UTC;
        let expected = jiff::Timestamp::from_str("2026-07-20T12:34:56Z").unwrap();
        let result = parse_date_expression("2026-07-20T12:34:56Z", &tz, true).unwrap();
        assert_eq!(result.as_second(), expected.as_second());
    }

    #[test]
    fn parse_date_expression_invalid_errors() {
        let tz = jiff::tz::TimeZone::UTC;
        assert!(parse_date_expression("hello", &tz, false).is_err());
        assert!(parse_date_expression("", &tz, false).is_err());
    }

    // ── estimate_from_samples ───────────────────────────────────────────

    #[test]
    fn estimate_from_samples_empty() {
        assert_eq!(estimate_from_samples(&[]), (0, 0));
    }

    #[test]
    fn estimate_from_samples_single() {
        assert_eq!(estimate_from_samples(&[42]), (42, 0));
    }

    #[test]
    fn estimate_from_samples_two() {
        let (avg, sigma) = estimate_from_samples(&[40, 60]);
        assert_eq!(avg, 50);
        assert_eq!(sigma, 14); // sample stddev of 40,60 is ~14.14
    }

    #[test]
    fn estimate_from_samples_clamps_avg() {
        assert_eq!(estimate_from_samples(&[99999]).0, 24 * 60);
        assert_eq!(estimate_from_samples(&[-10]).0, 5);
    }

    #[test]
    fn estimate_from_samples_sigma_minimum_clamp() {
        // Identical samples have stddev 0; clamped to 5 minutes (1 slot).
        let (_, sigma) = estimate_from_samples(&[10, 10]);
        assert_eq!(sigma, 5);
    }

    #[test]
    fn detect_outlier_indices_finds_clear_outlier() {
        let samples = &[30, 32, 31, 29, 28, 120];
        let outliers = detect_outlier_indices(samples);
        assert_eq!(outliers, vec![5]);

        let short = &[30, 32, 31, 210];
        let outliers_short = detect_outlier_indices(short);
        assert_eq!(outliers_short, vec![3]);
    }

    #[test]
    fn detect_outlier_indices_ignores_small_samples() {
        assert!(detect_outlier_indices(&[10, 100]).is_empty());
        assert!(detect_outlier_indices(&[100]).is_empty());
    }

    #[test]
    fn estimate_from_samples_with_outliers_excludes_and_returns_indices() {
        let samples = &[30, 32, 31, 29, 28, 120];
        let (avg, sigma, excluded) = estimate_from_samples_with_outliers(samples, true);
        assert_eq!(excluded, vec![5]);
        assert_eq!(avg, 30); // mean of 28..32, rounded
        assert_eq!(sigma, 5); // stddev clamped to MIN_ESTIMATE_MINUTES

        let (avg2, sigma2, excluded2) = estimate_from_samples_with_outliers(samples, false);
        assert!(excluded2.is_empty());
        // With outlier included, avg and sigma are much larger.
        assert!(avg2 > avg);
        assert!(sigma2 > sigma);
    }

    // ── weighted_estimate / estimate_progress (#1419) ───────────────────

    #[test]
    fn weighted_estimate_none_without_total_or_observations() {
        assert_eq!(weighted_estimate(&[(10, 1)], None), None);
        assert_eq!(weighted_estimate(&[(10, 1)], Some(0)), None);
        assert_eq!(weighted_estimate(&[], Some(10)), None);
        // Non-positive observations are filtered out.
        assert_eq!(
            weighted_estimate(&[(0, 5), (10, 0), (-3, 2)], Some(10)),
            None
        );
    }

    #[test]
    fn weighted_estimate_single_observation_has_zero_sigma() {
        // projection = (30 / 5) * 10 = 60
        assert_eq!(weighted_estimate(&[(30, 5)], Some(10)), Some((60, 0)));
    }

    #[test]
    fn weighted_estimate_weights_by_quantity_done() {
        // obs A: rate 10/unit -> projection 100, weight 1
        // obs B: rate 3.33/unit -> projection 33.33, weight 9
        // weighted mean = 10 * (10 + 30) / (1 + 9) = 40 (unweighted would be ~67)
        // weighted sigma = sqrt((1*(100-40)^2 + 9*(33.33-40)^2) / 10) = 20
        let (avg, sigma) = weighted_estimate(&[(10, 1), (30, 9)], Some(10)).unwrap();
        assert_eq!(avg, 40);
        assert_eq!(sigma, 20);
        // The did-more observation (B) dominates: weighted avg is far closer to
        // B's projection (33) than to A's (100).
        assert!(avg < 50);
    }

    #[test]
    fn weighted_estimate_clamps_projection() {
        // projection = (1 / 1) * 10000 clamps to MAX_ESTIMATE_MINUTES (1440)
        assert_eq!(
            weighted_estimate(&[(1, 1)], Some(10000)),
            Some((24 * 60, 0))
        );
    }

    #[test]
    fn estimate_progress_uses_weighted_aggregate() {
        // history [(10,1)] + new (30,9) == weighted_estimate test above -> (40, 20).
        // The supplied prior estimate (999, 999) is ignored once observations exist.
        let (avg, sigma) = estimate_progress(999, 999, Some(10), 30, 9, &[(10, 1)]);
        assert_eq!(avg, 40);
        assert_eq!(sigma, 20);
    }

    #[test]
    fn estimate_progress_falls_back_without_progress_or_total() {
        // Zero delta is a no-op.
        assert_eq!(
            estimate_progress(50, 10, Some(10), 0, 0, &[(10, 1)]),
            (50, 10)
        );
        // No usable total -> fallback to the prior estimate.
        assert_eq!(estimate_progress(50, 10, None, 30, 5, &[]), (50, 10));
    }

    #[test]
    fn parse_timezone_accepts_iana_and_fixed_offsets() {
        assert!(parse_timezone("Asia/Tokyo").is_ok());
        assert!(parse_timezone("UTC").is_ok());
        assert!(parse_timezone("+09:00").is_ok());
        assert!(parse_timezone("not/a/tz").is_err());
    }

    #[test]
    fn url_encode_preserves_unreserved_and_encodes_specials() {
        assert_eq!(url_encode("h1#5"), "h1%235");
        assert_eq!(url_encode("a?b&c"), "a%3Fb%26c");
        assert_eq!(url_encode("a/b"), "a%2Fb");
        assert_eq!(url_encode("abc-_.~"), "abc-_.~");
    }

    #[test]
    fn now_rfc3339_has_whole_second_precision() {
        let s = now_rfc3339();
        assert!(s.ends_with('Z'));
        assert!(!s.contains('.'));
        assert!(jiff::Timestamp::from_str(&s).is_ok());
    }

    #[test]
    fn minutes_between_parses_rfc3339_and_legacy_space_separated() {
        assert_eq!(
            minutes_between("2026-07-23T09:00:00Z", "2026-07-23T09:05:00Z"),
            5
        );
        assert_eq!(
            minutes_between("2026-07-23 09:00:00", "2026-07-23 09:05:00"),
            5
        );
        assert_eq!(
            minutes_between("2026-07-23T09:00:00Z", "2026-07-23 09:05:00"),
            5
        );
    }

    #[test]
    fn later_timestamp_parses_rfc3339_and_legacy_space_separated() {
        assert_eq!(
            later_timestamp("2026-07-23T09:00:00Z", "2026-07-23T09:05:00Z"),
            "2026-07-23T09:05:00Z"
        );
        assert_eq!(
            later_timestamp("2026-07-23 09:05:00", "2026-07-23 09:00:00"),
            "2026-07-23 09:05:00"
        );
        assert_eq!(
            later_timestamp("2026-07-23 09:00:00", "2026-07-23T09:05:00Z"),
            "2026-07-23T09:05:00Z"
        );
    }
}
