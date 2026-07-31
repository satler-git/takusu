use serde::{Deserialize, Serialize};
use takusu_types::{
    Abandonability, Date, DependencyList, JsonString, Quantity, ScheduleMode, Similarity,
    TaskStatusFilter, TimeOfDay, Timestamp,
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
    /// Total active work minutes from task_work_sessions (NULL when no work has been done).
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

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SaveScheduleRequest {
    pub entries: Vec<ScheduleEntry>,
    #[serde(default)]
    pub mark_scheduled_task_ids: Vec<String>,
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
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
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
}

// ── WI-9 active-session progress management ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct TaskWorkSessionRow {
    pub id: String,
    pub task_id: String,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ProgressEventRow {
    pub id: String,
    pub task_id: String,
    pub at: Timestamp,
    pub quantity_done: Option<Quantity>,
    pub delta_quantity: Option<i64>,
    pub active_minutes: i64,
    pub note: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RecordProgress {
    pub quantity_done: Quantity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProgressResult {
    pub task: TaskRow,
    /// The recorded event, or `None` when the reported quantity_done has not
    /// changed (no-op).
    pub event: Option<ProgressEventRow>,
    /// True when the reported quantity_done reaches or exceeds the task total.
    #[serde(default)]
    pub suggests_completion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskProgress {
    pub task: TaskRow,
    pub open_session: Option<TaskWorkSessionRow>,
    pub sessions: Vec<TaskWorkSessionRow>,
    pub events: Vec<ProgressEventRow>,
    pub total_active_minutes: i64,
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
