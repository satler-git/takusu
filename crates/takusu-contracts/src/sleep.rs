//! Sleep input and resolved sleep configuration for the planner.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use takusu_types::TimeOfDay;

/// Error returned when a string cannot be parsed into [`SleepInput`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "invalid sleep input: {value:?} (expected \"recommended\", \"disabled\", or \"HH:MM-HH:MM\")"
)]
pub struct SleepInputError {
    value: String,
}

impl SleepInputError {
    fn invalid(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }
}

/// Sleep configuration for schedule generation / reschedule / preview.
///
/// Parsed at the API/CLI boundary from a plain string; consumed by
/// `takusu-local-lib` to build a `SleepConfig` using settings + timezone.
#[derive(Debug, Clone, Default, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(with = "String")]
pub enum SleepInput {
    /// Use the sleep window from settings (`sleep_start` / `sleep_end`).
    #[default]
    Recommended,
    /// Disable the sleep window entirely.
    Disabled,
    /// A custom sleep window.
    Custom { start: TimeOfDay, end: TimeOfDay },
}

impl fmt::Display for SleepInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recommended => f.write_str("recommended"),
            Self::Disabled => f.write_str("disabled"),
            Self::Custom { start, end } => write!(f, "{start}-{end}"),
        }
    }
}

impl FromStr for SleepInput {
    type Err = SleepInputError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "recommended" => Ok(Self::Recommended),
            "disabled" => Ok(Self::Disabled),
            custom => {
                let parts: Vec<&str> = custom.splitn(2, '-').collect();
                if parts.len() == 2 {
                    let start = parts[0]
                        .parse::<TimeOfDay>()
                        .map_err(|_| SleepInputError::invalid(s))?;
                    let end = parts[1]
                        .parse::<TimeOfDay>()
                        .map_err(|_| SleepInputError::invalid(s))?;
                    Ok(Self::Custom { start, end })
                } else {
                    Err(SleepInputError::invalid(s))
                }
            }
        }
    }
}

impl Serialize for SleepInput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SleepInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ── SleepConfig ───────────────────────────────────────────────────────

/// 睡眠設定。
///
/// 一日の基点 (`day_start`) からの相対スロット数で睡眠時間帯を指定する。
/// 例: `day_start=0` (0:00基点), `start=264` (22:00), `end=360` (翌6:00) → 8時間睡眠。
///
/// 不変条件: `enabled == true` のとき `end > start`。
#[derive(Debug, Clone, Copy)]
pub struct SleepConfig {
    /// 一日の基点 (エポックからのスロット)。通常 0。
    day_start: i64,
    /// 睡眠開始 (基点からの相対スロット)。
    start: i64,
    /// 睡眠終了 (基点からの相対スロット)。enabled のとき end > start。
    end: i64,
    /// 睡眠制約が有効かどうか。
    enabled: bool,
}

impl SleepConfig {
    /// 全フィールドを指定して構築。
    ///
    /// `enabled == true` のとき `end > start` でなければならない。
    pub fn new(day_start: i64, start: i64, end: i64, enabled: bool) -> Self {
        if enabled {
            assert!(
                end > start,
                "SleepConfig::new: end ({end}) must be greater than start ({start}) when enabled"
            );
        }
        Self {
            day_start,
            start,
            end,
            enabled,
        }
    }

    /// 推奨設定: 22:00–06:00 (8時間), 一日は 0:00 基点。
    pub fn recommended() -> Self {
        Self::new(0, 264, 360, true) // 22 * 12, 30 * 12 = 6:00 next day
    }

    /// 睡眠制約なし。
    pub fn disabled() -> Self {
        Self::new(0, 0, 0, false)
    }

    /// タイムゾーンとローカル時計時刻から SleepConfig を構築。
    ///
    /// `per` は 1 スロットの分数 (通常 5)。`tz` は jiff タイムゾーン。
    /// `start_h`/`start_m` と `end_h`/`end_m` はローカル時刻による睡眠窓。
    /// 日跨ぎ (例: 22:00–06:00) は自動で処理される。
    pub fn from_local(
        per: u16,
        tz: &jiff::tz::TimeZone,
        start_h: u8,
        start_m: u8,
        end_h: u8,
        end_m: u8,
    ) -> Self {
        let slots_per_hour: i64 = 60 / per as i64;
        let slots_per_day: i64 = 24 * slots_per_hour;

        let offset_secs: i64 = tz.to_offset(jiff::Timestamp::now()).seconds().into();
        let offset_slots = offset_secs / (per as i64 * 60);

        let day_start = (slots_per_day - offset_slots).rem_euclid(slots_per_day);

        let start = start_h as i64 * slots_per_hour + start_m as i64 / per as i64;
        let mut end = end_h as i64 * slots_per_hour + end_m as i64 / per as i64;

        if end <= start {
            end += slots_per_day;
        }

        Self::new(day_start, start, end, true)
    }

    /// 一日の基点 (エポックからのスロット)。通常 0。
    pub fn day_start(&self) -> i64 {
        self.day_start
    }

    /// 睡眠開始 (基点からの相対スロット)。
    pub fn start(&self) -> i64 {
        self.start
    }

    /// 睡眠終了 (基点からの相対スロット)。enabled のとき end > start。
    pub fn end(&self) -> i64 {
        self.end
    }

    /// 睡眠制約が有効かどうか。
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for SleepConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_input_recommended_round_trips() {
        assert_eq!(
            "recommended".parse::<SleepInput>().unwrap(),
            SleepInput::Recommended
        );
        assert_eq!(SleepInput::Recommended.to_string(), "recommended");
    }

    #[test]
    fn sleep_input_disabled_round_trips() {
        assert_eq!(
            "disabled".parse::<SleepInput>().unwrap(),
            SleepInput::Disabled
        );
        assert_eq!(SleepInput::Disabled.to_string(), "disabled");
    }

    #[test]
    fn sleep_input_custom_round_trips() {
        let input = SleepInput::Custom {
            start: TimeOfDay::new(23, 0).unwrap(),
            end: TimeOfDay::new(6, 0).unwrap(),
        };
        assert_eq!(input.to_string(), "23:00-06:00");
        let parsed: SleepInput = "23:00-06:00".parse().unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn sleep_input_invalid_string_errors() {
        assert!("22:70-06:00".parse::<SleepInput>().is_err());
        assert!("25:00-06:00".parse::<SleepInput>().is_err());
        assert!("garbage".parse::<SleepInput>().is_err());
        assert!("22:00".parse::<SleepInput>().is_err());
    }

    #[test]
    fn sleep_input_serde_round_trips_as_string() {
        let input = SleepInput::Custom {
            start: TimeOfDay::new(22, 0).unwrap(),
            end: TimeOfDay::new(7, 0).unwrap(),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert_eq!(json, "\"22:00-07:00\"");
        let back: SleepInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back, input);
    }

    #[test]
    fn sleep_input_serde_recommended() {
        let json = serde_json::to_string(&SleepInput::Recommended).unwrap();
        assert_eq!(json, "\"recommended\"");
        let back: SleepInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SleepInput::Recommended);
    }

    #[test]
    fn sleep_config_disabled() {
        let sc = SleepConfig::disabled();
        assert!(!sc.enabled());
    }

    #[test]
    fn sleep_config_recommended() {
        let sc = SleepConfig::recommended();
        assert!(sc.enabled());
    }

    #[test]
    fn sleep_config_recommended_values() {
        let sc = SleepConfig::recommended();
        assert_eq!(sc.day_start(), 0);
        assert_eq!(sc.start(), 264);
        assert_eq!(sc.end(), 360);
        assert!(sc.enabled());
        assert!(
            sc.end() > sc.start(),
            "recommended sleep must have end > start"
        );
    }

    #[test]
    fn sleep_config_disabled_values() {
        let sc = SleepConfig::disabled();
        assert_eq!(sc.day_start(), 0);
        assert_eq!(sc.start(), 0);
        assert_eq!(sc.end(), 0);
        assert!(!sc.enabled());
    }

    #[test]
    #[should_panic(expected = "end (10) must be greater than start (20)")]
    fn sleep_config_new_rejects_end_le_start_when_enabled() {
        let _ = SleepConfig::new(0, 20, 10, true);
    }

    #[test]
    #[should_panic(expected = "end (20) must be greater than start (20)")]
    fn sleep_config_new_rejects_end_eq_start_when_enabled() {
        let _ = SleepConfig::new(0, 20, 20, true);
    }

    #[test]
    fn sleep_config_new_allows_end_le_start_when_disabled() {
        let sc = SleepConfig::new(0, 20, 10, false);
        assert!(!sc.enabled());
        assert_eq!(sc.start(), 20);
        assert_eq!(sc.end(), 10);
    }

    #[test]
    fn sleep_config_new_valid_when_end_gt_start() {
        let sc = SleepConfig::new(0, 22, 30, true);
        assert!(sc.enabled());
        assert_eq!(sc.start(), 22);
        assert_eq!(sc.end(), 30);
    }
}
