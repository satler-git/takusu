use async_trait::async_trait;
use serde_json::{Value, json};
use std::str::FromStr;
use takusu_client::{Client, SchedulePreviewRequest, TaskQuery};
use takusu_util::parse_datetime_tz;

use crate::{
    ChangeOperation, InvalidArgsError, Target, TargetKind, Tool, ToolError, ToolExposure,
    ToolOutput, ToolRegistry,
};

use super::common::*;
use super::read_tools::HabitScheduledSpans;

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
        registry.register(Box::new(MutationTool {
            client: client.clone(),
            tz_cache: tz_cache.clone(),
            kind,
        }));
    }
    registry.register(Box::new(crate::tool::Typed(HabitScheduledSpans { client, tz_cache })));
}

/// JSON schema for a single habit step input used in create/update habit.
fn habit_step_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "position": {"type": "integer", "description": "1-indexed display position of the step within the habit. On update, matching existing positions update existing steps; new positions create new steps."},
            "title": {"type": "string"},
            "description": {"type": "string"},
            "start_time": {"type": "string", "description": "Time of day (HH:MM)."},
            "end_time": {"type": "string", "description": "Time of day (HH:MM)."},
            "avg_minutes": {"type": "integer"},
            "sigma_minutes": {"type": "integer"},
            "parallelizable": {"type": "boolean"},
            "allows_parallel": {"type": "boolean"},
            "abandonability": {"type": "number", "description": "A value in [0.0, 1.0]; out-of-range values are silently clamped."},
            "fixed": {"type": "boolean"},
            "depends_on": {"type": "array", "items": {"type": "integer"}, "description": "Display positions (1-indexed) of steps this step depends on."}
        },
        "required": ["position", "title", "start_time", "end_time", "avg_minutes"],
        "additionalProperties": false
    })
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
    fn name(self) -> &'static str {
        match self {
            Self::CreateTask => "create_task",
            Self::UpdateTask => "update_task",
            Self::DeleteTask => "delete_task",
            Self::CreateHabit => "create_habit",
            Self::UpdateHabit => "update_habit",
            Self::DeleteHabit => "delete_habit",
            Self::GenerateSchedule => "generate_schedule",
            Self::Reschedule => "reschedule",
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

    fn schema(self) -> Value {
        let (required, properties) = match self {
            Self::CreateTask => (
                json!(["title", "end_at", "avg_minutes"]),
                json!({
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "start_at": {"type": "string", "description": "Start time; interpreted in server timezone if no offset is given."},
                    "end_at": {"type": "string", "description": "Deadline; interpreted in server timezone if no offset is given."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "depends": {"type": "array", "items": {"type": "string"}},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number", "description": "A value in [0.0, 1.0]; out-of-range values are silently clamped."},
                    "fixed": {"type": "boolean", "description": "If true, the start time is fixed and the scheduler will not move the task."},
                    "quantity_total": {"type": "integer", "description": "Total quantity for a quantitative task (e.g. 30)."},
                    "quantity_done": {"type": "integer", "description": "Quantity already completed; defaults to 0."},
                    "quantity_unit": {"type": "string", "description": "Unit for the quantity (e.g. 'pages', 'questions')."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::UpdateTask => (
                json!(["task_ref"]),
                json!({
                    "task_ref": {"type": "string"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "start_at": {"type": "string", "description": "Start time; interpreted in server timezone if no offset is given."},
                    "end_at": {"type": "string", "description": "Deadline; interpreted in server timezone if no offset is given."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "depends": {"type": "array", "items": {"type": "string"}},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number", "description": "A value in [0.0, 1.0]; out-of-range values are silently clamped."},
                    "status": {
                        "type": "string",
                        "enum": ["pending", "scheduled", "in_progress", "completed", "skipped"],
                        "description": "New task status. 'completed' means done."
                    },
                    "fixed": {"type": "boolean", "description": "If true, the start time is fixed and the scheduler will not move the task."},
                    "quantity_total": {"type": "integer", "description": "Total quantity for a quantitative task (e.g. 30)."},
                    "quantity_done": {"type": "integer", "description": "Quantity already completed."},
                    "quantity_unit": {"type": "string", "description": "Unit for the quantity (e.g. 'pages', 'questions')."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::DeleteTask => (
                json!(["task_ref"]),
                json!({
                    "task_ref": {"type": "string"},
                }),
            ),
            Self::CreateHabit => (
                json!([
                    "title",
                    "recurrence",
                    "start_time",
                    "end_time",
                    "avg_minutes"
                ]),
                json!({
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "recurrence": {"type": "string"},
                    "start_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "end_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number", "description": "A value in [0.0, 1.0]; out-of-range values are silently clamped."},
                    "fixed": {"type": "boolean", "description": "If true, generated tasks start at a fixed time and the scheduler will not move them."},
                    "window_mode": {"type": "string", "enum": ["day", "period"], "description": "Scheduling window mode for generated tasks."},
                    "steps": {"type": "array", "items": habit_step_schema(), "description": "Ordered steps for a multi-step habit. Existing step ids are omitted; match by position on update."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::UpdateHabit => (
                json!(["habit_ref"]),
                json!({
                    "habit_ref": {"type": "string"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "recurrence": {"type": "string"},
                    "start_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "end_time": {"type": "string", "description": "Time of day (HH:MM)."},
                    "avg_minutes": {"type": "integer"},
                    "sigma_minutes": {"type": "integer"},
                    "parallelizable": {"type": "boolean"},
                    "allows_parallel": {"type": "boolean"},
                    "abandonability": {"type": "number", "description": "A value in [0.0, 1.0]; out-of-range values are silently clamped."},
                    "active": {"type": "boolean"},
                    "fixed": {"type": "boolean", "description": "If true, generated tasks start at a fixed time and the scheduler will not move them."},
                    "window_mode": {"type": "string", "enum": ["day", "period"], "description": "Scheduling window mode for generated tasks."},
                    "steps": {"type": "array", "items": habit_step_schema(), "description": "Complete ordered steps to replace existing ones. Match existing steps by position."},
                    "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
                }),
            ),
            Self::DeleteHabit => (
                json!(["habit_ref"]),
                json!({
                    "habit_ref": {"type": "string"},
                }),
            ),
            Self::GenerateSchedule => (
                json!([]),
                json!({
                    "task_ids": {"type": "array", "items": {"type": "string"}},
                    "sleep": {"type": "string"},
                }),
            ),
            Self::Reschedule => (
                json!(["mode"]),
                json!({
                    "mode": {"type": "string"},
                    "from": {"type": "string", "description": "Start of range; interpreted in server timezone if no offset is given."},
                    "until": {"type": "string", "description": "End of range; interpreted in server timezone if no offset is given."},
                    "task_ids": {"type": "array", "items": {"type": "string"}},
                    "pinned": {"type": "array", "items": {"type": "string"}},
                    "sleep": {"type": "string"},
                }),
            ),
        };
        let properties = properties.as_object().cloned().unwrap_or_default();
        let mut properties = serde_json::Map::from_iter(properties);
        properties.insert(
            "why".into(),
            json!({"type": "string", "description": "Short user-facing reason for the proposed change."}),
        );
        properties.insert(
            "warnings".into(),
            json!({"type": "array", "items": {"type": "string"}}),
        );
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }
}

pub(super) struct MutationTool {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
    pub(super) kind: MutationKind,
}

#[async_trait]
impl Tool for MutationTool {
    fn name(&self) -> &'static str {
        self.kind.name()
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

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        validate_mutation(self.kind, &args)?;

        let tz = server_timezone(&self.tz_cache).await;
        normalize_mutation_args(self.kind, &mut args, &tz)?;

        let mut execution_args = args.clone();
        normalize_execution_references(self.kind, &mut execution_args)?;
        // Convert absolute datetimes back to the configured timezone for the
        // approval UI; execution_args retains the canonical UTC values.
        format_display_datetime_args(&mut args, &tz);

        let (before, observed_updated_at) = match self.kind {
            MutationKind::UpdateTask | MutationKind::DeleteTask => {
                let lookup = required_string(&execution_args, "task_ref")?;

                let default_query = TaskQuery::default();
                let c1 = self.client.clone();
                let c2 = self.client.clone();
                let c3 = self.client.clone();
                let (task, all_tasks, habits) = tokio::try_join!(
                    async { c1.get_task(&lookup).await },
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
                let reference = required_string(&args, "habit_ref")?;
                let habit = self
                    .client
                    .get_habit(&reference)
                    .await
                    .map_err(client_error)?;
                (
                    Some(habit_json(&habit)),
                    Some(habit.habit.updated_at.to_string()),
                )
            }
            _ => (None, None),
        };

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
            let entries = preview.get("entries").cloned().ok_or_else(|| {
                ToolError::InvalidArgs(InvalidArgsError::no_field(
                    "schedule preview did not return entries",
                ))
            })?;
            execution_args.insert("_preview_entries".into(), entries);
            args.insert(
                "_preview".into(),
                transform_preview(preview, &ctx, Some(&tz)),
            );
        }

        let (target, description) = self.kind.change_summary(&args);
        let inferred_fields = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields = serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields)
            .map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {error}"),
                ))
            })?;
        let why = optional_string(&args, "why")?;
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let proposal = crate::ProposedChange {
            operation: self.kind.operation(),
            target: Target::new(self.kind.target_type(), target),
            description,
            before,
            after: Some(Value::Object(args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at,
        };
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({
                "approval_required": true,
                "target": proposal.target.to_string(),
            }))
            .unwrap(),
            why,
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

pub(super) struct MoveTaskTool {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for MoveTaskTool {
    fn name(&self) -> &'static str {
        "move_task"
    }

    fn description(&self) -> &'static str {
        "Propose moving a scheduled task to a new start time. The task can also be marked fixed (default true). Generates a pending approval request; it does not write immediately."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_ref": {"type": "string", "description": "Task reference such as #42 or h1#3."},
                "start_at": {"type": "string", "description": "New start time; interpreted in server timezone if no offset is given."},
                "force": {"type": "boolean", "description": "Override deadline violation warnings."},
                "fixed": {"type": "boolean", "description": "Mark the task as fixed after moving; defaults to true."},
                "why": {"type": "string", "description": "Short user-facing reason for the proposed change."},
                "warnings": {"type": "array", "items": {"type": "string"}},
                "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
            },
            "required": ["task_ref", "start_at"],
            "additionalProperties": false,
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        let tz = server_timezone(&self.tz_cache).await;
        normalize_mutation_field(&mut args, "start_at", &tz)?;
        normalize_task_ref(&mut args, "task_ref")?;

        if args.get("fixed").is_none() {
            args.insert("fixed".to_string(), Value::Bool(true));
        }

        // Validate optional booleans; defaults are applied above.
        optional_bool(&args, "force")?;
        optional_bool(&args, "fixed")?;

        let task_ref = required_string(&args, "task_ref")?;
        let start_at = required_string(&args, "start_at")?;

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
        let entries: Vec<Value> = serde_json::from_str(&schedule_row.schedule)
            .map_err(|error| ToolError::Other(Box::new(error)))?;
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

        let mut display_args = args.clone();
        display_args.insert("task_ref".to_string(), Value::String(display_ref.clone()));
        display_args.insert("end_at".to_string(), Value::String(end_at));
        format_display_datetime_args(&mut display_args, &tz);

        let inferred_fields = args
            .get("inferred_fields")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let inferred_fields = serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields)
            .map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "inferred_fields",
                    format!("invalid: {error}"),
                ))
            })?;

        let why = optional_string(&args, "why")?;
        let warnings = args
            .get("warnings")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let display_start = display_args
            .get("start_at")
            .and_then(Value::as_str)
            .unwrap_or(&start_at);
        let description = format!("「{}」を {} に移動", task.title, display_start);

        let mut execution_args = args.clone();
        execution_args.remove("why");
        execution_args.remove("warnings");
        execution_args.remove("inferred_fields");

        let proposal = crate::ProposedChange {
            operation: ChangeOperation::Move,
            target: Target::new(TargetKind::Task, display_ref),
            description,
            before: Some(before),
            after: Some(Value::Object(display_args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at: Some(task.updated_at.to_string()),
        };
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({
                "approval_required": true,
                "target": proposal.target.to_string(),
            }))
            .unwrap(),
            why,
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

pub(super) fn normalize_mutation_field(
    args: &mut serde_json::Map<String, Value>,
    name: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<(), ToolError> {
    if let Some(value) = optional_string(args, name)? {
        let normalized = parse_datetime_tz(&value, tz).map_err(|error| {
            ToolError::InvalidArgs(InvalidArgsError::new(name, format!("invalid: {error}")))
        })?;
        args.insert(name.into(), Value::String(normalized));
    }
    Ok(())
}

pub(super) fn normalize_mutation_args(
    kind: MutationKind,
    args: &mut serde_json::Map<String, Value>,
    tz: &jiff::tz::TimeZone,
) -> Result<(), ToolError> {
    match kind {
        MutationKind::CreateTask | MutationKind::UpdateTask => {
            normalize_mutation_field(args, "start_at", tz)?;
            normalize_mutation_field(args, "end_at", tz)?;
            if let Some(status) = args.get("status").and_then(Value::as_str) {
                args.insert("status".into(), Value::String(normalize_status(status)));
            }
        }
        MutationKind::Reschedule => {
            normalize_mutation_field(args, "from", tz)?;
            normalize_mutation_field(args, "until", tz)?;
        }
        _ => {}
    }
    Ok(())
}

/// Strip a leading `#` from a single string reference field for backend execution.
fn normalize_task_ref(
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

/// Strip leading `#` characters from reference fields used for backend execution.
/// Display-facing `args` keep the original user input so approval diffs stay clean.
pub(super) fn normalize_execution_references(
    kind: MutationKind,
    args: &mut serde_json::Map<String, Value>,
) -> Result<(), ToolError> {
    match kind {
        MutationKind::CreateTask => {
            normalize_reference_array(args, "depends")?;
        }
        MutationKind::UpdateTask => {
            normalize_task_ref(args, "task_ref")?;
            normalize_reference_array(args, "depends")?;
        }
        MutationKind::DeleteTask => {
            normalize_task_ref(args, "task_ref")?;
        }
        MutationKind::GenerateSchedule => {
            normalize_reference_array(args, "task_ids")?;
        }
        MutationKind::Reschedule => {
            normalize_reference_array(args, "task_ids")?;
            normalize_reference_array(args, "pinned")?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_mutation(
    kind: MutationKind,
    args: &serde_json::Map<String, Value>,
) -> Result<(), ToolError> {
    match kind {
        MutationKind::CreateTask => {
            required_string(args, "title")?;
            required_string(args, "end_at")?;
            required_i64(args, "avg_minutes")?;
        }
        MutationKind::UpdateTask | MutationKind::DeleteTask => {
            required_string(args, "task_ref")?;
        }
        MutationKind::CreateHabit => {
            required_string(args, "title")?;
            required_string(args, "recurrence")?;
            required_string(args, "start_time")?;
            required_string(args, "end_time")?;
            required_i64(args, "avg_minutes")?;
        }
        MutationKind::UpdateHabit | MutationKind::DeleteHabit => {
            required_string(args, "habit_ref")?;
        }
        MutationKind::GenerateSchedule => {}
        MutationKind::Reschedule => {
            required_string(args, "mode")?;
        }
    }
    Ok(())
}
