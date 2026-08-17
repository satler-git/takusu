mod agent;
mod config;
mod display_common;
mod display_rich;
mod display_simple;
mod editor;
mod licenses;
#[cfg(feature = "mcp")]
mod mcp;
mod server;
mod task_ref;

use clap::{Args, CommandFactory, Parser, Subcommand};
use config::CliConfig;
use std::io::{self, Read, Write};
use std::process;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::response::Html;
use axum::routing::get;

use takusu_contracts::{
    CreateHabit, CreateHabitScheduledSpan, CreateMemory, CreateSkill, CreateTask, GenerateSchedule,
    MemoryQuery, RecordWorkSessionProgress, Reschedule, ScheduleEntry, SimilarTaskQuery,
    SleepInput, SplitTask, StartWorkSession, TaskQuery, UpdateHabit, UpdateMemory, UpdateSettings,
};
use takusu_local_lib::{
    app::TakusuApp,
    config::{LocalConfig, StorageKind},
    error::{AppError, BadRequestKind},
    storage_sqlite::SqliteStorage,
    storage_workers::WorkersStorage,
    token_cache::TokenCache,
};
use takusu_types::{
    Abandonability, Date, MemoryKind, Quantity, ScheduleMode, SubjectType, TaskStatus,
    TaskStatusFilter, TimeOfDay, Timestamp, WindowMode, parse_datetime_to_timestamp,
    parse_duration,
};

fn prompt(label: &str) -> Result<String, AppError> {
    print!("{label}: ");
    io::stdout()
        .flush()
        .map_err(|e| AppError::Internal(format!("failed to flush stdout: {e}")))?;
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .map_err(|e| AppError::Internal(format!("failed to read line: {e}")))?;
    Ok(buf.trim().to_string())
}

fn require_or_prompt(
    label: &str,
    value: Option<String>,
    interactive: bool,
) -> Result<String, AppError> {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => Ok(v),
        None if interactive => prompt(label),
        None => Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "{label} is required"
        )))),
    }
}

fn is_interactive() -> bool {
    atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout)
}

fn is_stdout_tty() -> bool {
    atty::is(atty::Stream::Stdout)
}

fn parse_dt(s: &str, tz: &jiff::tz::TimeZone) -> Result<Timestamp, AppError> {
    parse_datetime_to_timestamp(s, tz)
        .map(Timestamp::from)
        .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))
}

fn parse_time(s: &str) -> Result<TimeOfDay, AppError> {
    s.parse::<TimeOfDay>().map_err(|e| {
        AppError::BadRequest(BadRequestKind::Other(format!("invalid time '{s}': {e}")))
    })
}

fn parse_date(s: &str) -> Result<Date, AppError> {
    s.parse::<Date>().map_err(|e| {
        AppError::BadRequest(BadRequestKind::Other(format!("invalid date '{s}': {e}")))
    })
}

async fn find_open_session_id(app: &TakusuApp, task_id: &str) -> Result<String, AppError> {
    app.open_work_session_for_task(task_id)
        .await?
        .map(|s| s.id)
        .ok_or_else(|| {
            AppError::BadRequest(BadRequestKind::Other(
                "no open work session for task".into(),
            ))
        })
}

#[derive(Parser)]
#[command(name = "takusu", version, about = "CLI client for takusu scheduler")]
struct Cli {
    #[arg(long, env = "TAKUSU_TIMEZONE", global = true)]
    tz: Option<String>,

    /// Output mode (rich or simple). Auto-detected from TTY if omitted.
    #[arg(long, global = true)]
    mode: Option<DisplayMode>,

    /// Force plain/simple output.
    #[arg(long, global = true)]
    plain: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Clone, clap::ValueEnum)]
enum DisplayMode {
    Rich,
    Simple,
}

/// CLI-facing subject type for `--subject-type`.
///
/// `SubjectType::Empty` has the label `""` (for DB backward compatibility),
/// which renders as an awkward `""` entry in clap's `[possible values: ...]`
/// help text. This wrapper omits the empty variant from the CLI surface;
/// callers convert to `SubjectType` with `From`, where `None` maps to
/// `SubjectType::Empty`.
#[derive(Clone, clap::ValueEnum)]
enum SubjectTypeArg {
    Task,
    Habit,
    Skill,
    Schedule,
}

impl From<SubjectTypeArg> for SubjectType {
    fn from(value: SubjectTypeArg) -> Self {
        match value {
            SubjectTypeArg::Task => SubjectType::Task,
            SubjectTypeArg::Habit => SubjectType::Habit,
            SubjectTypeArg::Skill => SubjectType::Skill,
            SubjectTypeArg::Schedule => SubjectType::Schedule,
        }
    }
}

impl DisplayMode {
    /// Return the active display renderer.
    fn formatter(&self) -> &'static dyn display_common::DisplayFormatter {
        match self {
            DisplayMode::Rich => &display_rich::RichFormatter,
            DisplayMode::Simple => &display_simple::SimpleFormatter,
        }
    }
}

fn effective_display_mode(cli: &Cli) -> DisplayMode {
    if cli.plain {
        return DisplayMode::Simple;
    }
    if let Some(mode) = cli.mode.as_ref() {
        return mode.clone();
    }
    if is_stdout_tty() {
        DisplayMode::Rich
    } else {
        DisplayMode::Simple
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Task verbs, available at the top level.
    #[command(flatten)]
    Task(TaskVerbs),

    /// Schedule verbs, available at the top level.
    #[command(flatten)]
    Schedule(ScheduleVerbs),

    /// Habit management.
    Habit {
        #[command(subcommand)]
        command: HabitCommands,
    },

    /// Memory and similar-task search.
    Memory {
        #[command(subcommand)]
        command: MemoryCommands,
    },

    /// Skill management.
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },

    /// Token management.
    Token {
        #[command(subcommand)]
        command: TokenCommands,
    },

    /// Google Calendar sync.
    Sync {
        #[command(subcommand)]
        command: SyncCommands,
    },

    /// Show or initialize config file.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// System utilities.
    System {
        #[command(subcommand)]
        command: SystemCommands,
    },

    /// Agent assistant.
    Agent(AgentArgs),

    /// MCP server over stdio.
    #[cfg(feature = "mcp")]
    Mcp,

    /// Launch the interactive TUI.
    Tui,

    /// Launch the web UI server (localhost).
    #[cfg(feature = "web")]
    Web {
        /// Bind address (overrides config / TAKUSU_BIND), e.g. 127.0.0.1:3000
        #[arg(long)]
        bind: Option<String>,
    },
}

// ── Top-level task verbs ─────────────────────────────────────────────────

#[derive(Subcommand)]
enum TaskVerbs {
    /// Create a task (interactive if title is not given in a terminal).
    Add(AddArgs),

    /// List tasks.
    Ls(LsArgs),

    /// Show task detail.
    Show(RefArgs),

    /// Start work on a task (creates a session, status -> in_progress).
    Start(RefArgs),

    /// Pause work on a task (closes the open session).
    Pause(RefArgs),

    /// Complete work on a task (closes the session, status -> completed).
    Done(RefArgs),

    /// Mark a task as skipped.
    Skip(RefArgs),

    /// Edit a task in $EDITOR, or PATCH with flags.
    Edit(EditArgs),

    /// Delete a task.
    Rm(RefArgs),

    /// Record progress on a task.
    Progress(ProgressArgs),

    /// Split a task into the original (retained quantity) and a remainder.
    Split(SplitArgs),

    /// Import tasks from an iCalendar (.ics) file.
    Import(ImportArgs),

    /// Show task dependencies or detect redundant edges with --check.
    Deps(DepsArgs),
}

#[derive(Args)]
struct RefArgs {
    #[arg(value_name = "REF")]
    id: String,
}

#[derive(Args)]
struct AddArgs {
    /// Task title.
    title: Option<String>,

    #[arg(
        short = 'd',
        long,
        help = "Deadline (e.g. 2025-06-05, 2025-06-05T23:59:00Z)"
    )]
    due: Option<String>,

    #[arg(short = 'a', long, help = "Start time (same format as --due)")]
    at: Option<String>,

    #[arg(
        short = 't',
        long,
        default_value = "30m",
        help = "Average duration (e.g. 30m, 1h30m, 6s=6slots(30min))"
    )]
    time: String,

    #[arg(
        long,
        default_value = "0",
        help = "Std dev of duration (same format as --time). 0 = auto (time/5)"
    )]
    sigma: String,

    #[arg(long, default_value_t = 0.5, help = "Abandonability 0.0-1.0")]
    abandonability: f64,

    #[arg(long)]
    description: Option<String>,

    #[arg(long)]
    depends: Option<Vec<String>>,

    #[arg(long)]
    parallelizable: Option<bool>,

    #[arg(long)]
    allows_parallel: Option<bool>,

    #[arg(long, help = "Lock start time (scheduler cannot move)")]
    fixed: Option<bool>,

    #[arg(long)]
    quantity_total: Option<i64>,

    #[arg(long)]
    quantity_done: Option<i64>,

    #[arg(long)]
    quantity_unit: Option<String>,

    #[arg(long)]
    original_quantity_total: Option<i64>,
}

#[derive(Args)]
struct LsArgs {
    #[arg(
        long,
        help = "Filter by status (pending, scheduled, in_progress, completed, skipped, overdue, actionable)"
    )]
    status: Option<TaskStatusFilter>,

    #[arg(
        long,
        help = "Filter by start date (e.g. 2025-06-05, 2025-06-05T14:00)"
    )]
    from: Option<String>,

    #[arg(long, help = "Filter by end date (e.g. 2025-06-05, 2025-06-05T14:00)")]
    until: Option<String>,

    /// Show all tasks (do not filter to actionable statuses).
    #[arg(long)]
    all: bool,

    #[arg(long, help = "Maximum number of tasks to return")]
    limit: Option<i64>,

    #[arg(long, help = "Exclude overdue tasks")]
    no_overdue: bool,

    #[arg(long, help = "Filter by habit id")]
    habit_id: Option<String>,

    #[arg(long, help = "Filter by iCalendar UID")]
    ical_uid: Option<String>,

    #[arg(
        help = "Search query (e.g. status:pending OR 買い物)",
        trailing_var_arg = true,
        num_args = 0..,
    )]
    query: Vec<String>,
}

#[derive(Args)]
struct EditArgs {
    #[arg(value_name = "REF")]
    id: String,

    #[arg(long)]
    title: Option<String>,

    #[arg(long)]
    description: Option<String>,

    #[arg(
        short = 'a',
        long,
        help = "Start time (e.g. 2025-06-05, 2025-06-05T14:00)"
    )]
    at: Option<String>,

    #[arg(
        short = 'd',
        long,
        help = "Deadline (e.g. 2025-06-05, 2025-06-05T14:00)"
    )]
    due: Option<String>,

    #[arg(short = 't', long, help = "Average duration (e.g. 30m, 1h30m)")]
    time: Option<String>,

    #[arg(long, help = "Std dev of duration (same format as --time)")]
    sigma: Option<String>,

    #[arg(long)]
    depends: Option<Vec<String>>,

    #[arg(long)]
    parallelizable: Option<bool>,

    #[arg(long)]
    allows_parallel: Option<bool>,

    #[arg(long)]
    abandonability: Option<f64>,

    #[arg(long)]
    status: Option<TaskStatus>,

    #[arg(long, help = "Lock start time (scheduler cannot move)")]
    fixed: Option<bool>,

    #[arg(long)]
    quantity_total: Option<i64>,

    #[arg(long)]
    quantity_done: Option<i64>,

    #[arg(long)]
    quantity_unit: Option<String>,

    #[arg(long)]
    original_quantity_total: Option<i64>,
}

impl EditArgs {
    fn has_patch_flag(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.at.is_some()
            || self.due.is_some()
            || self.time.is_some()
            || self.sigma.is_some()
            || self.depends.is_some()
            || self.parallelizable.is_some()
            || self.allows_parallel.is_some()
            || self.abandonability.is_some()
            || self.status.is_some()
            || self.fixed.is_some()
            || self.quantity_total.is_some()
            || self.quantity_done.is_some()
            || self.quantity_unit.is_some()
            || self.original_quantity_total.is_some()
    }
}

#[derive(Args)]
struct ProgressArgs {
    #[arg(value_name = "REF")]
    id: String,

    #[arg(value_name = "QUANTITY")]
    quantity: i64,

    #[arg(long)]
    note: Option<String>,
}

#[derive(Args)]
struct SplitArgs {
    #[arg(value_name = "REF")]
    id: String,

    #[arg(short = 'k', long, help = "Quantity to keep on the original task")]
    keep: i64,

    #[arg(long, help = "Make the remainder depend on the original task")]
    dep: bool,

    #[arg(long, help = "Title for the remainder task")]
    title: Option<String>,

    #[arg(long, help = "Description for the remainder task")]
    description: Option<String>,

    #[arg(short = 'd', long, help = "Deadline for the remainder task")]
    due: Option<String>,
}

#[derive(Args)]
struct ImportArgs {
    /// Path to the .ics file, or "-" to read from stdin.
    file: String,
}

#[derive(Args)]
struct DepsArgs {
    /// Detect and offer to remove redundant (composite) dependency edges.
    #[arg(long)]
    check: bool,
}

// ── Top-level schedule verbs ─────────────────────────────────────────────

#[derive(Subcommand)]
enum ScheduleVerbs {
    /// Show active schedule. This is the default when no subcommand is given.
    Agenda(AgendaArgs),

    /// Generate or reschedule the active schedule.
    Plan(PlanArgs),

    /// Move a schedule entry.
    Move(MoveArgs),

    /// Clear active schedule.
    Unplan,
}

#[derive(Args)]
struct AgendaArgs {
    #[arg(
        long,
        help = "Show only schedule entries for the given day (YYYY-MM-DD)"
    )]
    day: Option<String>,
}

#[derive(Args)]
struct PlanArgs {
    #[arg(long, help = "Start time (e.g. 2025-06-05, 2025-06-05T06:00Z)")]
    from: Option<String>,

    #[arg(long, help = "End time (e.g. 2025-06-06, 2025-06-06T06:00Z)")]
    until: Option<String>,

    #[arg(long, num_args = 1.., help = "Reschedule only these task refs")]
    tasks: Option<Vec<String>>,

    #[arg(long, num_args = 1.., help = "Task refs to keep pinned")]
    pin: Option<Vec<String>>,

    #[arg(long, default_value = "recommended")]
    sleep: SleepInput,
}

#[derive(Args)]
struct MoveArgs {
    #[arg(value_name = "REF")]
    task_id: String,

    #[arg(value_name = "START_AT")]
    start_at: String,

    #[arg(long)]
    force: bool,
}

// ── Noun groups ──────────────────────────────────────────────────────────

#[derive(Subcommand)]
enum HabitCommands {
    /// Create a habit (interactive if no args in terminal).
    Add(HabitAddArgs),

    /// List habits.
    Ls,

    /// Show habit detail.
    Show(RefArgs),

    /// Edit a habit in $EDITOR, or PATCH with flags.
    Edit(HabitEditArgs),

    /// Delete a habit.
    Rm(RefArgs),

    /// Add a pause (scheduled span) to a habit.
    Pause(HabitPauseArgs),

    /// Manage habit pauses (scheduled spans).
    #[command(subcommand)]
    Pauses(HabitPausesCommands),

    /// Manage habit steps.
    #[command(subcommand)]
    Steps(HabitStepsCommands),
}

#[derive(Args)]
struct HabitAddArgs {
    #[arg(short, long, help = "Habit title")]
    title: Option<String>,

    #[arg(long, short, help = "Recurrence (daily, weekdays, Mon,Wed,Fri)")]
    recurrence: Option<String>,

    #[arg(long, help = "Start time (HH:MM)")]
    start_time: Option<String>,

    #[arg(long, help = "End time (HH:MM)")]
    end_time: Option<String>,

    #[arg(
        long,
        default_value = "30m",
        help = "Average duration (e.g. 30m, 1h30m)"
    )]
    avg_time: String,

    #[arg(
        long,
        default_value = "0",
        help = "Std dev of duration (same format as avg_time). 0 = auto (avg/5)"
    )]
    sigma_time: String,

    #[arg(long, default_value_t = 0.5, help = "Abandonability 0.0-1.0")]
    abandonability: f64,

    #[arg(long)]
    description: Option<String>,

    #[arg(long)]
    parallelizable: bool,

    #[arg(long)]
    allows_parallel: bool,

    #[arg(long, help = "Lock start time (scheduler cannot move)")]
    fixed: bool,

    #[arg(
        long,
        help = "Window mode: 'day' (occurrence day) or 'period' (until next occurrence)"
    )]
    window: Option<WindowMode>,
}

#[derive(Args)]
struct HabitEditArgs {
    #[arg(value_name = "REF")]
    id: String,

    #[arg(long)]
    title: Option<String>,

    #[arg(long)]
    description: Option<String>,

    #[arg(long)]
    recurrence: Option<String>,

    #[arg(long, help = "Start time (HH:MM)")]
    start_time: Option<String>,

    #[arg(long, help = "End time (HH:MM)")]
    end_time: Option<String>,

    #[arg(long, help = "Average duration (e.g. 30m, 1h30m)")]
    avg_time: Option<String>,

    #[arg(long, help = "Std dev of duration (same format as avg_time)")]
    sigma_time: Option<String>,

    #[arg(long)]
    parallelizable: Option<bool>,

    #[arg(long)]
    allows_parallel: Option<bool>,

    #[arg(long)]
    abandonability: Option<f64>,

    #[arg(long)]
    active: Option<bool>,

    #[arg(long, help = "Lock start time (scheduler cannot move)")]
    fixed: Option<bool>,

    #[arg(
        long,
        help = "Window mode: 'day' (occurrence day) or 'period' (until next occurrence)"
    )]
    window: Option<WindowMode>,
}

impl HabitEditArgs {
    fn has_patch_flag(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.recurrence.is_some()
            || self.start_time.is_some()
            || self.end_time.is_some()
            || self.avg_time.is_some()
            || self.sigma_time.is_some()
            || self.parallelizable.is_some()
            || self.allows_parallel.is_some()
            || self.abandonability.is_some()
            || self.active.is_some()
            || self.fixed.is_some()
            || self.window.is_some()
    }
}

#[derive(Args)]
struct HabitPauseArgs {
    #[arg(value_name = "REF")]
    id: String,

    #[arg(long, help = "Start date (YYYY-MM-DD, inclusive)")]
    from: String,

    #[arg(long, help = "End date (YYYY-MM-DD, inclusive)")]
    to: String,

    #[arg(long, help = "Optional reason (e.g. 休暇)")]
    reason: Option<String>,
}

#[derive(Subcommand)]
enum HabitPausesCommands {
    /// List pauses for a habit, or all habits if no id is given.
    Ls(HabitPausesLsArgs),

    /// Remove a pause.
    Rm(HabitPausesRmArgs),
}

#[derive(Args)]
struct HabitPausesLsArgs {
    #[arg(value_name = "REF")]
    id: Option<String>,
}

#[derive(Args)]
struct HabitPausesRmArgs {
    habit_id: String,
    span_id: String,
}

#[derive(Subcommand)]
enum HabitStepsCommands {
    /// List steps for a habit, or all habits if no id is given.
    Ls(HabitStepsLsArgs),

    /// Edit steps for a habit in $EDITOR (TOML array-of-tables).
    Edit(RefArgs),

    /// Replace steps from a TOML file or stdin ("-"; # comments are ignored).
    Set(HabitStepsSetArgs),

    /// Detect and offer to remove redundant step dependency edges.
    Check(RefArgs),
}

#[derive(Args)]
struct HabitStepsLsArgs {
    #[arg(value_name = "REF")]
    id: Option<String>,
}

#[derive(Args)]
struct HabitStepsSetArgs {
    #[arg(value_name = "REF")]
    id: String,

    #[arg(help = "TOML file path or '-' for stdin (# comments are ignored)")]
    file: String,
}

#[derive(Subcommand)]
enum MemoryCommands {
    /// Create a memory entry.
    Add(MemoryAddArgs),

    /// List memory entries.
    Ls(MemoryLsArgs),

    /// Show a memory entry.
    Show(RefArgs),

    /// Update a memory entry.
    Edit(MemoryEditArgs),

    /// Delete a memory entry.
    Rm(RefArgs),

    /// Search memory entries.
    Search(MemorySearchArgs),

    /// Find completed tasks similar to a title.
    Similar(SimilarArgs),
}

#[derive(Args)]
struct MemoryAddArgs {
    #[arg(value_name = "KIND")]
    kind: MemoryKind,

    #[arg(value_name = "KEY")]
    key: String,

    #[arg(value_name = "CONTENT")]
    content: String,

    #[arg(long)]
    subject_type: Option<SubjectTypeArg>,

    #[arg(long)]
    subject_id: Option<String>,

    #[arg(long)]
    upsert: bool,
}

#[derive(Args)]
struct MemoryLsArgs {
    #[arg(long)]
    limit: Option<i64>,
}

#[derive(Args)]
struct MemoryEditArgs {
    #[arg(value_name = "ID")]
    id: String,

    #[arg(long)]
    content: String,
}

#[derive(Args)]
struct MemorySearchArgs {
    #[arg(value_name = "QUERY")]
    q: String,

    #[arg(long)]
    kind: Option<MemoryKind>,

    #[arg(long)]
    subject_type: Option<SubjectTypeArg>,

    #[arg(long)]
    subject_id: Option<String>,

    #[arg(long)]
    limit: Option<i64>,
}

#[derive(Args)]
struct SimilarArgs {
    #[arg(value_name = "TITLE")]
    title: String,

    #[arg(long)]
    limit: Option<i64>,
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Create a skill (interactive if no args in terminal).
    Add(SkillAddArgs),

    /// List skills.
    Ls,

    /// Show skill detail.
    Show(SkillShowArgs),

    /// Edit a skill (flags), or edit the body in $EDITOR with no flags.
    Edit(SkillEditArgs),

    /// Delete a skill.
    Rm(SkillRmArgs),
}

#[derive(Args)]
struct SkillAddArgs {
    #[arg(short, long, help = "Skill slug")]
    slug: Option<String>,

    #[arg(short, long, help = "Skill name")]
    name: Option<String>,

    #[arg(long, help = "Skill description")]
    description: Option<String>,

    #[arg(long, help = "Skill body file or '-' for stdin")]
    body: Option<String>,
}

#[derive(Args)]
struct SkillShowArgs {
    slug: String,
}

#[derive(Args)]
struct SkillEditArgs {
    slug: String,

    #[arg(short, long, help = "Skill name")]
    name: Option<String>,

    #[arg(long, help = "Skill description")]
    description: Option<String>,

    #[arg(long, help = "Skill body file or '-' for stdin")]
    body: Option<String>,
}

#[derive(Args)]
struct SkillRmArgs {
    slug: String,
}

#[derive(Subcommand)]
enum TokenCommands {
    /// Issue a new token.
    Add(TokenAddArgs),

    /// List tokens.
    Ls,

    /// Revoke a token.
    Rm(TokenRmArgs),
}

#[derive(Args)]
struct TokenAddArgs {
    #[arg(long)]
    label: Option<String>,
}

#[derive(Args)]
struct TokenRmArgs {
    id: i64,
}

#[derive(Subcommand)]
enum SyncCommands {
    /// Show Google Calendar sync settings.
    Status,

    /// Update Google Calendar sync settings (prompts for missing values).
    Setup(SyncSetupArgs),

    /// Start a local server and complete Google OAuth2 login in one step.
    Login(SyncLoginArgs),

    /// Manually trigger Google Calendar sync.
    Run,

    /// List Google Calendar event mappings.
    Mappings,

    /// Delete all mapped Google Calendar events and clear local mappings.
    Purge,
}

#[derive(Args)]
struct SyncSetupArgs {
    #[arg(long)]
    enabled: Option<bool>,

    #[arg(long)]
    calendar_id: Option<String>,

    #[arg(long)]
    client_id: Option<String>,

    #[arg(long)]
    client_secret: Option<String>,

    #[arg(long)]
    refresh_token: Option<String>,

    #[arg(long)]
    reminder_minutes: Option<i64>,

    #[arg(long)]
    color_id: Option<i64>,

    #[arg(long)]
    visibility: Option<String>,

    #[arg(long)]
    transparency: Option<String>,

    #[arg(long, help = "Do not prompt for missing values")]
    no_ask: bool,
}

#[derive(Args)]
struct SyncLoginArgs {
    #[arg(long)]
    client_id: Option<String>,

    #[arg(long)]
    client_secret: Option<String>,

    #[arg(long)]
    calendar_id: Option<String>,

    #[arg(long, default_value_t = 8765)]
    port: u16,

    #[arg(long)]
    no_browser: bool,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Show config file path and contents.
    Show,

    /// Initialize config file with defaults.
    Init,

    /// Set a local or app setting value.
    Set(ConfigSetArgs),

    /// Worker storage configuration.
    #[command(subcommand)]
    Workers(ConfigWorkersCommands),
}

#[derive(Args)]
struct ConfigSetArgs {
    key: String,
    value: String,
}

#[derive(Subcommand)]
enum ConfigWorkersCommands {
    /// Update Worker endpoint and token at runtime.
    Set(WorkersSetArgs),

    /// Check storage backend health.
    Health,
}

#[derive(Args)]
struct WorkersSetArgs {
    url: String,
    token: String,
}

#[derive(Subcommand)]
enum SystemCommands {
    /// Check server health (no token required).
    Health,

    /// Generate a root token.
    GenRootToken,

    /// Show third-party licenses.
    License,

    /// Generate shell completions.
    Completion(SystemCompletionArgs),
}

#[derive(Args)]
struct SystemCompletionArgs {
    #[arg(value_name = "SHELL")]
    shell: clap_complete::Shell,
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
struct AgentArgs {
    /// Single text input for one agent turn.
    text: Option<String>,

    /// Auto-approve any pending changes without prompting.
    #[arg(long)]
    yes: bool,

    /// Auto-approve a permission for this session (e.g. task:create, *:*).
    #[arg(long, value_name = "PERM")]
    allow: Vec<String>,

    /// Deny a permission for this session, overriding provider settings.
    #[arg(long, value_name = "PERM")]
    deny: Vec<String>,

    /// Resume the previous agent session if one exists.
    #[arg(long, conflicts_with = "new_session")]
    continue_session: bool,

    /// Start a new agent session and do not resume the previous one.
    #[arg(long = "new", conflicts_with = "continue_session")]
    new_session: bool,

    #[command(subcommand)]
    command: Option<AgentSubCommands>,
}

#[derive(Subcommand)]
enum AgentSubCommands {
    /// Show or edit agent configuration.
    #[command(subcommand)]
    Config(AgentConfigCommands),

    /// Allow a permission persistently.
    Allow { key: String },

    /// Deny a permission persistently.
    Deny { key: String },

    /// Show or clear tool usage statistics.
    Stats(AgentStatsArgs),
}

#[derive(Subcommand)]
enum AgentConfigCommands {
    /// Show current agent config file.
    Show,

    /// Set a config value by key path.
    Set { key: String, value: String },
}

#[derive(Args)]
struct AgentStatsArgs {
    /// Clear all statistics.
    #[arg(long)]
    clear: bool,
}

fn is_local_config_key(key: &str) -> bool {
    matches!(
        key,
        "storage"
            | "db"
            | "worker_url"
            | "url"
            | "workers_token"
            | "token"
            | "root_token"
            | "jwt_secret"
            | "tz"
            | "sleep_start"
            | "sleep_end"
    )
}

fn is_app_settings_key(key: &str) -> bool {
    matches!(
        key,
        "comfortable"
            | "maximum"
            | "solver"
            | "time_budget_ms"
            | "seed"
            | "warm_start"
            | "tz"
            | "sleep_start"
            | "sleep_end"
    )
}

fn main() {
    let _guard = takusu_local_lib::sentry::init(
        "takusu_local_lib=info",
        Some(concat!(env!("CARGO_PKG_NAME"), "@", env!("CARGO_PKG_VERSION")).into()),
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let cli = Cli::parse();
        let mode = effective_display_mode(&cli);
        let mut cfg = config::load();

        let cmd = match cli.command {
            Some(cmd) => cmd,
            None => Commands::Schedule(ScheduleVerbs::Agenda(AgendaArgs { day: None })),
        };

        // Early-return commands that do not need an app/storage.
        match &cmd {
            Commands::System { command } => match command {
                SystemCommands::GenRootToken => {
                    let secret = std::env::var("TAKUSU_JWT_SECRET")
                        .ok()
                        .filter(|s| !s.is_empty())
                        .or_else(|| cfg.jwt_secret.clone().filter(|s| !s.is_empty()))
                        .unwrap_or_else(|| {
                            eprintln!("Error: TAKUSU_JWT_SECRET (or jwt_secret in config) is required to generate a root token");
                            process::exit(1);
                        });
                    let token = match takusu_types::jwt::generate_root_jwt(&secret, None) {
                        Ok(t) => t,
                        Err(e) => {
                            eprintln!("Error: failed to generate root token: {e}");
                            process::exit(1);
                        }
                    };
                    println!("{token}");
                    eprintln!("\nSet this as TAKUSU_ROOT_TOKEN env var or root_token in config for takusu.");
                    return;
                }
                SystemCommands::Completion(args) => {
                    let mut cmd = Cli::command();
                    clap_complete::generate(args.shell, &mut cmd, "takusu", &mut io::stdout());
                    return;
                }
                SystemCommands::License => {
                    licenses::print_licenses();
                    return;
                }
                _ => {}
            },
            Commands::Config { command } => match command {
                ConfigCommands::Show => {
                    config::show();
                    return;
                }
                ConfigCommands::Init => {
                    config::init();
                    return;
                }
                ConfigCommands::Set(args) if is_local_config_key(&args.key) => {
                    config::set(&args.key, &args.value).unwrap_or_else(|e| {
                        eprintln!("Error: {e}");
                        process::exit(1);
                    });
                    cfg = config::load();
                }
                _ => {}
            },
            _ => {}
        }

        // The web subcommand runs its own server (building its own app from the
        // shared config), so dispatch it before the CLI constructs storage.
        #[cfg(feature = "web")]
        if let Commands::Web { bind } = &cmd {
            if let Err(e) = takusu_web::run(bind.clone()).await {
                eprintln!("Error: {e}");
                process::exit(1);
            }
            return;
        }

        let tz_str = cli.tz.clone().or(cfg.tz.clone()).unwrap_or_else(|| "UTC".into());

        // Build local config from CLI config and environment overrides
        let mut local_cfg = LocalConfig::default();
        let env_storage = std::env::var("TAKUSU_STORAGE").ok().filter(|s| !s.is_empty());
        let env_db = std::env::var("TAKUSU_DB").ok().filter(|s| !s.is_empty());

        if let Some(v) = env_storage {
            local_cfg.storage = v.parse().unwrap_or_else(|e| {
                eprintln!("Error: invalid TAKUSU_STORAGE: {e}");
                process::exit(1);
            });
        } else if env_db.is_some() {
            local_cfg.storage = StorageKind::Sqlite;
        } else if let Some(v) = cfg.storage {
            local_cfg.storage = v;
        }
        if let Some(v) = env_db {
            local_cfg.db = v;
        } else if let Some(ref v) = cfg.db {
            local_cfg.db = v.clone();
        }
        if let Ok(v) = std::env::var("TAKUSU_WORKERS_URL") && !v.is_empty() {
            local_cfg.worker_url = v;
        } else if let Ok(v) = std::env::var("TAKUSU_WORKER_URL") && !v.is_empty() {
            local_cfg.worker_url = v;
        } else if let Some(ref v) = cfg.worker_url {
            local_cfg.worker_url = v.clone();
        }
        if let Ok(v) = std::env::var("TAKUSU_JWT_SECRET") && !v.is_empty() {
            local_cfg.jwt_secret = v;
        } else if let Some(ref v) = cfg.jwt_secret {
            local_cfg.jwt_secret = v.clone();
        }

        let env_root = std::env::var("TAKUSU_ROOT_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        let env_workers = std::env::var("TAKUSU_WORKERS_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());

        let workers_token = env_workers
            .clone()
            .or_else(|| cfg.workers_token.clone())
            .or_else(|| env_root.clone())
            .or_else(|| cfg.root_token.clone())
            .unwrap_or_default();

        let storage: Arc<dyn takusu_contracts::Storage> = match local_cfg.storage {
            StorageKind::Workers => {
                let url = local_cfg.workers_url().to_string();
                if url.is_empty() {
                    eprintln!("Error: worker_url is required for the workers backend");
                    process::exit(1);
                }
                if workers_token.is_empty() {
                    eprintln!("Error: workers_token (or TAKUSU_ROOT_TOKEN) is required for the workers backend");
                    process::exit(1);
                }
                Arc::new(WorkersStorage::new_with(url, workers_token))
            }
            StorageKind::Sqlite => {
                if local_cfg.jwt_secret.is_empty() {
                    eprintln!("Error: TAKUSU_JWT_SECRET (or jwt_secret in config) is required for the sqlite backend");
                    process::exit(1);
                }
                let storage = SqliteStorage::init(&local_cfg)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("Error initializing sqlite storage: {e}");
                        process::exit(1);
                    });
                Arc::new(storage)
            }
        };

        let token_cache = Arc::new(TokenCache::with_default_ttl());
        let app = Arc::new(TakusuApp::new(storage, token_cache));

        let tz = jiff::tz::TimeZone::get(&tz_str).unwrap_or_else(|_| {
            eprintln!("Error: invalid timezone '{tz_str}' (e.g. Asia/Tokyo)");
            process::exit(1);
        });

        if let Err(e) = run(mode, Arc::clone(&app), tz, cmd, &cfg, cli.plain).await {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    })
}

async fn run(
    mode: DisplayMode,
    app: Arc<TakusuApp>,
    tz: jiff::tz::TimeZone,
    cmd: Commands,
    cfg: &CliConfig,
    plain: bool,
) -> Result<(), AppError> {
    match cmd {
        Commands::Task(verbs) => run_task_verbs(mode, app.as_ref(), &tz, verbs).await?,
        Commands::Schedule(verbs) => run_schedule_verbs(mode, app.as_ref(), &tz, verbs).await?,
        Commands::Habit { command } => run_habit(mode, app.as_ref(), command).await?,
        Commands::Memory { command } => run_memory(app.as_ref(), command).await?,
        Commands::Skill { command } => run_skill(mode, app.as_ref(), command).await?,
        Commands::Token { command } => run_token(mode, app.as_ref(), command).await?,
        Commands::Sync { command } => run_sync(app.as_ref(), command).await?,
        Commands::Config { command } => run_config(command, app.as_ref(), cfg).await?,
        Commands::System { command } => run_system(app.as_ref(), command).await?,
        Commands::Agent(args) => run_agent(app, args, plain).await?,
        #[cfg(feature = "mcp")]
        Commands::Mcp => mcp::run(app).await?,
        Commands::Tui => {
            takusu_tui::run(app, tz)
                .await
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }
        #[cfg(feature = "web")]
        Commands::Web { .. } => {
            unreachable!("web subcommand is handled before run()")
        }
    }
    Ok(())
}

/// Build a habit_id (UUID) → display_id map for task ID labels (h1#5, #305).
/// Returns an empty map if the habit list cannot be fetched (e.g. empty DB),
/// in which case task labels fall back to the plain `#N` form.
async fn habit_display_map(app: &TakusuApp) -> std::collections::HashMap<String, i64> {
    app.list_habits()
        .await
        .map(|habits| habits.into_iter().map(|h| (h.id, h.display_id)).collect())
        .unwrap_or_default()
}

async fn run_task_verbs(
    mode: DisplayMode,
    app: &TakusuApp,
    tz: &jiff::tz::TimeZone,
    cmd: TaskVerbs,
) -> Result<(), AppError> {
    let habit_map = habit_display_map(app).await;
    match cmd {
        TaskVerbs::Ls(args) => {
            let q = if args.query.is_empty() {
                None
            } else {
                Some(args.query.join(" "))
            };

            let status = if args.all {
                None
            } else {
                Some(args.status.unwrap_or(TaskStatusFilter::Actionable))
            };

            let query = TaskQuery {
                status,
                from: args.from.map(|s| parse_dt(&s, tz)).transpose()?,
                until: args.until.map(|s| parse_dt(&s, tz)).transpose()?,
                no_overdue: if args.no_overdue { Some(true) } else { None },
                habit_id: args.habit_id,
                ical_uid: args.ical_uid,
                q,
                limit: args.limit,
            };

            let tasks = app.list_tasks(&query).await?;

            mode.formatter().display_tasks(&tasks, tz, &habit_map);
        }

        TaskVerbs::Show(args) => {
            let task = app.get_task(&args.id).await?;
            let entry = match app.get_schedule().await {
                Ok(schedule) => {
                    let entries: Vec<ScheduleEntry> = schedule.schedule.as_inner().clone();
                    entries.into_iter().find(|e| e.task_id == task.id)
                }
                Err(_) => None,
            };
            // Load the comment timeline (WI-5). A fetch failure is surfaced
            // distinctly rather than being mistaken for an empty timeline.
            let comments = match app.list_comments(&task.id).await {
                Ok(rows) => rows,
                Err(e) => {
                    eprintln!("warning: could not load comments: {e}");
                    Vec::new()
                }
            };
            mode.formatter()
                .display_task_detail(&task, entry.as_ref(), tz, &habit_map, &comments);

            // Show work sessions and progress events.
            let progress = app.get_task_progress(&args.id).await?;
            if !progress.sessions.is_empty() || !progress.events.is_empty() {
                println!("work sessions: {}", progress.sessions.len());
                for s in &progress.sessions {
                    println!(
                        "  {} - {}",
                        s.started_at,
                        s.ended_at
                            .as_ref()
                            .map(|t| t.to_string())
                            .unwrap_or_else(|| "(open)".into()),
                    );
                }
                println!("progress events: {}", progress.events.len());
                for e in &progress.events {
                    let q = e.quantity_done.map(|q| q.to_string()).unwrap_or_default();
                    let delta = e
                        .delta_quantity
                        .map(|d| format!("(+{d})"))
                        .unwrap_or_default();
                    println!(
                        "  {} {} {} (active {}min)",
                        e.at, q, delta, e.active_minutes
                    );
                }
            }
        }

        TaskVerbs::Add(args) => {
            let (title, due) = if is_interactive() {
                let title = match args.title.as_deref() {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => prompt("Title")?,
                };
                let due = match args.due.as_deref() {
                    Some(d) if !d.is_empty() => d.to_string(),
                    _ => prompt("Due (e.g. 2025-06-05 or 2025-06-05T23:59)")?,
                };
                (title, due)
            } else {
                let title = args.title.ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other("title is required".into()))
                })?;
                let due = args.due.ok_or_else(|| {
                    AppError::BadRequest(BadRequestKind::Other("due is required".into()))
                })?;
                (title, due)
            };

            let avg_minutes = parse_duration(&args.time)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let sigma_minutes: i64 = parse_duration(&args.sigma)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let quantity_total = args
                .quantity_total
                .map(Quantity::new)
                .transpose()
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let quantity_done = args
                .quantity_done
                .map(Quantity::new)
                .transpose()
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let original_quantity_total = args
                .original_quantity_total
                .map(Quantity::new)
                .transpose()
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;

            let body = CreateTask {
                title,
                end_at: parse_dt(&due, tz)?,
                start_at: args.at.map(|s| parse_dt(&s, tz)).transpose()?,
                avg_minutes,
                sigma_minutes: if sigma_minutes > 0 {
                    Some(sigma_minutes)
                } else {
                    None
                },
                depends: args.depends,
                parallelizable: args.parallelizable,
                allows_parallel: args.allows_parallel,
                abandonability: Some(args.abandonability.into()),
                description: args.description,
                ical_uid: None,
                habit_id: None,
                fixed: args.fixed,
                habit_step_id: None,
                quantity_total,
                quantity_done,
                quantity_unit: args.quantity_unit,
                original_quantity_total,
            };
            let task = app.create_task(&body).await?;
            mode.formatter().display_tasks(&[task], tz, &habit_map);
        }

        TaskVerbs::Edit(args) => {
            let task = app.get_task(&args.id).await?;
            if !args.has_patch_flag() {
                let all_tasks = app.list_tasks(&Default::default()).await?;
                let update = editor::edit_task(&task, &all_tasks, &habit_map, tz)
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let updated = app.update_task(&args.id, &update).await?;
                mode.formatter().display_tasks(&[updated], tz, &habit_map);
            } else {
                let avg_minutes = args
                    .time
                    .as_ref()
                    .map(|s| parse_duration(s))
                    .transpose()
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let sigma_minutes = args
                    .sigma
                    .as_ref()
                    .map(|s| parse_duration(s))
                    .transpose()
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let abandonability = args.abandonability.map(Abandonability::new);
                let quantity_total = args
                    .quantity_total
                    .map(Quantity::new)
                    .transpose()
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let quantity_done = args
                    .quantity_done
                    .map(Quantity::new)
                    .transpose()
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let original_quantity_total = args
                    .original_quantity_total
                    .map(Quantity::new)
                    .transpose()
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;

                let body = takusu_contracts::UpdateTask {
                    title: args.title,
                    description: args.description,
                    start_at: args.at.map(|s| parse_dt(&s, tz)).transpose()?.map(Some),
                    end_at: args.due.map(|s| parse_dt(&s, tz)).transpose()?,
                    avg_minutes,
                    sigma_minutes,
                    depends: args.depends,
                    parallelizable: args.parallelizable,
                    allows_parallel: args.allows_parallel,
                    abandonability,
                    status: args.status,
                    habit_id: None,
                    user_edited: None,
                    fixed: args.fixed,
                    habit_step_id: None,
                    quantity_total,
                    quantity_done,
                    quantity_unit: args.quantity_unit,
                    original_quantity_total,
                };
                let updated = app.update_task(&args.id, &body).await?;
                mode.formatter().display_tasks(&[updated], tz, &habit_map);
            }
        }

        TaskVerbs::Rm(args) => {
            app.delete_task(&args.id).await?;
            println!("Task {} deleted.", args.id);
        }

        TaskVerbs::Start(args) => {
            let body = StartWorkSession {
                task_id: Some(args.id.clone()),
                title: None,
                note: None,
                quantity_total: None,
                quantity_unit: None,
            };
            let session = app.start_work_session(&body, None).await?;
            let task = if let Some(ref task_id) = session.task_id {
                app.get_task(task_id).await?
            } else {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "work session was not linked to a task".into(),
                )));
            };
            mode.formatter().display_tasks(&[task], tz, &habit_map);
        }

        TaskVerbs::Pause(args) => {
            let session_id = find_open_session_id(app, &args.id).await?;
            let _session = app.pause_work_session(&session_id, None).await?;
            let task = app.get_task(&args.id).await?;
            mode.formatter().display_tasks(&[task], tz, &habit_map);
        }

        TaskVerbs::Done(args) => {
            let session_id = find_open_session_id(app, &args.id).await?;
            let _session = app.complete_work_session(&session_id, None).await?;
            let task = app.get_task(&args.id).await?;
            mode.formatter().display_tasks(&[task], tz, &habit_map);
        }

        TaskVerbs::Skip(args) => {
            let body = takusu_contracts::UpdateTask {
                status: Some(TaskStatus::Skipped),
                ..Default::default()
            };
            let task = app.update_task(&args.id, &body).await?;
            mode.formatter().display_tasks(&[task], tz, &habit_map);
        }

        TaskVerbs::Progress(args) => {
            let quantity_done = Quantity::new(args.quantity)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let session_id = find_open_session_id(app, &args.id).await?;
            let body = RecordWorkSessionProgress {
                quantity_done,
                note: args.note,
                quantity_total: None,
            };
            let result = app
                .record_work_session_progress(&session_id, &body, None)
                .await?;
            if let Some(task) = result.task {
                mode.formatter().display_tasks(&[task], tz, &habit_map);
            }
            if let Some(event) = result.event {
                println!(
                    "recorded: quantity {} (+{}), active {}min",
                    event.quantity_done.unwrap_or(quantity_done),
                    event.delta_quantity.unwrap_or(0),
                    event.active_minutes
                );
            } else {
                println!("no change");
            }
            if result.suggests_completion {
                println!("suggests completion");
            }
        }

        TaskVerbs::Split(args) => {
            let retained_quantity = Quantity::new(args.keep)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let end_at = args.due.map(|s| parse_dt(&s, tz)).transpose()?;
            let body = SplitTask {
                retained_quantity,
                set_dependency: Some(args.dep),
                title: args.title,
                description: args.description,
                end_at,
            };
            let result = app.split_task(&args.id, &body, None).await?;
            let (original, remainder) = (result.original, result.remainder);
            let tasks = vec![original, remainder];
            mode.formatter().display_tasks(&tasks, tz, &habit_map);
        }

        TaskVerbs::Import(args) => {
            let content = read_text_file(&args.file).await?;
            let result = app.import_ical(&content).await?;
            if result.task_ids.is_empty() {
                println!("No tasks imported.");
            } else {
                let id_set: std::collections::HashSet<String> =
                    result.task_ids.iter().cloned().collect();
                let all_tasks = app.list_tasks(&TaskQuery::default()).await?;
                let tasks: Vec<_> = all_tasks
                    .into_iter()
                    .filter(|t| id_set.contains(&t.id))
                    .collect();
                mode.formatter().display_tasks(&tasks, tz, &habit_map);
            }
        }

        TaskVerbs::Deps(args) => {
            if args.check {
                deps_check_tasks(app).await?;
            } else {
                display_task_dependencies(app, &habit_map).await?;
            }
        }
    }
    Ok(())
}

async fn display_task_dependencies(
    app: &TakusuApp,
    habit_map: &std::collections::HashMap<String, i64>,
) -> Result<(), AppError> {
    let tasks = app.list_tasks(&Default::default()).await?;
    let task_map: std::collections::HashMap<String, &takusu_contracts::TaskRow> =
        tasks.iter().map(|t| (t.id.clone(), t)).collect();

    let mut printed = false;
    for t in &tasks {
        if !t.depends.is_empty() {
            printed = true;
            let deps: Vec<String> = t
                .depends
                .iter()
                .map(|dep_id| {
                    task_map
                        .get(dep_id)
                        .map(|dep| task_ref::task_reference(dep, habit_map))
                        .unwrap_or_else(|| dep_id.clone())
                })
                .collect();
            println!(
                "{} -> {}",
                task_ref::task_reference(t, habit_map),
                deps.join(", ")
            );
        }
    }
    if !printed {
        println!("No dependencies found.");
    }
    Ok(())
}

async fn run_schedule_verbs(
    mode: DisplayMode,
    app: &TakusuApp,
    tz: &jiff::tz::TimeZone,
    cmd: ScheduleVerbs,
) -> Result<(), AppError> {
    let habit_map = habit_display_map(app).await;
    match cmd {
        ScheduleVerbs::Agenda(args) => {
            let schedule = app.get_schedule().await?;
            let mut entries: Vec<ScheduleEntry> = schedule.schedule.as_inner().clone();

            if let Some(day) = args.day {
                let (start, end) = day_range(&day, tz)?;
                entries.retain(|e| !(e.end_at < start || e.start_at > end));
            }

            let tasks = app
                .list_tasks(&TaskQuery::default())
                .await
                .unwrap_or_default();
            mode.formatter()
                .display_schedule(&entries, &tasks, tz, &habit_map);
        }

        ScheduleVerbs::Plan(args) => {
            let has_tasks = args.tasks.as_ref().is_some_and(|v| !v.is_empty());
            let has_range = args.from.is_some() || args.until.is_some();
            let has_pin = args.pin.as_ref().is_some_and(|v| !v.is_empty());

            if has_pin && !has_range && !has_tasks {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "--pin requires --from/--until or --tasks".into(),
                )));
            }

            let has_partial = has_range || has_tasks || has_pin;

            if !has_partial {
                let body = GenerateSchedule {
                    task_ids: None,
                    sleep: args.sleep,
                };
                let schedule = app.generate_schedule(&body).await?;
                let entries: Vec<ScheduleEntry> = schedule.schedule.as_inner().clone();
                let tasks = app
                    .list_tasks(&TaskQuery::default())
                    .await
                    .unwrap_or_default();
                mode.formatter()
                    .display_schedule(&entries, &tasks, tz, &habit_map);
            } else {
                let schedule_mode = if has_tasks {
                    ScheduleMode::Tasks
                } else if has_range {
                    ScheduleMode::Range
                } else {
                    unreachable!("pin-only case is handled above")
                };

                if schedule_mode == ScheduleMode::Range
                    && (args.from.is_none() || args.until.is_none())
                {
                    return Err(AppError::BadRequest(BadRequestKind::Other(
                        "--from and --until are both required for range mode".into(),
                    )));
                }

                let body = Reschedule {
                    mode: schedule_mode,
                    from: args
                        .from
                        .map(|s| parse_dt(&s, tz).map(|t| t.to_string()))
                        .transpose()?,
                    until: args
                        .until
                        .map(|s| parse_dt(&s, tz).map(|t| t.to_string()))
                        .transpose()?,
                    task_ids: if schedule_mode == ScheduleMode::Tasks {
                        args.tasks
                    } else {
                        None
                    },
                    pinned: args.pin.unwrap_or_default(),
                    sleep: args.sleep,
                };
                let schedule = app.reschedule(&body).await?;
                let entries: Vec<ScheduleEntry> = schedule.schedule.as_inner().clone();
                let tasks = app
                    .list_tasks(&TaskQuery::default())
                    .await
                    .unwrap_or_default();
                mode.formatter()
                    .display_schedule(&entries, &tasks, tz, &habit_map);
            }
        }

        ScheduleVerbs::Move(args) => {
            let result = app
                .move_entry(&args.task_id, parse_dt(&args.start_at, tz)?, args.force, None)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }

        ScheduleVerbs::Unplan => {
            app.clear_schedule().await?;
            println!("Schedule cleared.");
        }
    }
    Ok(())
}

fn day_range(day: &str, tz: &jiff::tz::TimeZone) -> Result<(Timestamp, Timestamp), AppError> {
    let date = parse_date(day)?.to_jiff();
    let start = date
        .at(0, 0, 0, 0)
        .to_zoned(tz.clone())
        .map_err(|e| AppError::BadRequest(BadRequestKind::Other(format!("invalid day: {e}"))))?
        .timestamp()
        .into();
    let end = date
        .at(23, 59, 59, 0)
        .to_zoned(tz.clone())
        .map_err(|e| AppError::BadRequest(BadRequestKind::Other(format!("invalid day: {e}"))))?
        .timestamp()
        .into();
    Ok((start, end))
}

async fn run_habit(mode: DisplayMode, app: &TakusuApp, cmd: HabitCommands) -> Result<(), AppError> {
    match cmd {
        HabitCommands::Ls => {
            let habits = app.list_habits().await?;
            mode.formatter().display_habits(&habits);
        }

        HabitCommands::Show(args) => {
            let detail = app.get_habit(&args.id).await?;
            mode.formatter().display_habit_detail(&detail.habit);
            if !detail.steps.is_empty() {
                println!("   steps:");
                for s in &detail.steps {
                    let deps: Vec<String> = s.depends_on.to_vec();
                    println!(
                        "     {} [{}] {} ({}–{}, {}min){}",
                        s.id,
                        s.position,
                        s.title,
                        s.start_time,
                        s.end_time,
                        s.avg_minutes,
                        if deps.is_empty() {
                            String::new()
                        } else {
                            format!(" ← {}", deps.join(","))
                        }
                    );
                }
            }
            let spans = app
                .list_habit_scheduled_spans(&args.id)
                .await
                .unwrap_or_default();
            if !spans.is_empty() {
                let label = if detail.habit.active {
                    "scheduled spans (pauses)"
                } else {
                    "scheduled spans (activation windows)"
                };
                println!("   {label}:");
                for s in &spans {
                    println!(
                        "     {} {}..{} ({})",
                        s.id,
                        s.start_date,
                        s.end_date,
                        s.reason.as_deref().unwrap_or("")
                    );
                }
            }
        }

        HabitCommands::Add(args) => {
            let interactive = is_interactive();
            let title = require_or_prompt("Title", args.title, interactive)?;
            let recurrence = require_or_prompt(
                "Recurrence (e.g. daily, weekdays, Mon,Wed,Fri)",
                args.recurrence,
                interactive,
            )?;
            let start_time = require_or_prompt("Start time (HH:MM)", args.start_time, interactive)?;
            let end_time = require_or_prompt("End time (HH:MM)", args.end_time, interactive)?;
            let avg_minutes = parse_duration(&args.avg_time)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let sigma_minutes: i64 = parse_duration(&args.sigma_time)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let body = CreateHabit {
                title,
                recurrence,
                start_time: parse_time(&start_time)?,
                end_time: parse_time(&end_time)?,
                avg_minutes,
                sigma_minutes: if sigma_minutes > 0 {
                    Some(sigma_minutes)
                } else {
                    None
                },
                parallelizable: if args.parallelizable {
                    Some(true)
                } else {
                    None
                },
                allows_parallel: if args.allows_parallel {
                    Some(true)
                } else {
                    None
                },
                abandonability: Some(args.abandonability.into()),
                description: args.description,
                fixed: if args.fixed { Some(true) } else { None },
                window_mode: args.window,
            };
            let habit = app.create_habit(&body).await?;
            mode.formatter().display_habit_detail(&habit);
        }

        HabitCommands::Edit(args) => {
            let detail = app.get_habit(&args.id).await?;
            let habit = &detail.habit;

            if !args.has_patch_flag() {
                let update = editor::edit_habit(habit)
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let updated = app.update_habit(&args.id, &update).await?;
                mode.formatter().display_habit_detail(&updated);
            } else {
                let avg_minutes = args
                    .avg_time
                    .as_ref()
                    .map(|s| parse_duration(s))
                    .transpose()
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let sigma_minutes = args
                    .sigma_time
                    .as_ref()
                    .map(|s| parse_duration(s))
                    .transpose()
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let abandonability = args.abandonability.map(Abandonability::new);
                let body = UpdateHabit {
                    title: args.title,
                    description: args.description,
                    recurrence: args.recurrence,
                    start_time: args.start_time.map(|s| parse_time(&s)).transpose()?,
                    end_time: args.end_time.map(|s| parse_time(&s)).transpose()?,
                    avg_minutes,
                    sigma_minutes,
                    parallelizable: args.parallelizable,
                    allows_parallel: args.allows_parallel,
                    abandonability,
                    active: args.active,
                    fixed: args.fixed,
                    window_mode: args.window,
                };
                let updated = app.update_habit(&args.id, &body).await?;
                mode.formatter().display_habit_detail(&updated);
            }
        }

        HabitCommands::Rm(args) => {
            app.delete_habit(&args.id).await?;
            println!("Habit {} deleted.", args.id);
        }

        HabitCommands::Pause(args) => {
            let body = CreateHabitScheduledSpan {
                start_date: parse_date(&args.from)?,
                end_date: parse_date(&args.to)?,
                reason: args.reason,
            };
            let span = app.create_habit_scheduled_span(&args.id, &body).await?;
            println!(
                "Pause added: {} {}..{} ({})",
                span.id,
                span.start_date,
                span.end_date,
                span.reason.as_deref().unwrap_or("")
            );
        }

        HabitCommands::Pauses(command) => run_habit_pauses(mode, app, command).await?,

        HabitCommands::Steps(command) => run_habit_steps(mode, app, command).await?,
    }
    Ok(())
}

async fn run_habit_pauses(
    mode: DisplayMode,
    app: &TakusuApp,
    cmd: HabitPausesCommands,
) -> Result<(), AppError> {
    match cmd {
        HabitPausesCommands::Ls(args) => {
            if let Some(id) = args.id {
                let spans = app.list_habit_scheduled_spans(&id).await?;
                if spans.is_empty() {
                    println!("No pauses for habit {id}.");
                } else {
                    for s in &spans {
                        println!(
                            "{}\t{}\t{}\t{}",
                            s.id,
                            s.start_date,
                            s.end_date,
                            s.reason.as_deref().unwrap_or("")
                        );
                    }
                }
            } else {
                let (spans, habits) =
                    tokio::try_join!(app.list_all_habit_scheduled_spans(), app.list_habits())?;
                mode.formatter()
                    .display_all_habit_scheduled_spans(&spans, &habits);
            }
        }
        HabitPausesCommands::Rm(args) => {
            app.delete_habit_scheduled_span(&args.habit_id, &args.span_id)
                .await?;
            println!("Pause {} removed.", args.span_id);
        }
    }
    Ok(())
}

async fn run_habit_steps(
    mode: DisplayMode,
    app: &TakusuApp,
    cmd: HabitStepsCommands,
) -> Result<(), AppError> {
    match cmd {
        HabitStepsCommands::Ls(args) => {
            if let Some(id) = args.id {
                let steps = app.list_habit_steps(&id).await?;
                mode.formatter().display_habit_steps(&steps);
            } else {
                let (steps, habits) =
                    tokio::try_join!(app.list_all_habit_steps(), app.list_habits())?;
                mode.formatter().display_all_habit_steps(&steps, &habits);
            }
        }
        HabitStepsCommands::Edit(args) => {
            let steps = app.list_habit_steps(&args.id).await?;
            let inputs = editor::edit_steps(&args.id, &steps)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let replaced = app.replace_habit_steps(&args.id, &inputs).await?;
            mode.formatter().display_habit_steps(&replaced);
        }
        HabitStepsCommands::Set(args) => {
            let content = read_text_file(&args.file).await?;
            let inputs = editor::parse_edited_steps(&content)
                .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
            let replaced = app.replace_habit_steps(&args.id, &inputs).await?;
            mode.formatter().display_habit_steps(&replaced);
        }
        HabitStepsCommands::Check(args) => {
            deps_check_steps(app, &args.id).await?;
        }
    }
    Ok(())
}

async fn read_text_file(path: &str) -> Result<String, AppError> {
    match path {
        "-" => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                AppError::BadRequest(BadRequestKind::Other(format!("failed to read stdin: {e}")))
            })?;
            Ok(buf)
        }
        path => tokio::fs::read_to_string(path).await.map_err(|e| {
            AppError::BadRequest(BadRequestKind::Other(format!("failed to read {path}: {e}")))
        }),
    }
}

async fn run_skill(mode: DisplayMode, app: &TakusuApp, cmd: SkillCommands) -> Result<(), AppError> {
    match cmd {
        SkillCommands::Ls => {
            let skills = app.list_skills().await?;
            mode.formatter().display_skills(&skills);
        }
        SkillCommands::Show(args) => {
            let skill = app.get_skill(&args.slug).await?;
            mode.formatter().display_skill_detail(&skill);
        }
        SkillCommands::Add(args) => {
            let interactive = is_interactive();
            let slug = require_or_prompt("Slug", args.slug, interactive)?;
            let name = require_or_prompt("Name", args.name, interactive)?;
            let description = match args.description {
                Some(d) => d,
                None if interactive => prompt("Description (optional)")?,
                None => String::new(),
            };
            let body_path =
                require_or_prompt("Body file (or - for stdin)", args.body, interactive)?;
            let body = read_skill_body(Some(body_path)).await?;
            let body = body.ok_or_else(|| {
                AppError::BadRequest(BadRequestKind::Other("body is required".into()))
            })?;
            let created = app
                .create_skill(&CreateSkill {
                    slug,
                    name,
                    description,
                    body,
                    built_in: None,
                })
                .await?;
            mode.formatter().display_skill_detail(&created);
        }
        SkillCommands::Edit(args) => {
            if args.name.is_none() && args.description.is_none() && args.body.is_none() {
                let skill = app.get_skill(&args.slug).await?;
                let original = format!(
                    "# Edit skill body. Lines starting with '#' are comments.\n{}",
                    skill.body
                );
                let edited = editor::open_editor(&original, &args.slug, "txt")
                    .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
                let body = edited
                    .lines()
                    .filter(|line| !line.trim_start().starts_with('#'))
                    .collect::<Vec<_>>()
                    .join("\n");
                let body = body.trim().to_string();
                if body == skill.body {
                    println!("No changes.");
                    return Ok(());
                }
                let updated = app
                    .update_skill(
                        &args.slug,
                        &takusu_contracts::UpdateSkill {
                            name: None,
                            description: None,
                            body: Some(body),
                        },
                    )
                    .await?;
                mode.formatter().display_skill_detail(&updated);
            } else {
                let body = read_skill_body(args.body).await?;
                if args.name.is_none() && args.description.is_none() && body.is_none() {
                    return Err(AppError::BadRequest(BadRequestKind::Other(
                        "at least one of --name, --description, or --body is required".into(),
                    )));
                }
                let updated = app
                    .update_skill(
                        &args.slug,
                        &takusu_contracts::UpdateSkill {
                            name: args.name,
                            description: args.description,
                            body,
                        },
                    )
                    .await?;
                mode.formatter().display_skill_detail(&updated);
            }
        }
        SkillCommands::Rm(args) => {
            app.delete_skill(&args.slug).await?;
            println!("Skill {} deleted.", args.slug);
        }
    }
    Ok(())
}

async fn read_skill_body(path: Option<String>) -> Result<Option<String>, AppError> {
    match path.as_deref() {
        None => Ok(None),
        Some("-") => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                AppError::BadRequest(BadRequestKind::Other(format!("failed to read stdin: {e}")))
            })?;
            Ok(Some(buf))
        }
        Some(path) => tokio::fs::read_to_string(path)
            .await
            .map(Some)
            .map_err(|e| {
                AppError::BadRequest(BadRequestKind::Other(format!("failed to read {path}: {e}")))
            }),
    }
}

async fn run_memory(app: &TakusuApp, cmd: MemoryCommands) -> Result<(), AppError> {
    match cmd {
        MemoryCommands::Show(args) => {
            let memory = app.get_memory(&args.id).await?;
            println!("{}", serde_json::to_string_pretty(&memory).unwrap());
        }
        MemoryCommands::Add(args) => {
            let subject_type = args.subject_type.map(SubjectType::from);
            let body = CreateMemory {
                kind: args.kind,
                key: args.key,
                content: args.content,
                subject_type,
                subject_id: args.subject_id,
                upsert: args.upsert,
            };
            let memory = app.create_memory(&body, None).await?;
            println!("{}", serde_json::to_string_pretty(&memory).unwrap());
        }
        MemoryCommands::Edit(args) => {
            let memory = app.get_memory(&args.id).await?;
            let body = UpdateMemory {
                observed_revision: memory.revision,
                content: Some(args.content),
            };
            let memory = app.update_memory(&args.id, &body, None).await?;
            println!("{}", serde_json::to_string_pretty(&memory).unwrap());
        }
        MemoryCommands::Rm(args) => {
            let memory = app.get_memory(&args.id).await?;
            app.delete_memory(&args.id, memory.revision, None).await?;
            println!("Memory {} deleted.", args.id);
        }
        MemoryCommands::Ls(args) => {
            let query = MemoryQuery {
                q: String::new(),
                kind: None,
                subject_type: None,
                subject_id: None,
                limit: args.limit,
            };
            let rows = app.search_memories(&query).await?;
            println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        }
        MemoryCommands::Search(args) => {
            let subject_type = args.subject_type.map(SubjectType::from);
            let query = MemoryQuery {
                q: args.q,
                kind: args.kind,
                subject_type,
                subject_id: args.subject_id,
                limit: args.limit,
            };
            let rows = app.search_memories(&query).await?;
            println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        }
        MemoryCommands::Similar(args) => {
            let query = SimilarTaskQuery {
                title: args.title,
                limit: args.limit,
            };
            let rows = app.find_similar_tasks(&query).await?;
            println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        }
    }
    Ok(())
}

async fn run_token(mode: DisplayMode, app: &TakusuApp, cmd: TokenCommands) -> Result<(), AppError> {
    match cmd {
        TokenCommands::Add(args) => {
            let resp = app.create_token(args.label.as_deref()).await?;
            println!("Token issued:");
            println!("  ID:    {}", resp.id);
            println!("  Token: {}", resp.token);
            println!("  Label: {}", resp.label.as_deref().unwrap_or("—"));
            println!("  Created: {}", resp.created_at);
            eprintln!("\nWarning: Save the token value; it won't be shown again.");
        }
        TokenCommands::Ls => {
            let tokens = app.list_tokens().await?;
            mode.formatter().display_tokens(&tokens);
        }
        TokenCommands::Rm(args) => {
            app.revoke_token(args.id).await?;
            println!("Token {} revoked.", args.id);
        }
    }
    Ok(())
}

async fn run_sync(app: &TakusuApp, cmd: SyncCommands) -> Result<(), AppError> {
    match cmd {
        SyncCommands::Status => {
            let settings = app.get_gcal_settings().await?;
            println!("Google Calendar sync settings:");
            println!("  enabled:          {}", settings.enabled);
            println!("  calendar_id:      {}", settings.calendar_id);
            println!("  client_id:        {}", settings.client_id);
            println!("  has_client_secret: {}", settings.has_client_secret);
            println!("  has_refresh_token:  {}", settings.has_refresh_token);
            print_optional_i64("reminder_minutes", settings.reminder_minutes);
            print_optional_i64("color_id", settings.color_id);
            print_optional_str("visibility", settings.visibility);
            print_optional_str("transparency", settings.transparency);
        }
        SyncCommands::Setup(args) => {
            let (
                enabled,
                calendar_id,
                client_id,
                client_secret,
                refresh_token,
                reminder_minutes,
                color_id,
                visibility,
                transparency,
            ) = if !args.no_ask && is_interactive()
            {
                let settings = app.get_gcal_settings().await?;
                let enabled = match args.enabled {
                    Some(v) => Some(v),
                    None => prompt_bool("enabled", settings.enabled)?,
                };
                let calendar_id = match args.calendar_id {
                    Some(v) => Some(v),
                    None => prompt_optional("calendar_id", &settings.calendar_id)?,
                };
                let client_id = match args.client_id {
                    Some(v) => Some(v),
                    None => prompt_optional("client_id", &settings.client_id)?,
                };
                let client_secret = match args.client_secret {
                    Some(v) => Some(v),
                    None => prompt_secret_optional("client_secret", settings.has_client_secret)?,
                };
                let refresh_token = match args.refresh_token {
                    Some(v) => Some(v),
                    None => prompt_secret_optional("refresh_token", settings.has_refresh_token)?,
                };
                let reminder_minutes = match args.reminder_minutes {
                    Some(v) => Some(v),
                    None => prompt_optional_i64(
                        "reminder_minutes",
                        settings.reminder_minutes,
                    )?,
                };
                let color_id = match args.color_id {
                    Some(v) => Some(v),
                    None => prompt_optional_i64("color_id", settings.color_id)?,
                };
                let visibility = match args.visibility {
                    Some(v) => Some(v),
                    None => prompt_optional_str(
                        "visibility",
                        &settings.visibility,
                        &["default", "public", "private", "confidential"],
                    )?,
                };
                let transparency = match args.transparency {
                    Some(v) => Some(v),
                    None => prompt_optional_str(
                        "transparency",
                        &settings.transparency,
                        &["opaque", "transparent"],
                    )?,
                };
                (
                    enabled,
                    calendar_id,
                    client_id,
                    client_secret,
                    refresh_token,
                    reminder_minutes,
                    color_id,
                    visibility,
                    transparency,
                )
            } else {
                (
                    args.enabled,
                    args.calendar_id,
                    args.client_id,
                    args.client_secret,
                    args.refresh_token,
                    args.reminder_minutes,
                    args.color_id,
                    args.visibility,
                    args.transparency,
                )
            };
            let body = takusu_contracts::UpdateGoogleCalSettings {
                enabled,
                calendar_id,
                client_id,
                client_secret,
                refresh_token,
                reminder_minutes: reminder_minutes.map(Some),
                color_id: color_id.map(Some),
                visibility: visibility.map(Some),
                transparency: transparency.map(Some),
            };
            let settings = app.update_gcal_settings(&body).await?;
            println!("Sync settings updated:");
            println!("  enabled:           {}", settings.enabled);
            println!("  calendar_id:      {}", settings.calendar_id);
            println!("  has_client_secret: {}", settings.has_client_secret);
            println!("  has_refresh_token:  {}", settings.has_refresh_token);
            print_optional_i64("reminder_minutes", settings.reminder_minutes);
            print_optional_i64("color_id", settings.color_id);
            print_optional_str("visibility", settings.visibility);
            print_optional_str("transparency", settings.transparency);
        }
        SyncCommands::Login(args) => {
            oauth_login(
                app,
                args.client_id,
                args.client_secret,
                args.calendar_id,
                args.port,
                args.no_browser,
            )
            .await?;
        }
        SyncCommands::Run => {
            app.do_sync().await.map_err(AppError::Internal)?;
            println!("Sync triggered.");
        }
        SyncCommands::Purge => {
            let result = app.delete_all_gcal_events().await?;
            println!("Deleted {} Google Calendar event(s).", result.deleted);
            if !result.failed.is_empty() {
                eprintln!("{} deletion(s) failed:", result.failed.len());
                for f in &result.failed {
                    eprintln!("  - {}: {}", f.task_id, f.error);
                }
            }
        }
        SyncCommands::Mappings => {
            let rows = app.list_gcal_mappings().await?;
            if rows.is_empty() {
                println!("(no mappings)");
            } else {
                for row in rows {
                    println!("{} -> {}", row.task_id, row.google_event_id);
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn oauth_callback_handler(
    State(tx): State<tokio::sync::mpsc::Sender<Result<String, String>>>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Html<&'static str> {
    if let Some(error) = query.error {
        let msg = match query.error_description {
            Some(desc) => format!("{error}: {desc}"),
            None => error,
        };
        let _ = tx.send(Err(msg)).await;
        return Html(
            "<html><body><h1>認証に失敗しました</h1><p>ターミナルを確認してください。</p></body></html>",
        );
    }
    if let Some(code) = query.code {
        let _ = tx.send(Ok(code)).await;
        return Html(
            "<html><body><h1>認証成功</h1><p>このウィンドウを閉じて、ターミナルに戻ってください。</p></body></html>",
        );
    }
    Html("<html><body><h1>不正なリクエストです</h1></body></html>")
}

fn open_browser(url: &str) {
    let (program, arg) = if cfg!(target_os = "macos") {
        ("open", None)
    } else if cfg!(target_os = "windows") {
        ("cmd", Some("/c"))
    } else {
        ("xdg-open", None)
    };
    let mut cmd = process::Command::new(program);
    if let Some(a) = arg {
        cmd.arg(a);
    }
    if cfg!(target_os = "windows") {
        cmd.arg("start").arg("").arg(url);
    } else {
        cmd.arg(url);
    }
    let _ = cmd.spawn();
}

fn prompt_secret(label: &str) -> Result<String, AppError> {
    rpassword::prompt_password(format!("{label}: "))
        .map_err(|e| AppError::Internal(format!("failed to read secret: {e}")))
}

fn prompt_optional(label: &str, current: &str) -> Result<Option<String>, AppError> {
    let display = if current.is_empty() {
        "(not set)"
    } else {
        current
    };
    print!("{label} [{display}]: ");
    io::stdout()
        .flush()
        .map_err(|e| AppError::Internal(format!("failed to flush stdout: {e}")))?;
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .map_err(|e| AppError::Internal(format!("failed to read line: {e}")))?;
    let trimmed = buf.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    })
}

fn prompt_optional_str(
    label: &str,
    current: &Option<String>,
    allowed: &[&str],
) -> Result<Option<String>, AppError> {
    let display = current.as_deref().unwrap_or("(not set)");
    let allowed_list = allowed.join(" / ");
    print!("{label} [{display}] ({allowed_list}, empty=keep): ");
    io::stdout()
        .flush()
        .map_err(|e| AppError::Internal(format!("failed to flush stdout: {e}")))?;
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .map_err(|e| AppError::Internal(format!("failed to read line: {e}")))?;
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !allowed.contains(&trimmed) {
        return Err(AppError::BadRequest(BadRequestKind::Other(format!(
            "{label} must be one of: {}",
            allowed_list
        ))));
    }
    Ok(Some(trimmed.to_string()))
}

fn prompt_bool(label: &str, current: bool) -> Result<Option<bool>, AppError> {
    loop {
        print!("{label} [{current}] (true/false/yes/no/1/0, empty=keep): ");
        io::stdout()
            .flush()
            .map_err(|e| AppError::Internal(format!("failed to flush stdout: {e}")))?;
        let mut buf = String::new();
        io::stdin()
            .read_line(&mut buf)
            .map_err(|e| AppError::Internal(format!("failed to read line: {e}")))?;
        let s = buf.trim();
        if s.is_empty() {
            return Ok(None);
        }
        match s.to_lowercase().as_str() {
            "true" | "t" | "yes" | "y" | "1" => return Ok(Some(true)),
            "false" | "f" | "no" | "n" | "0" => return Ok(Some(false)),
            _ => eprintln!("invalid input; enter true/false/yes/no/1/0 or leave empty"),
        }
    }
}

fn print_optional_i64(label: &str, value: Option<i64>) {
    match value {
        Some(v) => println!("  {label}: {v}"),
        None => println!("  {label}: (not set)"),
    }
}

fn print_optional_str(label: &str, value: Option<String>) {
    match value {
        Some(v) => println!("  {label}: {v}"),
        None => println!("  {label}: (not set)"),
    }
}

fn prompt_optional_i64(label: &str, current: Option<i64>) -> Result<Option<i64>, AppError> {
    let display = match current {
        Some(v) => v.to_string(),
        None => "(not set)".to_string(),
    };
    print!("{label} [{display}]: ");
    io::stdout()
        .flush()
        .map_err(|e| AppError::Internal(format!("failed to flush stdout: {e}")))?;
    let mut buf = String::new();
    io::stdin()
        .read_line(&mut buf)
        .map_err(|e| AppError::Internal(format!("failed to read line: {e}")))?;
    let trimmed = buf.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(
            trimmed
                .parse::<i64>()
                .map_err(|_| AppError::BadRequest(BadRequestKind::Other(
                    format!("{label} must be an integer"),
                )))?,
        )
    })
}

fn prompt_secret_optional(label: &str, current_set: bool) -> Result<Option<String>, AppError> {
    let display = if current_set { "(set)" } else { "(not set)" };
    let value = rpassword::prompt_password(format!("{label} [{display}]: "))
        .map_err(|e| AppError::Internal(format!("failed to read secret: {e}")))?;
    let trimmed = value.trim();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    })
}

async fn oauth_login(
    app: &TakusuApp,
    client_id: Option<String>,
    client_secret: Option<String>,
    calendar_id: Option<String>,
    port: u16,
    no_browser: bool,
) -> Result<(), AppError> {
    let settings = app.get_gcal_settings().await?;

    let client_id = if let Some(id) = client_id {
        if id.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "client_id must not be empty".into(),
            )));
        }
        id
    } else if !settings.client_id.is_empty() {
        settings.client_id
    } else if is_interactive() {
        let id = prompt("Google OAuth client_id")?;
        if id.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "client_id is required".into(),
            )));
        }
        id
    } else {
        return Err(AppError::BadRequest(BadRequestKind::Other(
            "client_id is required".into(),
        )));
    };

    let client_secret_opt = if let Some(secret) = client_secret {
        if secret.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "client_secret must not be empty".into(),
            )));
        }
        Some(secret)
    } else if settings.has_client_secret {
        None
    } else if is_interactive() {
        let secret = prompt_secret("Google OAuth client_secret")?;
        if secret.is_empty() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "client_secret is required".into(),
            )));
        }
        Some(secret)
    } else {
        return Err(AppError::BadRequest(BadRequestKind::Other(
            "client_secret is required".into(),
        )));
    };

    let calendar_id = if let Some(id) = calendar_id {
        if id.is_empty() {
            if settings.calendar_id.is_empty() {
                "primary".to_string()
            } else {
                settings.calendar_id
            }
        } else {
            id
        }
    } else if settings.calendar_id.is_empty() {
        "primary".to_string()
    } else {
        settings.calendar_id
    };

    app.update_gcal_settings(&takusu_contracts::UpdateGoogleCalSettings {
        enabled: Some(true),
        calendar_id: Some(calendar_id.clone()),
        client_id: Some(client_id.clone()),
        client_secret: client_secret_opt,
        refresh_token: None,
        reminder_minutes: None,
        color_id: None,
        visibility: None,
        transparency: None,
    })
    .await?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<String, String>>(1);
    let router = Router::new()
        .route("/callback", get(oauth_callback_handler))
        .with_state(tx);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| AppError::Internal(format!("failed to bind callback server: {e}")))?;
    let actual_port = listener
        .local_addr()
        .map_err(|e| AppError::Internal(format!("{e}")))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{actual_port}/callback");
    let auth_url = app.oauth_url(&redirect_uri).await?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, router).with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    });
    let server_handle = tokio::spawn(async move { server.await });

    println!("Starting local callback server on 127.0.0.1:{actual_port}");
    if no_browser {
        println!("Open this URL in your browser:\n  {auth_url}");
    } else {
        open_browser(&auth_url);
    }

    let code = tokio::time::timeout(Duration::from_secs(300), rx.recv())
        .await
        .map_err(|_| AppError::Internal("OAuth callback timed out".into()))?
        .ok_or_else(|| AppError::Internal("callback channel closed".into()))?
        .map_err(|e| AppError::Internal(format!("oauth error: {e}")))?;

    let _ = shutdown_tx.send(());
    let _ = server_handle.await;

    app.oauth_callback(&code, Some(&redirect_uri)).await?;
    println!("Google Calendar OAuth login completed successfully.");
    Ok(())
}

async fn run_config(cmd: ConfigCommands, app: &TakusuApp, cfg: &CliConfig) -> Result<(), AppError> {
    match cmd {
        ConfigCommands::Show => config::show(),
        ConfigCommands::Init => config::init(),
        ConfigCommands::Set(args) => {
            if !is_app_settings_key(&args.key) && !is_local_config_key(&args.key) {
                return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                    "unknown config key: {}",
                    args.key
                ))));
            }

            if is_local_config_key(&args.key) && !is_app_settings_key(&args.key) {
                println!("Config updated: {} = {}", args.key, args.value);
                return Ok(());
            }

            let mut update = UpdateSettings::default();
            let value = &args.value;
            match args.key.as_str() {
                "tz" => update.tz = Some(value.clone()),
                "sleep_start" => update.sleep_start = Some(parse_time(value)?),
                "sleep_end" => update.sleep_end = Some(parse_time(value)?),
                "comfortable" => {
                    let h: f64 = value.parse().map_err(|e| {
                        AppError::BadRequest(BadRequestKind::Other(format!(
                            "invalid comfortable value '{value}': {e}"
                        )))
                    })?;
                    update.comfortable_minutes = Some((h * 60.0).round() as i64);
                }
                "maximum" => {
                    let h: f64 = value.parse().map_err(|e| {
                        AppError::BadRequest(BadRequestKind::Other(format!(
                            "invalid maximum value '{value}': {e}"
                        )))
                    })?;
                    update.maximum_minutes = Some((h * 60.0).round() as i64);
                }
                "solver" => {
                    update.solver = Some(value.parse().map_err(|e| {
                        AppError::BadRequest(BadRequestKind::Other(format!(
                            "invalid solver '{value}': {e}"
                        )))
                    })?);
                }
                "time_budget_ms" => {
                    update.time_budget_ms = Some(value.parse().map_err(|e| {
                        AppError::BadRequest(BadRequestKind::Other(format!(
                            "invalid time_budget_ms '{value}': {e}"
                        )))
                    })?);
                }
                "seed" => {
                    update.seed = Some(value.parse().map_err(|e| {
                        AppError::BadRequest(BadRequestKind::Other(format!(
                            "invalid seed '{value}': {e}"
                        )))
                    })?);
                }
                "warm_start" => {
                    update.warm_start = Some(value.parse().map_err(|e| {
                        AppError::BadRequest(BadRequestKind::Other(format!(
                            "invalid warm_start '{value}': {e}"
                        )))
                    })?);
                }
                _ => {}
            }

            if update.tz.is_none() && cfg.tz.is_some() {
                update.tz = cfg.tz.clone();
            }
            if update.sleep_start.is_none() && cfg.sleep_start.is_some() {
                update.sleep_start = Some(parse_time(cfg.sleep_start.as_deref().unwrap())?);
            }
            if update.sleep_end.is_none() && cfg.sleep_end.is_some() {
                update.sleep_end = Some(parse_time(cfg.sleep_end.as_deref().unwrap())?);
            }

            let resp = app.update_settings(&update).await?;
            let comfortable_h = resp.comfortable_minutes.unwrap_or(0) as f64 / 60.0;
            let maximum_h = resp.maximum_minutes.unwrap_or(0) as f64 / 60.0;
            println!(
                "Settings updated: tz={}, sleep_start={}, sleep_end={}, comfortable={:.2}h, maximum={:.2}h, solver={}, time_budget_ms={:?}, seed={:?}, warm_start={}",
                resp.tz,
                resp.sleep_start,
                resp.sleep_end,
                comfortable_h,
                maximum_h,
                resp.solver,
                resp.time_budget_ms,
                resp.seed,
                resp.warm_start
            );
        }
        ConfigCommands::Workers(cmd) => match cmd {
            ConfigWorkersCommands::Set(args) => {
                app.update_workers_credentials(&args.url, &args.token)
                    .await?;
                config::set("worker_url", &args.url).map_err(AppError::Internal)?;
                config::set("workers_token", &args.token).map_err(AppError::Internal)?;
                println!("Worker config updated.");
            }
            ConfigWorkersCommands::Health => {
                let status = app.health_check().await?;
                println!("{status}");
            }
        },
    }
    Ok(())
}

async fn run_system(app: &TakusuApp, cmd: SystemCommands) -> Result<(), AppError> {
    match cmd {
        SystemCommands::Health => {
            let status = app.health_check().await?;
            println!("{status}");
        }
        SystemCommands::GenRootToken | SystemCommands::License | SystemCommands::Completion(_) => {
            unreachable!("system subcommand is handled before run()")
        }
    }
    Ok(())
}

async fn run_agent(app: Arc<TakusuApp>, args: AgentArgs, plain: bool) -> Result<(), AppError> {
    if let Some(command) = args.command {
        match command {
            AgentSubCommands::Config(cmd) => match cmd {
                AgentConfigCommands::Show => agent::config_show()?,
                AgentConfigCommands::Set { key, value } => agent::config_set(&key, &value)?,
            },
            AgentSubCommands::Allow { key } => agent::permissions_set(&key, "true")?,
            AgentSubCommands::Deny { key } => agent::permissions_set(&key, "false")?,
            AgentSubCommands::Stats(args) => agent::stats(args.clear)?,
        }
    } else {
        agent::run(agent::AgentRunArgs {
            app,
            text: args.text,
            yes: args.yes,
            allow: args.allow,
            deny: args.deny,
            plain,
            continue_session: args.continue_session,
            new_session: args.new_session,
        })
        .await?;
    }
    Ok(())
}

// ── Dependency analysis (#355) ─────────────────────────────────────────

use takusu_local_lib::app::DependencyNode;

fn format_path(via: &[DependencyNode]) -> String {
    via.iter()
        .map(|n| n.title.clone())
        .collect::<Vec<_>>()
        .join("→")
}

/// Remove `to_id` from the `depends` list of task `from_id` via PATCH.
async fn remove_task_dep(app: &TakusuApp, from_id: &str, to_id: &str) -> Result<(), AppError> {
    let task = app.get_task(from_id).await?;
    let mut deps: Vec<String> = task.depends.to_vec();
    deps.retain(|d| d != to_id);
    let body = takusu_contracts::UpdateTask {
        depends: Some(deps),
        ..Default::default()
    };
    app.update_task(from_id, &body).await?;
    Ok(())
}

/// Interactive loop: detect redundant task dependency edges and let the
/// user choose which edge to remove. Iterates through all detected edges;
/// re-analyzes only after a deletion (which may introduce new redundancies
/// or remove some).
async fn deps_check_tasks(app: &TakusuApp) -> Result<(), AppError> {
    let mut redundant = app.analyze_task_dependencies().await?;
    if redundant.is_empty() {
        println!("冗長な依存はありません");
        return Ok(());
    }
    if !is_interactive() {
        println!("冗長な依存が見つかりました:");
        for r in &redundant {
            println!(
                "  「{}」→「{}」  (経路: {})",
                r.from_title,
                r.to_title,
                format_path(&r.via)
            );
        }
        return Ok(());
    }
    let mut idx = 0;
    while idx < redundant.len() {
        let r = &redundant[idx];
        println!(
            "冗長な依存が見つかりました ({}/{}):",
            idx + 1,
            redundant.len()
        );
        println!(
            "  「{}」 の経路があるため「{}」→「{}」 は冗長です。",
            format_path(&r.via),
            r.from_title,
            r.to_title
        );
        let path_pairs: Vec<(String, String)> = r
            .via
            .windows(2)
            .map(|w| (w[0].id.clone(), w[1].id.clone()))
            .collect();
        println!("[1] 冗長な辺 {}→{} を削除", r.from_title, r.to_title);
        for (i, (a, b)) in path_pairs.iter().enumerate() {
            let ta = r.via.iter().find(|n| &n.id == a).unwrap().title.clone();
            let tb = r.via.iter().find(|n| &n.id == b).unwrap().title.clone();
            println!("[2.{}] 経路上の辺 {}→{} を削除", i + 1, ta, tb);
        }
        println!("[s] スキップ  [q] 終了");
        let choice = prompt(">")?;
        if choice == "q" || choice == "Q" {
            return Ok(());
        }
        if choice == "s" || choice == "S" {
            idx += 1;
            continue;
        }
        if choice == "1" {
            remove_task_dep(app, &r.from, &r.to).await?;
            println!("削除しました: {}→{}", r.from_title, r.to_title);
            redundant = app.analyze_task_dependencies().await?;
            if idx >= redundant.len() {
                idx = 0;
            }
            continue;
        }
        if let Some(rest) = choice.strip_prefix("2.")
            && let Ok(n) = rest.parse::<usize>()
            && n >= 1
            && n <= path_pairs.len()
        {
            let (a, b) = &path_pairs[n - 1];
            remove_task_dep(app, a, b).await?;
            println!("削除しました: 経路上の辺");
            redundant = app.analyze_task_dependencies().await?;
            if idx >= redundant.len() {
                idx = 0;
            }
            continue;
        }
        println!("無効な選択です");
    }
    Ok(())
}

/// Remove `to_id` from the `depends_on` of step `from_id` within habit
/// `habit_id` via bulk replace.
async fn remove_step_dep(
    app: &TakusuApp,
    habit_id: &str,
    from_id: &str,
    to_id: &str,
) -> Result<(), AppError> {
    let steps = app.list_habit_steps(habit_id).await?;
    let inputs: Vec<takusu_contracts::HabitStepInput> = steps
        .iter()
        .map(|s| {
            let mut deps: Vec<String> = s.depends_on.to_vec();
            if s.id == from_id {
                deps.retain(|d| d != to_id);
            }
            takusu_contracts::HabitStepInput {
                id: Some(s.id.clone()),
                position: s.position,
                title: s.title.clone(),
                description: s.description.clone(),
                start_time: s.start_time,
                end_time: s.end_time,
                avg_minutes: s.avg_minutes,
                sigma_minutes: if s.sigma_minutes > 0 {
                    Some(s.sigma_minutes)
                } else {
                    None
                },
                parallelizable: Some(s.parallelizable),
                allows_parallel: Some(s.allows_parallel),
                abandonability: Some(s.abandonability),
                fixed: Some(s.fixed),
                depends_on: deps,
            }
        })
        .collect();
    app.replace_habit_steps(habit_id, &inputs).await?;
    Ok(())
}

/// Interactive loop for habit step redundant dependencies (#355).
async fn deps_check_steps(app: &TakusuApp, habit_id: &str) -> Result<(), AppError> {
    let mut redundant = app.analyze_habit_step_dependencies(habit_id).await?;
    if redundant.is_empty() {
        println!("冗長な依存はありません");
        return Ok(());
    }
    if !is_interactive() {
        println!("冗長な依存が見つかりました:");
        for r in &redundant {
            println!(
                "  「{}」→「{}」  (経路: {})",
                r.from_title,
                r.to_title,
                format_path(&r.via)
            );
        }
        return Ok(());
    }
    let mut idx = 0;
    while idx < redundant.len() {
        let r = &redundant[idx];
        println!(
            "冗長な依存が見つかりました ({}/{}):",
            idx + 1,
            redundant.len()
        );
        println!(
            "  「{}」 の経路があるため「{}」→「{}」 は冗長です。",
            format_path(&r.via),
            r.from_title,
            r.to_title
        );
        let path_pairs: Vec<(String, String)> = r
            .via
            .windows(2)
            .map(|w| (w[0].id.clone(), w[1].id.clone()))
            .collect();
        println!("[1] 冗長な辺 {}→{} を削除", r.from_title, r.to_title);
        for (i, (a, b)) in path_pairs.iter().enumerate() {
            let ta = r.via.iter().find(|n| &n.id == a).unwrap().title.clone();
            let tb = r.via.iter().find(|n| &n.id == b).unwrap().title.clone();
            println!("[2.{}] 経路上の辺 {}→{} を削除", i + 1, ta, tb);
        }
        println!("[s] スキップ  [q] 終了");
        let choice = prompt(">")?;
        if choice == "q" || choice == "Q" {
            return Ok(());
        }
        if choice == "s" || choice == "S" {
            idx += 1;
            continue;
        }
        if choice == "1" {
            remove_step_dep(app, habit_id, &r.from, &r.to).await?;
            println!("削除しました: {}→{}", r.from_title, r.to_title);
            redundant = app.analyze_habit_step_dependencies(habit_id).await?;
            if idx >= redundant.len() {
                idx = 0;
            }
            continue;
        }
        if let Some(rest) = choice.strip_prefix("2.")
            && let Ok(n) = rest.parse::<usize>()
            && n >= 1
            && n <= path_pairs.len()
        {
            let (a, b) = &path_pairs[n - 1];
            remove_step_dep(app, habit_id, a, b).await?;
            println!("削除しました: 経路上の辺");
            redundant = app.analyze_habit_step_dependencies(habit_id).await?;
            if idx >= redundant.len() {
                idx = 0;
            }
            continue;
        }
        println!("無効な選択です");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_verb_add_parses_positional_title() {
        let cli = Cli::parse_from(["takusu", "add", "hello"]);
        let Commands::Task(TaskVerbs::Add(args)) = cli.command.expect("subcommand") else {
            panic!("expected TaskVerbs::Add");
        };
        assert_eq!(args.title.as_deref(), Some("hello"));
    }

    #[test]
    fn schedule_verb_plan_range_parses() {
        let cli = Cli::parse_from([
            "takusu",
            "plan",
            "--from",
            "2025-06-05T08:00Z",
            "--until",
            "2025-06-05T18:00Z",
        ]);
        let Commands::Schedule(ScheduleVerbs::Plan(args)) = cli.command.expect("subcommand") else {
            panic!("expected ScheduleVerbs::Plan");
        };
        assert!(args.from.is_some());
        assert!(args.until.is_some());
    }

    #[test]
    fn global_plain_forces_simple() {
        let cli = Cli::parse_from(["takusu", "--plain", "ls"]);
        assert!(cli.plain);
    }

    #[test]
    fn bare_takusu_is_agenda() {
        let cli = Cli::parse_from(["takusu"]);
        assert!(
            cli.command.is_none(),
            "bare takusu should have no subcommand"
        );
    }

    #[test]
    fn task_verb_ls_default_status_is_actionable() {
        let cli = Cli::parse_from(["takusu", "ls"]);
        let Commands::Task(TaskVerbs::Ls(args)) = cli.command.expect("subcommand") else {
            panic!("expected TaskVerbs::Ls");
        };
        assert!(args.status.is_none());
        assert!(!args.all);
    }

    #[test]
    fn task_verb_ls_filter_flags_parse() {
        let cli = Cli::parse_from([
            "takusu",
            "ls",
            "--status",
            "in_progress",
            "--no-overdue",
            "--habit-id",
            "h1",
            "--ical-uid",
            "uid@example",
            "--limit",
            "10",
            "buy",
            "milk",
        ]);
        let Commands::Task(TaskVerbs::Ls(args)) = cli.command.expect("subcommand") else {
            panic!("expected TaskVerbs::Ls");
        };
        assert_eq!(args.status, Some(TaskStatusFilter::InProgress));
        assert!(args.no_overdue);
        assert_eq!(args.habit_id.as_deref(), Some("h1"));
        assert_eq!(args.ical_uid.as_deref(), Some("uid@example"));
        assert_eq!(args.limit, Some(10));
        assert_eq!(args.query, vec!["buy", "milk"]);
    }

    #[test]
    fn schedule_verb_move_positionals_parse() {
        let cli = Cli::parse_from(["takusu", "move", "#5", "2025-06-05T08:00Z"]);
        let Commands::Schedule(ScheduleVerbs::Move(args)) = cli.command.expect("subcommand") else {
            panic!("expected ScheduleVerbs::Move");
        };
        assert_eq!(args.task_id, "#5");
        assert_eq!(args.start_at, "2025-06-05T08:00Z");
    }

    #[test]
    fn schedule_verb_plan_pin_requires_range_or_tasks() {
        let cli = Cli::parse_from(["takusu", "plan", "--pin", "#5"]);
        let Commands::Schedule(ScheduleVerbs::Plan(args)) = cli.command.expect("subcommand") else {
            panic!("expected ScheduleVerbs::Plan");
        };
        assert_eq!(args.pin.as_deref().unwrap(), &["#5"]);
        assert!(args.from.is_none() && args.until.is_none() && args.tasks.is_none());
    }

    #[test]
    fn habit_steps_set_positionals_parse() {
        let cli = Cli::parse_from(["takusu", "habit", "steps", "set", "h1", "steps.json"]);
        let Commands::Habit {
            command: HabitCommands::Steps(HabitStepsCommands::Set(args)),
        } = cli.command.expect("subcommand")
        else {
            panic!("expected HabitStepsCommands::Set");
        };
        assert_eq!(args.id, "h1");
        assert_eq!(args.file, "steps.json");
    }

    #[test]
    fn config_set_positionals_parse() {
        let cli = Cli::parse_from(["takusu", "config", "set", "comfortable", "4"]);
        let Commands::Config {
            command: ConfigCommands::Set(args),
        } = cli.command.expect("subcommand")
        else {
            panic!("expected ConfigCommands::Set");
        };
        assert_eq!(args.key, "comfortable");
        assert_eq!(args.value, "4");
    }

    #[test]
    fn config_workers_set_positionals_parse() {
        let cli = Cli::parse_from(["takusu", "config", "workers", "set", "http://w", "tok"]);
        let Commands::Config {
            command: ConfigCommands::Workers(ConfigWorkersCommands::Set(args)),
        } = cli.command.expect("subcommand")
        else {
            panic!("expected ConfigWorkersCommands::Set");
        };
        assert_eq!(args.url, "http://w");
        assert_eq!(args.token, "tok");
    }

    #[test]
    fn system_completion_positional_parse() {
        let cli = Cli::parse_from(["takusu", "system", "completion", "bash"]);
        let Commands::System {
            command: SystemCommands::Completion(args),
        } = cli.command.expect("subcommand")
        else {
            panic!("expected SystemCommands::Completion");
        };
        assert_eq!(args.shell, clap_complete::Shell::Bash);
    }

    #[test]
    fn agent_repl_flags_and_subcommand_differ() {
        let cli = Cli::parse_from(["takusu", "agent", "--allow", "task:create"]);
        let Commands::Agent(args) = cli.command.expect("subcommand") else {
            panic!("expected Agent");
        };
        assert!(args.text.is_none());
        assert!(args.command.is_none());
        assert_eq!(args.allow, vec!["task:create"]);

        let cli = Cli::parse_from(["takusu", "agent", "allow", "task:create"]);
        let Commands::Agent(args) = cli.command.expect("subcommand") else {
            panic!("expected Agent");
        };
        assert!(args.text.is_none());
        assert!(args.allow.is_empty());
        assert!(
            matches!(args.command, Some(AgentSubCommands::Allow { key }) if key == "task:create")
        );
    }

    #[test]
    fn agent_text_positional_conflicts_with_subcommand() {
        let result = Cli::try_parse_from(["takusu", "agent", "hello", "config"]);
        assert!(
            result.is_err(),
            "positional text and subcommand must conflict"
        );
    }
}
