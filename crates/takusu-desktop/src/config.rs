//! Desktop daemon configuration.
//!
//! Reads the shared `~/.config/takusu/config.toml` and extracts the `[desktop]`
//! section so the rest of the CLI config remains unaffected.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use takusu_local_lib::config::StorageKind;

/// Active theme, matching mobile's `AppTheme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    Light,
    Dark,
    Catppuccin,
    #[serde(rename = "aura-soft-dark")]
    AuraSoftDark,
}

impl Theme {
    /// Label used in the tray menu and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::Catppuccin => "catppuccin",
            Theme::AuraSoftDark => "aura-soft-dark",
        }
    }
}

/// `[desktop]` section of `~/.config/takusu/config.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub theme: Theme,
    /// URL of the local takusu host. When empty (the default), the daemon
    /// starts an embedded `takusu-local` server on a random loopback port.
    /// Set this explicitly (or via `TAKUSU_DESKTOP_LOCAL_URL`) to use an
    /// existing external server.
    pub local_url: String,
    /// Bearer token for the local API. If empty, the daemon reads
    /// `TAKUSU_TOKEN` or the file from `TAKUSU_TOKEN_FILE`.
    pub token: String,
}

/// Top-level fields shared with `takusu-cli`.
#[derive(Debug, Default, Deserialize)]
struct SharedConfig {
    #[serde(default)]
    storage: Option<StorageKind>,
    #[serde(default)]
    db: Option<String>,
    #[serde(default, alias = "url")]
    worker_url: Option<String>,
    #[serde(default, alias = "token")]
    workers_token: Option<String>,
    #[serde(default)]
    root_token: Option<String>,
    #[serde(default)]
    jwt_secret: Option<String>,
    #[serde(default)]
    tz: Option<String>,
}

/// Full file layout. Unknown fields are ignored so the file can also hold
/// `takusu-cli` and `takusu-agent` settings.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    desktop: DesktopConfig,
    #[serde(flatten)]
    shared: SharedConfig,
}

/// Merged runtime config with resolved defaults.
#[derive(Debug, Clone)]
pub struct Config {
    pub theme: Theme,
    pub local_url: String,
    pub token: String,
    pub tz: String,
    pub storage: StorageKind,
    pub db: String,
    pub worker_url: String,
    pub workers_token: String,
    pub root_token: String,
    pub jwt_secret: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            local_url: String::new(),
            token: String::new(),
            tz: "UTC".into(),
            storage: StorageKind::default(),
            db: String::new(),
            worker_url: String::new(),
            workers_token: String::new(),
            root_token: String::new(),
            jwt_secret: String::new(),
        }
    }
}

/// Return the value of `var` if it is set and non-empty.
fn env_var(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}

/// Read the file pointed to by `var` if it is set and non-empty.
fn env_file(var: &str) -> Result<Option<String>, ConfigError> {
    std::env::var(var)
        .ok()
        .filter(|s| !s.is_empty())
        .map(|p| {
            std::fs::read_to_string(&p)
                .map(|s| s.trim().to_string())
                .map_err(ConfigError::Io)
        })
        .transpose()
}

/// Treat empty strings in an `Option<String>` as `None`.
fn maybe(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

impl Config {
    /// Load from `~/.config/takusu/config.toml` and environment overrides.
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_path()?;
        let file: FileConfig = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            toml::from_str(&content)?
        } else {
            FileConfig::default()
        };

        let mut desktop = file.desktop;

        if let Some(theme) = env_var("TAKUSU_DESKTOP_THEME") {
            desktop.theme = match theme.as_str() {
                "dark" => Theme::Dark,
                "catppuccin" => Theme::Catppuccin,
                "aura-soft-dark" => Theme::AuraSoftDark,
                _ => Theme::Light,
            };
        }
        if let Some(url) = env_var("TAKUSU_DESKTOP_LOCAL_URL") {
            desktop.local_url = url;
        }
        if let Some(token) = env_var("TAKUSU_TOKEN") {
            desktop.token = token;
        } else if let Some(token) = env_file("TAKUSU_TOKEN_FILE")? {
            desktop.token = token;
        }

        // Storage precedence:
        // 1. TAKUSU_STORAGE env
        // 2. If TAKUSU_DB is set without TAKUSU_STORAGE, force sqlite
        //    (matches takusu-cli and takusu-local)
        // 3. Top-level `storage` in config.toml
        // 4. Default sqlite
        let env_db = env_var("TAKUSU_DB");
        let storage = if let Some(v) = env_var("TAKUSU_STORAGE") {
            v.parse::<StorageKind>()
                .map_err(|e| ConfigError::Invalid("TAKUSU_STORAGE".into(), e))?
        } else if env_db.is_some() {
            StorageKind::Sqlite
        } else {
            file.shared.storage.unwrap_or_default()
        };

        let db = env_db
            .or(maybe(file.shared.db.clone()))
            .unwrap_or_default();

        let worker_url = env_var("TAKUSU_WORKERS_URL")
            .or(env_var("TAKUSU_WORKER_URL"))
            .or(maybe(file.shared.worker_url.clone()))
            .unwrap_or_default();

        let root_token = env_var("TAKUSU_ROOT_TOKEN")
            .or(maybe(file.shared.root_token.clone()))
            .unwrap_or_default();

        // workers_token falls back to root_token, mirroring takusu-cli and
        // takusu-local.
        let workers_token = env_var("TAKUSU_WORKERS_TOKEN")
            .or(env_file("TAKUSU_WORKERS_TOKEN_FILE")?)
            .or(maybe(file.shared.workers_token.clone()))
            .or(if root_token.is_empty() { None } else { Some(root_token.clone()) })
            .unwrap_or_default();

        let jwt_secret = env_var("TAKUSU_JWT_SECRET")
            .or(env_file("TAKUSU_JWT_SECRET_FILE")?)
            .or(maybe(file.shared.jwt_secret.clone()))
            .unwrap_or_default();

        let tz = maybe(file.shared.tz.clone()).unwrap_or_else(|| {
            jiff::tz::TimeZone::system()
                .iana_name()
                .unwrap_or("UTC")
                .to_string()
        });

        // Fall back to the root token for the local API bearer token, mirroring
        // how `takusu-cli` treats `root_token` as an authoritative credential.
        let token = if desktop.token.is_empty() && !root_token.is_empty() {
            root_token.clone()
        } else {
            desktop.token
        };

        Ok(Config {
            theme: desktop.theme,
            local_url: desktop.local_url,
            token,
            tz,
            storage,
            db,
            worker_url,
            workers_token,
            root_token,
            jwt_secret,
        })
    }

    /// Path to the shared config file.
    pub fn path() -> Result<PathBuf, ConfigError> {
        config_path()
    }
}

fn config_path() -> Result<PathBuf, ConfigError> {
    let base = if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(dir)
    } else {
        dirs::config_dir().ok_or_else(|| {
            ConfigError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "XDG_CONFIG_HOME or a home directory is required",
            ))
        })?
    };
    Ok(base.join("takusu").join("config.toml"))
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid value for {0}: {1}")]
    Invalid(String, String),
}
