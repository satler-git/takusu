//! A newtype wrapper that serializes its inner value as a JSON string.
//!
//! Several DB columns (`tasks.depends`, `habit_steps.depends_on`,
//! `schedules.schedule`) store a JSON-encoded array as TEXT.  The wire format
//! is likewise a JSON string inside the outer JSON response — e.g.
//! `"depends": "[\"t1\",\"t2\"]"`.  `JsonString<T>` makes the Rust field type
//! `Vec<String>` / `Vec<ScheduleEntry>` (gaining compile-time type safety and
//! eliminating manual `serde_json::from_str` / `to_string` at every call site)
//! while preserving the existing wire and DB representation exactly.
//!
//! ## Serde
//!
//! `Serialize` produces a JSON string (`serializer.serialize_str(...)`).
//! `Deserialize` reads a JSON string and parses the inner JSON.
//!
//! ## sqlx (behind the `sqlx` feature)
//!
//! The type is TEXT-backed: `Encode` serializes the inner value to a JSON
//! string and binds it as `String`; `Decode` reads a `String` and parses.

use std::ops::{Deref, DerefMut};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A wrapper that serializes `T` as a JSON string on the wire and in the DB.
///
/// Common instantiations:
/// - `JsonString<Vec<String>>` — task / step dependency lists
/// - `JsonString<Vec<ScheduleEntry>>` — schedule entry arrays
#[derive(Debug, Clone, Default, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(with = "String")]
pub struct JsonString<T>(pub T);

impl<T> JsonString<T> {
    /// Create from the inner value.
    pub const fn new(inner: T) -> Self {
        JsonString(inner)
    }

    /// Consume and return the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Borrow the inner value.
    pub fn as_inner(&self) -> &T {
        &self.0
    }

    /// Mutably borrow the inner value.
    pub fn as_inner_mut(&mut self) -> &mut T {
        &mut self.0
    }

    /// Serialize the inner value to a JSON string.  Falls back to `"null"`
    /// on serialization failure (should not happen for well-formed types).
    pub fn to_json_string(&self) -> String
    where
        T: Serialize,
    {
        serde_json::to_string(&self.0).unwrap_or_else(|_| "null".to_string())
    }
}

// ── Conversions ───────────────────────────────────────────────────────────

impl<T> From<T> for JsonString<T> {
    fn from(inner: T) -> Self {
        JsonString(inner)
    }
}

impl<T> Deref for JsonString<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for JsonString<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// ── Serde ─────────────────────────────────────────────────────────────────

impl<T: Serialize> Serialize for JsonString<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let json =
            serde_json::to_string(&self.0).map_err(<S::Error as serde::ser::Error>::custom)?;
        serializer.serialize_str(&json)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for JsonString<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        let val = serde_json::from_str(&s).map_err(<D::Error as serde::de::Error>::custom)?;
        Ok(JsonString(val))
    }
}

// ── sqlx (TEXT-backed) ────────────────────────────────────────────────────

#[cfg(feature = "sqlx")]
mod sqlx_impl {
    use super::JsonString;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use sqlx::encode::IsNull;
    use sqlx::error::BoxDynError;
    use sqlx::{Database, Decode, Encode, Type};

    impl<DB: Database, T: 'static> Type<DB> for JsonString<T>
    where
        String: Type<DB>,
    {
        fn type_info() -> <DB as Database>::TypeInfo {
            <String as Type<DB>>::type_info()
        }

        fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
            <String as Type<DB>>::compatible(ty)
        }
    }

    impl<'q, DB: Database, T> Encode<'q, DB> for JsonString<T>
    where
        String: Encode<'q, DB> + Type<DB>,
        T: Serialize,
    {
        fn encode_by_ref(
            &self,
            buf: &mut <DB as Database>::ArgumentBuffer,
        ) -> Result<IsNull, BoxDynError> {
            let json = serde_json::to_string(&self.0)?;
            <String as Encode<'q, DB>>::encode(json, buf)
        }

        fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
            Some(<String as Type<DB>>::type_info())
        }
    }

    impl<'r, DB: Database, T: DeserializeOwned> Decode<'r, DB> for JsonString<T>
    where
        String: Decode<'r, DB>,
    {
        fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
            let s = <String as Decode<'r, DB>>::decode(value)?;
            let val = serde_json::from_str(&s)?;
            Ok(JsonString(val))
        }
    }
}

/// Type alias for the common case of a JSON-encoded `Vec<String>`.
///
/// Used by `TaskRow.depends` and `HabitStepRow.depends_on`.
pub type DependencyList = JsonString<Vec<String>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_list_default_is_empty_vec() {
        let dl = DependencyList::default();
        assert!(dl.is_empty());
        assert_eq!(dl.to_json_string(), "[]");
    }

    #[test]
    fn dependency_list_serialize_produces_json_string() {
        let dl = DependencyList::new(vec!["t1".to_string(), "t2".to_string()]);
        let json = serde_json::to_string(&dl).unwrap();
        assert_eq!(json, r#""[\"t1\",\"t2\"]""#);
    }

    #[test]
    fn dependency_list_deserialize_from_json_string() {
        let json = r#""[\"t1\",\"t2\"]""#;
        let dl: DependencyList = serde_json::from_str(json).unwrap();
        assert_eq!(dl.as_inner(), &vec!["t1".to_string(), "t2".to_string()]);
    }

    #[test]
    fn dependency_list_roundtrip() {
        let dl = DependencyList::new(vec!["a".to_string(), "b".to_string()]);
        let json = serde_json::to_string(&dl).unwrap();
        let back: DependencyList = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_inner(), dl.as_inner());
    }

    #[test]
    fn dependency_list_deref_to_vec() {
        let dl = DependencyList::new(vec!["x".to_string()]);
        assert_eq!(dl.len(), 1);
        assert_eq!(dl[0], "x");
    }

    #[test]
    fn json_string_empty_vec_serializes_as_empty_array_string() {
        let dl = DependencyList::default();
        let json = serde_json::to_string(&dl).unwrap();
        assert_eq!(json, r#""[]""#);
    }

    #[test]
    fn json_string_deserialize_empty_array_string() {
        let dl: DependencyList = serde_json::from_str(r#""[]""#).unwrap();
        assert!(dl.is_empty());
    }

    #[test]
    fn from_vec_creates_wrapper() {
        let dl = DependencyList::from(vec!["a".to_string()]);
        assert_eq!(dl.as_inner(), &vec!["a".to_string()]);
    }

    #[test]
    fn into_inner_consumes_wrapper() {
        let dl = DependencyList::new(vec!["a".to_string()]);
        let inner: Vec<String> = dl.into_inner();
        assert_eq!(inner, vec!["a".to_string()]);
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestEntry {
        id: u32,
        name: String,
    }

    #[test]
    fn json_string_with_struct_vec_roundtrip() {
        let entries = vec![
            TestEntry {
                id: 1,
                name: "a".into(),
            },
            TestEntry {
                id: 2,
                name: "b".into(),
            },
        ];
        let js = JsonString::new(entries.clone());
        let json = serde_json::to_string(&js).unwrap();
        // The outer JSON should be a string containing the inner JSON array.
        assert!(json.starts_with(r#""["#));
        let back: JsonString<Vec<TestEntry>> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_inner(), &entries);
    }
}
