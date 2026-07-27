//! Row types that mirror `takusu_storage` shapes for the JSON wire format.
//! They are intentionally duplicated here to avoid pulling in `sqlx` (whose
//! `FromRow` derive the storage crate uses) into the WASM bundle.

use serde::{Deserialize, Serialize};
pub use takusu_util::{Abandonability, EnumLabel, Quantity};

use crate::memory;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: String,
    #[serde(default)]
    pub display_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_at: Option<String>,
    pub end_at: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub depends: String,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub parallelizable: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub allows_parallel: bool,
    pub abandonability: Abandonability,
    #[serde(with = "takusu_util::enum_serde")]
    pub status: takusu_util::TaskStatus,
    pub habit_id: Option<String>,
    pub ical_uid: Option<String>,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub user_edited: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub fixed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub habit_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_total: Option<Quantity>,
    #[serde(default)]
    pub quantity_done: Quantity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_from_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_quantity_total: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_minutes: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTask {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    pub end_at: String,
    pub avg_minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ical_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub habit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub habit_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_total: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_done: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_quantity_total: Option<Quantity>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateTask {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "takusu_util::enum_serde::option")]
    pub status: Option<takusu_util::TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub habit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_edited: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub habit_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_total: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_done: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_quantity_total: Option<Quantity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitRow {
    pub id: String,
    #[serde(default)]
    pub display_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub recurrence: String,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub parallelizable: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub allows_parallel: bool,
    pub abandonability: Abandonability,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub active: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub fixed: bool,
    #[serde(with = "takusu_util::enum_serde", default)]
    pub window_mode: takusu_util::WindowMode,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateHabit {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub recurrence: String,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "takusu_util::enum_serde::option")]
    pub window_mode: Option<takusu_util::WindowMode>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateHabit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "takusu_util::enum_serde::option")]
    pub window_mode: Option<takusu_util::WindowMode>,
}

/// A scheduled span for a habit (#303 / #503).
///
/// Effect depends on `habits.active`:
/// - active habit: span dates suppress task generation (a pause).
/// - disabled habit: span dates enable task generation (an activation window).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitScheduledSpanRow {
    pub id: String,
    pub habit_id: String,
    pub start_date: String,
    pub end_date: String,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateHabitScheduledSpan {
    pub start_date: String,
    pub end_date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A step of a multi-step habit (#95). Mirrors `takusu_storage::HabitStepRow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitStepRow {
    pub id: String,
    pub habit_id: String,
    pub position: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub parallelizable: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub allows_parallel: bool,
    pub abandonability: Abandonability,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub fixed: bool,
    pub depends_on: String,
    pub created_at: String,
}

/// Input element for `PUT /api/habits/:id/steps` (bulk replace, #95).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitStepInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub position: i64,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<Abandonability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Step estimate update element for `POST /api/habits/:id/estimate` (#919).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitStepEstimateInput {
    pub step_id: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
}

/// Request body for `POST /api/habits/:id/estimate` (#919).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyHabitEstimateRequest {
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub steps: Vec<HabitStepEstimateInput>,
}

/// Habit detail response: the habit row plus its steps (#95).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitDetail {
    #[serde(flatten)]
    pub habit: HabitRow,
    pub steps: Vec<HabitStepRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRow {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub schedule: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub task_id: String,
    pub start_at: String,
    pub end_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveScheduleRequest {
    pub entries: Vec<ScheduleEntry>,
    #[serde(default)]
    pub mark_scheduled_task_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRow {
    pub id: i64,
    pub jti: String,
    #[serde(with = "takusu_util::enum_serde")]
    pub scope: takusu_util::TokenScope,
    pub label: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenCreateResponse {
    pub id: i64,
    pub token: String,
    #[serde(with = "takusu_util::enum_serde")]
    pub scope: takusu_util::TokenScope,
    pub label: Option<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleCalSettingsRow {
    pub id: String,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub enabled: bool,
    pub calendar_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateGoogleCalSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleCalEventRow {
    pub task_id: String,
    pub google_event_id: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsRow {
    pub id: String,
    pub tz: String,
    pub sleep_start: String,
    pub sleep_end: String,
    /// #459: 1 日の快適な作業時間（分）。`None` または未設定の場合はデフォルトを使う。
    pub comfortable_minutes: Option<i64>,
    /// #459: 1 日の最大作業時間（分）。`None` または未設定の場合はデフォルトを使う。
    pub maximum_minutes: Option<i64>,
    /// 使用する solver。`"sa"` / `"priority"` / `"auto"`。空または不明な場合は `sa`。
    #[serde(default)]
    pub solver: String,
    /// 求解時間の上限（ミリ秒）。`None` または `0` の場合は制限なし。
    #[serde(default)]
    pub time_budget_ms: Option<i64>,
    /// 乱数シード。`None` の場合は決定的なデフォルト。
    #[serde(default)]
    pub seed: Option<i64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start する。
    #[serde(with = "takusu_util::bool_compat", default)]
    pub warm_start: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRow {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub built_in: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSkill {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_in: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateSkill {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: String,
    #[serde(with = "takusu_util::enum_serde")]
    pub kind: takusu_util::MemoryKind,
    pub key: String,
    #[serde(skip_serializing)]
    pub normalized_key: String,
    pub content: String,
    #[serde(skip_serializing)]
    pub normalized_content: String,
    #[serde(with = "takusu_util::enum_serde", default)]
    pub subject_type: takusu_util::SubjectType,
    pub subject_id: String,
    pub source: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

impl memory::MemoryRankable for MemoryRow {
    fn id(&self) -> &str {
        &self.id
    }
    fn normalized_key(&self) -> &str {
        &self.normalized_key
    }
    fn normalized_content(&self) -> &str {
        &self.normalized_content
    }
    fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateMemory {
    #[serde(with = "takusu_util::enum_serde")]
    pub kind: takusu_util::MemoryKind,
    pub key: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "takusu_util::enum_serde::option")]
    pub subject_type: Option<takusu_util::SubjectType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(default)]
    pub upsert: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateMemory {
    pub observed_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarTaskRow {
    pub task_id: String,
    pub display_id: i64,
    pub title: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub actual_minutes: Option<i64>,
    pub completed_at: Option<String>,
    #[serde(skip_serializing)]
    pub updated_at: String,
    pub similarity: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep_end: Option<String>,
    /// #459: 1 日の快適な作業時間（分）。`None` または未設定の場合はデフォルトを使う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comfortable_minutes: Option<i64>,
    /// #459: 1 日の最大作業時間（分）。`None` または未設定の場合はデフォルトを使う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_minutes: Option<i64>,
    /// 使用する solver。`"sa"` / `"priority"` / `"auto"`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver: Option<String>,
    /// 求解時間の上限（ミリ秒）。`None` または `0` で制限なし。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_budget_ms: Option<i64>,
    /// 乱数シード。`None` でデフォルト。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start する。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "takusu_util::option_bool_compat"
    )]
    pub warm_start: Option<bool>,
}

// ── WI-9 active-session progress management ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskWorkSessionRow {
    pub id: String,
    pub task_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEventRow {
    pub id: String,
    pub task_id: String,
    pub at: String,
    pub quantity_done: Option<Quantity>,
    pub delta_quantity: Option<i64>,
    pub active_minutes: i64,
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RecordProgress {
    pub quantity_done: Quantity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressResult {
    pub task: TaskRow,
    pub event: Option<ProgressEventRow>,
    #[serde(default)]
    pub suggests_completion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub task: TaskRow,
    pub open_session: Option<TaskWorkSessionRow>,
    pub sessions: Vec<TaskWorkSessionRow>,
    pub events: Vec<ProgressEventRow>,
    pub total_active_minutes: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SplitTask {
    pub retained_quantity: Quantity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_dependency: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitResult {
    pub original: TaskRow,
    pub remainder: TaskRow,
}

takusu_util::impl_search_task!(TaskRow);
takusu_util::impl_search_habit!(HabitRow);
