#![allow(clippy::manual_async_fn)]
//! Dispatch table for `execute_proposed_change` arms.
//!
//! Each `(TargetKind, ChangeOperation)` pair is implemented as a separate
//! `ChangeExecutor` impl and registered in [`dispatch`]. Adding a new
//! operation only requires adding a new impl and a `dispatch` entry, rather
//! than editing a central 20+ arm `match`. The arms previously lived inline
//! in `AgentSession::execute_proposed_change`; see issue #1222.
//!
//! Target fetching is also resolved through [`dispatch`] via
//! [`ChangeExecutor::fetch_target`], so adding a new `TargetKind` only
//! requires editing this module; see issue #1330.
//!
//! Error-mapping conventions:
//! - Task/Habit/Skill/Schedule create/update/delete/fetch client failures
//!   map to `ToolError::Other`.
//! - Memory client failures map via `tools::memory::client_error`.
//! - Task work-state operations (start/pause/progress/complete/split) map
//!   via `tools::takusu::client_error`, which classifies 400 as `InvalidArgs`,
//!   404 as `NotFound`, and 409 as `Conflict`.
//! - `Move` keeps its own mapping: 409 becomes `Conflict`, and all other
//!   client errors become `ToolError::Other`.

use std::future::Future;
use std::str::FromStr;

use async_trait::async_trait;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use takusu_client::{
    Client, CreateHabit, CreateHabitScheduledSpan, CreateMemory, CreateSkill, CreateTask,
    HabitDetail, MoveEntry, RecordWorkSessionProgress, SaveScheduleRequest, ScheduleEntry,
    SplitTask, StartWorkSession, UpdateHabit, UpdateMemory, UpdateSkill, UpdateTask,
};
use takusu_types::Timestamp;

use crate::tools::memory::client_error as memory_client_error;
use crate::tools::takusu::client_error as takusu_client_error;
use crate::{
    AgentError, AgentSession, ChangeOperation, InvalidArgsError, ProposedChange, TargetKind,
    ToolError,
};

/// Outcome of executing a single proposed-change arm.
///
/// Mirrors the tuple previously returned by the central `match`:
/// `(result_id, before, after, target_revision)`.
pub(crate) struct ExecutionOutcome {
    pub result_id: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub target_revision: Option<i64>,
}

async fn open_session_for_task(client: &Client, task_id: &str) -> Result<String, AgentError> {
    client
        .open_work_session_for_task(task_id)
        .await
        .map_err(|e| AgentError::Tool(takusu_client_error(e)))?
        .map(|s| s.id)
        .ok_or_else(|| {
            AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                "no open work session for task",
            )))
        })
}

/// Resolved target identity and freshness snapshot fetched before executing
/// a proposed change.
///
/// `target_id` is empty for `Create` and `Schedule` operations, which do not
/// address an existing row. `existing_habit` is only populated for `Habit`
/// targets, where `Update` needs the current steps to diff against.
#[derive(Default)]
pub(crate) struct TargetInfo {
    pub target_id: String,
    pub current_updated_at: Option<Timestamp>,
    pub existing_habit: Option<HabitDetail>,
}

/// Read-only context handed to [`ChangeExecutor::fetch_target`].
///
/// This is a slimmer view than [`ChangeContext`] because the target id and
/// habit detail are not known until `fetch_target` runs.
pub(crate) struct FetchContext<'a> {
    pub session: &'a AgentSession,
    pub change: &'a ProposedChange,
}

impl<'a> FetchContext<'a> {
    fn client(&self) -> &takusu_client::Client {
        self.session.client()
    }
}

/// Shared context handed to every `ChangeExecutor` arm.
pub(crate) struct ChangeContext<'a> {
    pub session: &'a AgentSession,
    /// Resolved target id (empty for `Create` and `Schedule`).
    pub target_id: String,
    /// Arguments with the `steps` field already stripped out.
    pub args: Value,
    /// The `steps` field removed from `args`, if present.
    pub steps_value: Option<Value>,
    /// Fetched habit detail for `Habit` targets, used by `Update` to diff
    /// existing steps.
    pub existing_habit: Option<HabitDetail>,
    /// Operation id propagated to the server for work-state tracking.
    pub operation_id: Option<&'a str>,
    /// The original proposed change; used for `before` snapshots.
    pub change: &'a ProposedChange,
}

impl<'a> ChangeContext<'a> {
    fn client(&self) -> &takusu_client::Client {
        self.session.client()
    }
}

/// Execute one `(TargetKind, ChangeOperation)` arm.
///
/// Implementors are zero-sized markers; all state comes from [`ChangeContext`].
/// This trait is object-safe: `dispatch` returns `&'static dyn ChangeExecutor`.
#[async_trait]
pub(crate) trait ChangeExecutor: Send + Sync {
    /// Resolve the existing target row addressed by the proposed change.
    ///
    /// Returns an empty [`TargetInfo`] for `Create` and `Schedule` operations,
    /// which do not address an existing row. For other operations the
    /// per-kind fetch is resolved through the same `dispatch` table that
    /// selects [`execute`](Self::execute), so adding a new `TargetKind` only
    /// requires editing this module (see issue #1330).
    async fn fetch_target(&self, ctx: &FetchContext<'_>) -> Result<TargetInfo, AgentError>;

    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError>;
}

/// Typed extension of [`ChangeExecutor`] that associates each executor with
/// the concrete argument type it deserializes from `ChangeContext::args`.
///
/// This trait is **not** object-safe (it has an associated type), so it is
/// only used in concrete impls. A blanket impl bridges every `ChangeHandler`
/// to the object-safe `ChangeExecutor` by deserializing the args and
/// forwarding to [`execute_typed`](ChangeHandler::execute_typed).
///
/// This replaces the ad-hoc `from_args::<T>(&ctx.args)?` calls that were
/// scattered inside each `execute` body, making the expected argument type
/// visible at the trait level and catching type mismatches at compile time.
///
/// Uses native `async fn` in trait (stabilized in Rust 1.75 / edition 2024)
/// instead of `async_trait` or manual `Pin<Box<dyn Future>>` boxing, since
/// this is a crate-private trait and the object-safety / edition-mismatch
/// concerns that apply to public traits do not apply here.
pub(crate) trait ChangeHandler: Send + Sync {
    /// Concrete type deserialized from `ChangeContext::args`.
    type Args: DeserializeOwned + Send;

    /// Deserialize typed arguments from the raw JSON value.
    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError>;

    /// Execute with typed arguments.
    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send;
}

/// Blanket bridge: every `ChangeHandler` automatically implements the
/// object-safe `ChangeExecutor` by deserializing args and forwarding.
#[async_trait]
impl<T> ChangeExecutor for T
where
    T: ChangeHandler,
{
    async fn fetch_target(&self, ctx: &FetchContext<'_>) -> Result<TargetInfo, AgentError> {
        // `Create` and `Schedule` never address an existing row.
        if matches!(ctx.change.operation, ChangeOperation::Create)
            || ctx.change.target.kind == TargetKind::Schedule
        {
            return Ok(TargetInfo::default());
        }
        fetch_target_for_kind(ctx).await
    }

    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let args = T::deserialize_args(&ctx.args)?;
        self.execute_typed(ctx, &args).await
    }
}

// --- error-mapping helpers ---------------------------------------------------

/// Resolve the existing target row for a non-`Create`, non-`Schedule` change.
///
/// Branches on `TargetKind` only; the `(kind, operation)` dispatch is handled
/// by [`dispatch`]. Error-mapping mirrors the original `execute_proposed_change`
/// arms: Task/Habit/Skill use [`other_err`], Memory uses
/// [`memory_client_error`].
async fn fetch_target_for_kind(ctx: &FetchContext<'_>) -> Result<TargetInfo, AgentError> {
    let display_id = &ctx.change.target.display_id;
    match ctx.change.target.kind {
        TargetKind::Task => {
            let task = ctx.client().get_task(display_id).await.map_err(other_err)?;
            Ok(TargetInfo {
                target_id: task.id,
                current_updated_at: Some(task.updated_at),
                existing_habit: None,
            })
        }
        TargetKind::Habit => {
            let habit = ctx
                .client()
                .get_habit(display_id)
                .await
                .map_err(other_err)?;
            Ok(TargetInfo {
                target_id: habit.habit.id.clone(),
                current_updated_at: Some(habit.habit.updated_at),
                existing_habit: Some(habit),
            })
        }
        TargetKind::Skill => {
            let skill = ctx
                .client()
                .get_skill(display_id)
                .await
                .map_err(other_err)?;
            Ok(TargetInfo {
                target_id: skill.slug,
                current_updated_at: Some(skill.updated_at),
                existing_habit: None,
            })
        }
        TargetKind::Memory => {
            let memory = ctx
                .client()
                .get_memory(display_id)
                .await
                .map_err(|e| AgentError::Tool(memory_client_error(e)))?;
            Ok(TargetInfo {
                target_id: memory.id,
                current_updated_at: Some(memory.updated_at),
                existing_habit: None,
            })
        }
        // `Schedule` is short-circuited by the blanket impl; this arm is
        // unreachable but kept exhaustive for `TargetKind`.
        TargetKind::Schedule => Ok(TargetInfo::default()),
    }
}

/// Map a serde failure into the recoverable `InvalidArgs` error used by every
/// arm that deserializes typed arguments.
fn invalid_args(e: impl std::fmt::Display) -> AgentError {
    AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::no_field(
        e.to_string(),
    )))
}

/// Deserialize typed arguments from the JSON value, mapping failures to the
/// recoverable `InvalidArgs` error.
fn from_args<T: DeserializeOwned>(args: &Value) -> Result<T, AgentError> {
    serde_json::from_value::<T>(args.clone()).map_err(invalid_args)
}

/// Box a client/transport error as `ToolError::Other`. This is the mapping
/// used by the Task/Habit/Skill/Schedule arms.
fn other_err<E>(e: E) -> AgentError
where
    E: std::error::Error + Send + Sync + 'static,
{
    AgentError::Tool(ToolError::Other(Box::new(e)))
}

/// Serialize a response row for the `after` snapshot, mapping failures to
/// `ToolError::Other`.
fn to_after<T: serde::Serialize>(value: &T) -> Result<Option<Value>, AgentError> {
    serde_json::to_value(value)
        .map(Some)
        .map_err(|e| AgentError::Tool(ToolError::Other(Box::new(e))))
}

// --- typed argument structs (moved from lib.rs) ------------------------------

/// Serde helper: deserialize a `Date` from a trimmed string.
/// Mirrors the old `str::trim` applied before `Date::from_str`.
fn deserialize_trimmed_date<'de, D>(deserializer: D) -> Result<takusu_types::Date, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: String = String::deserialize(deserializer)?;
    takusu_types::Date::from_str(raw.trim()).map_err(serde::de::Error::custom)
}

/// Serde helper: deserialize a required string, trim whitespace, and reject
/// empty results. Mirrors the old `required_string` + trim behavior.
fn deserialize_trimmed_required_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: String = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err(serde::de::Error::custom("missing or empty"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn default_true() -> bool {
    true
}

/// Typed extraction for `CreateScheduledSpan` arguments.
#[derive(Debug, Deserialize)]
struct ScheduledSpanArgs {
    #[serde(deserialize_with = "deserialize_trimmed_date")]
    start_date: takusu_types::Date,
    #[serde(deserialize_with = "deserialize_trimmed_date")]
    end_date: takusu_types::Date,
    #[serde(default, deserialize_with = "crate::deserialize_trimmed_optional")]
    reason: Option<String>,
}

/// Typed extraction for `DeleteScheduledSpan` arguments.
#[derive(Debug, Deserialize)]
struct DeleteScheduledSpanArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required_string")]
    span_id: String,
}

/// Typed extraction for memory-delete arguments.
#[derive(Debug, Deserialize)]
struct MemoryDeleteArgs {
    observed_revision: i64,
}

/// Typed extraction for move arguments (includes `fixed` which is not part
/// of `MoveEntry`).
#[derive(Debug, Deserialize)]
struct MoveArgs {
    start_at: takusu_types::Timestamp,
    #[serde(default)]
    force: bool,
    #[serde(default = "default_true")]
    fixed: bool,
}

// --- Task arms ---------------------------------------------------------------

struct TaskCreate;
impl ChangeHandler for TaskCreate {
    type Args = CreateTask;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let id = ctx.client().create_task(args).await.map_err(other_err)?.id;
            Ok(ExecutionOutcome {
                result_id: id,
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

struct TaskUpdate;
impl ChangeHandler for TaskUpdate {
    type Args = UpdateTask;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let id = ctx
                .client()
                .update_task(&ctx.target_id, args)
                .await
                .map_err(other_err)?
                .id;
            Ok(ExecutionOutcome {
                result_id: id,
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

struct TaskDelete;
impl ChangeHandler for TaskDelete {
    type Args = ();

    fn deserialize_args(_args: &Value) -> Result<Self::Args, AgentError> {
        Ok(())
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        _args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            ctx.client()
                .delete_task(&ctx.target_id)
                .await
                .map_err(other_err)?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

struct TaskMove;
impl ChangeHandler for TaskMove {
    type Args = MoveArgs;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let move_result = ctx
                .client()
                .move_entry(
                    &ctx.target_id,
                    &MoveEntry {
                        start_at: args.start_at,
                        force: args.force,
                    },
                )
                .await
                .map_err(|error| match error {
                    takusu_client::ClientError::Api { status: 409, body } => {
                        AgentError::Tool(ToolError::Conflict(body))
                    }
                    _ => other_err(error),
                })?;
            if args.fixed {
                ctx.client()
                    .update_task(
                        &ctx.target_id,
                        &UpdateTask {
                            fixed: Some(true),
                            ..Default::default()
                        },
                    )
                    .await
                    .map_err(other_err)?;
            }
            let after = to_after(&move_result)?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after,
                target_revision: None,
            })
        }
    }
}

struct TaskStart;
impl ChangeHandler for TaskStart {
    type Args = ();

    fn deserialize_args(_args: &Value) -> Result<Self::Args, AgentError> {
        Ok(())
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        _args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let body = StartWorkSession {
                task_id: Some(ctx.target_id.clone()),
                title: None,
                note: None,
                quantity_total: None,
                quantity_unit: None,
            };
            let _session = ctx
                .client()
                .start_work_session(&body, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            let task = ctx
                .client()
                .get_task(&ctx.target_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after: to_after(&task)?,
                target_revision: None,
            })
        }
    }
}

struct TaskPause;
impl ChangeHandler for TaskPause {
    type Args = ();

    fn deserialize_args(_args: &Value) -> Result<Self::Args, AgentError> {
        Ok(())
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        _args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let session_id = open_session_for_task(ctx.client(), &ctx.target_id).await?;
            let _session = ctx
                .client()
                .pause_work_session(&session_id, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            let task = ctx
                .client()
                .get_task(&ctx.target_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after: to_after(&task)?,
                target_revision: None,
            })
        }
    }
}

struct TaskProgress;
impl ChangeHandler for TaskProgress {
    type Args = RecordWorkSessionProgress;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let session_id = open_session_for_task(ctx.client(), &ctx.target_id).await?;
            let result = ctx
                .client()
                .record_work_session_progress(&session_id, args, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            let task = result.task.as_ref().ok_or_else(|| {
                AgentError::Tool(ToolError::Other(Box::new(std::io::Error::other(
                    "record_work_session_progress did not return a task",
                ))))
            })?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after: to_after(task)?,
                target_revision: None,
            })
        }
    }
}

struct TaskComplete;
impl ChangeHandler for TaskComplete {
    type Args = ();

    fn deserialize_args(_args: &Value) -> Result<Self::Args, AgentError> {
        Ok(())
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        _args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let session_id = open_session_for_task(ctx.client(), &ctx.target_id).await?;
            let _session = ctx
                .client()
                .complete_work_session(&session_id, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            let task = ctx
                .client()
                .get_task(&ctx.target_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after: to_after(&task)?,
                target_revision: None,
            })
        }
    }
}

struct TaskSplit;
impl ChangeHandler for TaskSplit {
    type Args = SplitTask;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let result = ctx
                .client()
                .split_task(&ctx.target_id, args, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(takusu_client_error(e)))?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after: to_after(&result)?,
                target_revision: None,
            })
        }
    }
}

// --- Habit arms --------------------------------------------------------------

struct HabitCreate;
impl ChangeHandler for HabitCreate {
    type Args = CreateHabit;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let id = ctx.client().create_habit(args).await.map_err(other_err)?.id;
            if let Some(steps) = ctx.steps_value.clone() {
                ctx.session
                    .replace_habit_steps_from_input(&id, steps, &[])
                    .await?;
            }
            Ok(ExecutionOutcome {
                result_id: id,
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

struct HabitUpdate;
impl ChangeHandler for HabitUpdate {
    type Args = UpdateHabit;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let id = ctx
                .client()
                .update_habit(&ctx.target_id, args)
                .await
                .map_err(other_err)?
                .id;
            if let Some(steps) = ctx.steps_value.clone() {
                let existing = ctx
                    .existing_habit
                    .as_ref()
                    .map(|h| h.steps.clone())
                    .unwrap_or_default();
                ctx.session
                    .replace_habit_steps_from_input(&id, steps, &existing)
                    .await?;
            }
            Ok(ExecutionOutcome {
                result_id: id,
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

struct HabitDelete;
impl ChangeHandler for HabitDelete {
    type Args = ();

    fn deserialize_args(_args: &Value) -> Result<Self::Args, AgentError> {
        Ok(())
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        _args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            ctx.client()
                .delete_habit(&ctx.target_id)
                .await
                .map_err(other_err)?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

struct HabitCreateScheduledSpan;
impl ChangeHandler for HabitCreateScheduledSpan {
    type Args = ScheduledSpanArgs;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let row = ctx
                .client()
                .create_habit_scheduled_span(
                    &ctx.target_id,
                    &CreateHabitScheduledSpan {
                        start_date: args.start_date,
                        end_date: args.end_date,
                        reason: args.reason.clone(),
                    },
                )
                .await
                .map_err(other_err)?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: None,
                after: to_after(&row)?,
                target_revision: None,
            })
        }
    }
}

struct HabitDeleteScheduledSpan;
impl ChangeHandler for HabitDeleteScheduledSpan {
    type Args = DeleteScheduledSpanArgs;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            ctx.client()
                .delete_habit_scheduled_span(&ctx.target_id, &args.span_id)
                .await
                .map_err(other_err)?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after: None,
                target_revision: None,
            })
        }
    }
}

// --- Skill arms --------------------------------------------------------------

struct SkillCreate;
impl ChangeHandler for SkillCreate {
    type Args = CreateSkill;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let slug = ctx
                .client()
                .create_skill(args)
                .await
                .map_err(other_err)?
                .slug;
            Ok(ExecutionOutcome {
                result_id: slug,
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

struct SkillUpdate;
impl ChangeHandler for SkillUpdate {
    type Args = UpdateSkill;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let slug = ctx
                .client()
                .update_skill(&ctx.target_id, args)
                .await
                .map_err(other_err)?
                .slug;
            Ok(ExecutionOutcome {
                result_id: slug,
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

// --- Memory arms -------------------------------------------------------------

struct MemoryCreate;
impl ChangeHandler for MemoryCreate {
    type Args = CreateMemory;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let row = ctx
                .client()
                .create_memory(args, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(memory_client_error(e)))?;
            let after = to_after(&row)?;
            Ok(ExecutionOutcome {
                result_id: row.id,
                before: None,
                after,
                target_revision: Some(row.revision),
            })
        }
    }
}

struct MemoryUpdate;
impl ChangeHandler for MemoryUpdate {
    type Args = UpdateMemory;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let row = ctx
                .client()
                .update_memory(&ctx.target_id, args, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(memory_client_error(e)))?;
            let after = to_after(&row)?;
            Ok(ExecutionOutcome {
                result_id: row.id,
                before: ctx.change.before.clone(),
                after,
                target_revision: Some(row.revision),
            })
        }
    }
}

struct MemoryDelete;
impl ChangeHandler for MemoryDelete {
    type Args = MemoryDeleteArgs;

    fn deserialize_args(args: &Value) -> Result<Self::Args, AgentError> {
        from_args(args)
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            ctx.client()
                .delete_memory(&ctx.target_id, args.observed_revision, ctx.operation_id)
                .await
                .map_err(|e| AgentError::Tool(memory_client_error(e)))?;
            Ok(ExecutionOutcome {
                result_id: ctx.target_id.clone(),
                before: ctx.change.before.clone(),
                after: None,
                target_revision: None,
            })
        }
    }
}

// --- Schedule arms -----------------------------------------------------------

struct ScheduleGenerate;
impl ChangeHandler for ScheduleGenerate {
    type Args = ();

    fn deserialize_args(_args: &Value) -> Result<Self::Args, AgentError> {
        Ok(())
    }

    fn execute_typed<'a>(
        &'a self,
        ctx: &ChangeContext<'a>,
        _args: &'a Self::Args,
    ) -> impl Future<Output = Result<ExecutionOutcome, AgentError>> + Send {
        async move {
            let entries = ctx.args.get("_preview_entries").cloned().ok_or_else(|| {
                AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::new(
                    "_preview_entries",
                    "schedule preview is missing",
                )))
            })?;
            let request = SaveScheduleRequest {
                entries: serde_json::from_value::<Vec<ScheduleEntry>>(entries.clone())
                    .map_err(invalid_args)?,
                mark_scheduled_task_ids: entries
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|entry| entry.get("task_id").and_then(Value::as_str))
                            .map(ToOwned::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            let id = ctx
                .client()
                .replace_schedule(&request)
                .await
                .map_err(other_err)?
                .id;
            Ok(ExecutionOutcome {
                result_id: id,
                before: None,
                after: None,
                target_revision: None,
            })
        }
    }
}

// --- dispatch ----------------------------------------------------------------

/// Resolve the executor for a `(kind, operation)` pair.
///
/// This is the single dispatch point; each arm body lives in its own
/// `ChangeExecutor` impl above. Returns `None` for unsupported pairs so the
/// caller can surface the original "unsupported proposal" error.
pub(crate) fn dispatch(
    kind: TargetKind,
    operation: ChangeOperation,
) -> Option<&'static dyn ChangeExecutor> {
    use TargetKind::{Habit, Memory, Schedule, Skill, Task};
    match (kind, operation) {
        (Task, ChangeOperation::Create) => Some(&TaskCreate),
        (Task, ChangeOperation::Update) => Some(&TaskUpdate),
        (Task, ChangeOperation::Delete) => Some(&TaskDelete),
        (Task, ChangeOperation::Move) => Some(&TaskMove),
        (Task, ChangeOperation::Start) => Some(&TaskStart),
        (Task, ChangeOperation::Pause) => Some(&TaskPause),
        (Task, ChangeOperation::Progress) => Some(&TaskProgress),
        (Task, ChangeOperation::Complete) => Some(&TaskComplete),
        (Task, ChangeOperation::Split) => Some(&TaskSplit),
        (Habit, ChangeOperation::Create) => Some(&HabitCreate),
        (Habit, ChangeOperation::Update) => Some(&HabitUpdate),
        (Habit, ChangeOperation::Delete) => Some(&HabitDelete),
        (Habit, ChangeOperation::CreateScheduledSpan) => Some(&HabitCreateScheduledSpan),
        (Habit, ChangeOperation::DeleteScheduledSpan) => Some(&HabitDeleteScheduledSpan),
        (Skill, ChangeOperation::Create) => Some(&SkillCreate),
        (Skill, ChangeOperation::Update) => Some(&SkillUpdate),
        (Memory, ChangeOperation::Create) => Some(&MemoryCreate),
        (Memory, ChangeOperation::Update) => Some(&MemoryUpdate),
        (Memory, ChangeOperation::Delete) => Some(&MemoryDelete),
        (Schedule, ChangeOperation::Generate) | (Schedule, ChangeOperation::Reschedule) => {
            Some(&ScheduleGenerate)
        }
        _ => None,
    }
}
