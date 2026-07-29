//! Type-safe sleep configuration input for schedule generation / reschedule.
//!
//! Replaces the raw `String` that was interpreted at runtime by `parse_sleep`
//! (see `doc/code-quality-issues.md` §13). The enum is serialized/deserialized
//! as a plain string so the HTTP API stays backwards compatible:
//!
//! | Variant                | String form           |
//! |------------------------|-----------------------|
//! | `Recommended`          | `"recommended"`       |
//! | `Disabled`             | `"disabled"`          |
//! | `Custom { start, end }`| `"HH:MM-HH:MM"`       |

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::TimeOfDay;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_round_trips() {
        assert_eq!(
            "recommended".parse::<SleepInput>().unwrap(),
            SleepInput::Recommended
        );
        assert_eq!(SleepInput::Recommended.to_string(), "recommended");
    }

    #[test]
    fn disabled_round_trips() {
        assert_eq!(
            "disabled".parse::<SleepInput>().unwrap(),
            SleepInput::Disabled
        );
        assert_eq!(SleepInput::Disabled.to_string(), "disabled");
    }

    #[test]
    fn custom_round_trips() {
        let input = SleepInput::Custom {
            start: TimeOfDay::new(23, 0).unwrap(),
            end: TimeOfDay::new(6, 0).unwrap(),
        };
        assert_eq!(input.to_string(), "23:00-06:00");
        let parsed: SleepInput = "23:00-06:00".parse().unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn invalid_string_errors() {
        assert!("22:70-06:00".parse::<SleepInput>().is_err());
        assert!("25:00-06:00".parse::<SleepInput>().is_err());
        assert!("garbage".parse::<SleepInput>().is_err());
        assert!("22:00".parse::<SleepInput>().is_err());
    }

    #[test]
    fn serde_round_trips_as_string() {
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
    fn serde_recommended() {
        let json = serde_json::to_string(&SleepInput::Recommended).unwrap();
        assert_eq!(json, "\"recommended\"");
        let back: SleepInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SleepInput::Recommended);
    }
}
