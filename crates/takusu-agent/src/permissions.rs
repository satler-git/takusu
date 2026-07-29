use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use takusu_types::EnumLabel;

use crate::tool::{ChangeOperation, TargetKind};

/// Typed key for a permission entry.
///
/// `None` means wildcard (`*`). The wire format (JSON/TOML) is still the flat
/// string `"target:operation"` so mobile clients and config files are
/// unaffected; the typed key just removes string-allocation and typo risk on
/// the Rust side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PermissionKey {
    pub target: Option<TargetKind>,
    pub operation: Option<ChangeOperation>,
}

impl PermissionKey {
    pub const fn new(target: Option<TargetKind>, operation: Option<ChangeOperation>) -> Self {
        Self { target, operation }
    }

    /// Exact (non-wildcard) key for a `(target, operation)` pair.
    pub const fn exact(target: TargetKind, operation: ChangeOperation) -> Self {
        Self::new(Some(target), Some(operation))
    }
}

impl fmt::Display for PermissionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.target {
            Some(t) => f.write_str(t.as_str())?,
            None => f.write_str("*")?,
        }
        f.write_str(":")?;
        match self.operation {
            Some(o) => f.write_str(o.as_str()),
            None => f.write_str("*"),
        }
    }
}

impl FromStr for PermissionKey {
    type Err = PermissionKeyParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Single scan: splitn yields at most two parts; a string with no `:`
        // yields one part (operation stays None -> invalid), and a string
        // with multiple `:`s yields a second part that still contains `:`,
        // which we reject after parsing.
        let mut parts = s.splitn(2, ':');
        let target_str = parts.next().unwrap_or("");
        let operation_str = parts.next().unwrap_or("");
        if target_str.is_empty() || operation_str.is_empty() {
            return Err(PermissionKeyParseError::InvalidFormat(s.to_owned()));
        }
        let target = if target_str == "*" {
            None
        } else {
            Some(TargetKind::from_str(target_str).map_err(PermissionKeyParseError::UnknownTarget)?)
        };
        let operation = if operation_str == "*" {
            None
        } else {
            Some(
                ChangeOperation::from_str(operation_str)
                    .map_err(PermissionKeyParseError::UnknownOperation)?,
            )
        };
        Ok(Self { target, operation })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PermissionKeyParseError {
    #[error("permission key must be 'target:operation' (got '{0}')")]
    InvalidFormat(String),
    #[error("{0}")]
    UnknownTarget(takusu_types::UnknownLabel),
    #[error("{0}")]
    UnknownOperation(takusu_types::UnknownLabel),
}

/// Permission map for auto-approving proposed changes.
///
/// Serialized as a flat map of `"target:operation"` -> bool so that mobile
/// clients can send it directly without wrapping it in an `allow` field.
/// Internally the keys are typed (`PermissionKey`) to prevent typos and avoid
/// per-lookup string allocation.
#[derive(Debug, Clone, Default)]
pub struct Permissions {
    pub allow: BTreeMap<PermissionKey, bool>,
}

impl Permissions {
    /// Returns the explicitly configured value for a `(target, operation)`
    /// pair, checking wildcard patterns from most specific to least specific.
    ///
    /// Lookup order:
    /// 1. `target:operation`
    /// 2. `target:*`
    /// 3. `*:operation`
    /// 4. `*:*`
    pub fn resolve(&self, target: TargetKind, operation: ChangeOperation) -> Option<bool> {
        if let Some(&allowed) = self.allow.get(&PermissionKey::exact(target, operation)) {
            return Some(allowed);
        }
        if let Some(&allowed) = self.allow.get(&PermissionKey::new(Some(target), None)) {
            return Some(allowed);
        }
        if let Some(&allowed) = self.allow.get(&PermissionKey::new(None, Some(operation))) {
            return Some(allowed);
        }
        if let Some(&allowed) = self.allow.get(&PermissionKey::new(None, None)) {
            return Some(allowed);
        }
        None
    }

    pub fn is_allowed(&self, target: TargetKind, operation: ChangeOperation) -> bool {
        self.resolve(target, operation).unwrap_or(false)
    }

    pub fn set(
        &mut self,
        target: impl Into<Option<TargetKind>>,
        operation: impl Into<Option<ChangeOperation>>,
        allowed: bool,
    ) {
        self.allow
            .insert(PermissionKey::new(target.into(), operation.into()), allowed);
    }
}

impl Serialize for Permissions {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.allow.len()))?;
        for (key, &val) in &self.allow {
            map.serialize_entry(&key.to_string(), &val)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let flat: BTreeMap<String, bool> = BTreeMap::deserialize(deserializer)?;
        let mut allow = BTreeMap::new();
        for (k, v) in flat {
            let key = PermissionKey::from_str(&k).map_err(de::Error::custom)?;
            allow.insert(key, v);
        }
        Ok(Self { allow })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_json_deserializes_into_permissions() {
        // Mobile sends permissions as a flat map, not wrapped in `allow`.
        let flat = r#"{"schedule:generate":true}"#;
        let parsed: Permissions = serde_json::from_str(flat).unwrap();
        assert!(parsed.is_allowed(TargetKind::Schedule, ChangeOperation::Generate));
    }

    #[test]
    fn wildcard_lookup_order() {
        let mut perms = Permissions::default();
        // `*:*` allows everything.
        perms.set(None, None, true);
        assert!(perms.is_allowed(TargetKind::Task, ChangeOperation::Create));
        // `task:*` overrides `*:*` for task target.
        perms.set(Some(TargetKind::Task), None, false);
        assert!(!perms.is_allowed(TargetKind::Task, ChangeOperation::Create));
        assert!(perms.is_allowed(TargetKind::Habit, ChangeOperation::Create));
        // `task:create` overrides `task:*` for exact pair.
        perms.set(Some(TargetKind::Task), Some(ChangeOperation::Create), true);
        assert!(perms.is_allowed(TargetKind::Task, ChangeOperation::Create));
        assert!(!perms.is_allowed(TargetKind::Task, ChangeOperation::Delete));
    }

    #[test]
    fn round_trips_through_json() {
        let mut perms = Permissions::default();
        perms.set(Some(TargetKind::Schedule), Some(ChangeOperation::Generate), true);
        perms.set(Some(TargetKind::Task), None, false);
        let json = serde_json::to_string(&perms).unwrap();
        assert!(json.contains("\"schedule:generate\":true"));
        assert!(json.contains("\"task:*\":false"));
        let back: Permissions = serde_json::from_str(&json).unwrap();
        assert!(back.is_allowed(TargetKind::Schedule, ChangeOperation::Generate));
        assert!(!back.is_allowed(TargetKind::Task, ChangeOperation::Create));
    }

    #[test]
    fn permission_key_display_and_parse_round_trip() {
        for key in [
            PermissionKey::exact(TargetKind::Task, ChangeOperation::Create),
            PermissionKey::new(Some(TargetKind::Task), None),
            PermissionKey::new(None, Some(ChangeOperation::Create)),
            PermissionKey::new(None, None),
        ] {
            let s = key.to_string();
            assert_eq!(PermissionKey::from_str(&s).unwrap(), key);
        }
    }

    #[test]
    fn permission_key_parse_rejects_invalid() {
        for bad in ["invalid", "task", "task:", ":create", "task:create:sub"] {
            assert!(PermissionKey::from_str(bad).is_err(), "{bad} should be rejected");
        }
    }
}
