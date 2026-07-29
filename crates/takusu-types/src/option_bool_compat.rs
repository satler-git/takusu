//! serde helper for optional boolean fields that may be encoded as 0/1/null on the wire.

use serde::{Deserialize, Deserializer, Serializer};

pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Bool(b) => Ok(Some(b)),
        serde_json::Value::Number(n) => Ok(Some(n.as_f64().map(|f| f != 0.0).unwrap_or(false))),
        _ => Err(serde::de::Error::custom(
            "expected bool, number, or null for optional boolean field",
        )),
    }
}

pub fn serialize<S>(value: &Option<bool>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(b) => serde::Serialize::serialize(b, serializer),
        None => serializer.serialize_none(),
    }
}
