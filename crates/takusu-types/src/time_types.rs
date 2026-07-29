//! Type-safe date/time newtypes for Phase 6 (see `doc/type-safety-issues.md` §5).
//!
//! These types wrap `jiff` types and provide `Display` / `FromStr` / `Serialize`
//! / `Deserialize` / sqlx integration so they can replace raw `String` fields in
//! `takusu-storage`, `takusu-client`, and `takusu-worker` model structs.
//!
//! - [`TimeOfDay`]: `HH:MM` with 5-minute slot snapping (moved from `takusu-habit`).
//! - [`Date`]: `YYYY-MM-DD` calendar date.
//! - [`Timestamp`]: RFC 3339 timestamp.
//!
//! All three serialize as strings (preserving the existing JSON representation)
//! and are stored as `TEXT` in SQLite.

use std::cmp::Ordering;
use std::str::FromStr;

use jiff::Timestamp as JiffTimestamp;
use jiff::civil::Date as JiffDate;
use jiff::civil::DateTime as JiffDateTime;
use serde::{Deserialize, Serialize};

use crate::SLOT_MINUTES;

// ── TimeOfDay ─────────────────────────────────────────────────────────────

/// A time of day in `HH:MM` format with minutes snapped to 5-minute slots.
///
/// Serialized as a `"HH:MM"` string for JSON and stored as `TEXT` in SQLite.
/// This type was originally defined in `takusu-habit` and has been moved here
/// so that `takusu-storage` / `takusu-client` / `takusu-worker` can use it
/// without depending on `takusu-habit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
}

impl TimeOfDay {
    /// Create a `TimeOfDay` from hour and minute.
    ///
    /// Returns `None` if `hour > 23` or `minute > 59`.
    /// Minutes are snapped down to the nearest 5-minute slot.
    pub fn new(hour: u8, minute: u8) -> Option<Self> {
        if hour > 23 || minute > 59 {
            return None;
        }
        let snapped = (minute as i64 / SLOT_MINUTES * SLOT_MINUTES) as u8;
        Some(Self {
            hour,
            minute: snapped,
        })
    }

    /// Hour (0–23).
    pub fn hour(self) -> u8 {
        self.hour
    }

    /// Minute (0–55, snapped to 5-minute slots).
    pub fn minute(self) -> u8 {
        self.minute
    }

    /// Total minutes since midnight (`hour * 60 + minute`).
    pub fn to_minutes(self) -> i64 {
        self.hour as i64 * 60 + self.minute as i64
    }
}

impl std::fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

impl FromStr for TimeOfDay {
    type Err = TimeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(TimeParseError::invalid(s));
        }
        let h: u8 = parts[0].parse().map_err(|_| TimeParseError::invalid(s))?;
        let m: u8 = parts[1].parse().map_err(|_| TimeParseError::invalid(s))?;
        TimeOfDay::new(h, m).ok_or_else(|| TimeParseError::invalid(s))
    }
}

impl TryFrom<String> for TimeOfDay {
    type Error = TimeParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ── Date ──────────────────────────────────────────────────────────────────

/// A calendar date in `YYYY-MM-DD` format.
///
/// Serialized as a `"YYYY-MM-DD"` string for JSON and stored as `TEXT` in
/// SQLite. Wraps [`jiff::civil::Date`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Date(pub JiffDate);

impl Date {
    pub fn new(year: i16, month: u8, day: u8) -> Option<Self> {
        JiffDate::new(year, month as i8, day as i8).ok().map(Self)
    }

    pub fn year(self) -> i16 {
        self.0.year()
    }

    pub fn month(self) -> u8 {
        self.0.month() as u8
    }

    pub fn day(self) -> u8 {
        self.0.day() as u8
    }

    pub fn to_jiff(self) -> JiffDate {
        self.0
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // jiff::civil::Date displays as "YYYY-MM-DD"
        write!(f, "{}", self.0)
    }
}

impl FromStr for Date {
    type Err = TimeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Enforce strict YYYY-MM-DD (zero-padded, 4-digit year) to match
        // the existing parse_calendar_date validation.
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
            return Err(TimeParseError::invalid(s));
        }
        let d = JiffDate::from_str(s).map_err(|_| TimeParseError::invalid(s))?;
        Ok(Self(d))
    }
}

impl TryFrom<String> for Date {
    type Error = TimeParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<JiffDate> for Date {
    fn from(d: JiffDate) -> Self {
        Self(d)
    }
}

impl From<Date> for JiffDate {
    fn from(d: Date) -> Self {
        d.0
    }
}

impl PartialOrd for Date {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Date {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl Serialize for Date {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ── Timestamp ─────────────────────────────────────────────────────────────

/// An RFC 3339 timestamp.
///
/// Serialized as an RFC 3339 string for JSON and stored as `TEXT` in SQLite.
/// Wraps [`jiff::Timestamp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Timestamp(pub JiffTimestamp);

impl Timestamp {
    /// Current time truncated to whole seconds (matches SQLite `strftime`).
    pub fn now() -> Self {
        let ts = JiffTimestamp::now();
        Self(JiffTimestamp::from_second(ts.as_second()).unwrap_or(ts))
    }

    pub fn from_second(second: i64) -> Option<Self> {
        JiffTimestamp::from_second(second).ok().map(Self)
    }

    pub fn as_second(self) -> i64 {
        self.0.as_second()
    }

    pub fn to_jiff(self) -> JiffTimestamp {
        self.0
    }

    /// Convert to a [`jiff::Zoned`] in the given timezone.
    pub fn to_zoned(self, tz: jiff::tz::TimeZone) -> jiff::Zoned {
        self.0.to_zoned(tz)
    }

    /// Parse a datetime string, interpreting naive (timezone-less) strings
    /// in the given timezone rather than UTC.
    ///
    /// Accepts RFC 3339 (`2025-01-01T00:00:00Z`), SQLite datetime formats
    /// (`2025-01-01T12:00:00`, `2025-01-01 12:00:00`), and other formats
    /// supported by [`takusu_types::parse_datetime_to_timestamp`].
    pub fn parse_with_tz(s: &str, tz: &jiff::tz::TimeZone) -> Result<Self, TimeParseError> {
        let ts = crate::date::parse_datetime_to_timestamp(s, tz).map_err(TimeParseError::msg)?;
        Ok(Self(ts))
    }
}

/// Whole minutes between two timestamps, returning at least 1 to avoid
/// degenerate speed observations (mirrors `crate::date::minutes_between`
/// but operates on typed `Timestamp` values instead of strings).
pub fn minutes_between_ts(start: Timestamp, end: Timestamp) -> i64 {
    ((end.as_second() - start.as_second()) / 60).max(1)
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // jiff::Timestamp displays as RFC 3339 (e.g. "2025-01-01T00:00:00Z")
        write!(f, "{}", self.0)
    }
}

impl FromStr for Timestamp {
    type Err = TimeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First try RFC 3339 (e.g. "2025-01-01T00:00:00Z").
        if let Ok(ts) = JiffTimestamp::from_str(s) {
            return Ok(Self(ts));
        }
        // Fall back to SQLite datetime formats without timezone:
        // "2025-01-01T12:00:00" or "2025-01-01 12:00:00" → interpret as UTC.
        for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
            if let Ok(dt) = JiffDateTime::strptime(fmt, s)
                && let Ok(zdt) = dt.to_zoned(jiff::tz::TimeZone::UTC)
            {
                return Ok(Self(zdt.timestamp()));
            }
        }
        Err(TimeParseError::invalid(s))
    }
}

impl TryFrom<String> for Timestamp {
    type Error = TimeParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<JiffTimestamp> for Timestamp {
    fn from(ts: JiffTimestamp) -> Self {
        Self(ts)
    }
}

impl From<Timestamp> for JiffTimestamp {
    fn from(ts: Timestamp) -> Self {
        ts.0
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self(JiffTimestamp::from_second(0).unwrap_or(JiffTimestamp::UNIX_EPOCH))
    }
}

impl PartialOrd for Timestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ── Error ─────────────────────────────────────────────────────────────────

/// Error returned when parsing a date/time string fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid date/time: {0:?}")]
pub struct TimeParseError(String);

impl TimeParseError {
    fn invalid(value: &str) -> Self {
        Self(value.to_string())
    }

    /// Create a `TimeParseError` from an arbitrary message.
    pub fn msg(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_of_day_display() {
        let t = TimeOfDay::new(9, 30).unwrap();
        assert_eq!(t.to_string(), "09:30");
        let t = TimeOfDay::new(0, 0).unwrap();
        assert_eq!(t.to_string(), "00:00");
        let t = TimeOfDay::new(23, 55).unwrap();
        assert_eq!(t.to_string(), "23:55");
    }

    #[test]
    fn time_of_day_from_str() {
        assert_eq!(
            "09:30".parse::<TimeOfDay>().unwrap(),
            TimeOfDay::new(9, 30).unwrap()
        );
        assert_eq!(
            "00:00".parse::<TimeOfDay>().unwrap(),
            TimeOfDay::new(0, 0).unwrap()
        );
        assert!("9:3".parse::<TimeOfDay>().is_ok()); // parses, then snaps
        assert!("24:00".parse::<TimeOfDay>().is_err());
        assert!("12:60".parse::<TimeOfDay>().is_err());
        assert!("abc".parse::<TimeOfDay>().is_err());
    }

    #[test]
    fn time_of_day_snaps_to_5_min_slots() {
        let t = TimeOfDay::new(9, 33).unwrap();
        assert_eq!(t.minute(), 30);
        let t = TimeOfDay::new(9, 37).unwrap();
        assert_eq!(t.minute(), 35);
    }

    #[test]
    fn time_of_day_to_minutes() {
        assert_eq!(TimeOfDay::new(9, 30).unwrap().to_minutes(), 570);
        assert_eq!(TimeOfDay::new(0, 0).unwrap().to_minutes(), 0);
    }

    #[test]
    fn time_of_day_serde_roundtrip() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Wrap {
            t: TimeOfDay,
        }
        let json = serde_json::to_string(&Wrap {
            t: TimeOfDay::new(9, 30).unwrap(),
        })
        .unwrap();
        assert_eq!(json, r#"{"t":"09:30"}"#);
        let back: Wrap = serde_json::from_str(&json).unwrap();
        assert_eq!(back.t, TimeOfDay::new(9, 30).unwrap());
    }

    #[test]
    fn date_display() {
        let d = Date::new(2025, 6, 15).unwrap();
        assert_eq!(d.to_string(), "2025-06-15");
    }

    #[test]
    fn date_from_str_strict_format() {
        assert!("2025-06-15".parse::<Date>().is_ok());
        // Reject non-zero-padded
        assert!("2025-6-15".parse::<Date>().is_err());
        assert!("25-06-15".parse::<Date>().is_err());
        // Reject invalid dates
        assert!("2025-13-01".parse::<Date>().is_err());
        assert!("2025-02-30".parse::<Date>().is_err());
    }

    #[test]
    fn date_serde_roundtrip() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Wrap {
            d: Date,
        }
        let json = serde_json::to_string(&Wrap {
            d: Date::new(2025, 6, 15).unwrap(),
        })
        .unwrap();
        assert_eq!(json, r#"{"d":"2025-06-15"}"#);
        let back: Wrap = serde_json::from_str(&json).unwrap();
        assert_eq!(back.d, Date::new(2025, 6, 15).unwrap());
    }

    #[test]
    fn date_ord() {
        let a = Date::new(2025, 1, 1).unwrap();
        let b = Date::new(2025, 6, 15).unwrap();
        let c = Date::new(2025, 12, 31).unwrap();
        assert!(a < b);
        assert!(b < c);
        assert!(a <= a);
    }

    #[test]
    fn timestamp_display() {
        let ts = Timestamp::from_second(1234567890).unwrap();
        assert_eq!(ts.to_string(), "2009-02-13T23:31:30Z");
    }

    #[test]
    fn timestamp_from_str() {
        let ts: Timestamp = "2025-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(ts.as_second(), 1735689600);
    }

    #[test]
    fn timestamp_from_str_sqlite_datetime_t_format() {
        // SQLite datetime('now') produces "YYYY-MM-DDTHH:MM:SS" (no Z).
        let ts: Timestamp = "2025-01-01T00:00:00".parse().unwrap();
        assert_eq!(ts.as_second(), 1735689600);
    }

    #[test]
    fn timestamp_from_str_sqlite_datetime_space_format() {
        // SQLite also produces "YYYY-MM-DD HH:MM:SS" with a space.
        let ts: Timestamp = "2025-01-01 00:00:00".parse().unwrap();
        assert_eq!(ts.as_second(), 1735689600);
    }

    #[test]
    fn timestamp_serde_roundtrip() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Wrap {
            ts: Timestamp,
        }
        let json = serde_json::to_string(&Wrap {
            ts: Timestamp::from_second(1234567890).unwrap(),
        })
        .unwrap();
        assert_eq!(json, r#"{"ts":"2009-02-13T23:31:30Z"}"#);
        let back: Wrap = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ts, Timestamp::from_second(1234567890).unwrap());
    }

    #[test]
    fn timestamp_ord() {
        let a = Timestamp::from_second(1000).unwrap();
        let b = Timestamp::from_second(2000).unwrap();
        assert!(a < b);
    }

    #[test]
    fn timestamp_now_is_whole_seconds() {
        let now = Timestamp::now();
        assert_eq!(now.as_second() * 1000, now.0.as_millisecond());
    }

    #[test]
    fn timestamp_parse_with_tz_naive_uses_tz() {
        // "2025-01-01T18:00:00" without timezone → JST (UTC+9) = 09:00 UTC.
        let jst = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        let ts = Timestamp::parse_with_tz("2025-01-01T18:00:00", &jst).unwrap();
        assert_eq!(ts.as_second(), 1735722000); // 2025-01-01T09:00:00Z
    }

    #[test]
    fn timestamp_parse_with_tz_rfc3339_ignores_tz_param() {
        // RFC 3339 with explicit Z should not be shifted by the tz argument.
        let jst = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        let ts = Timestamp::parse_with_tz("2025-01-01T00:00:00Z", &jst).unwrap();
        assert_eq!(ts.as_second(), 1735689600);
    }

    #[test]
    fn minutes_between_ts_returns_whole_minutes() {
        let a = Timestamp::from_second(1735689600).unwrap(); // 2025-01-01T00:00:00Z
        let b = Timestamp::from_second(1735689660).unwrap(); // +60s
        assert_eq!(minutes_between_ts(a, b), 1);
        assert_eq!(minutes_between_ts(a, a), 1); // degenerate → at least 1
    }
}
