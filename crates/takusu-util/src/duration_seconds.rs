//! Serde adapter for `std::time::Duration` stored as whole seconds.

use serde::{Deserialize, Deserializer, Serializer};
use std::time::Duration;

/// Serialize a `Duration` as the number of whole seconds.
pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(value.as_secs())
}

/// Deserialize a `Duration` from a `u64` number of seconds.
pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let secs = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs))
}

/// Adapter for `Option<Duration>` fields.
pub mod option {
    use super::*;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(v) => super::serialize(v, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt = Option::<u64>::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Config {
        #[serde(with = "super")]
        timeout: Duration,
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct OptionalConfig {
        #[serde(with = "super::option", default)]
        timeout: Option<Duration>,
    }

    #[test]
    fn serialize_seconds() {
        let c = Config {
            timeout: Duration::from_secs(60),
        };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"timeout":60}"#);
    }

    #[test]
    fn deserialize_seconds() {
        let c: Config = serde_json::from_str(r#"{"timeout":90}"#).unwrap();
        assert_eq!(c.timeout, Duration::from_secs(90));
    }

    #[test]
    fn optional_some() {
        let c: OptionalConfig = serde_json::from_str(r#"{"timeout":30}"#).unwrap();
        assert_eq!(c.timeout, Some(Duration::from_secs(30)));
    }

    #[test]
    fn optional_null() {
        let c: OptionalConfig = serde_json::from_str(r#"{"timeout":null}"#).unwrap();
        assert_eq!(c.timeout, None);
    }

    #[test]
    fn optional_missing() {
        let c: OptionalConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(c.timeout, None);
    }
}
