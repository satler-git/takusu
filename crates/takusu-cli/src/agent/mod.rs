use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

pub(crate) mod repl;
use takusu_agent::tool::{ProposalDecision, ProposedChange};
use takusu_agent::{
    AgentConfig, AgentError, AgentSession, ApprovalRequest, Permissions, SessionSnapshot,
    ToolError, TurnEvent, UserInputAnswer, UserInputProvider, UserInputQuestion,
};
use takusu_client::Client;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::error::{AppError, BadRequestKind};

use crate::server::start_in_process;

pub struct AgentRunArgs {
    pub app: Arc<TakusuApp>,
    pub text: Option<String>,
    pub yes: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub plain: bool,
    pub continue_session: bool,
    pub new_session: bool,
    pub voice: bool,
    pub voice_session: bool,
}

pub async fn run(args: AgentRunArgs) -> Result<(), AppError> {
    let session_permissions = parse_session_permissions(&args.allow, &args.deny)?;
    let local_server = start_in_process(args.app).await?;
    let mut config = AgentConfig::load()
        .map_err(|e| AppError::Internal(format!("failed to load agent config: {e}")))?;
    config.server.url = local_server.url;
    config.server.token = local_server.token;

    let client = Client::new(&config.server.url, &config.server.token);
    let plain = args.plain || !atty::is(atty::Stream::Stdout);

    if args.voice {
        let client = client.clone();
        let mut session = takusu_agent::runner::build_session(&config, client)
            .map_err(|e| AppError::Internal(format!("failed to build agent session: {e}")))?;
        apply_permissions_and_resume(
            &mut session,
            &session_permissions,
            args.new_session || !args.continue_session,
        )?;
        let session = Arc::new(session);
        if args.voice_session {
            if args.yes {
                eprintln!(
                    "warning: --yes has no effect with --voice-session; continuous sessions defer approvals to the surface"
                );
            }
            // Continuous multi-turn session: record → act → speak → listen
            // until the user stops talking or the idle timeout elapses.
            let mut output = String::new();
            let mut last_asr = false;
            let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);
            let outcome = takusu_agent::runner::run_voice_session(
                Arc::clone(&session),
                takusu_agent::InputOrigin::Voice,
                takusu_agent::VoiceSessionConfig::default(),
                stop_rx,
                move |event| emit_stream_event(event, true, &mut output, &mut last_asr),
                |_callback| {},
            )
            .await
            .map_err(agent_err)?;
            save_session_snapshot(&session)?;
            eprintln!("voice session ended: {outcome:?}");
            return Ok(());
        }
        let mut output = String::new();
        let mut last_asr = false;
        let result =
            takusu_agent::runner::run_audio(Arc::clone(&session), false, args.yes, move |event| {
                emit_stream_event(event, true, &mut output, &mut last_asr)
            })
            .await
            .map_err(agent_err);
        save_session_snapshot(&session)?;
        return result;
    }

    if let Some(text) = args.text {
        let mut session = takusu_agent::runner::build_session_with_provider(
            &config,
            client,
            Arc::new(ConsoleUserInputProvider),
        )
        .map_err(|e| AppError::Internal(format!("failed to build agent session: {e}")))?;
        apply_permissions_and_resume(
            &mut session,
            &session_permissions,
            args.new_session || !args.continue_session,
        )?;
        let result = run_text(&session, &text, args.yes, args.plain).await;
        save_session_snapshot(&session)?;
        return result;
    }

    if plain {
        let mut session = takusu_agent::runner::build_session_with_provider(
            &config,
            client,
            Arc::new(ConsoleUserInputProvider),
        )
        .map_err(|e| AppError::Internal(format!("failed to build agent session: {e}")))?;
        apply_permissions_and_resume(&mut session, &session_permissions, args.new_session)?;
        let result = run_repl(&session, args.yes, true).await;
        save_session_snapshot(&session)?;
        return result;
    }

    let (question_tx, question_rx) = mpsc::unbounded_channel();
    let mut session = takusu_agent::runner::build_session_with_provider(
        &config,
        client,
        Arc::new(repl::ReplUserInputProvider::new(question_tx)),
    )
    .map_err(|e| AppError::Internal(format!("failed to build agent session: {e}")))?;
    apply_permissions_and_resume(&mut session, &session_permissions, args.new_session)?;
    let session = Arc::new(tokio::sync::Mutex::new(session));
    let result = repl::run(Arc::clone(&session), question_rx, args.yes).await;
    let guard = session.lock().await;
    save_session_snapshot(&guard)?;
    result
}

pub(crate) fn agent_state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let mut p = PathBuf::from(h);
                p.push(".local");
                p.push("state");
                p
            })
        })
        .unwrap_or_else(std::env::temp_dir)
}

pub(crate) fn agent_session_path() -> PathBuf {
    let mut path = agent_state_dir();
    path.push("takusu");
    path.push("agent-session.json");
    path
}

pub(crate) fn load_session_snapshot() -> Result<Option<SessionSnapshot>, AppError> {
    let path = agent_session_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| AppError::Internal(format!("failed to read session snapshot: {e}")))?;
    if content.trim().is_empty() {
        return Ok(None);
    }
    let snapshot: SessionSnapshot = serde_json::from_str(&content)
        .map_err(|e| AppError::Internal(format!("invalid session snapshot: {e}")))?;
    Ok(Some(snapshot))
}

pub(crate) fn save_session_snapshot(session: &AgentSession) -> Result<(), AppError> {
    let path = agent_session_path();
    let snapshot = session
        .snapshot()
        .map_err(|e| AppError::Internal(format!("failed to snapshot agent session: {e}")))?;
    let content = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| AppError::Internal(format!("failed to serialize session snapshot: {e}")))?;
    write_private_file(&path, &content)
        .map_err(|e| AppError::Internal(format!("failed to write session snapshot: {e}")))?;
    Ok(())
}

pub(crate) fn write_private_file(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        std::io::Write::write_all(&mut file, content.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content)?;
    }
    Ok(())
}

fn apply_permissions_and_resume(
    session: &mut AgentSession,
    session_permissions: &Permissions,
    skip_resume: bool,
) -> Result<(), AppError> {
    if !session_permissions.allow.is_empty() {
        session
            .set_session_permissions(session_permissions.clone())
            .map_err(|e| AppError::Internal(format!("failed to set session permissions: {e}")))?;
    }
    if !skip_resume && let Some(snapshot) = load_session_snapshot()? {
        session
            .restore_from_snapshot(&snapshot)
            .map_err(|e| AppError::Internal(format!("failed to restore agent session: {e}")))?;
        if !session_permissions.allow.is_empty() {
            session
                .set_session_permissions(session_permissions.clone())
                .map_err(|e| {
                    AppError::Internal(format!("failed to set session permissions: {e}"))
                })?;
        }
    }
    Ok(())
}

fn parse_session_permissions(allow: &[String], deny: &[String]) -> Result<Permissions, AppError> {
    let mut permissions = Permissions::default();
    for key in allow {
        let parsed = takusu_agent::PermissionKey::from_str(key)
            .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
        permissions.set(parsed.target, parsed.operation, true);
    }
    for key in deny {
        let parsed = takusu_agent::PermissionKey::from_str(key)
            .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
        permissions.set(parsed.target, parsed.operation, false);
    }
    Ok(permissions)
}

/// Validate a permission key string using the typed `PermissionKey` parser.
///
/// This catches unknown target/operation labels (e.g. `foo:bar`) at
/// config-write time rather than deferring the failure to `AgentConfig::load()`.
fn validate_permission_key(key: &str) -> Result<(), AppError> {
    takusu_agent::PermissionKey::from_str(key)
        .map(|_| ())
        .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))
}

async fn run_text(
    session: &AgentSession,
    text: &str,
    yes: bool,
    plain: bool,
) -> Result<(), AppError> {
    let tty = atty::is(atty::Stream::Stdout);
    let stream = !plain && tty;

    let mut output = String::new();
    let mut last_asr = false;
    let result = session
        .run_turn_stream(
            text,
            |event| emit_stream_event(event, stream, &mut output, &mut last_asr),
            |_block| {},
        )
        .await
        .map_err(agent_err)?;
    if stream && last_asr {
        eprintln!();
    }
    if stream && !output.is_empty() && !output.ends_with('\n') {
        println!();
    } else if !stream && !result.text.is_empty() {
        println!("{}", result.text);
    }

    let schedule_dirty = if let Some(approval) = result.approval_request.as_ref() {
        let decision = if yes {
            AskApprovalResult {
                approve: true,
                proposals: None,
                always_ids: Vec::new(),
            }
        } else {
            ask_approval_proposals(approval, stream)?
        };
        if !decision.always_ids.is_empty() {
            let perms = permissions_for_approval(approval, Some(&decision.always_ids));
            if let Err(e) = session.set_session_permissions(perms) {
                eprintln!("failed to set session permissions: {e}");
            }
        }
        let res = session
            .resolve_approval(&approval.id, decision.approve, decision.proposals)
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

fn emit_stream_event(event: TurnEvent, stream: bool, output: &mut String, last_asr: &mut bool) {
    if !stream {
        return;
    }
    match event {
        TurnEvent::AsrText(text) => {
            eprint!("\r[ASR] {text}");
            let _ = io::stderr().flush();
            *last_asr = true;
        }
        TurnEvent::Thinking(_) => {
            if *last_asr {
                eprintln!();
                *last_asr = false;
            }
        }
        TurnEvent::Text(delta) => {
            if *last_asr {
                eprintln!();
                *last_asr = false;
            }
            output.push_str(&delta);
            print!("{delta}");
            let _ = io::stdout().flush();
        }
        TurnEvent::ToolCall { name, .. } => {
            if *last_asr {
                eprintln!();
                *last_asr = false;
            }
            if !output.is_empty() && !output.ends_with('\n') {
                println!();
                output.push('\n');
            }
            println!("  [tool call] {name}");
            output.push_str("  [tool call] ");
            output.push_str(&name);
            output.push('\n');
        }
        TurnEvent::ToolResult { name, is_error, .. } => {
            if *last_asr {
                eprintln!();
                *last_asr = false;
            }
            let status = if is_error { "error" } else { "ok" };
            println!("  [tool result] {name}: {status}");
            output.push_str("  [tool result] ");
            output.push_str(&name);
            output.push_str(": ");
            output.push_str(status);
            output.push('\n');
        }
        _ => {}
    }
}

fn permissions_for_approval(
    approval: &ApprovalRequest,
    approved_ids: Option<&[String]>,
) -> Permissions {
    let mut permissions = Permissions::default();
    for change in &approval.changes {
        if let Some(ids) = approved_ids {
            let Some(id) = &change.proposal_id else {
                continue;
            };
            if !ids.contains(id) {
                continue;
            }
        }
        if let Ok(key) = takusu_agent::PermissionKey::from_str(&format!(
            "{}:{}",
            change.target.kind, change.operation
        )) {
            permissions.set(key.target, key.operation, true);
        }
    }
    permissions
}

struct AskApprovalResult {
    pub approve: bool,
    pub proposals: Option<Vec<ProposalDecision>>,
    pub always_ids: Vec<String>,
}

fn ask_approval_proposals(
    approval: &ApprovalRequest,
    _stream: bool,
) -> Result<AskApprovalResult, AppError> {
    if !approval.why.is_empty() {
        println!("Why: {}", approval.why);
    }
    if !approval.inferred_fields.is_empty() {
        println!("Inferred:");
        for field in &approval.inferred_fields {
            println!("  {} = {} ({})", field.field, field.value, field.reason);
        }
    }
    if !approval.warnings.is_empty() {
        println!("Warnings:");
        for warning in &approval.warnings {
            println!("  - {warning}");
        }
    }

    let groups = group_proposals(&approval.changes);
    let total = groups.len();
    let mut decisions = Vec::with_capacity(total);
    let mut always_ids = Vec::new();
    for (i, (proposal_id, changes)) in groups.iter().enumerate() {
        let display_id = proposal_id.as_deref().unwrap_or("(no id)");
        display_proposal_group(display_id, i + 1, total, changes);
        let answer = ask_approve_choice("Approve this proposal? (y/n/a/N): ")?;
        let decision_id = proposal_id
            .clone()
            .unwrap_or_else(|| format!("<missing-{}>", i));
        let (approve, set_always) = match answer {
            ApprovalChoice::Yes => (true, false),
            ApprovalChoice::Always => (true, true),
            ApprovalChoice::No => (false, false),
        };
        if set_always {
            always_ids.push(decision_id.clone());
        }
        decisions.push(ProposalDecision {
            proposal_id: decision_id,
            approve,
        });
    }
    let approve = decisions.iter().any(|d| d.approve);
    Ok(AskApprovalResult {
        approve,
        proposals: Some(decisions),
        always_ids,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalChoice {
    Yes,
    No,
    Always,
}

fn group_proposals(changes: &[ProposedChange]) -> Vec<(Option<String>, Vec<&ProposedChange>)> {
    let mut groups: Vec<(Option<String>, Vec<&ProposedChange>)> = Vec::new();
    let mut index: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for change in changes {
        match &change.proposal_id {
            Some(id) => {
                if let Some(i) = index.get(id) {
                    groups[*i].1.push(change);
                } else {
                    index.insert(id.clone(), groups.len());
                    groups.push((Some(id.clone()), vec![change]));
                }
            }
            None => {
                groups.push((None, vec![change]));
            }
        }
    }
    groups
}

fn display_proposal_group(
    proposal_id: &str,
    current: usize,
    total: usize,
    changes: &[&ProposedChange],
) {
    println!("\n提案 {}/{} (id: {}):", current, total, proposal_id);
    for change in changes {
        println!(
            "  {} {}: {}",
            change.operation, change.target, change.description
        );
        if let Some(before) = &change.before {
            println!("    before: {}", before);
        }
        if let Some(after) = &change.after {
            println!("    after:  {}", after);
        }
    }
}

fn ask_approve_choice(label: &str) -> Result<ApprovalChoice, AppError> {
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
        .map(|l| match l.trim().to_lowercase().as_str() {
            "y" | "yes" => ApprovalChoice::Yes,
            "a" | "always" => ApprovalChoice::Always,
            _ => ApprovalChoice::No,
        })
        .unwrap_or(ApprovalChoice::No))
}

async fn run_repl(session: &AgentSession, yes: bool, plain: bool) -> Result<(), AppError> {
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
        run_text(session, line, yes, plain).await?;
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
                if q.text.is_empty() {
                    println!("Question: {}", q.purpose);
                } else {
                    println!("{}", q.purpose);
                    println!("  context: {}", q.text);
                }
                eprint!("  answer (empty to keep context): ");
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

pub(crate) fn agent_err(e: AgentError) -> AppError {
    AppError::Internal(e.to_string())
}

pub fn stats(clear: bool) -> Result<(), AppError> {
    let tool_stats = takusu_agent::ToolStats::load();
    if clear {
        tool_stats.clear();
        println!("Tool statistics cleared.");
        return Ok(());
    }
    let snapshot = tool_stats.snapshot();
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| AppError::Internal(format!("failed to serialize stats: {e}")))?;
    println!("{json}");
    Ok(())
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
    let mut cfg = AgentConfig::load()
        .map_err(|e| AppError::Internal(format!("failed to load agent config: {e}")))?;
    mask_secrets(&mut cfg);
    let rendered = toml::to_string_pretty(&cfg)
        .map_err(|e| AppError::Internal(format!("failed to serialize agent config: {e}")))?;
    println!("# Effective agent configuration (defaults included)\n{rendered}");
    Ok(())
}

fn mask_secrets(cfg: &mut AgentConfig) {
    if !cfg.llm.api_key.is_empty() {
        cfg.llm.api_key = "<set>".into();
    }
    if !cfg.server.token.is_empty() {
        cfg.server.token = "<set>".into();
    }
    if !cfg.audio.tts.api_key.is_empty() {
        cfg.audio.tts.api_key = "<set>".into();
    }
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

#[allow(dead_code)]
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
    validate_permission_key(key)?;
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

#[allow(dead_code)]
fn permissions_unset_at(path: &std::path::Path, key: &str) -> Result<(), AppError> {
    validate_permission_key(key)?;
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
    fn validate_permission_key_accepts_valid_keys() {
        for key in [
            "task:create",
            "*:*",
            "task:*",
            "*:create",
            "schedule:generate",
        ] {
            assert!(
                validate_permission_key(key).is_ok(),
                "{key} should be valid"
            );
        }
    }

    #[test]
    fn validate_permission_key_rejects_invalid_keys() {
        for key in ["invalid", "task", "task:", ":create", "task:create:sub"] {
            assert!(
                validate_permission_key(key).is_err(),
                "{key} should be rejected"
            );
        }
    }

    #[test]
    fn validate_permission_key_rejects_unknown_labels() {
        // Unknown target/operation labels are caught at config-write time,
        // not deferred to AgentConfig::load().
        assert!(validate_permission_key("foo:bar").is_err());
        assert!(validate_permission_key("task:bar").is_err());
        assert!(validate_permission_key("foo:create").is_err());
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
        use takusu_agent::{ChangeOperation, TargetKind};
        let perms = parse_session_permissions(
            &["task:create".into(), "schedule:generate".into()],
            &["task:delete".into()],
        )
        .unwrap();
        assert!(perms.is_allowed(TargetKind::Task, ChangeOperation::Create));
        assert!(perms.is_allowed(TargetKind::Schedule, ChangeOperation::Generate));
        assert!(!perms.is_allowed(TargetKind::Task, ChangeOperation::Delete));
        assert!(!perms.is_allowed(TargetKind::Memory, ChangeOperation::Create));
    }

    #[test]
    fn parse_session_permissions_deny_overrides_allow() {
        use takusu_agent::{ChangeOperation, TargetKind};
        let perms =
            parse_session_permissions(&["task:create".into()], &["task:create".into()]).unwrap();
        assert!(!perms.is_allowed(TargetKind::Task, ChangeOperation::Create));
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
