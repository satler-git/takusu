use serde::{Deserialize, Serialize};
use takusu_types::{
    Abandonability, Date, DependencyList, JsonString, Quantity, ScheduleMode, Similarity,
    TaskStatus, TaskStatusFilter, TimeOfDay, Timestamp, UnknownLabel,
};

pub use crate::sleep::{SleepConfig, SleepInput, SleepInputError};
pub use crate::workload::WorkloadConfig;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct TaskRow {
    pub id: String,
    #[serde(default)]
    pub display_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_at: Option<Timestamp>,
    pub end_at: Timestamp,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    #[serde(default)]
    pub depends: DependencyList,
    #[serde(default)]
    pub parallelizable: bool,
    #[serde(default)]
    pub allows_parallel: bool,
    pub abandonability: Abandonability,
    #[serde(with = "takusu_types::enum_serde")]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::TaskStatus")]
    pub status: takusu_types::TaskStatus,
    pub habit_id: Option<String>,
    pub ical_uid: Option<String>,
    #[serde(default)]
    pub user_edited: bool,
    #[serde(default)]
    pub fixed: bool,
    /// The habit step that generated this task, if any (#95). NULL for simple
    /// (step-less) habits and manually created tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub habit_step_id: Option<String>,
    /// WI-9: total quantity for a quantitative task (e.g. 30 題).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_total: Option<Quantity>,
    /// WI-9: quantity already done. Defaults to 0.
    #[serde(default)]
    pub quantity_done: Quantity,
    /// WI-9: unit for the quantity (e.g. "題").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_unit: Option<String>,
    /// WI-9: wall-clock completion time, set by `complete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<Timestamp>,
    /// WI-9: for a remainder task, the id of the task it was split from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_from_task_id: Option<String>,
    /// WI-9: pre-split total quantity, kept for lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_quantity_total: Option<Quantity>,
    /// Total active work minutes from work_sessions (NULL when no work has been done).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub actual_minutes: Option<i64>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateTask {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<Timestamp>,
    pub end_at: Timestamp,
    pub avg_minutes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ical_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    /// habit step link (#95). Set by sync_habit_tasks for step-generated tasks.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_step_id: Option<String>,
    /// WI-9: total quantity for a quantitative task.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_total: Option<Quantity>,
    /// WI-9: initial quantity already done (defaults to 0).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_done: Option<Quantity>,
    /// WI-9: unit for the quantity.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_unit: Option<String>,
    /// WI-9: pre-split total quantity, kept for lineage.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub original_quantity_total: Option<Quantity>,
}

/// A single task inside a batch create request (#1083).
/// `client_id` is a caller-supplied temporary id that can be referenced by
/// `depends` of other items in the same batch.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateTaskBatchItem {
    #[serde(flatten)]
    pub task: CreateTask,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

/// Request body for `POST /api/tasks/batch` (#1083).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateTaskBatch {
    pub tasks: Vec<CreateTaskBatchItem>,
}

/// A single result from a batch create request (#1083).
/// The caller can correlate results with input items by `client_id` and by
/// position.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateTaskBatchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(flatten)]
    pub task: TaskRow,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UpdateTask {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<Option<Timestamp>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<takusu_types::TaskStatus>")]
    pub status: Option<takusu_types::TaskStatus>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_edited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_step_id: Option<String>,
    /// WI-9: total quantity for a quantitative task.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_total: Option<Quantity>,
    /// WI-9: quantity already done.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_done: Option<Quantity>,
    /// WI-9: unit for the quantity.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_unit: Option<String>,
    /// WI-9: pre-split total quantity, kept for lineage.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub original_quantity_total: Option<Quantity>,
}

#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct TaskQuery {
    #[serde(default, with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<TaskStatusFilter>")]
    pub status: Option<TaskStatusFilter>,
    pub from: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub no_overdue: Option<bool>,
    pub habit_id: Option<String>,
    pub ical_uid: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct HabitRow {
    pub id: String,
    #[serde(default)]
    pub display_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub recurrence: String,
    pub start_time: TimeOfDay,
    pub end_time: TimeOfDay,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    #[serde(default)]
    pub parallelizable: bool,
    #[serde(default)]
    pub allows_parallel: bool,
    pub abandonability: Abandonability,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub fixed: bool,
    /// Window mode for generated tasks (#window_mode).
    /// `'day'` (default) = occurrence day's start_time..end_time.
    /// `'period'` = occurrence start_time .. next occurrence's start_time.
    #[serde(with = "takusu_types::enum_serde", default)]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::WindowMode")]
    pub window_mode: takusu_types::WindowMode,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateHabit {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub recurrence: String,
    pub start_time: TimeOfDay,
    pub end_time: TimeOfDay,
    pub avg_minutes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    /// Window mode: `'day'` or `'period'` (#window_mode).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<takusu_types::WindowMode>")]
    pub window_mode: Option<takusu_types::WindowMode>,
}

/// A single habit inside a batch create request (#1083).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateHabitBatchItem {
    #[serde(flatten)]
    pub habit: CreateHabit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

/// Request body for `POST /api/habits/batch` (#1083).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateHabitBatch {
    pub habits: Vec<CreateHabitBatchItem>,
}

/// A single result from a batch create request (#1083).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateHabitBatchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(flatten)]
    pub habit: HabitRow,
}

#[derive(Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UpdateHabit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<TimeOfDay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<TimeOfDay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    /// Window mode: `'day'` or `'period'` (#window_mode).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<takusu_types::WindowMode>")]
    pub window_mode: Option<takusu_types::WindowMode>,
}

/// A scheduled span for a habit (#503).
///
/// Its effect depends on `habits.active`:
/// - `active = true`: the span suppresses task generation (a pause).
/// - `active = false`: the span enables task generation (an activation window).
///
/// `start_date` / `end_date` are inclusive `YYYY-MM-DD` strings in the
/// user's local timezone.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct HabitScheduledSpanRow {
    pub id: String,
    pub habit_id: String,
    pub start_date: Date,
    pub end_date: Date,
    pub reason: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateHabitScheduledSpan {
    pub start_date: Date,
    pub end_date: Date,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A step of a multi-step habit (#95). Each step produces one task per
/// occurrence with its own window / cost / flags. Steps form a DAG via
/// `depends_on` (JSON array of step ids within the same habit).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct HabitStepRow {
    pub id: String,
    pub habit_id: String,
    pub position: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_time: TimeOfDay,
    pub end_time: TimeOfDay,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    #[serde(default)]
    pub parallelizable: bool,
    #[serde(default)]
    pub allows_parallel: bool,
    pub abandonability: Abandonability,
    #[serde(default)]
    pub fixed: bool,
    /// JSON array of step ids this step depends on (within the same habit).
    #[serde(default)]
    pub depends_on: DependencyList,
    pub created_at: Timestamp,
}

/// Input element for `PUT /api/habits/:id/steps` (bulk replace, #95).
/// An `id` present in the DB keeps the existing step (preserving its link to
/// generated tasks); an `id` absent or unknown creates a new step. Existing
/// steps not in the array are deleted. `depends_on` references step ids that
/// must exist in the resulting set.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitStepInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub position: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub start_time: TimeOfDay,
    pub end_time: TimeOfDay,
    pub avg_minutes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Preview request for `POST /api/habits/preview`. Mirrors `CreateHabit`
/// plus an optional step list and preview range.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitPreviewRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub recurrence: String,
    pub start_time: TimeOfDay,
    pub end_time: TimeOfDay,
    pub avg_minutes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    /// Window mode: `'day'` or `'period'` (#window_mode).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<takusu_types::WindowMode>")]
    pub window_mode: Option<takusu_types::WindowMode>,
    #[serde(default)]
    pub steps: Vec<HabitStepInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_occurrences: Option<i64>,
}

/// A single task occurrence produced by `HabitPreviewRequest`.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitPreviewTask {
    pub title: String,
    pub start_at: Timestamp,
    pub end_at: Timestamp,
}

/// Step estimate update element for `Storage::apply_habit_estimate` (#919).
/// Only the estimate fields are updated; the step row is otherwise left
/// untouched.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitStepEstimateInput {
    pub step_id: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
}

/// Request body for the backend-specific apply-habit-estimate call (#919).
/// `TakusuApp` computes estimates locally and asks the storage backend to
/// persist the habit and step values in one batch.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApplyHabitEstimateRequest {
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub steps: Vec<HabitStepEstimateInput>,
}

/// Habit detail response: the habit row plus its steps (#95). Used by
/// `GET /api/habits/:id` so clients receive steps in one round-trip.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitDetail {
    #[serde(flatten)]
    pub habit: HabitRow,
    pub steps: Vec<HabitStepRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ScheduleRow {
    pub id: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default)]
    pub schedule: ScheduleData,
    #[serde(default)]
    pub horizon_task_ids: JsonString<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScheduleEntry {
    pub task_id: String,
    pub start_at: Timestamp,
    pub end_at: Timestamp,
}

/// Type alias for the JSON-string-encoded schedule entries stored in
/// `ScheduleRow.schedule` (#1252).
pub type ScheduleData = JsonString<Vec<ScheduleEntry>>;

/// Delivery state persisted by the resident event ledger (WI-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventDeliveryState {
    PendingDelivery,
    Delivered,
    DeferredQuietHours,
    Acknowledged,
    Ignored,
    Resolved,
}

impl EventDeliveryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingDelivery => "pending_delivery",
            Self::Delivered => "delivered",
            Self::DeferredQuietHours => "deferred_quiet_hours",
            Self::Acknowledged => "acknowledged",
            Self::Ignored => "ignored",
            Self::Resolved => "resolved",
        }
    }
}

impl std::fmt::Display for EventDeliveryState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for EventDeliveryState {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl std::str::FromStr for EventDeliveryState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending_delivery" => Ok(Self::PendingDelivery),
            "delivered" => Ok(Self::Delivered),
            "deferred_quiet_hours" => Ok(Self::DeferredQuietHours),
            "acknowledged" => Ok(Self::Acknowledged),
            "ignored" => Ok(Self::Ignored),
            "resolved" => Ok(Self::Resolved),
            _ => Err(format!("unknown event delivery state: {value}")),
        }
    }
}

/// Coverage trust state consumed by the resident agent (WI-10).
///
/// Precedence is `bootstrap -> stale -> today-covered -> trusted`. A stale
/// state triggers a settlement prompt; today-covered makes the current task
/// authoritative; trusted is reached by a target-period procedure.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "sqlx", derive(sqlx::Type))]
pub enum CoverageState {
    /// No coverage confirmation has been recorded.
    #[default]
    Bootstrap,
    /// Coverage for today has been confirmed.
    TodayCovered,
    /// The coverage record is trusted for the target period.
    Trusted,
    /// The coverage is stale: unresolved interval, expired confirmation, or
    /// stale calendar sync.
    Stale,
}

impl CoverageState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::TodayCovered => "today_covered",
            Self::Trusted => "trusted",
            Self::Stale => "stale",
        }
    }
}

impl std::fmt::Display for CoverageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for CoverageState {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl std::str::FromStr for CoverageState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "bootstrap" => Ok(Self::Bootstrap),
            "today_covered" => Ok(Self::TodayCovered),
            "trusted" => Ok(Self::Trusted),
            "stale" => Ok(Self::Stale),
            _ => Err(format!("unknown coverage state: {value}")),
        }
    }
}

/// A recorded coverage confirmation (WI-10).
///
/// Confirms that a local period was covered: the user (or an intake/capture
/// flow) stated what happened during that interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct CoverageConfirmationRow {
    pub id: String,
    pub start_at: Timestamp,
    pub end_at: Timestamp,
    pub timezone: String,
    pub source: String,
    pub schedule_revision: i64,
    pub calendar_health: String,
    pub created_at: Timestamp,
    pub settled_at: Option<Timestamp>,
    pub operation_id: Option<String>,
}

/// An unresolved elapsed-time interval that needs settlement (WI-10 / WI-18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct UnsettledIntervalRow {
    pub id: String,
    pub start_at: Timestamp,
    pub end_at: Timestamp,
    pub classification: String,
    pub source: String,
    pub created_at: Timestamp,
    pub settled_at: Option<Timestamp>,
    pub operation_id: Option<String>,
}

/// Coverage data assembled for one planner evaluation (WI-10).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CoverageEvaluation {
    pub state: CoverageState,
    pub confirmations: Vec<CoverageConfirmationRow>,
    pub unsettled_intervals: Vec<UnsettledIntervalRow>,
    /// Unclassified schedule gaps detected for the current evaluation.
    /// These are synthetic unsettled intervals derived from the active schedule.
    #[serde(default)]
    pub unclassified_gaps: Vec<UnsettledIntervalRow>,
    pub schedule_revision: i64,
}

/// Request body for recording a coverage confirmation (WI-10).
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateCoverageConfirmation {
    pub start_at: Timestamp,
    pub end_at: Timestamp,
    pub timezone: String,
    #[serde(default)]
    pub source: String,
    pub schedule_revision: i64,
    #[serde(default)]
    pub calendar_health: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

/// Request body for recording an unsettled interval (WI-10 / WI-18).
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateUnsettledInterval {
    pub start_at: Timestamp,
    pub end_at: Timestamp,
    #[serde(default)]
    pub classification: String,
    #[serde(default)]
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

/// Storage representation of an immutable planner event.
///
/// Presentation and action templates remain JSON strings at this boundary so
/// `takusu-contracts` does not depend on `takusu-agent`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct EventLedgerRow {
    pub id: String,
    pub kind: String,
    pub task_id: Option<String>,
    pub presentation: String,
    pub urgency: String,
    pub schedule_revision: i64,
    pub distribution_revision: Option<i64>,
    pub observation_kind: String,
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    pub delivery_state: EventDeliveryState,
    pub created_at: Timestamp,
    pub delivered_at: Option<Timestamp>,
}

/// Values written when an evaluator commits a newly discovered event.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventLedgerInsert {
    pub id: String,
    pub kind: String,
    pub task_id: Option<String>,
    pub presentation: String,
    pub urgency: String,
    pub schedule_revision: i64,
    pub distribution_revision: Option<i64>,
    pub observation_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScheduleRevisionResponse {
    pub revision: i64,
}

/// Raw inputs for one consistent planner-event evaluation.
///
/// The storage backend collects these in a single atomic read (or as close to
/// atomic as the backend supports) so the pure evaluator receives a coherent
/// snapshot. The caller still supplies `now`, gap classification, and coverage.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvaluationInputs {
    pub schedule_revision: i64,
    pub tasks: Vec<TaskRow>,
    pub schedule: Vec<ScheduleEntry>,
    pub progress: Vec<EvaluationTaskProgress>,
    pub ledger: Vec<EventLedgerRow>,
    /// Coverage trust state for the current evaluation (WI-10).
    #[serde(default)]
    pub coverage: CoverageEvaluation,
}

/// Per-task progress for evaluation. Only in-progress tasks are included; the
/// estimator is pre-computed by the storage layer so callers do not have to
/// re-derive the fallback distribution.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvaluationTaskProgress {
    pub task_id: String,
    pub total_active_minutes: i64,
    pub estimator: Option<EvaluationEstimator>,
}

/// Estimator distribution snapshot for a single task.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvaluationEstimator {
    pub revision: i64,
    pub mean_minutes: f64,
    pub sigma_minutes: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<EstimatorBand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_crossing_time: Option<Timestamp>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SaveScheduleRequest {
    pub entries: Vec<ScheduleEntry>,
    #[serde(default)]
    pub mark_scheduled_task_ids: Vec<String>,
    #[serde(default)]
    pub horizon_task_ids: Vec<String>,
}

// ── Schedule request/response types (#1324) ──────────────────────────────
//
// Previously duplicated across takusu-client, takusu-local handlers, and
// takusu-local-lib app layer. Consolidated here so all three layers share
// a single definition; takusu-client re-exports via `pub use
// takusu_contracts::model::*`.

fn default_sleep() -> SleepInput {
    SleepInput::Recommended
}

fn default_schedule_mode() -> ScheduleMode {
    ScheduleMode::Full
}

/// Request body for `POST /api/schedule/preview`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SchedulePreviewRequest {
    #[serde(default = "default_schedule_mode")]
    pub mode: ScheduleMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: SleepInput,
}

/// Response body for `POST /api/schedule/preview`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SchedulePreviewResponse {
    pub entries: Vec<ScheduleEntry>,
    #[serde(default)]
    pub unscheduled_task_ids: Vec<String>,
    #[serde(default)]
    pub displaced_task_ids: Vec<String>,
    #[serde(default)]
    pub sleep_minutes_before: i64,
    #[serde(default)]
    pub sleep_minutes_after: i64,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Request body for `POST /api/schedule/generate`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GenerateSchedule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    #[serde(default = "default_sleep")]
    pub sleep: SleepInput,
}

/// Request body for `POST /api/schedule/reschedule`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Reschedule {
    pub mode: ScheduleMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: SleepInput,
}

/// Request body for `PATCH /api/schedule/entries/:task_id`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MoveEntry {
    pub start_at: Timestamp,
    #[serde(default)]
    #[schemars(default)]
    pub force: bool,
}

/// Response body for `PATCH /api/schedule/entries/:task_id`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MoveEntryResponse {
    pub task_id: String,
    pub start_at: Timestamp,
    pub end_at: Timestamp,
    #[serde(default)]
    pub warnings: Vec<String>,
}

takusu_search::impl_search_task!(TaskRow);
takusu_search::impl_search_habit!(HabitRow);

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct TokenRow {
    pub id: i64,
    pub jti: String,
    #[serde(with = "takusu_types::enum_serde")]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::TokenScope")]
    pub scope: takusu_types::TokenScope,
    pub label: Option<String>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TokenCreateResponse {
    pub id: i64,
    pub token: String,
    #[serde(with = "takusu_types::enum_serde")]
    #[schemars(with = "takusu_types::TokenScope")]
    pub scope: takusu_types::TokenScope,
    pub label: Option<String>,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct GoogleCalSettingsRow {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    pub calendar_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: Option<String>,
    /// Google Calendar イベントの共通リマインダー時間（分）。
    /// `None` または `0` の場合はリマインダーを設定しない。
    pub reminder_minutes: Option<i64>,
    /// Google Calendar イベントの共通の色 ID（1〜11）。
    pub color_id: Option<i64>,
    /// Google Calendar イベントの共通の公開範囲。
    /// `default`, `public`, `private`, `confidential` のいずれか。
    pub visibility: Option<String>,
    /// Google Calendar イベントの共通の予定/空き状態。
    /// `opaque` または `transparent`。
    pub transparency: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UpdateGoogleCalSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reminder_minutes: Option<Option<i64>>,
    /// `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_id: Option<Option<i64>>,
    /// `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Option<String>>,
    /// `None` = 更新しない、`Some(None)` = クリア、`Some(Some(v))` = 値を設定。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transparency: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct GoogleCalEventRow {
    pub task_id: String,
    pub google_event_id: String,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct SettingsRow {
    pub id: String,
    pub tz: String,
    pub sleep_start: TimeOfDay,
    pub sleep_end: TimeOfDay,
    /// #459: 1 日の快適な作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    pub comfortable_minutes: Option<i64>,
    /// #459: 1 日の最大作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    pub maximum_minutes: Option<i64>,
    /// 使用する solver。`"sa"` / `"priority"` / `"auto"`。未設定の場合は `sa`。未知値はエラー。
    #[serde(with = "takusu_types::enum_serde", default)]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::Solver")]
    pub solver: takusu_types::Solver,
    /// 求解時間の上限（ミリ秒）。`None` または `0` の場合は制限なし。
    #[serde(default)]
    pub time_budget_ms: Option<i64>,
    /// 乱数シード。`None` の場合は決定的なデフォルト。
    #[serde(default)]
    pub seed: Option<i64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start する。
    #[serde(default)]
    pub warm_start: bool,
    /// スケジュール計画の期間（日数）。horizon 計算に使う。デフォルト 14。
    #[serde(default = "default_plan_length_days")]
    pub plan_length_days: i64,
    /// デバイス優先度リスト。既定は desktop > android。
    #[serde(default = "default_device_priority")]
    pub device_priority: JsonString<Vec<String>>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

fn default_plan_length_days() -> i64 {
    14
}

fn default_device_priority() -> JsonString<Vec<String>> {
    JsonString::new(vec!["desktop".to_string(), "android".to_string()])
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct SkillRow {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub built_in: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateSkill {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_in: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UpdateSkill {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// A single entry in a task's comment timeline (WI-1).
///
/// Comments are append-only: there is no edit operation, and `author` is
/// server-assigned when the row is created. `seq` is a per-task monotonic
/// sequence assigned by storage, so ordering is deterministic even when
/// multiple rows share a `created_at` timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct CommentRow {
    pub id: String,
    pub task_id: String,
    #[serde(with = "takusu_types::enum_serde")]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::CommentAuthor")]
    pub author: takusu_types::CommentAuthor,
    pub content: String,
    pub seq: i64,
    pub created_at: Timestamp,
}

/// Request body for creating a task comment (WI-1).
///
/// Contains only `content`. `author` is deliberately absent: it is assigned by
/// the server based on which endpoint is used (public `/tasks/:id/comments` →
/// `user`, `/tasks/:id/comments/agent` → `agent`), so ordinary clients cannot
/// impersonate the agent or system (invariant 2).
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateComment {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct MemoryRow {
    pub id: String,
    #[serde(with = "takusu_types::enum_serde")]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::MemoryKind")]
    pub kind: takusu_types::MemoryKind,
    pub key: String,
    #[serde(default, skip_serializing)]
    pub normalized_key: String,
    pub content: String,
    #[serde(default, skip_serializing)]
    pub normalized_content: String,
    #[serde(with = "takusu_types::enum_serde", default)]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::SubjectType")]
    pub subject_type: takusu_types::SubjectType,
    pub subject_id: String,
    #[serde(with = "takusu_types::enum_serde", default)]
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    #[schemars(with = "takusu_types::MemorySource")]
    pub source: takusu_types::MemorySource,
    pub revision: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

impl takusu_search::memory::MemoryRankable for MemoryRow {
    fn id(&self) -> &str {
        &self.id
    }
    fn normalized_key(&self) -> &str {
        &self.normalized_key
    }
    fn normalized_content(&self) -> &str {
        &self.normalized_content
    }
    fn updated_at(&self) -> String {
        self.updated_at.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateMemory {
    #[serde(with = "takusu_types::enum_serde")]
    #[schemars(with = "takusu_types::MemoryKind")]
    pub kind: takusu_types::MemoryKind,
    pub key: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<takusu_types::SubjectType>")]
    pub subject_type: Option<takusu_types::SubjectType>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject_id: Option<String>,
    #[serde(default)]
    pub upsert: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UpdateMemory {
    pub observed_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryQuery {
    pub q: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<takusu_types::MemoryKind>")]
    pub kind: Option<takusu_types::MemoryKind>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[serde(with = "takusu_types::enum_serde::option")]
    #[schemars(with = "Option<takusu_types::SubjectType>")]
    pub subject_type: Option<takusu_types::SubjectType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// Query for the memory read auto-injection retrieval path (WI-4 / #1003).
///
/// Unlike [`MemoryQuery`] (user-facing keyword search), this is a *reverse*
/// lookup: memories whose `normalized_key` occurs as a substring of `text`
/// are candidates, ranked server-side by key specificity and recency. Used at
/// turn start to surface `proper_noun` / `fact` memories without the agent
/// calling any search tool.
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryInjectionQuery {
    /// The raw user utterance. Normalized server-side before matching.
    pub text: String,
    /// Maximum number of memories to return (default 5, capped at 20).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub limit: Option<u32>,
}

/// Result of a memory read auto-injection retrieval (WI-4 / #1003).
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryInjectionResult {
    /// Matching `proper_noun` / `fact` memories, ranked for injection.
    pub memories: Vec<MemoryRow>,
    /// Per-kind memory counts so the agent knows whether the store is
    /// non-empty even when no memory matches the utterance.
    pub counts: MemoryKindCounts,
}

/// Total memory rows per kind, used for the system-prompt memory hint.
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryKindCounts {
    pub proper_noun: i64,
    pub fact: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct SimilarTaskRow {
    pub task_id: String,
    pub display_id: i64,
    pub title: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub actual_minutes: Option<i64>,
    pub completed_at: Option<Timestamp>,
    #[serde(default, skip_serializing)]
    pub updated_at: Timestamp,
    #[serde(default)]
    #[cfg_attr(feature = "sqlx", sqlx(skip))]
    pub similarity: Similarity,
}

#[derive(Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SimilarTaskQuery {
    #[serde(rename = "q")]
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UpdateSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sleep_start: Option<TimeOfDay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sleep_end: Option<TimeOfDay>,
    /// #459: 1 日の快適な作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comfortable_minutes: Option<i64>,
    /// #459: 1 日の最大作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_minutes: Option<i64>,
    /// 使用する solver。`"sa"` / `"priority"` / `"auto"`。
    #[serde(
        with = "takusu_types::enum_serde::option",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<takusu_types::Solver>")]
    pub solver: Option<takusu_types::Solver>,
    /// 求解時間の上限（ミリ秒）。`None` または `0` で制限なし。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_budget_ms: Option<i64>,
    /// 乱数シード。`None` でデフォルト。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm_start: Option<bool>,
    /// スケジュール計画の期間（日数）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_length_days: Option<i64>,
    /// デバイス優先度リスト。`None` の場合は更新しない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_priority: Option<Vec<String>>,
}

// ── WI-9 active-session progress management ─────────────────────────────────

/// A top-level work session. It may be linked to a task, or it may be a
/// standalone session that is later converted into a task.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct WorkSessionRow {
    pub id: String,
    pub task_id: Option<String>,
    pub title: Option<String>,
    pub note: Option<String>,
    pub quantity_total: Option<Quantity>,
    pub quantity_done: Quantity,
    pub quantity_unit: Option<String>,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ProgressEventRow {
    pub id: String,
    pub work_session_id: String,
    pub task_id: Option<String>,
    pub at: Timestamp,
    pub quantity_done: Option<Quantity>,
    pub delta_quantity: Option<i64>,
    pub active_minutes: i64,
    pub note: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StartWorkSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_total: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_unit: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RecordWorkSessionProgress {
    pub quantity_done: Quantity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_total: Option<Quantity>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConvertWorkSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AttachWorkSession {
    pub task_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EstimatorBand {
    Usual,
    Attention,
    Replan,
}

impl std::fmt::Display for EstimatorBand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Usual => "usual",
            Self::Attention => "attention",
            Self::Replan => "replan",
        })
    }
}

impl std::str::FromStr for EstimatorBand {
    type Err = UnknownLabel;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "usual" => Ok(Self::Usual),
            "attention" => Ok(Self::Attention),
            "replan" => Ok(Self::Replan),
            other => Err(UnknownLabel::new("EstimatorBand", other)),
        }
    }
}

impl TryFrom<String> for EstimatorBand {
    type Error = UnknownLabel;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<EstimatorBand> for takusu_types::estimator::InterventionBand {
    fn from(band: EstimatorBand) -> Self {
        match band {
            EstimatorBand::Usual => Self::Usual,
            EstimatorBand::Attention => Self::Attention,
            EstimatorBand::Replan => Self::Replan,
        }
    }
}

impl From<takusu_types::estimator::InterventionBand> for EstimatorBand {
    fn from(band: takusu_types::estimator::InterventionBand) -> Self {
        match band {
            takusu_types::estimator::InterventionBand::Usual => Self::Usual,
            takusu_types::estimator::InterventionBand::Attention => Self::Attention,
            takusu_types::estimator::InterventionBand::Replan => Self::Replan,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct EstimatorStateRow {
    pub task_id: String,
    pub revision: i64,
    pub mean_minutes: f64,
    pub sigma_minutes: f64,
    pub source: String,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub band: Option<EstimatorBand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub next_crossing_time: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EstimatorResult {
    pub band: EstimatorBand,
    pub revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_crossing_time: Option<Timestamp>,
    pub survival_probability: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_shift_z: Option<f64>,
    pub observation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkSessionProgressResult {
    pub work_session: WorkSessionRow,
    pub task: Option<TaskRow>,
    /// The recorded event, or `None` when the reported quantity_done has not
    /// changed (no-op).
    pub event: Option<ProgressEventRow>,
    /// True when the reported quantity_done reaches or exceeds the task total.
    #[serde(default)]
    pub suggests_completion: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimator: Option<EstimatorResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskProgress {
    pub task: TaskRow,
    pub open_session: Option<WorkSessionRow>,
    pub sessions: Vec<WorkSessionRow>,
    pub events: Vec<ProgressEventRow>,
    pub total_active_minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimator: Option<EstimatorStateRow>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SplitTask {
    /// Quantity to keep on the original task.
    pub retained_quantity: Quantity,
    /// If true, make the remainder depend on the original task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_dependency: Option<bool>,
    /// Optional title for the remainder (defaults to the original title).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description for the remainder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional deadline for the remainder (defaults to the original end_at).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SplitResult {
    pub original: TaskRow,
    pub remainder: TaskRow,
}

// ── Habit estimation from completed task actuals (#919) ────────────────────

/// Request body for `POST /api/habits/{id}/estimate`.
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitEstimateRequest {
    /// When true, detect and exclude outliers using the median absolute
    /// deviation (MAD) before computing the estimate.
    #[serde(default)]
    pub detect_outliers: bool,
    /// When true, persist the computed `avg_minutes` / `sigma_minutes` to the
    /// habit (and its steps). When false, return a preview only.
    #[serde(default)]
    pub apply: bool,
}

/// One completed task observation included in a habit estimate.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitEstimateSample {
    pub task_id: String,
    pub title: String,
    pub actual_minutes: i64,
    pub excluded: bool,
}

/// Estimate result for a single habit step.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitEstimateStep {
    pub step_id: String,
    pub title: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub sample_count: usize,
    pub excluded_count: usize,
    pub applied: bool,
}

/// Response from `POST /api/habits/{id}/estimate`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HabitEstimateResult {
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub sample_count: usize,
    pub excluded_count: usize,
    /// Task-level samples for non-step habits. Empty for step-based habits.
    pub samples: Vec<HabitEstimateSample>,
    /// Per-step estimates for step-based habits.
    pub steps: Vec<HabitEstimateStep>,
    /// True when the result was written back to the habit/steps.
    pub applied: bool,
    /// The updated habit row, present only when `apply` was true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub habit: Option<HabitRow>,
}

// ── WI-11 multi-device arbitration ───────────────────────────────────────

/// Platform kind for a registered device.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    #[default]
    Desktop,
    Android,
}

impl DevicePlatform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Android => "android",
        }
    }
}

impl std::fmt::Display for DevicePlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for DevicePlatform {
    type Err = takusu_types::UnknownLabel;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "desktop" => Ok(Self::Desktop),
            "android" => Ok(Self::Android),
            _ => Err(takusu_types::UnknownLabel::new("DevicePlatform", value)),
        }
    }
}

impl TryFrom<String> for DevicePlatform {
    type Error = takusu_types::UnknownLabel;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl takusu_types::EnumLabel for DevicePlatform {
    fn enum_default() -> Self {
        Self::Desktop
    }
    fn all_variants() -> &'static [Self] {
        &[Self::Desktop, Self::Android]
    }
    fn as_str(&self) -> &'static str {
        Self::as_str(*self)
    }
}

/// A registered device that may hold or contend for resident authority.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    #[cfg_attr(feature = "sqlx", sqlx(try_from = "String"))]
    pub platform: DevicePlatform,
    pub priority: i64,
    pub evaluator_heartbeat_until: Option<Timestamp>,
    pub evaluator_lease_until: Option<Timestamp>,
    pub next_eval_at: Option<Timestamp>,
    pub audio_service_running: bool,
    pub private_output_route: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Request body for registering a new device.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CreateDevice {
    pub id: String,
    pub name: String,
    pub platform: DevicePlatform,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
}

/// Request body for updating a registered device.
#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UpdateDevice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_service_running: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_output_route: Option<bool>,
}

/// Current speech capability for a device.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpeechCapability {
    pub can_speak_proactively: bool,
}

/// Result of resolving which device currently holds resident authority.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResidentAuthority {
    /// The resident device, or `None` when no device currently holds a valid
    /// evaluator heartbeat or lease.
    pub device_id: Option<String>,
    /// `true` when the requesting `candidate_id` is the resident authority.
    pub is_resident: bool,
    /// The next scheduled evaluation time advertised by the resident device,
    /// when known.
    pub next_eval_at: Option<Timestamp>,
}

/// Request body for refreshing a desktop evaluator heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RefreshEvaluatorHeartbeat {
    pub device_id: String,
    pub until: Timestamp,
}

/// Request body for reserving or renewing an Android evaluator lease.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RefreshEvaluatorLease {
    pub device_id: String,
    pub lease_until: Timestamp,
    pub next_eval_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_row_defaults_optional_bools_when_missing() {
        // TaskRow has #[serde(default)] on parallelizable/allows_parallel/user_edited.
        // A minimal JSON missing those fields should still deserialize.
        let json = r#"{
            "id": "t1",
            "display_id": 1,
            "title": "T",
            "description": null,
            "start_at": null,
            "end_at": "2025-01-01T00:00:00Z",
            "avg_minutes": 30,
            "sigma_minutes": 0,
            "depends": "[]",
            "abandonability": 0.5,
            "status": "pending",
            "habit_id": null,
            "ical_uid": null,
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        }"#;
        let row: TaskRow = serde_json::from_str(json).unwrap();
        assert!(!row.parallelizable);
        assert!(!row.allows_parallel);
        assert!(!row.user_edited);
    }

    #[test]
    fn update_task_skip_serializing_none() {
        let u = UpdateTask::default();
        let json = serde_json::to_string(&u).unwrap();
        // All fields None → serialized JSON should be empty object.
        assert_eq!(json, "{}");
    }

    #[test]
    fn create_task_roundtrip() {
        let c = CreateTask {
            title: "Test".into(),
            description: Some("desc".into()),
            start_at: None,
            end_at: "2025-01-01T00:00:00Z".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: Some(5),
            depends: Some(vec!["t1".into()]),
            parallelizable: Some(true),
            allows_parallel: Some(false),
            abandonability: Some(0.3.into()),
            ical_uid: None,
            habit_id: None,
            fixed: None,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: None,
            quantity_unit: None,
            original_quantity_total: None,
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: CreateTask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title, "Test");
        assert_eq!(back.avg_minutes, 30);
        assert_eq!(back.sigma_minutes, Some(5));
        assert_eq!(back.parallelizable, Some(true));
    }

    #[test]
    fn save_schedule_request_default_mark_ids_empty() {
        let json = r#"{"entries":[]}"#;
        let req: SaveScheduleRequest = serde_json::from_str(json).unwrap();
        assert!(req.entries.is_empty());
        assert!(req.mark_scheduled_task_ids.is_empty());
    }
}
