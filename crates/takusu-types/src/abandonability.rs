//! Newtype for a task/habit abandonment weight in `[0.0, 1.0]`.
//!
//! Out-of-range values are silently clamped to the nearest bound and `NaN`
//! falls back to the default `0.5`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A value in `[0.0, 1.0]` representing how easily a task can be abandoned.
///
/// Higher values mean the task is more likely to be dropped when the schedule
/// does not fit. `new` silently clamps the input to `[0.0, 1.0]`; `NaN`
/// becomes `0.5`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, schemars::JsonSchema)]
#[schemars(transparent)]
pub struct Abandonability(f64);

impl Abandonability {
    pub const MIN: f64 = 0.0;
    pub const MAX: f64 = 1.0;
    pub const DEFAULT_VALUE: f64 = 0.5;

    /// Clamp `value` to `[MIN, MAX]`. `NaN` becomes `DEFAULT_VALUE`.
    pub fn new(value: f64) -> Self {
        if value.is_nan() {
            Self(Self::DEFAULT_VALUE)
        } else if value < Self::MIN {
            Self(Self::MIN)
        } else if value > Self::MAX {
            Self(Self::MAX)
        } else {
            Self(value)
        }
    }

    pub const fn default_value() -> Self {
        Self(Self::DEFAULT_VALUE)
    }

    pub fn get(&self) -> f64 {
        self.0
    }
}

impl Default for Abandonability {
    fn default() -> Self {
        Self::default_value()
    }
}

impl fmt::Display for Abandonability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Abandonability> for f64 {
    fn from(value: Abandonability) -> Self {
        value.0
    }
}

impl From<f64> for Abandonability {
    fn from(value: f64) -> Self {
        Self::new(value)
    }
}

impl TryFrom<String> for Abandonability {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse::<f64>()
            .map(Self::new)
            .map_err(|e| format!("invalid abandonability: {e}"))
    }
}

impl FromStr for Abandonability {
    type Err = std::num::ParseFloatError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s.parse()?))
    }
}

impl Serialize for Abandonability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for Abandonability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Ok(Self::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_to_min() {
        assert_eq!(Abandonability::new(-0.5).get(), 0.0);
    }

    #[test]
    fn clamp_to_max() {
        assert_eq!(Abandonability::new(1.5).get(), 1.0);
    }

    #[test]
    fn nan_becomes_default() {
        assert_eq!(Abandonability::new(f64::NAN).get(), 0.5);
    }

    #[test]
    fn boundaries_preserved() {
        assert_eq!(Abandonability::new(0.0).get(), 0.0);
        assert_eq!(Abandonability::new(1.0).get(), 1.0);
    }

    #[test]
    fn default_is_half() {
        assert_eq!(Abandonability::default().get(), 0.5);
    }

    #[test]
    fn from_f64_clamps() {
        let a: Abandonability = 2.0.into();
        assert_eq!(a.get(), 1.0);
    }

    #[test]
    fn into_f64() {
        let a = Abandonability::new(0.3);
        assert_eq!(f64::from(a), 0.3);
    }

    #[test]
    fn serde_roundtrip() {
        let a = Abandonability::new(0.7);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, "0.7");
        let back: Abandonability = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn serde_clamps_out_of_range() {
        let back: Abandonability = serde_json::from_str("-1.2").unwrap();
        assert_eq!(back.get(), 0.0);
    }

    #[test]
    fn serde_rejects_null() {
        assert!(serde_json::from_str::<Abandonability>("null").is_err());
    }
}
