use async_trait::async_trait;
use futures_util::future::try_join_all;
use jiff::civil::Date;
use serde_json::{Value, json};
use takusu_client::{
    Client, HabitDetail, HabitScheduledSpanRow, SchedulePreviewRequest, TaskQuery,
};
use takusu_util::parse_date_expression;

use crate::{
    ChangeOperation, InvalidArgsError, Target, TargetKind, Tool, ToolError, ToolExposure,
    ToolOutput, ToolRegistry,
};

use super::common::*;
use super::mutation::normalize_mutation_field;

/// Registers the read-only planner tools used by the agent.
pub(super) fn register_read_tools(registry: &mut ToolRegistry, client: Client, tz_cache: TimeZoneCache) {
    registry.register(Box::new(ListTasks {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    }));
    registry.register(Box::new(GetTask {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    }));
    registry.register(Box::new(ListHabits {
        client: client.clone(),
    }));
    registry.register(Box::new(GetHabit {
        client: client.clone(),
    }));
    registry.register(Box::new(GetSchedule {
        client: client.clone(),
        tz_cache,
    }));
    registry.register(Box::new(GetSettings { client }));
}

pub(super) struct ListTasks {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for ListTasks {
    fn name(&self) -> &'static str {
        "list_tasks"
    }
    fn description(&self) -> &'static str {
        "List tasks, optionally filtered by status, time range, or habit."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["pending", "scheduled", "in_progress", "completed", "skipped", "overdue"],
                    "description": "Task status filter. Use 'completed' for done tasks, 'overdue' for tasks whose end_at has passed but are not completed or skipped."
                },
                "from": {"type": "string", "description": "Start of range; interpreted in server timezone."},
                "until": {"type": "string", "description": "End of range; interpreted in server timezone."},
                "no_overdue": {"type": "boolean", "description": "If true, exclude tasks whose end_at has passed. Do not use together with status='overdue'."},
                "habit_id": {"type": "string", "description": "Habit reference such as h1."},
                "q": {"type": "string", "description": "Free-form search query with qualifiers. Boolean: AND (space/AND), OR, -/NOT. Parentheses supported. Qualifiers: status:<pending|scheduled|in_progress|completed|skipped|overdue>, title:<text>, desc:<text>, start:<date>, end:<date>, scheduled-start:<date>, scheduled-end:<date>, from:<date> (alias end:>=), until:<date> (alias start:<=), habit:<hN|N>, depends:<#N|UUID>, dependents:<#N|UUID>, deps_count:<op>N, is:<overdue|fixed|parallelizable|allows_parallel>, has:<description|completed_at|schedule|depends>. Date: YYYY-MM-DD, today, tomorrow, yesterday, Nd (relative), or operators like >=2026-07-25. Examples: status:pending 買い物, end:today OR end:tomorrow, -habit:h1, depends:#42"},
                "limit": {"type": "integer", "description": "Maximum number of tasks to return."},
            },
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let habit_ref = optional_string(&args, "habit_id")?;
        let habit = match habit_ref {
            Some(reference) => Some(
                self.client
                    .get_habit(&reference)
                    .await
                    .map_err(client_error)?,
            ),
            None => None,
        };
        let tz = server_timezone(&self.tz_cache).await;
        let query = TaskQuery {
            status: optional_string(&args, "status")?.map(|s| normalize_status(&s)),
            from: normalize_datetime(optional_string(&args, "from")?, &tz, "from")?,
            until: normalize_datetime(optional_string(&args, "until")?, &tz, "until")?,
            no_overdue: optional_bool(&args, "no_overdue")?,
            habit_id: habit.as_ref().map(|habit| habit.habit.id.clone()),
            ical_uid: None,
            q: optional_string(&args, "q")?,
            limit: optional_i64(&args, "limit")?,
        };

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let (tasks, all_tasks, habits) = tokio::try_join!(
            async { c1.list_tasks(&query).await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&all_tasks, &habits);
        let content = tasks
            .iter()
            .map(|task| task_json(task, &ctx, Some(&tz)))
            .collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

pub(super) struct GetTask {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for GetTask {
    fn name(&self) -> &'static str {
        "get_task"
    }
    fn description(&self) -> &'static str {
        "Get one or more tasks by display reference. task_ref may be a single #id or h<habit>#<task>, or an array of such references. Returns an object with `tasks` (requested), `dependencies` (all transitive dependencies), and `missing_dependencies` (dependency IDs not found in the task list)."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_ref": {
                    "anyOf": [
                        {"type": "string", "description": "#42 or h1#5"},
                        {"type": "array", "items": {"type": "string"}, "description": "Array of task references such as [\"#1\", \"h1#5\"]"}
                    ]
                },
            },
            "required": ["task_ref"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let task_refs = refs_from_args(&args, "task_ref")?;

        let tz = server_timezone(&self.tz_cache).await;
        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();

        let fetch_tasks = try_join_all(task_refs.iter().cloned().map(|r| {
            let client = c1.clone();
            async move { client.get_task(&r).await }
        }));

        let (tasks, all_tasks, habits) = tokio::try_join!(
            fetch_tasks,
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&all_tasks, &habits);
        let (deps, missing) = transitive_dependencies(&tasks, &all_tasks);

        let tasks_json: Vec<Value> = tasks
            .iter()
            .map(|task| task_json(task, &ctx, Some(&tz)))
            .collect();
        let deps_json: Vec<Value> = deps
            .into_iter()
            .map(|task| task_json(task, &ctx, Some(&tz)))
            .collect();

        let result = json!({
            "tasks": tasks_json,
            "dependencies": deps_json,
            "missing_dependencies": missing,
        });
        Ok(ToolOutput {
            content: serde_json::to_string(&result).unwrap(),
            ..Default::default()
        })
    }
}

pub(super) struct ListHabits {
    pub(super) client: Client,
}

#[async_trait]
impl Tool for ListHabits {
    fn name(&self) -> &'static str {
        "list_habits"
    }
    fn description(&self) -> &'static str {
        "List all habits."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let _ = object(args)?;
        let habits = self.client.list_habits().await.map_err(client_error)?;
        let content = habits.iter().map(habit_summary_json).collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

pub(super) struct GetHabit {
    pub(super) client: Client,
}

#[async_trait]
impl Tool for GetHabit {
    fn name(&self) -> &'static str {
        "get_habit"
    }
    fn description(&self) -> &'static str {
        "Get one or more habits by display reference. habit_ref may be a single h<id> or an array of such references. Returns an object with `habits`."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "habit_ref": {
                    "anyOf": [
                        {"type": "string", "description": "Habit reference such as h1"},
                        {"type": "array", "items": {"type": "string"}, "description": "Array of habit references such as [\"h1\", \"h2\"]"}
                    ]
                },
            },
            "required": ["habit_ref"],
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let habit_refs = refs_from_args(&args, "habit_ref")?;

        let habits = try_join_all(habit_refs.iter().cloned().map(|r| {
            let client = self.client.clone();
            async move { client.get_habit(&r).await }
        }))
        .await
        .map_err(client_error)?;

        let content = habits.iter().map(habit_json).collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&json!({"habits": content})).unwrap(),
            ..Default::default()
        })
    }
}

pub(super) struct GetSchedule {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for GetSchedule {
    fn name(&self) -> &'static str {
        "get_schedule"
    }
    fn description(&self) -> &'static str {
        "Get the current generated schedule with absolute timestamps. Optionally filter by a date range using from/to (e.g. 2026-07-20, 7d, today, now). Includes overdue tasks by default."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "from": {
                    "type": "string",
                    "description": "Start of the range; omitted means unbounded. Accepts absolute date (YYYY-MM-DD), relative days (e.g. '7d' for 7 days from now), 'today', or 'now'."
                },
                "to": {
                    "type": "string",
                    "description": "End of the range; omitted means unbounded. Accepts absolute date (YYYY-MM-DD), relative days (e.g. '7d' for 7 days from now), 'today', or 'now'."
                },
                "no_overdue": {
                    "type": "boolean",
                    "description": "If true, omit the overdue tasks section from the response."
                }
            },
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args = object(args)?;
        let no_overdue = optional_bool(&args, "no_overdue")?.unwrap_or(false);

        let tz = server_timezone(&self.tz_cache).await;
        let from = optional_string(&args, "from")?;
        let to = optional_string(&args, "to")?;
        let from_ts = from
            .map(|s| parse_date_expression(&s, &tz, false))
            .transpose()
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("from", format!("invalid: {e}")))
            })?;
        let to_ts = to
            .map(|s| parse_date_expression(&s, &tz, true))
            .transpose()
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("to", format!("invalid: {e}")))
            })?;

        if let (Some(from), Some(to)) = (from_ts, to_ts)
            && from > to
        {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "from",
                "must not be later than to",
            )));
        }

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let (schedule, tasks, habits) = tokio::try_join!(
            async { c1.get_schedule().await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&tasks, &habits);
        let entries: Value = serde_json::from_str(&schedule.schedule)
            .map_err(|error| ToolError::Other(Box::new(error)))?;
        let entries = match entries {
            Value::Array(entries) => entries,
            _ => Vec::new(),
        };
        let entries = entries
            .iter()
            .filter(|entry| entry_in_range(entry, from_ts, to_ts, &tz))
            .map(|entry| schedule_entry_value(entry, &ctx, Some(&tz)))
            .collect::<Vec<_>>();

        let mut content = json!({
            "id": schedule.id,
            "created_at": format_datetime_for_display(&schedule.created_at.to_string(), &tz),
            "updated_at": format_datetime_for_display(&schedule.updated_at.to_string(), &tz),
            "entries": entries,
        });

        if !no_overdue {
            let overdue: Vec<Value> = tasks
                .iter()
                .filter(|task| is_overdue(task, &tz))
                .filter(|task| overdue_in_range(task, from_ts, to_ts, &tz))
                .map(|task| {
                    let (reference, display_id, title) = match ctx.ref_by_id(&task.id) {
                        Some(r) => (
                            Value::String(r.reference.clone()),
                            json!(r.display_id),
                            Value::String(r.title.clone()),
                        ),
                        None => (
                            Value::String("unknown".into()),
                            Value::Null,
                            Value::String("unknown task".into()),
                        ),
                    };
                    json!({
                        "reference": reference,
                        "display_id": display_id,
                        "title": title,
                        "end_at": format_datetime_for_display(&task.end_at.to_string(), &tz),
                    })
                })
                .collect();
            if !overdue.is_empty() {
                content
                    .as_object_mut()
                    .unwrap()
                    .insert("overdue".into(), Value::Array(overdue));
            }
        }

        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

pub(super) struct HabitScheduledSpans {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for HabitScheduledSpans {
    fn name(&self) -> &'static str {
        "habit_scheduled_spans"
    }

    fn description(&self) -> &'static str {
        "List, create, or delete scheduled spans for a habit. The effect of a span depends on the habit's active flag: for active habits it is a pause period, for disabled habits it is an activation window. action=list returns existing spans; action=create and action=delete generate approval proposals."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "habit_ref": {"type": "string", "description": "Habit reference such as h1."},
                "action": {"type": "string", "enum": ["list", "create", "delete"], "description": "Operation to perform."},
                "start_date": {"type": "string", "description": "Start date (YYYY-MM-DD) for action=create."},
                "end_date": {"type": "string", "description": "End date (YYYY-MM-DD) for action=create."},
                "reason": {"type": "string", "description": "Optional reason for action=create."},
                "span_id": {"type": "string", "description": "Span id to delete for action=delete. Use the id returned by action=list."},
                "why": {"type": "string", "description": "Short user-facing reason for the proposed change."},
                "warnings": {"type": "array", "items": {"type": "string"}},
                "inferred_fields": crate::inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
            },
            "required": ["habit_ref", "action"],
            "additionalProperties": false,
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        let habit_ref = required_string(&args, "habit_ref")?;
        let habit_ref = strip_leading_hash(&habit_ref).to_string();
        args.insert("habit_ref".to_string(), Value::String(habit_ref.clone()));
        let action = required_string(&args, "action")?.to_lowercase();
        args.remove("action");

        let habit = self
            .client
            .get_habit(&habit_ref)
            .await
            .map_err(client_error)?;

        let tz = server_timezone(&self.tz_cache).await;

        match action.as_str() {
            "list" => self.list(&habit, &tz).await,
            "create" => self.propose_create(&mut args, &habit).await,
            "delete" => self.propose_delete(&mut args, &habit, &tz).await,
            _ => Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "action",
                format!("must be 'list', 'create', or 'delete', got {action}"),
            ))),
        }
    }
}

impl HabitScheduledSpans {
    async fn list(
        &self,
        habit: &HabitDetail,
        tz: &jiff::tz::TimeZone,
    ) -> Result<ToolOutput, ToolError> {
        let spans = self
            .client
            .list_habit_scheduled_spans(&habit.habit.id)
            .await
            .map_err(client_error)?;
        let content = json!({
            "habit_ref": format!("h{}", habit.habit.display_id),
            "active": habit.habit.active,
            "kind": if habit.habit.active { "pause" } else { "active" },
            "spans": spans.iter().map(|span| span_json(span, tz)).collect::<Vec<_>>(),
        });
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }

    async fn propose_create(
        &self,
        args: &mut serde_json::Map<String, Value>,
        habit: &HabitDetail,
    ) -> Result<ToolOutput, ToolError> {
        let start_date = required_string(args, "start_date")?;
        args.insert("start_date".to_string(), Value::String(start_date.clone()));
        let end_date = required_string(args, "end_date")?;
        args.insert("end_date".to_string(), Value::String(end_date.clone()));

        if let Some(reason) = optional_string(args, "reason")? {
            args.insert("reason".to_string(), Value::String(reason));
        } else {
            args.remove("reason");
        }

        validate_scheduled_span_dates(&start_date, &end_date)?;

        self.proposal_output(args, habit, ChangeOperation::CreateScheduledSpan, None)
            .await
    }

    async fn propose_delete(
        &self,
        args: &mut serde_json::Map<String, Value>,
        habit: &HabitDetail,
        tz: &jiff::tz::TimeZone,
    ) -> Result<ToolOutput, ToolError> {
        let span_id = required_string(args, "span_id")?;
        args.insert("span_id".to_string(), Value::String(span_id.clone()));
        let spans = self
            .client
            .list_habit_scheduled_spans(&habit.habit.id)
            .await
            .map_err(client_error)?;
        let span = spans
            .into_iter()
            .find(|span| span.id == span_id)
            .ok_or_else(|| {
                ToolError::NotFound(format!(
                    "scheduled span {span_id} not found for habit h{}",
                    habit.habit.display_id
                ))
            })?;
        let before = span_json(&span, tz);
        self.proposal_output(
            args,
            habit,
            ChangeOperation::DeleteScheduledSpan,
            Some(before),
        )
        .await
    }

    async fn proposal_output(
        &self,
        args: &mut serde_json::Map<String, Value>,
        habit: &HabitDetail,
        operation: ChangeOperation,
        before: Option<Value>,
    ) -> Result<ToolOutput, ToolError> {
        let mut start_date = optional_string(args, "start_date")?.unwrap_or_default();
        let mut end_date = optional_string(args, "end_date")?.unwrap_or_default();

        // For delete, prefer the actual span dates from `before`.
        if let Some(before_obj) = before.as_ref().and_then(Value::as_object) {
            if let Some(s) = before_obj.get("start_date").and_then(Value::as_str) {
                start_date = s.to_owned();
            }
            if let Some(s) = before_obj.get("end_date").and_then(Value::as_str) {
                end_date = s.to_owned();
            }
        }

        let description = match operation {
            ChangeOperation::CreateScheduledSpan => {
                format!(
                    "h{}にscheduled span {start_date}〜{end_date}を追加",
                    habit.habit.display_id
                )
            }
            ChangeOperation::DeleteScheduledSpan => {
                format!(
                    "h{}のscheduled span {start_date}〜{end_date}を削除",
                    habit.habit.display_id
                )
            }
            _ => unreachable!("unsupported operation for habit scheduled span: {operation}"),
        };

        let why = optional_string(args, "why")?.unwrap_or_default();
        if why.is_empty() {
            args.remove("why");
        } else {
            args.insert("why".to_string(), Value::String(why.clone()));
        }
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
        let inferred_fields = parse_inferred_fields(args)?;

        args.remove("action");
        let mut display_args = args.clone();
        format_display_datetime_args(&mut display_args, &server_timezone(&self.tz_cache).await);
        let execution_args = args.clone();

        let proposal = crate::ProposedChange {
            operation,
            target: Target::new(TargetKind::Habit, format!("h{}", habit.habit.display_id)),
            description,
            before,
            after: Some(Value::Object(display_args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at: None,
        };

        let content = json!({
            "approval_required": true,
            "target": proposal.target.to_string(),
        });

        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            why: Some(why),
            warnings,
            proposed_changes: vec![proposal],
            inferred_fields,
            schedule_dirty: false,
            ..Default::default()
        })
    }
}

fn span_json(span: &HabitScheduledSpanRow, tz: &jiff::tz::TimeZone) -> Value {
    json!({
        "id": span.id,
        "start_date": span.start_date,
        "end_date": span.end_date,
        "reason": span.reason,
        "created_at": format_datetime_for_display(&span.created_at.to_string(), tz),
    })
}

fn parse_inferred_fields(
    args: &serde_json::Map<String, Value>,
) -> Result<Vec<crate::InferredField>, ToolError> {
    let inferred_fields = args
        .get("inferred_fields")
        .cloned()
        .unwrap_or_else(|| json!([]));
    serde_json::from_value::<Vec<crate::InferredField>>(inferred_fields).map_err(|error| {
        ToolError::InvalidArgs(InvalidArgsError::new(
            "inferred_fields",
            format!("invalid: {error}"),
        ))
    })
}

fn validate_scheduled_span_dates(start: &str, end: &str) -> Result<(), ToolError> {
    let start = Date::strptime("%Y-%m-%d", start).map_err(|error| {
        ToolError::InvalidArgs(InvalidArgsError::new(
            "start_date",
            format!("invalid date: {error}"),
        ))
    })?;
    let end = Date::strptime("%Y-%m-%d", end).map_err(|error| {
        ToolError::InvalidArgs(InvalidArgsError::new(
            "end_date",
            format!("invalid date: {error}"),
        ))
    })?;
    if start > end {
        return Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "end_date",
            "must be on or after start_date",
        )));
    }
    Ok(())
}

pub(super) struct GetSettings {
    pub(super) client: Client,
}

#[async_trait]
impl Tool for GetSettings {
    fn name(&self) -> &'static str {
        "get_settings"
    }
    fn description(&self) -> &'static str {
        "Get server timezone and sleep/work settings."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let _ = object(args)?;
        let settings = self.client.get_settings().await.map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&settings).unwrap(),
            ..Default::default()
        })
    }
}

pub(super) struct PreviewScheduleTool {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

#[async_trait]
impl Tool for PreviewScheduleTool {
    fn name(&self) -> &'static str {
        "preview_schedule"
    }
    fn description(&self) -> &'static str {
        "Preview a schedule without replacing the active schedule; reports moved, unscheduled, and sleep impact."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": {"type": "string"},
                "from": {"type": "string", "description": "Start of range; interpreted in server timezone."},
                "until": {"type": "string", "description": "End of range; interpreted in server timezone."},
                "task_ids": {"type": "array", "items": {"type": "string"}},
                "pinned": {"type": "array", "items": {"type": "string"}},
                "sleep": {"type": "string"},
            },
            "additionalProperties": false,
        })
    }
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let mut args = object(args)?;
        args.entry("mode")
            .or_insert_with(|| Value::String("full".into()));

        let tz = server_timezone(&self.tz_cache).await;
        normalize_mutation_field(&mut args, "from", &tz)?;
        normalize_mutation_field(&mut args, "until", &tz)?;
        normalize_reference_array(&mut args, "task_ids")?;
        normalize_reference_array(&mut args, "pinned")?;

        let request: SchedulePreviewRequest =
            serde_json::from_value(Value::Object(args)).map_err(|error| {
                ToolError::InvalidArgs(InvalidArgsError::no_field(error.to_string()))
            })?;

        let default_query = TaskQuery::default();
        let c1 = self.client.clone();
        let c2 = self.client.clone();
        let c3 = self.client.clone();
        let req = request;
        let (preview, tasks, habits) = tokio::try_join!(
            async { c1.preview_schedule(&req).await },
            async { c2.list_tasks(&default_query).await },
            async { c3.list_habits().await },
        )
        .map_err(client_error)?;

        let ctx = TaskContext::new(&tasks, &habits);
        Ok(ToolOutput {
            content: serde_json::to_string(&transform_preview(preview, &ctx, Some(&tz))).unwrap(),
            ..Default::default()
        })
    }
}
