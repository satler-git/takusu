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
    /// Timezone for notification scheduling. Falls back to the top-level `tz`
    /// field or the system local timezone.
    pub tz: Option<String>,
    /// Ambient-listening opt-in and wake-word evaluation log.
    pub ambient: DesktopAmbientConfig,
}

/// Desktop-specific ambient settings (WI-21). The wake-word model and backend
/// live in the shared agent audio config; this section controls whether the
/// daemon starts listening on launch and where it writes the evaluation log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopAmbientConfig {
    /// Start ambient listening when the daemon launches. The user must also
    /// enable `audio.ambient.enabled` in the agent config.
    pub auto_start: bool,
    /// Path to the wake-word evaluation log. Empty uses the XDG state dir.
    pub log_path: String,
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
    pub ambient: DesktopAmbientConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            local_url: "http://127.0.0.1:3000".into(),
            token: String::new(),
            tz: "UTC".into(),
            ambient: DesktopAmbientConfig::default(),
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
        if let Ok(v) = std::env::var("TAKUSU_DESKTOP_AMBIENT_AUTO_START") {
            desktop.ambient.auto_start = matches!(v.as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("TAKUSU_DESKTOP_AMBIENT_LOG_PATH") {
            desktop.ambient.log_path = v;
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
            ambient: desktop.ambient.clone(),
        })
    }

    /// Resolved path for the ambient wake-word evaluation log.
    ///
    /// Empty paths resolve to `state_dir/data_dir/home_dir/takusu/ambient-wake.log`.
    /// Relative explicit paths resolve against the same private base so they do
    /// not accidentally land in a world-readable working directory.
    pub fn ambient_log_path(&self) -> Result<PathBuf, ConfigError> {
        let base = dirs::state_dir()
            .or_else(dirs::data_dir)
            .or_else(dirs::home_dir)
            .ok_or_else(|| {
                ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no state, data, or home directory available for ambient log",
                ))
            })?;

        if self.ambient.log_path.is_empty() {
            return Ok(base.join("takusu").join("ambient-wake.log"));
        }

        let path = PathBuf::from(&self.ambient.log_path);
        if path.is_absolute() {
            return Ok(path);
        }
        Ok(base.join("takusu").join(path))
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
