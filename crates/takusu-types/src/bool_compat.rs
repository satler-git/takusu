//! serde helper for boolean fields that may be encoded as 0/1 on the wire.

use serde::{Deserialize, Deserializer, Serializer};

pub fn deserialize<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer)?;
    match v {
        serde_json::Value::Bool(b) => Ok(b),
        serde_json::Value::Number(n) => Ok(n.as_f64().map(|f| f != 0.0).unwrap_or(false)),
        serde_json::Value::Null => Ok(false),
        _ => Err(serde::de::Error::custom(
            "expected bool or number for boolean field",
        )),
    }
}

pub fn serialize<S>(value: &bool, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serde::Serialize::serialize(value, serializer)
}
