use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use takusu_agent::{
    AgentConfig, AgentError, AgentSession, ApprovalRequest, Permissions, ToolError,
    UserInputAnswer, UserInputProvider, UserInputQuestion,
};
use takusu_client::Client;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::error::{AppError, BadRequestKind};

use crate::server::start_in_process;

pub async fn run(
    app: Arc<TakusuApp>,
    text: Option<String>,
    yes: bool,
    allow: Vec<String>,
    deny: Vec<String>,
) -> Result<(), AppError> {
    let session_permissions = parse_session_permissions(&allow, &deny)?;
    let local_server = start_in_process(app).await?;
    let mut config = AgentConfig::load()
        .map_err(|e| AppError::Internal(format!("failed to load agent config: {e}")))?;
    config.server.url = local_server.url;
    config.server.token = local_server.token;

    let client = Client::new(&config.server.url, &config.server.token);
    let session = takusu_agent::runner::build_session_with_provider(
        &config,
        client,
        Arc::new(ConsoleUserInputProvider),
    )
    .map_err(|e| AppError::Internal(format!("failed to build agent session: {e}")))?;

    if !session_permissions.allow.is_empty() {
        session.set_session_permissions(session_permissions)
            .map_err(|e| AppError::Internal(format!("failed to set session permissions: {e}")))?;
    }

    if let Some(text) = text {
        run_text(&session, &text, yes).await
    } else {
        run_repl(&session, yes).await
    }
}

fn parse_session_permissions(allow: &[String], deny: &[String]) -> Result<Permissions, AppError> {
    let mut permissions = Permissions::default();
    for key in allow {
        let (target, operation) = parse_permission_key(key)?;
        permissions.set(target, operation, true);
    }
    for key in deny {
        let (target, operation) = parse_permission_key(key)?;
        permissions.set(target, operation, false);
    }
    Ok(permissions)
}

fn parse_permission_key(key: &str) -> Result<(&str, &str), AppError> {
    if key.matches(':').count() != 1 {
        return Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "permission key must be 'target:operation' (got '{key}')"
        ))));
    }
    let mut parts = key.splitn(2, ':');
    let target = parts.next().filter(|s| !s.is_empty()).ok_or_else(|| {
        AppError::BadRequest(BadRequestKind::Other(format!(
            "invalid permission key '{key}'"
        )))
    })?;
    let operation = parts.next().filter(|s| !s.is_empty()).ok_or_else(|| {
        AppError::BadRequest(BadRequestKind::Other(format!(
            "invalid permission key '{key}'"
        )))
    })?;
    Ok((target, operation))
}

async fn run_text(session: &AgentSession, text: &str, yes: bool) -> Result<(), AppError> {
    let result = session.run_turn(text).await.map_err(agent_err)?;
    println!("{}", result.text);

    let schedule_dirty = if let Some(approval) = result.approval_request.as_ref() {
        display_approval(approval);
        let approve = if yes {
            true
        } else {
            ask_approve("Approve? (y/N): ")?
        };
        let res = session
            .resolve_approval(&approval.id, approve)
            .await
            .map_err(agent_err)?;
        if res.approved {
            println!("approved {} change(s)", res.changes.len());
            for receipt in &res.changes {
                println!(
                    "  {} {}: {}",
                    receipt.operation, receipt.target.target_type, receipt.target.target_id
                );
            }
        } else {
            println!("denied");
        }
        res.schedule_dirty
    } else {
        if !result.changes.is_empty() {
            eprintln!("changes:");
            for receipt in &result.changes {
                eprintln!(
                    "  {} {}: {}",
                    receipt.operation, receipt.target.target_type, receipt.target.target_id
                );
            }
        }
        result.schedule_dirty
    };

    if schedule_dirty {
        eprintln!("schedule dirty: true");
    }

    Ok(())
}

fn display_approval(req: &ApprovalRequest) {
    if !req.why.is_empty() {
        println!("Why: {}", req.why);
    }
    if !req.inferred_fields.is_empty() {
        println!("Inferred:");
        for field in &req.inferred_fields {
            println!("  {} = {} ({})", field.field, field.value, field.reason);
        }
    }
    if !req.warnings.is_empty() {
        println!("Warnings:");
        for warning in &req.warnings {
            println!("  - {warning}");
        }
    }
    println!("Changes:");
    for change in &req.changes {
        println!(
            "  {} {}: {}",
            change.operation, change.target, change.description
        );
    }
    println!("expires at: {}", req.expires_at);
}

fn ask_approve(label: &str) -> Result<bool, AppError> {
    print!("{label}");
    io::stdout()
        .flush()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let stdin = io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .transpose()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(line
        .map(|l| {
            let l = l.trim().to_lowercase();
            l == "y" || l == "yes"
        })
        .unwrap_or(false))
}

async fn run_repl(session: &AgentSession, yes: bool) -> Result<(), AppError> {
    loop {
        print!("> ");
        io::stdout()
            .flush()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let mut line = String::new();
        let n = io::stdin()
            .read_line(&mut line)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        if n == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }
        run_text(session, line, yes).await?;
    }
    Ok(())
}

#[derive(Debug)]
struct ConsoleUserInputProvider;

#[async_trait]
impl UserInputProvider for ConsoleUserInputProvider {
    async fn request(
        &self,
        _call_id: &str,
        questions: Vec<UserInputQuestion>,
    ) -> Result<Vec<UserInputAnswer>, ToolError> {
        tokio::task::spawn_blocking(move || {
            let mut answers = Vec::with_capacity(questions.len());
            for q in questions {
                eprintln!("ASR correction: original = \"{}\"", q.text);
                eprintln!("  purpose: {}", q.purpose);
                eprint!("  corrected text (empty to keep original): ");
                io::stdout()
                    .flush()
                    .map_err(|e| ToolError::Other(Box::new(e)))?;
                let mut line = String::new();
                io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| ToolError::Other(Box::new(e)))?;
                let text = line.trim();
                answers.push(UserInputAnswer {
                    text: if text.is_empty() { q.text } else { text.into() },
                });
            }
            Ok(answers)
        })
        .await
        .map_err(|e| ToolError::Other(Box::new(e)))?
    }

    async fn resolve(
        &self,
        _call_id: &str,
        _answers: Vec<UserInputAnswer>,
    ) -> Result<(), ToolError> {
        Ok(())
    }
}

fn agent_err(e: AgentError) -> AppError {
    AppError::Internal(e.to_string())
}

fn agent_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let mut p = PathBuf::from(h);
                p.push(".config");
                p
            })
        })
}

fn agent_config_path() -> PathBuf {
    let mut path = agent_config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("takusu");
    path.push("agent.toml");
    path
}

pub fn config_show() -> Result<(), AppError> {
    let path = agent_config_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| AppError::Internal(format!("failed to read agent config: {e}")))?;
        println!("{}\n{}", path.display(), content);
    } else {
        println!(
            "No agent config file at {}; defaults will be used.",
            path.display()
        );
    }
    Ok(())
}

pub fn config_set(key: &str, value: &str) -> Result<(), AppError> {
    if key == "llm.permissions" || key.starts_with("llm.permissions.") {
        return Err(AppError::BadRequest(BadRequestKind::Other(
            "use 'takusu agent config permissions set' to manage permissions".into(),
        )));
    }
    let path = agent_config_path();
    let mut doc = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| AppError::Internal(format!("failed to read agent config: {e}")))?;
        content.parse::<toml_edit::DocumentMut>().map_err(|e| {
            AppError::BadRequest(BadRequestKind::Other(format!("invalid agent config: {e}")))
        })?
    } else {
        toml_edit::DocumentMut::new()
    };

    set_toml_path(&mut doc, key, value).map_err(|e| {
        AppError::BadRequest(BadRequestKind::Other(format!("failed to set {key}: {e}")))
    })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Internal(format!("failed to create config dir: {e}")))?;
    }
    std::fs::write(&path, doc.to_string())
        .map_err(|e| AppError::Internal(format!("failed to write agent config: {e}")))?;

    println!("Updated agent config: {key} = {value}");
    Ok(())
}

fn parse_toml_edit_value(s: &str) -> toml_edit::Value {
    if let Ok(b) = s.parse::<bool>() {
        return b.into();
    }
    if let Ok(i) = s.parse::<i64>() {
        return i.into();
    }
    if let Ok(f) = s.parse::<f64>() {
        return f.into();
    }
    toml_edit::Value::String(toml_edit::Formatted::new(s.to_string()))
}

fn set_toml_path(doc: &mut toml_edit::DocumentMut, path: &str, value: &str) -> Result<(), String> {
    let keys: Vec<&str> = path.split('.').collect();
    if keys.is_empty() {
        return Err("empty key path".into());
    }

    let table = doc.as_table_mut();
    let mut item: &mut toml_edit::Item = &mut table[keys[0]];
    for key in &keys[1..keys.len() - 1] {
        if !item.is_table() {
            *item = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let t = item.as_table_mut().ok_or("expected table")?;
        item = &mut t[*key];
    }

    if keys.len() > 1 {
        if !item.is_table() {
            *item = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let t = item.as_table_mut().ok_or("expected table")?;
        t.insert(
            keys.last().unwrap(),
            toml_edit::value(parse_toml_edit_value(value)),
        );
    } else {
        table.insert(keys[0], toml_edit::value(parse_toml_edit_value(value)));
    }

    Ok(())
}

pub fn permissions_show() -> Result<(), AppError> {
    permissions_show_at(&agent_config_path())
}

fn permissions_show_at(path: &std::path::Path) -> Result<(), AppError> {
    if !path.exists() {
        println!(
            "No agent config file at {}; no permissions configured.",
            path.display()
        );
        return Ok(());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::Internal(format!("failed to read agent config: {e}")))?;
    let doc = content.parse::<toml_edit::DocumentMut>().map_err(|e| {
        AppError::BadRequest(BadRequestKind::Other(format!("invalid agent config: {e}")))
    })?;

    let Some(llm) = doc.as_table().get("llm") else {
        println!("No permissions configured.");
        return Ok(());
    };
    let llm_table = llm
        .as_table()
        .ok_or_else(|| AppError::BadRequest(BadRequestKind::Other("llm is not a table".into())))?;
    let Some(perms) = llm_table.get("permissions") else {
        println!("No permissions configured.");
        return Ok(());
    };
    let table = perms.as_table().ok_or_else(|| {
        AppError::BadRequest(BadRequestKind::Other(
            "llm.permissions is not a table".into(),
        ))
    })?;
    if table.is_empty() {
        println!("No permissions configured.");
        return Ok(());
    }
    for (key, item) in table.iter() {
        let value = item
            .as_value()
            .and_then(|v| v.as_bool())
            .map(|b| b.to_string())
            .unwrap_or_else(|| item.to_string().trim().to_string());
        println!("{key} = {value}");
    }
    Ok(())
}

pub fn permissions_set(key: &str, value: &str) -> Result<(), AppError> {
    permissions_set_at(&agent_config_path(), key, value)
}

fn permissions_set_at(path: &std::path::Path, key: &str, value: &str) -> Result<(), AppError> {
    parse_permission_key(key)?;
    let allowed = parse_permission_value(value)?;
    let mut doc = if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(|e| AppError::Internal(format!("failed to read agent config: {e}")))?;
        content.parse::<toml_edit::DocumentMut>().map_err(|e| {
            AppError::BadRequest(BadRequestKind::Other(format!("invalid agent config: {e}")))
        })?
    } else {
        toml_edit::DocumentMut::new()
    };

    let perms = ensure_permissions_table(&mut doc)?;
    perms.insert(key, toml_edit::value(allowed));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Internal(format!("failed to create config dir: {e}")))?;
    }
    std::fs::write(path, doc.to_string())
        .map_err(|e| AppError::Internal(format!("failed to write agent config: {e}")))?;

    println!("Updated permission: {key} = {allowed}");
    Ok(())
}

pub fn permissions_unset(key: &str) -> Result<(), AppError> {
    permissions_unset_at(&agent_config_path(), key)
}

fn permissions_unset_at(path: &std::path::Path, key: &str) -> Result<(), AppError> {
    parse_permission_key(key)?;
    if !path.exists() {
        println!("Permission not found: {key}");
        return Ok(());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::Internal(format!("failed to read agent config: {e}")))?;
    let mut doc = content.parse::<toml_edit::DocumentMut>().map_err(|e| {
        AppError::BadRequest(BadRequestKind::Other(format!("invalid agent config: {e}")))
    })?;

    let table = doc.as_table_mut();
    let Some(llm) = table.get_mut("llm") else {
        println!("Permission not found: {key}");
        return Ok(());
    };
    let llm_table = llm
        .as_table_mut()
        .ok_or_else(|| AppError::BadRequest(BadRequestKind::Other("llm is not a table".into())))?;
    let Some(perms) = llm_table.get_mut("permissions") else {
        println!("Permission not found: {key}");
        return Ok(());
    };
    let perms_table = perms.as_table_mut().ok_or_else(|| {
        AppError::BadRequest(BadRequestKind::Other(
            "llm.permissions is not a table".into(),
        ))
    })?;
    if perms_table.remove(key).is_some() {
        std::fs::write(path, doc.to_string())
            .map_err(|e| AppError::Internal(format!("failed to write agent config: {e}")))?;
        println!("Removed permission: {key}");
    } else {
        println!("Permission not found: {key}");
    }
    Ok(())
}

fn ensure_permissions_table(
    doc: &mut toml_edit::DocumentMut,
) -> Result<&mut toml_edit::Table, AppError> {
    let table = doc.as_table_mut();
    if !table.contains_key("llm") {
        table.insert("llm", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let llm = table.get_mut("llm").unwrap();
    if !llm.is_table() {
        return Err(AppError::BadRequest(BadRequestKind::Other(
            "llm is not a table".into(),
        )));
    }
    let llm_table = llm.as_table_mut().unwrap();
    if !llm_table.contains_key("permissions") {
        llm_table.insert(
            "permissions",
            toml_edit::Item::Table(toml_edit::Table::new()),
        );
    }
    let perms = llm_table.get_mut("permissions").unwrap();
    if !perms.is_table() {
        return Err(AppError::BadRequest(BadRequestKind::Other(
            "llm.permissions is not a table".into(),
        )));
    }
    Ok(perms.as_table_mut().unwrap())
}

fn parse_permission_value(s: &str) -> Result<bool, AppError> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("true")
        || t.eq_ignore_ascii_case("yes")
        || t.eq_ignore_ascii_case("y")
        || t == "1"
        || t.eq_ignore_ascii_case("on")
    {
        Ok(true)
    } else if t.eq_ignore_ascii_case("false")
        || t.eq_ignore_ascii_case("no")
        || t.eq_ignore_ascii_case("n")
        || t == "0"
        || t.eq_ignore_ascii_case("off")
    {
        Ok(false)
    } else {
        Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "expected boolean value, got '{s}'"
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    struct TempConfig(PathBuf);

    impl TempConfig {
        fn new() -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!("takusu-agent-test-{}", Uuid::now_v7()));
            p.push("takusu");
            p.push("agent.toml");
            Self(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempConfig {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    fn write_config(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn parse_permission_key_accepts_valid_keys() {
        assert_eq!(
            parse_permission_key("task:create").unwrap(),
            ("task", "create")
        );
        assert_eq!(parse_permission_key("*:*").unwrap(), ("*", "*"));
        assert_eq!(parse_permission_key("task:*").unwrap(), ("task", "*"));
        assert_eq!(parse_permission_key("*:create").unwrap(), ("*", "create"));
    }

    #[test]
    fn parse_permission_key_rejects_invalid_keys() {
        for key in ["invalid", "task", "task:", ":create", "task:create:sub"] {
            assert!(
                parse_permission_key(key).is_err(),
                "{key} should be rejected"
            );
        }
    }

    #[test]
    fn parse_permission_value_accepts_booleans() {
        for v in ["true", "True", "TRUE", "yes", "Yes", "Y", "1", "on", "ON"] {
            assert!(parse_permission_value(v).unwrap(), "{v} should be true");
        }
        for v in [
            "false", "False", "FALSE", "no", "No", "NO", "n", "0", "off", "OFF",
        ] {
            assert!(!parse_permission_value(v).unwrap(), "{v} should be false");
        }
    }

    #[test]
    fn parse_permission_value_rejects_garbage() {
        assert!(parse_permission_value("maybe").is_err());
    }

    #[test]
    fn parse_session_permissions_builds_map() {
        let perms = parse_session_permissions(
            &["task:create".into(), "schedule:generate".into()],
            &["task:delete".into()],
        )
        .unwrap();
        assert!(perms.is_allowed("task", "create"));
        assert!(perms.is_allowed("schedule", "generate"));
        assert!(!perms.is_allowed("task", "delete"));
        assert!(!perms.is_allowed("memory", "create"));
    }

    #[test]
    fn parse_session_permissions_deny_overrides_allow() {
        let perms =
            parse_session_permissions(&["task:create".into()], &["task:create".into()]).unwrap();
        assert!(!perms.is_allowed("task", "create"));
    }

    #[test]
    fn config_set_rejects_permissions_path() {
        assert!(config_set("llm.permissions.task:create", "true").is_err());
        assert!(config_set("llm.permissions", "{}").is_err());
    }

    #[test]
    fn permissions_set_and_unset_round_trip() {
        let tmp = TempConfig::new();
        permissions_set_at(tmp.path(), "task:create", "true").unwrap();
        permissions_set_at(tmp.path(), "schedule:generate", "false").unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("\"task:create\" = true"));
        assert!(content.contains("\"schedule:generate\" = false"));

        permissions_unset_at(tmp.path(), "task:create").unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(!content.contains("\"task:create\""));
    }

    #[test]
    fn permissions_show_errors_on_malformed_llm() {
        let tmp = TempConfig::new();
        write_config(tmp.path(), "llm = 123\n");
        assert!(permissions_show_at(tmp.path()).is_err());
    }

    #[test]
    fn permissions_show_is_ok_when_missing() {
        let tmp = TempConfig::new();
        assert!(permissions_show_at(tmp.path()).is_ok());
    }

    #[test]
    fn permissions_set_rejects_invalid_key() {
        let tmp = TempConfig::new();
        assert!(permissions_set_at(tmp.path(), "invalid", "true").is_err());
    }

    #[test]
    fn ensure_permissions_table_creates_missing_tables() {
        let mut doc = toml_edit::DocumentMut::new();
        let perms = ensure_permissions_table(&mut doc).unwrap();
        perms.insert("task:create", toml_edit::value(true));
        assert!(doc.to_string().contains("[llm.permissions]"));
    }
}
