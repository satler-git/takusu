use std::path::PathBuf;

use serde::Deserialize;
use takusu_local_lib::config::LocalConfig;

/// Web server configuration. Mirrors the fields of the shared CLI config
/// (`~/.config/takusu/config.toml`) that the server needs, then maps onto
/// `LocalConfig`. Unknown keys are ignored so the file can be shared with the
/// CLI and other clients.
#[derive(Debug, Default, Deserialize)]
pub struct WebConfig {
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(default)]
    pub db: Option<String>,
    #[serde(default, alias = "url")]
    pub worker_url: Option<String>,
    #[serde(default)]
    pub jwt_secret: Option<String>,
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default, alias = "token")]
    pub workers_token: Option<String>,
    #[serde(default)]
    pub root_token: Option<String>,
}

/// Fully resolved settings: the `LocalConfig` consumed by `takusu-local-lib`
/// plus the workers/root tokens used to authenticate against the workers
/// backend. Tokens are resolved from env first, then the config file, matching
/// the CLI's precedence.
// `Debug` is intentionally not derived so the tokens cannot leak through
// `{:?}` formatting in future logging.
#[derive(Clone)]
pub struct Settings {
    pub local: LocalConfig,
    pub workers_token: String,
    pub root_token: String,
}

pub fn config_path() -> PathBuf {
    let base = if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(dir)
    } else {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config")
    };
    base.join("takusu").join("config.toml")
}

/// Load the shared TOML config and fold in `TAKUSU_*` env overrides. Storage
/// selection mirrors the CLI: `TAKUSU_STORAGE` wins, then `TAKUSU_DB` (which
/// implies sqlite), then the config file.
pub fn load() -> Settings {
    let mut cfg = LocalConfig::default();
    let mut file_workers_token: Option<String> = None;
    let mut file_root_token: Option<String> = None;

    let path = config_path();
    if path.exists()
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(file) = toml::from_str::<WebConfig>(&content)
    {
        if let Some(v) = file.storage {
            cfg.storage = v;
        }
        if let Some(v) = file.db {
            cfg.db = v;
        }
        if let Some(v) = file.worker_url {
            cfg.worker_url = v;
        }
        if let Some(v) = file.jwt_secret {
            cfg.jwt_secret = v;
        }
        if let Some(v) = file.bind {
            cfg.bind = v;
        }
        file_workers_token = file.workers_token.filter(|s| !s.is_empty());
        file_root_token = file.root_token.filter(|s| !s.is_empty());
    }

    let env_storage = std::env::var("TAKUSU_STORAGE")
        .ok()
        .filter(|s| !s.is_empty());
    let env_db = std::env::var("TAKUSU_DB").ok().filter(|s| !s.is_empty());

    if let Some(v) = env_storage {
        cfg.storage = v;
    } else if env_db.is_some() {
        // TAKUSU_DB only makes sense for the sqlite backend, so prefer it over a
        // config file that may point at production workers.
        cfg.storage = "sqlite".to_string();
    }
    if let Some(v) = env_db {
        cfg.db = v;
    }

    if let Ok(v) = std::env::var("TAKUSU_BIND")
        && !v.is_empty()
    {
        cfg.bind = v;
    }
    if let Ok(v) = std::env::var("TAKUSU_WORKERS_URL")
        && !v.is_empty()
    {
        cfg.worker_url = v;
    } else if let Ok(v) = std::env::var("TAKUSU_WORKER_URL")
        && !v.is_empty()
    {
        cfg.worker_url = v;
    }
    if let Ok(v) = std::env::var("TAKUSU_JWT_SECRET")
        && !v.is_empty()
    {
        cfg.jwt_secret = v;
    }

    let env_workers = std::env::var("TAKUSU_WORKERS_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    let env_root = std::env::var("TAKUSU_ROOT_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());

    // Mirror the CLI: workers token falls back through env workers -> config
    // workers -> env root -> config root.
    let workers_token = env_workers
        .clone()
        .or_else(|| file_workers_token.clone())
        .or_else(|| env_root.clone())
        .or_else(|| file_root_token.clone())
        .unwrap_or_default();
    let root_token = env_root.or(file_root_token).unwrap_or_default();

    Settings {
        local: cfg,
        workers_token,
        root_token,
    }
}
