use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocalConfig {
    #[serde(default = "default_db_url")]
    pub db: String,
    #[serde(default = "default_bind_addr")]
    pub bind: String,
    #[serde(default = "default_worker_url")]
    pub worker_url: String,
    #[serde(default = "default_storage")]
    pub storage: StorageKind,
    #[serde(default)]
    pub jwt_secret: String,
}

fn default_db_url() -> String {
    "sqlite:./takusu.db".into()
}

fn default_bind_addr() -> String {
    "127.0.0.1:3000".into()
}

fn default_worker_url() -> String {
    String::new()
}

fn default_storage() -> StorageKind {
    StorageKind::Sqlite
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind {
    #[default]
    Sqlite,
    #[serde(alias = "cloudflare", alias = "d1")]
    Workers,
}

impl FromStr for StorageKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("sqlite") {
            Ok(StorageKind::Sqlite)
        } else if s.eq_ignore_ascii_case("workers")
            || s.eq_ignore_ascii_case("cloudflare")
            || s.eq_ignore_ascii_case("d1")
        {
            Ok(StorageKind::Workers)
        } else {
            Err(format!(
                "unknown storage kind `{s}` (expected sqlite, workers, cloudflare, or d1)"
            ))
        }
    }
}

impl std::fmt::Display for StorageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageKind::Sqlite => f.write_str("sqlite"),
            StorageKind::Workers => f.write_str("workers"),
        }
    }
}

impl LocalConfig {
    pub fn db_url(&self) -> &str {
        &self.db
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind
    }

    pub fn workers_url(&self) -> &str {
        &self.worker_url
    }
}
