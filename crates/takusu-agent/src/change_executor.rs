//! Dispatch table for `execute_proposed_change` arms.
//!
//! Each `(TargetKind, ChangeOperation)` pair is implemented as a separate
//! `ChangeExecutor` impl and registered in [`dispatch`]. Adding a new
//! operation only requires adding a new impl and a `dispatch` entry, rather
//! than editing a central 20+ arm `match`. The arms previously lived inline
//! in `AgentSession::execute_proposed_change`; see issue #1222.
//!
//! Error-mapping conventions are preserved exactly from the original arms:
//! - Task/Habit/Skill/Schedule client failures map to `ToolError::Other`.
//! - Memory client failures map via `tools::memory::client_error`.
//! - Task work-state operations (move/start/pause/progress/complete/split)
//!   map via `tools::takusu::client_error`, except `Move` which has its own
//!   409 → `Conflict` mapping.

use std::str::FromStr;

use async_trait::async_trait;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use takusu_client::{
    CreateHabit, CreateHabitScheduledSpan, CreateMemory, CreateSkill, CreateTask, HabitDetail,
    MoveEntry, RecordProgress, SaveScheduleRequest, ScheduleEntry, SplitTask, UpdateHabit,
    UpdateMemory, UpdateSkill, UpdateTask,
};

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
#[async_trait]
pub(crate) trait ChangeExecutor: Send + Sync {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError>;
}

// --- error-mapping helpers ---------------------------------------------------

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
fn deserialize_trimmed_date<'de, D>(deserializer: D) -> Result<takusu_util::Date, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: String = String::deserialize(deserializer)?;
    takusu_util::Date::from_str(raw.trim()).map_err(serde::de::Error::custom)
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
    start_date: takusu_util::Date,
    #[serde(deserialize_with = "deserialize_trimmed_date")]
    end_date: takusu_util::Date,
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
    start_at: takusu_util::Timestamp,
    #[serde(default)]
    force: bool,
    #[serde(default = "default_true")]
    fixed: bool,
}

// --- Task arms ---------------------------------------------------------------

struct TaskCreate;
#[async_trait]
impl ChangeExecutor for TaskCreate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let id = ctx
            .client()
            .create_task(&from_args::<CreateTask>(&ctx.args)?)
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

struct TaskUpdate;
#[async_trait]
impl ChangeExecutor for TaskUpdate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let id = ctx
            .client()
            .update_task(&ctx.target_id, &from_args::<UpdateTask>(&ctx.args)?)
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

struct TaskDelete;
#[async_trait]
impl ChangeExecutor for TaskDelete {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
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

struct TaskMove;
#[async_trait]
impl ChangeExecutor for TaskMove {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let move_args: MoveArgs = from_args(&ctx.args)?;
        let move_result = ctx
            .client()
            .move_entry(
                &ctx.target_id,
                &MoveEntry {
                    start_at: move_args.start_at,
                    force: move_args.force,
                },
            )
            .await
            .map_err(|error| match error {
                takusu_client::ClientError::Api { status: 409, body } => {
                    AgentError::Tool(ToolError::Conflict(body))
                }
                _ => other_err(error),
            })?;
        if move_args.fixed {
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

struct TaskStart;
#[async_trait]
impl ChangeExecutor for TaskStart {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let task = ctx
            .client()
            .start_task_work(&ctx.target_id, ctx.operation_id)
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

struct TaskPause;
#[async_trait]
impl ChangeExecutor for TaskPause {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let task = ctx
            .client()
            .pause_task_work(&ctx.target_id, ctx.operation_id)
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

struct TaskProgress;
#[async_trait]
impl ChangeExecutor for TaskProgress {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let record: RecordProgress = from_args(&ctx.args)?;
        let result = ctx
            .client()
            .record_progress(&ctx.target_id, &record, ctx.operation_id)
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

struct TaskComplete;
#[async_trait]
impl ChangeExecutor for TaskComplete {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let task = ctx
            .client()
            .complete_task_work(&ctx.target_id, ctx.operation_id)
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

struct TaskSplit;
#[async_trait]
impl ChangeExecutor for TaskSplit {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let split: SplitTask = from_args(&ctx.args)?;
        let result = ctx
            .client()
            .split_task(&ctx.target_id, &split, ctx.operation_id)
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

// --- Habit arms --------------------------------------------------------------

struct HabitCreate;
#[async_trait]
impl ChangeExecutor for HabitCreate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let id = ctx
            .client()
            .create_habit(&from_args::<CreateHabit>(&ctx.args)?)
            .await
            .map_err(other_err)?
            .id;
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

struct HabitUpdate;
#[async_trait]
impl ChangeExecutor for HabitUpdate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let id = ctx
            .client()
            .update_habit(&ctx.target_id, &from_args::<UpdateHabit>(&ctx.args)?)
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

struct HabitDelete;
#[async_trait]
impl ChangeExecutor for HabitDelete {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
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

struct HabitCreateScheduledSpan;
#[async_trait]
impl ChangeExecutor for HabitCreateScheduledSpan {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let span_args: ScheduledSpanArgs = from_args(&ctx.args)?;
        let row = ctx
            .client()
            .create_habit_scheduled_span(
                &ctx.target_id,
                &CreateHabitScheduledSpan {
                    start_date: span_args.start_date,
                    end_date: span_args.end_date,
                    reason: span_args.reason,
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

struct HabitDeleteScheduledSpan;
#[async_trait]
impl ChangeExecutor for HabitDeleteScheduledSpan {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let span_args: DeleteScheduledSpanArgs = from_args(&ctx.args)?;
        ctx.client()
            .delete_habit_scheduled_span(&ctx.target_id, &span_args.span_id)
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

// --- Skill arms --------------------------------------------------------------

struct SkillCreate;
#[async_trait]
impl ChangeExecutor for SkillCreate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let slug = ctx
            .client()
            .create_skill(&from_args::<CreateSkill>(&ctx.args)?)
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

struct SkillUpdate;
#[async_trait]
impl ChangeExecutor for SkillUpdate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let slug = ctx
            .client()
            .update_skill(&ctx.target_id, &from_args::<UpdateSkill>(&ctx.args)?)
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

// --- Memory arms -------------------------------------------------------------

struct MemoryCreate;
#[async_trait]
impl ChangeExecutor for MemoryCreate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let row = ctx
            .client()
            .create_memory(&from_args::<CreateMemory>(&ctx.args)?, ctx.operation_id)
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

struct MemoryUpdate;
#[async_trait]
impl ChangeExecutor for MemoryUpdate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let update: UpdateMemory = from_args(&ctx.args)?;
        let row = ctx
            .client()
            .update_memory(&ctx.target_id, &update, ctx.operation_id)
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

struct MemoryDelete;
#[async_trait]
impl ChangeExecutor for MemoryDelete {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
        let del_args: MemoryDeleteArgs = from_args(&ctx.args)?;
        ctx.client()
            .delete_memory(&ctx.target_id, del_args.observed_revision, ctx.operation_id)
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

// --- Schedule arms -----------------------------------------------------------

struct ScheduleGenerate;
#[async_trait]
impl ChangeExecutor for ScheduleGenerate {
    async fn execute(&self, ctx: &ChangeContext<'_>) -> Result<ExecutionOutcome, AgentError> {
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
