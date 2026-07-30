use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use takusu_client::{Client, SchedulePreviewRequest, TaskQuery};
use takusu_types::{TaskStatusFilter, parse_datetime_tz};

use crate::{
    ChangeOperation, InferredField, InvalidArgsError, ProposalContent, ProposedChange, Target,
    TargetKind, ToolError, ToolExposure, ToolName, ToolOutput, ToolRegistry, TypedTool,
    deserialize_trimmed_optional, deserialize_trimmed_required,
};

use super::common::{
    format_display_datetime_args, habit_json, normalize_reference_array, normalize_status,
    optional_string, summary_string, transform_preview,
};
use super::read_tools::HabitScheduledSpans;
use super::{
    TaskContext, TimeZoneCache, client_error, server_timezone, strip_leading_hash, task_json,
};

/// Registers planner mutation tools and the hybrid habit_scheduled_spans tool.
/// Calls produce approval proposals or read data; they never write directly.
pub(super) fn register_mutation_tools(
    registry: &mut ToolRegistry,
    client: Client,
    tz_cache: TimeZoneCache,
) {
    for kind in [
        MutationKind::CreateTask,
        MutationKind::UpdateTask,
        MutationKind::DeleteTask,
        MutationKind::CreateHabit,
        MutationKind::UpdateHabit,
        MutationKind::DeleteHabit,
        MutationKind::GenerateSchedule,
        MutationKind::Reschedule,
    ] {
        registry.register(Box::new(crate::tool::Typed(MutationTool {
            client: client.clone(),
            tz_cache: tz_cache.clone(),
            kind,
        })));
    }
    registry.register(Box::new(crate::tool::Typed(HabitScheduledSpans {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    })));
    registry.register(Box::new(crate::tool::Typed(MoveTaskTool {
        client,
        tz_cache,
    })));
}

// Schema-only struct for a single habit step input. Used by schemars to
// generate the `steps` items schema in create/update habit tools.
// NOTE: no doc comment — schemars would embed it as the items `description`,
// which the hand-written schemas never carried.
#[derive(schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct HabitStepSchema {
    /// 1-indexed display position of the step within the habit. On update, matching existing positions update existing steps; new positions create new steps.
    position: i64,
    title: String,
    description: Option<String>,
    /// Time of day (HH:MM).
    start_time: String,
    /// Time of day (HH:MM).
    end_time: String,
    avg_minutes: i64,
    sigma_minutes: Option<i64>,
    parallelizable: Option<bool>,
    allows_parallel: Option<bool>,
    /// A value in [0.0, 1.0]; out-of-range values are silently clamped.
    abandonability: Option<f64>,
    fixed: Option<bool>,
    /// Display positions (1-indexed) of steps this step depends on.
    depends_on: Option<Vec<i64>>,
}

#[derive(Clone, Copy)]
pub(super) enum MutationKind {
    CreateTask,
    UpdateTask,
    DeleteTask,
    CreateHabit,
    UpdateHabit,
    DeleteHabit,
    GenerateSchedule,
    Reschedule,
}

impl MutationKind {
    pub(super) fn tool_name(self) -> ToolName {
        match self {
            Self::CreateTask => ToolName::CreateTask,
            Self::UpdateTask => ToolName::UpdateTask,
            Self::DeleteTask => ToolName::DeleteTask,
            Self::CreateHabit => ToolName::CreateHabit,
            Self::UpdateHabit => ToolName::UpdateHabit,
            Self::DeleteHabit => ToolName::DeleteHabit,
            Self::GenerateSchedule => ToolName::GenerateSchedule,
            Self::Reschedule => ToolName::Reschedule,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::CreateTask => {
                "Create a task proposal. Calling this tool generates a pending approval request; it does not write immediately. For example, \"演習30題追加\"."
            }
            Self::UpdateTask => {
                "Create a task update proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::DeleteTask => {
                "Create a task deletion proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::CreateHabit => {
                "Create a recurring habit proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::UpdateHabit => {
                "Create a recurring habit update proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::DeleteHabit => {
                "Create a recurring habit deletion proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::GenerateSchedule => {
                "Create a schedule generation proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
            Self::Reschedule => {
                "Create a partial reschedule proposal. Calling this tool generates a pending approval request; it does not write immediately."
            }
        }
    }

    fn target_type(self) -> TargetKind {
        match self {
            Self::CreateTask | Self::UpdateTask | Self::DeleteTask => TargetKind::Task,
            Self::CreateHabit | Self::UpdateHabit | Self::DeleteHabit => TargetKind::Habit,
            Self::GenerateSchedule | Self::Reschedule => TargetKind::Schedule,
        }
    }

    fn operation(self) -> ChangeOperation {
        match self {
            Self::CreateTask | Self::CreateHabit => ChangeOperation::Create,
            Self::UpdateTask | Self::UpdateHabit => ChangeOperation::Update,
            Self::DeleteTask | Self::DeleteHabit => ChangeOperation::Delete,
            Self::GenerateSchedule => ChangeOperation::Generate,
            Self::Reschedule => ChangeOperation::Reschedule,
        }
    }

    pub(super) fn change_summary(self, args: &serde_json::Map<String, Value>) -> (String, String) {
        let title = summary_string(args, "title");
        let task_ref = summary_string(args, "task_ref");
        let habit_ref = summary_string(args, "habit_ref");
        match self {
            Self::CreateTask => {
                let t = title.unwrap_or_else(|| "(名称未設定)".to_owned());
                (t.clone(), format!("「{t}」を作成"))
            }
            Self::UpdateTask => {
                let r = task_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                let description =
                    title.map_or_else(|| format!("{r}を更新"), |t| format!("「{t}」を更新"));
                (r, description)
            }
            Self::DeleteTask => {
                let r = task_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                (r.clone(), format!("{r}を削除"))
            }
            Self::CreateHabit => {
                let t = title.unwrap_or_else(|| "(名称未設定)".to_owned());
                (t.clone(), format!("「{t}」を作成"))
            }
            Self::UpdateHabit => {
                let r = habit_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                let description =
                    title.map_or_else(|| format!("{r}を更新"), |t| format!("「{t}」を更新"));
                (r, description)
            }
            Self::DeleteHabit => {
                let r = habit_ref.unwrap_or_else(|| "(参照不明)".to_owned());
                (r.clone(), format!("{r}を削除"))
            }
            Self::GenerateSchedule => (String::new(), "スケジュールを生成".to_owned()),
            Self::Reschedule => (String::new(), "スケジュールを再調整".to_owned()),
        }
    }

    /// Fields allowed in the schema for each mutation kind, paired with the
    /// required fields. The full schema is generated from [`MutationArgs`] via
    /// schemars, then filtered to only include the allowed fields for the
    /// specific kind.
    fn allowed_fields(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Self::CreateTask => (
                &[
                    "title",
                    "description",
                    "start_at",
                    "end_at",
                    "avg_minutes",
                    "sigma_minutes",
                    "depends",
                    "parallelizable",
                    "allows_parallel",
                    "abandonability",
                    "fixed",
                    "quantity_total",
                    "quantity_done",
                    "quantity_unit",
                    "inferred_fields",
                    "why",
                    "warnings",
                ],
                &["title", "end_at", "avg_minutes"],
            ),
            Self::UpdateTask => (
                &[
                    "task_ref",
                    "title",
                    "description",
                    "start_at",
                    "end_at",
                    "avg_minutes",
                    "sigma_minutes",
                    "depends",
                    "parallelizable",
                    "allows_parallel",
                    "abandonability",
                    "status",
                    "fixed",
                    "quantity_total",
                    "quantity_done",
                    "quantity_unit",
                    "inferred_fields",
                    "why",
                    "warnings",
                ],
                &["task_ref"],
            ),
            Self::DeleteTask => (["task_ref", "why", "warnings"].as_slice(), &["task_ref"]),
            Self::CreateHabit => (
                &[
                    "title",
                    "description",
                    "recurrence",
                    "start_time",
                    "end_time",
                    "avg_minutes",
                    "sigma_minutes",
                    "parallelizable",
                    "allows_parallel",
                    "abandonability",
                    "fixed",
                    "window_mode",
                    "steps",
                    "inferred_fields",
                    "why",
                    "warnings",
                ],
                &[
                    "title",
                    "recurrence",
                    "start_time",
                    "end_time",
                    "avg_minutes",
                ],
            ),
            Self::UpdateHabit => (
                &[
                    "habit_ref",
                    "title",
                    "description",
                    "recurrence",
                    "start_time",
                    "end_time",
                    "avg_minutes",
                    "sigma_minutes",
                    "parallelizable",
                    "allows_parallel",
                    "abandonability",
                    "active",
                    "fixed",
                    "window_mode",
                    "steps",
                    "inferred_fields",
                    "why",
                    "warnings",
                ],
                &["habit_ref"],
            ),
            Self::DeleteHabit => (["habit_ref", "why", "warnings"].as_slice(), &["habit_ref"]),
            Self::GenerateSchedule => (["task_ids", "sleep", "why", "warnings"].as_slice(), &[]),
            Self::Reschedule => (
                &[
                    "mode", "from", "until", "task_ids", "pinned", "sleep", "why", "warnings",
                ],
                &["mode"],
            ),
        }
    }

    fn schema(self) -> Value {
        use schemars::generate::{SchemaGenerator, SchemaSettings};
        let (allowed, required) = self.allowed_fields();
        let mut settings = SchemaSettings::default();
        settings.inline_subschemas = true;
        let mut generator = SchemaGenerator::new(settings);
        let full_schema = <MutationArgs as schemars::JsonSchema>::json_schema(&mut generator);
        let mut full_value = full_schema.to_value();
        // Filter properties to only the allowed fields for this kind.
        if let Some(obj) = full_value.as_object_mut() {
            if let Some(Value::Object(props)) = obj.get_mut("properties") {
                props.retain(|key, _| allowed.contains(&key.as_str()));
            }
            obj.insert(
                "required".to_string(),
                Value::Array(
                    required
                        .iter()
                        .map(|s| Value::String(s.to_string()))
                        .collect(),
                ),
            );
            obj.insert("additionalProperties".to_string(), Value::Bool(false));
        }
        crate::normalize_schema(full_value)
    }
}

/// Schema-only enum for the `status` field in `update_task`.
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum StatusSchema {
    Pending,
    Scheduled,
    InProgress,
    Completed,
    Skipped,
}

/// Schema-only enum for the `window_mode` field in create/update habit.
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum WindowModeSchema {
    Day,
    Period,
}

/// Typed arguments for all mutation kinds. Fields not relevant to a given
/// kind are left as `None`/empty and skipped during serialization so the
/// proposal's `after`/`arguments` only carry keys the caller supplied.
///
/// Doc comments on each field provide the JSON Schema `description` that
/// schemars embeds in the generated schema, replacing the hand-written
/// `json!` schema blocks.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MutationArgs {
    // Task fields
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    task_ref: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    description: Option<String>,
    /// Start time; interpreted in server timezone if no offset is given.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    start_at: Option<String>,
    /// Deadline; interpreted in server timezone if no offset is given.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    end_at: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_minutes: Option<i64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sigma_minutes: Option<i64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    depends: Option<Vec<String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    parallelizable: Option<bool>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    allows_parallel: Option<bool>,
    /// A value in [0.0, 1.0]; out-of-range values are silently clamped.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    abandonability: Option<f64>,
    /// If true, the start time is fixed and the scheduler will not move the task.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    fixed: Option<bool>,
    /// Total quantity for a quantitative task (e.g. 30).
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    quantity_total: Option<i64>,
    /// Quantity already completed; defaults to 0.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    quantity_done: Option<i64>,
    /// Unit for the quantity (e.g. 'pages', 'questions').
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    quantity_unit: Option<String>,
    /// New task status. 'completed' means done.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<StatusSchema>")]
    status: Option<String>,

    // Habit fields
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    habit_ref: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    recurrence: Option<String>,
    /// Time of day (HH:MM).
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    start_time: Option<String>,
    /// Time of day (HH:MM).
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    end_time: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    active: Option<bool>,
    /// Scheduling window mode for generated tasks.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<WindowModeSchema>")]
    window_mode: Option<String>,
    /// Ordered steps for a multi-step habit. Existing step ids are omitted; match by position on update.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<HabitStepSchema>>")]
    steps: Option<Vec<Value>>,

    // Schedule fields
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    mode: Option<String>,
    /// Start of range; interpreted in server timezone if no offset is given.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    from: Option<String>,
    /// End of range; interpreted in server timezone if no offset is given.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    until: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    task_ids: Option<Vec<String>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    sleep: Option<String>,

    // Meta fields (for proposals)
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inferred_fields: Vec<InferredField>,
}

impl MutationArgs {
    /// Return each schema-relevant field paired with whether it was supplied.
    /// `why`, `warnings`, and `inferred_fields` are proposal metadata and are
    /// always allowed, so they are excluded.
    ///
    /// **Keep in sync with `MutationArgs` fields**: when a new field is added
    /// to the struct, add a corresponding entry here so
    /// `validate_no_foreign_fields` can detect it.
    fn set_fields(&self) -> Vec<(&'static str, bool)> {
        vec![
            ("task_ref", self.task_ref.is_some()),
            ("title", self.title.is_some()),
            ("description", self.description.is_some()),
            ("start_at", self.start_at.is_some()),
            ("end_at", self.end_at.is_some()),
            ("avg_minutes", self.avg_minutes.is_some()),
            ("sigma_minutes", self.sigma_minutes.is_some()),
            ("depends", self.depends.is_some()),
            ("parallelizable", self.parallelizable.is_some()),
            ("allows_parallel", self.allows_parallel.is_some()),
            ("abandonability", self.abandonability.is_some()),
            ("fixed", self.fixed.is_some()),
            ("quantity_total", self.quantity_total.is_some()),
            ("quantity_done", self.quantity_done.is_some()),
            ("quantity_unit", self.quantity_unit.is_some()),
            ("status", self.status.is_some()),
            ("habit_ref", self.habit_ref.is_some()),
            ("recurrence", self.recurrence.is_some()),
            ("start_time", self.start_time.is_some()),
            ("end_time", self.end_time.is_some()),
            ("active", self.active.is_some()),
            ("window_mode", self.window_mode.is_some()),
            ("steps", self.steps.is_some()),
            ("mode", self.mode.is_some()),
            ("from", self.from.is_some()),
            ("until", self.until.is_some()),
            ("task_ids", self.task_ids.is_some()),
            ("pinned", self.pinned.is_some()),
            ("sleep", self.sleep.is_some()),
        ]
    }
}

pub(super) struct MutationTool {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
    pub(super) kind: MutationKind,
}

impl MutationTool {
    /// Reject fields that are not relevant to the current mutation kind so
    /// schema-foreign values don't leak into `execution_args`.
    fn validate_no_foreign_fields(&self, args: &MutationArgs) -> Result<(), InvalidArgsError> {
        let allowed: &[&str] = match self.kind {
            MutationKind::CreateTask => &[
                "title",
                "end_at",
                "avg_minutes",
                "description",
                "start_at",
                "sigma_minutes",
                "depends",
                "parallelizable",
                "allows_parallel",
                "abandonability",
                "fixed",
                "quantity_total",
                "quantity_done",
                "quantity_unit",
            ],
            MutationKind::UpdateTask => &[
                "task_ref",
                "title",
                "description",
                "start_at",
                "end_at",
                "avg_minutes",
                "sigma_minutes",
                "depends",
                "parallelizable",
                "allows_parallel",
                "abandonability",
                "status",
                "fixed",
                "quantity_total",
                "quantity_done",
                "quantity_unit",
            ],
            MutationKind::DeleteTask => &["task_ref"],
            MutationKind::CreateHabit => &[
                "title",
                "recurrence",
                "start_time",
                "end_time",
                "avg_minutes",
                "description",
                "sigma_minutes",
                "parallelizable",
                "allows_parallel",
                "abandonability",
                "fixed",
                "window_mode",
                "steps",
            ],
            MutationKind::UpdateHabit => &[
                "habit_ref",
                "title",
                "description",
                "recurrence",
                "start_time",
                "end_time",
                "avg_minutes",
                "sigma_minutes",
                "parallelizable",
                "allows_parallel",
                "abandonability",
                "active",
                "fixed",
                "window_mode",
                "steps",
            ],
            MutationKind::DeleteHabit => &["habit_ref"],
            MutationKind::GenerateSchedule => &["task_ids", "sleep"],
            MutationKind::Reschedule => &["mode", "from", "until", "task_ids", "pinned", "sleep"],
        };
        for (name, is_set) in args.set_fields() {
            if is_set && !allowed.contains(&name) {
                return Err(InvalidArgsError::new(
                    name,
                    format!(
                        "not applicable to {}",
                        <&'static str>::from(self.kind.tool_name())
                    ),
                ));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl TypedTool for MutationTool {
    type Params = MutationArgs;

    fn name(&self) -> &'static str {
        self.kind.tool_name().into()
    }
    fn description(&self) -> &'static str {
        self.kind.description()
    }
    fn exposure(&self) -> ToolExposure {
        match self.kind {
            MutationKind::GenerateSchedule | MutationKind::Reschedule => ToolExposure::Deferred,
            _ => ToolExposure::Direct,
        }
    }
    fn parameters_schema(&self) -> Value {
        self.kind.schema()
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        match self.kind {
            MutationKind::CreateTask => {
                if args.title.is_none() {
                    return Err(InvalidArgsError::new("title", "missing or empty"));
                }
                if args.end_at.is_none() {
                    return Err(InvalidArgsError::new("end_at", "missing or empty"));
                }
                if args.avg_minutes.is_none() {
                    return Err(InvalidArgsError::new("avg_minutes", "missing or invalid"));
                }
            }
            MutationKind::UpdateTask | MutationKind::DeleteTask => {
                if args.task_ref.is_none() {
                    return Err(InvalidArgsError::new("task_ref", "missing or empty"));
                }
            }
            MutationKind::CreateHabit => {
                if args.title.is_none() {
                    return Err(InvalidArgsError::new("title", "missing or empty"));
                }
                if args.recurrence.is_none() {
                    return Err(InvalidArgsError::new("recurrence", "missing or empty"));
                }
                if args.start_time.is_none() {
                    return Err(InvalidArgsError::new("start_time", "missing or empty"));
                }
                if args.end_time.is_none() {
                    return Err(InvalidArgsError::new("end_time", "missing or empty"));
                }
                if args.avg_minutes.is_none() {
                    return Err(InvalidArgsError::new("avg_minutes", "missing or invalid"));
                }
            }
            MutationKind::UpdateHabit | MutationKind::DeleteHabit => {
                if args.habit_ref.is_none() {
                    return Err(InvalidArgsError::new("habit_ref", "missing or empty"));
                }
            }
            MutationKind::GenerateSchedule => {}
            MutationKind::Reschedule => {
                if args.mode.is_none() {
                    return Err(InvalidArgsError::new("mode", "missing or empty"));
                }
            }
        }
        // Reject schema-foreign fields so they don't leak into execution_args.
        self.validate_no_foreign_fields(args)?;
        Ok(())
    }

    async fn call_typed(&self, mut args: MutationArgs) -> Result<ToolOutput, ToolError> {
        let tz = server_timezone(&self.tz_cache).await;

        // Normalize datetime fields and status in the typed args.
        match self.kind {
            MutationKind::CreateTask | MutationKind::UpdateTask => {
                if let Some(s) = args.start_at.take() {
                    args.start_at = Some(parse_datetime_tz(&s, &tz).map_err(|e| {
                        ToolError::InvalidArgs(InvalidArgsError::new(
                            "start_at",
                            format!("invalid: {e}"),
                        ))
                    })?);
                }
                if let Some(s) = args.end_at.take() {
                    args.end_at = Some(parse_datetime_tz(&s, &tz).map_err(|e| {
                        ToolError::InvalidArgs(InvalidArgsError::new(
                            "end_at",
                            format!("invalid: {e}"),
                        ))
                    })?);
                }
                // `status` is only a valid field for UpdateTask; CreateTask's
                // schema does not include it.
                if let MutationKind::UpdateTask = self.kind
                    && let Some(s) = args.status.take()
                {
                    let status = normalize_status(&s).map_err(|e| {
                        ToolError::InvalidArgs(InvalidArgsError::new(
                            "status",
                            format!("invalid: {e}"),
                        ))
                    })?;
                    if status == TaskStatusFilter::Overdue {
                        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                            "status",
                            "overdue is not a valid task status",
                        )));
                    }
                    args.status = Some(status.to_string());
                }
            }
            MutationKind::Reschedule => {
                if let Some(s) = args.from.take() {
                    args.from = Some(parse_datetime_tz(&s, &tz).map_err(|e| {
                        ToolError::InvalidArgs(InvalidArgsError::new(
                            "from",
                            format!("invalid: {e}"),
                        ))
                    })?);
                }
                if let Some(s) = args.until.take() {
                    args.until = Some(parse_datetime_tz(&s, &tz).map_err(|e| {
                        ToolError::InvalidArgs(InvalidArgsError::new(
                            "until",
                            format!("invalid: {e}"),
                        ))
                    })?);
                }
            }
            _ => {}
        }

        // Serialize typed args to a Map for display and execution args.
        let value = serde_json::to_value(&args).map_err(|e| ToolError::Other(Box::new(e)))?;
        let mut display_args = value.as_object().cloned().unwrap_or_default();
        let mut execution_args = display_args.clone();
        // `why`, `warnings`, and `inferred_fields` are proposal metadata, not
        // backend arguments (matching MoveTaskTool behavior).
        execution_args.remove("why");
        execution_args.remove("warnings");
        execution_args.remove("inferred_fields");

        // Strip leading `#` from reference fields in execution_args only.
        match self.kind {
            MutationKind::CreateTask => {
                normalize_reference_array(&mut execution_args, "depends")?;
            }
            MutationKind::UpdateTask => {
                normalize_task_ref(&mut execution_args, "task_ref")?;
                normalize_reference_array(&mut execution_args, "depends")?;
            }
            MutationKind::DeleteTask => {
                normalize_task_ref(&mut execution_args, "task_ref")?;
            }
            MutationKind::GenerateSchedule => {
                normalize_reference_array(&mut execution_args, "task_ids")?;
            }
            MutationKind::Reschedule => {
                normalize_reference_array(&mut execution_args, "task_ids")?;
                normalize_reference_array(&mut execution_args, "pinned")?;
            }
            _ => {}
        }

        // Convert absolute datetimes back to the configured timezone for the
        // approval UI; execution_args retains the canonical UTC values.
        format_display_datetime_args(&mut display_args, &tz);

        // Fetch "before" state for update/delete operations.
        let (before, observed_updated_at) = match self.kind {
            MutationKind::UpdateTask | MutationKind::DeleteTask => {
                let lookup = strip_leading_hash(args.task_ref.as_deref().unwrap_or(""));

                let default_query = TaskQuery::default();
                let c1 = self.client.clone();
                let c2 = self.client.clone();
                let c3 = self.client.clone();
                let (task, all_tasks, habits) = tokio::try_join!(
                    async { c1.get_task(lookup).await },
                    async { c2.list_tasks(&default_query).await },
                    async { c3.list_habits().await },
                )
                .map_err(client_error)?;

                let ctx = TaskContext::new(&all_tasks, &habits);
                (
                    Some(task_json(&task, &ctx, Some(&tz))),
                    Some(task.updated_at.to_string()),
                )
            }
            MutationKind::UpdateHabit | MutationKind::DeleteHabit => {
                let reference = args.habit_ref.as_deref().unwrap_or("");
                let habit = self
                    .client
                    .get_habit(reference)
                    .await
                    .map_err(client_error)?;
                (
                    Some(habit_json(&habit)),
                    Some(habit.habit.updated_at.to_string()),
                )
            }
            _ => (None, None),
        };

        // Run schedule preview and insert results into the args maps.
        if matches!(
            self.kind,
            MutationKind::GenerateSchedule | MutationKind::Reschedule
        ) {
            let mut preview_args = execution_args.clone();
            if matches!(self.kind, MutationKind::GenerateSchedule) {
                preview_args.insert("mode".into(), Value::String("full".into()));
            }
            let request: SchedulePreviewRequest =
                serde_json::from_value(Value::Object(preview_args)).map_err(|error| {
                    ToolError::InvalidArgs(InvalidArgsError::no_field(error.to_string()))
                })?;

            let default_query = TaskQuery::default();
            let c1 = self.client.clone();
            let c2 = self.client.clone();
            let c3 = self.client.clone();
            let req = request;
            let (preview, all_tasks, habits) = tokio::try_join!(
                async { c1.preview_schedule(&req).await },
                async { c2.list_tasks(&default_query).await },
                async { c3.list_habits().await },
            )
            .map_err(client_error)?;

            let ctx = TaskContext::new(&all_tasks, &habits);
            let entries = serde_json::to_value(&preview.entries).unwrap();
            execution_args.insert("_preview_entries".into(), entries);
            let preview_value = serde_json::to_value(&preview).unwrap();
            display_args.insert(
                "_preview".into(),
                transform_preview(preview_value, &ctx, Some(&tz)),
            );
        }

        let (target, description) = self.kind.change_summary(&display_args);
        let why = args.why.clone();
        let warnings = args.warnings.clone();
        let inferred_fields = args.inferred_fields.clone();

        let proposal = ProposedChange {
            operation: self.kind.operation(),
            target: Target::new(self.kind.target_type(), target),
            description,
            before,
            after: Some(Value::Object(display_args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at,
        };
        Ok(ToolOutput {
            content: ProposalContent::new(&proposal.target).to_json_string(),
            why,
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

// ── MoveTaskTool ────────────────────────────────────────────────────────

/// Arguments for [`MoveTaskTool`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MoveTaskArgs {
    /// Task reference such as #42 or h1#3.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    task_ref: String,
    /// New start time; interpreted in server timezone if no offset is given.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    start_at: String,
    /// Override deadline violation warnings.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    force: Option<bool>,
    /// Mark the task as fixed after moving; defaults to true.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    fixed: Option<bool>,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inferred_fields: Vec<InferredField>,
}

pub(super) struct MoveTaskTool {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

#[async_trait]
impl TypedTool for MoveTaskTool {
    type Params = MoveTaskArgs;

    fn name(&self) -> &'static str {
        ToolName::MoveTask.into()
    }

    fn description(&self) -> &'static str {
        "Propose moving a scheduled task to a new start time. The task can also be marked fixed (default true). Generates a pending approval request; it does not write immediately."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        use schemars::generate::{SchemaGenerator, SchemaSettings};
        let mut settings = SchemaSettings::default();
        settings.inline_subschemas = true;
        let mut generator = SchemaGenerator::new(settings);
        let schema = <MoveTaskArgs as schemars::JsonSchema>::json_schema(&mut generator);
        crate::normalize_schema(schema.to_value())
    }

    async fn call_typed(&self, mut args: MoveTaskArgs) -> Result<ToolOutput, ToolError> {
        let tz = server_timezone(&self.tz_cache).await;

        // Strip leading `#` from task_ref for backend execution.
        let task_ref = strip_leading_hash(&args.task_ref).to_string();
        args.task_ref = task_ref.clone();

        // Normalize start_at datetime.
        let start_at = parse_datetime_tz(&args.start_at, &tz).map_err(|e| {
            ToolError::InvalidArgs(InvalidArgsError::new("start_at", format!("invalid: {e}")))
        })?;
        args.start_at = start_at.clone();

        // Apply fixed=true default if not provided.
        if args.fixed.is_none() {
            args.fixed = Some(true);
        }

        let display_ref = if task_ref.starts_with('h') || task_ref.starts_with('H') {
            task_ref.clone()
        } else {
            format!("#{task_ref}")
        };

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let (task, all_tasks, habits) = tokio::try_join!(
            async { c1.get_task(&task_ref).await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let schedule_row = self.client.get_schedule().await.map_err(client_error)?;
        let entries: Vec<Value> = schedule_row
            .schedule
            .as_inner()
            .iter()
            .map(|entry| {
                serde_json::to_value(entry).map_err(|error| ToolError::Other(Box::new(error)))
            })
            .collect::<Result<_, _>>()?;
        let current_entry = entries
            .into_iter()
            .find(|e| e.get("task_id").and_then(Value::as_str) == Some(&task.id));
        let Some(current_entry) = current_entry else {
            return Err(ToolError::NotFound(format!(
                "task {display_ref} is not in the active schedule"
            )));
        };

        let ctx = TaskContext::new(&all_tasks, &habits);
        let mut before = task_json(&task, &ctx, Some(&tz));
        before["schedule_start_at"] = current_entry
            .get("start_at")
            .cloned()
            .unwrap_or(Value::Null);
        before["schedule_end_at"] = current_entry.get("end_at").cloned().unwrap_or(Value::Null);

        let start_ts = jiff::Timestamp::from_str(&start_at).map_err(|error| {
            ToolError::InvalidArgs(InvalidArgsError::new(
                "start_at",
                format!("invalid: {error}"),
            ))
        })?;
        let duration_minutes = if let (Some(old_start_str), Some(old_end_str)) = (
            current_entry.get("start_at").and_then(Value::as_str),
            current_entry.get("end_at").and_then(Value::as_str),
        ) {
            let old_start = jiff::Timestamp::from_str(old_start_str).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "schedule_start_at",
                    format!("invalid: {error}"),
                ))
            })?;
            let old_end = jiff::Timestamp::from_str(old_end_str).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "schedule_end_at",
                    format!("invalid: {error}"),
                ))
            })?;
            let duration = old_end - old_start;
            duration
                .total(jiff::Unit::Minute)
                .map_err(|error| ToolError::Other(Box::new(error)))? as i64
        } else {
            task.avg_minutes
        };
        let end_ts = start_ts
            .checked_add(jiff::Span::new().minutes(duration_minutes))
            .expect("valid end time");
        let end_at = end_ts.to_string();

        // Serialize typed args to a Map for display and execution args.
        let value = serde_json::to_value(&args).map_err(|e| ToolError::Other(Box::new(e)))?;
        let base = value.as_object().cloned().unwrap_or_default();

        let mut display_args = base.clone();
        display_args.insert("task_ref".to_string(), Value::String(display_ref.clone()));
        display_args.insert("end_at".to_string(), Value::String(end_at));
        format_display_datetime_args(&mut display_args, &tz);

        let mut execution_args = base;
        execution_args.remove("why");
        execution_args.remove("warnings");
        execution_args.remove("inferred_fields");

        let display_start = display_args
            .get("start_at")
            .and_then(Value::as_str)
            .unwrap_or(&start_at);
        let description = format!("「{}」を {} に移動", task.title, display_start);

        let proposal = ProposedChange {
            operation: ChangeOperation::Move,
            target: Target::new(TargetKind::Task, display_ref),
            description,
            before: Some(before),
            after: Some(Value::Object(display_args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at: Some(task.updated_at.to_string()),
        };
        Ok(ToolOutput {
            content: ProposalContent::new(&proposal.target).to_json_string(),
            why: args.why,
            warnings: args.warnings,
            proposed_changes: vec![proposal],
            inferred_fields: args.inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

/// Strip a leading `#` from a single string reference field for backend execution.
pub(super) fn normalize_task_ref(
    args: &mut serde_json::Map<String, Value>,
    key: &str,
) -> Result<(), ToolError> {
    if let Some(value) = optional_string(args, key)? {
        args.insert(
            key.to_string(),
            Value::String(strip_leading_hash(&value).to_string()),
        );
    }
    Ok(())
}
