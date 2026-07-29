use serde::Deserialize;
use std::path::PathBuf;
use toml_edit::DocumentMut;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct CliConfig {
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(default)]
    pub db: Option<String>,
    #[serde(default, alias = "url")]
    pub worker_url: Option<String>,
    #[serde(default, alias = "token")]
    pub workers_token: Option<String>,
    #[serde(default)]
    pub root_token: Option<String>,
    #[serde(default)]
    pub jwt_secret: Option<String>,
    #[serde(default)]
    pub tz: Option<String>,
    #[serde(default)]
    pub sleep_start: Option<String>,
    #[serde(default)]
    pub sleep_end: Option<String>,
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

pub fn load() -> CliConfig {
    let path = config_path();
    if !path.exists() {
        return CliConfig::default();
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return CliConfig::default(),
    };
    toml::from_str(&content).unwrap_or_default()
}

pub fn show() {
    let path = config_path();
    println!("Config file: {}", path.display());
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => println!("{}", content.trim()),
            Err(e) => println!("  (error reading: {e})"),
        }
    } else {
        println!("  (not found)");
    }
}

pub fn init() {
    let path = config_path();
    if path.exists() {
        println!("Config file already exists: {}", path.display());
        return;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let default_config = r#"# takusu CLI configuration
#
# storage = "sqlite"
# db = "sqlite:./takusu.db"
# worker_url = "http://127.0.0.1:8787"
# workers_token = "eyJ..."
# root_token = "eyJ..."
# jwt_secret = "<generate with `openssl rand -hex 32` or `takusu-types::jwt::generate_secret()`>"
# tz = "Asia/Tokyo"
# sleep_start = "22:00"
# sleep_end = "06:00"
"#;
    match std::fs::write(&path, default_config) {
        Ok(()) => println!("Created: {}", path.display()),
        Err(e) => eprintln!("Error creating config: {e}"),
    }
}

pub fn set(key: &str, value: &str) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let content = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;

    let (target_key, alias) = match key {
        "storage" => ("storage", None),
        "db" => ("db", None),
        "worker_url" | "url" => ("worker_url", Some("url")),
        "workers_token" | "token" => ("workers_token", Some("token")),
        "root_token" => ("root_token", None),
        "jwt_secret" => ("jwt_secret", None),
        "tz" => ("tz", None),
        "sleep_start" => ("sleep_start", None),
        "sleep_end" => ("sleep_end", None),
        _ => return Err(format!("unknown key: {key}")),
    };
    if let Some(alias) = alias {
        doc.remove(alias);
    }
    doc[target_key] = toml_edit::value(value);

    std::fs::write(&path, doc.to_string()).map_err(|e| e.to_string())
}
