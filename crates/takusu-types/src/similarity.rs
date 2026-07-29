//! Structured similarity score for `SimilarTaskRow` (see
//! `doc/code-quality-issues.md` #33).
//!
//! Replaces the old `"dice:0.85"` packed string with a typed struct so the
//! metric and score are separately accessible without split-and-parse.

use serde::{Deserialize, Serialize};

use crate::EnumLabel;
use crate::SimilarityMetric;

/// A similarity score pairing a metric with a numeric value.
///
/// Serialized as `{"metric":"dice","score":0.85}`. Implements [`Default`]
/// (`Dice`, `0.0`) so it can be used with `#[sqlx(skip)]` / `#[serde(default)]`
/// on row structs where the SQL/JSON result does not include the field.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Similarity {
    pub metric: SimilarityMetric,
    pub score: f64,
}

impl Default for Similarity {
    fn default() -> Self {
        Self {
            metric: SimilarityMetric::enum_default(),
            score: 0.0,
        }
    }
}

impl Similarity {
    /// Construct a `Similarity` with the Dice metric.
    pub fn dice(score: f64) -> Self {
        Self {
            metric: SimilarityMetric::Dice,
            score,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dice_constructor_sets_metric() {
        let s = Similarity::dice(0.85);
        assert_eq!(s.metric, SimilarityMetric::Dice);
        assert!((s.score - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn default_is_dice_zero() {
        let s = Similarity::default();
        assert_eq!(s.metric, SimilarityMetric::Dice);
        assert_eq!(s.score, 0.0);
    }

    #[test]
    fn serde_round_trips() {
        let s = Similarity::dice(0.123);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"metric":"dice","score":0.123}"#);
        let back: Similarity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn serde_default_round_trips() {
        let s = Similarity::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Similarity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
