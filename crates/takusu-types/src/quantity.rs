//! Newtype for a non-negative quantity count.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A non-negative integer quantity.
///
/// Negative values are rejected at construction time. The inner `i64` can be
/// retrieved via [`Quantity::get`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, schemars::JsonSchema)]
#[schemars(transparent)]
pub struct Quantity(i64);

impl Quantity {
    pub fn new(value: i64) -> Result<Self, QuantityError> {
        if value < 0 {
            Err(QuantityError::Negative(value))
        } else {
            Ok(Self(value))
        }
    }

    pub const fn get(&self) -> i64 {
        self.0
    }
}

impl From<Quantity> for i64 {
    fn from(value: Quantity) -> Self {
        value.0
    }
}

impl From<Quantity> for u64 {
    fn from(value: Quantity) -> Self {
        value.0 as u64
    }
}

impl From<Quantity> for f64 {
    fn from(value: Quantity) -> Self {
        value.0 as f64
    }
}

impl PartialEq<i64> for Quantity {
    fn eq(&self, other: &i64) -> bool {
        self.0 == *other
    }
}

impl PartialOrd<i64> for Quantity {
    fn partial_cmp(&self, other: &i64) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(other)
    }
}

impl TryFrom<i64> for Quantity {
    type Error = QuantityError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<u64> for Quantity {
    type Error = QuantityError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        i64::try_from(value)
            .map_err(|_| QuantityError::Overflow)?
            .try_into()
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Quantity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct QuantityVisitor;

        impl<'de> serde::de::Visitor<'de> for QuantityVisitor {
            type Value = Quantity;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a non-negative integer quantity")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Quantity::new(value).map_err(serde::de::Error::custom)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let value = i64::try_from(value)
                    .map_err(|_| serde::de::Error::custom(QuantityError::Overflow))?;
                Quantity::new(value).map_err(serde::de::Error::custom)
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if !value.is_finite() {
                    return Err(serde::de::Error::custom("quantity must be a finite number"));
                }
                if value < 0.0 {
                    return Err(serde::de::Error::custom(QuantityError::Negative(
                        value as i64,
                    )));
                }
                if value > i64::MAX as f64 {
                    return Err(serde::de::Error::custom(QuantityError::Overflow));
                }
                if value.fract() != 0.0 {
                    return Err(serde::de::Error::custom("quantity must be an integer"));
                }
                let value = value as i64;
                Quantity::new(value).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(QuantityVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuantityError {
    #[error("quantity cannot be negative: {0}")]
    Negative(i64),
    #[error("quantity overflow")]
    Overflow,
}

impl FromStr for Quantity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let v: i64 = s
            .parse()
            .map_err(|e: std::num::ParseIntError| e.to_string())?;
        Self::new(v).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_valid() {
        assert_eq!(Quantity::new(0).unwrap().get(), 0);
    }

    #[test]
    fn positive_is_valid() {
        assert_eq!(Quantity::new(42).unwrap().get(), 42);
    }

    #[test]
    fn negative_is_rejected() {
        assert!(Quantity::new(-1).is_err());
    }

    #[test]
    fn max_is_valid() {
        assert_eq!(Quantity::new(i64::MAX).unwrap().get(), i64::MAX);
    }

    #[test]
    fn default_is_zero() {
        assert_eq!(Quantity::default().get(), 0);
    }

    #[test]
    fn try_from_i64() {
        assert_eq!(Quantity::try_from(7i64).unwrap().get(), 7);
        assert!(Quantity::try_from(-3i64).is_err());
    }

    #[test]
    fn into_i64() {
        let q = Quantity::new(5).unwrap();
        assert_eq!(i64::from(q), 5);
    }

    #[test]
    fn serde_roundtrip() {
        let q = Quantity::new(10).unwrap();
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(json, "10");
        let back: Quantity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, q);
    }

    #[test]
    fn serde_rejects_negative() {
        let err = serde_json::from_str::<Quantity>("-5");
        assert!(err.is_err());
    }

    #[test]
    fn serde_accepts_float_integer() {
        let q: Quantity = serde_json::from_str("5.0").unwrap();
        assert_eq!(q.get(), 5);
    }

    #[test]
    fn serde_rejects_float_fraction() {
        let err = serde_json::from_str::<Quantity>("5.5");
        assert!(err.is_err());
    }

    #[test]
    fn serde_rejects_float_negative() {
        let err = serde_json::from_str::<Quantity>("-1.0");
        assert!(err.is_err());
    }
}
