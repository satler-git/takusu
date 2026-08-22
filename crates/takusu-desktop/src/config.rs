//! Desktop daemon configuration.
//!
//! Reads the shared `~/.config/takusu/config.toml` and extracts the `[desktop]`
//! section so the rest of the CLI config remains unaffected.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopConfig {
    pub theme: Theme,
    /// URL of the local takusu host. Defaults to `http://127.0.0.1:3000`.
    pub local_url: String,
    /// Bearer token for the local API. If empty, the daemon reads
    /// `TAKUSU_TOKEN` or the file from `TAKUSU_TOKEN_FILE`.
    pub token: String,
    /// Timezone for notification scheduling. Falls back to the top-level `tz`
    /// field or the system local timezone.
    pub tz: Option<String>,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            local_url: "http://127.0.0.1:3000".into(),
            token: String::new(),
            tz: None,
        }
    }
}

/// Full file layout. Unknown fields are ignored so the file can also hold
/// `takusu-cli` and `takusu-agent` settings.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    desktop: DesktopConfig,
    #[serde(default)]
    tz: Option<String>,
}

/// Merged runtime config with resolved defaults.
#[derive(Debug, Clone)]
pub struct Config {
    pub theme: Theme,
    pub local_url: String,
    pub token: String,
    pub tz: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            local_url: "http://127.0.0.1:3000".into(),
            token: String::new(),
            tz: "UTC".into(),
        }
    }
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

        if let Ok(theme) = std::env::var("TAKUSU_DESKTOP_THEME") {
            desktop.theme = match theme.as_str() {
                "dark" => Theme::Dark,
                "catppuccin" => Theme::Catppuccin,
                "aura-soft-dark" => Theme::AuraSoftDark,
                _ => Theme::Light,
            };
        }
        if let Ok(url) = std::env::var("TAKUSU_DESKTOP_LOCAL_URL") {
            desktop.local_url = url;
        }
        if let Ok(token) = std::env::var("TAKUSU_TOKEN") {
            desktop.token = token;
        } else if let Ok(file) = std::env::var("TAKUSU_TOKEN_FILE") {
            desktop.token = std::fs::read_to_string(file)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
        }

        // Do not fall back to a workers backend token: agent routes need a root
        // token, and using a worker token would fail at runtime with 401/403.
        if desktop.token.is_empty() {
            tracing::warn!("no desktop bearer token configured; agent routes may fail");
        }

        let tz = desktop.tz.clone().or(file.tz).unwrap_or_else(|| {
            jiff::tz::TimeZone::system()
                .iana_name()
                .unwrap_or("UTC")
                .to_string()
        });

        Ok(Config {
            theme: desktop.theme,
            local_url: desktop.local_url,
            token: desktop.token,
            tz,
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
}
