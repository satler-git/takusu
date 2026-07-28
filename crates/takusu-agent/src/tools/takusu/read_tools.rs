use async_trait::async_trait;
use futures_util::future::try_join_all;
use jiff::civil::Date;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use takusu_client::{
    Client, HabitDetail, HabitScheduledSpanRow, SchedulePreviewRequest, TaskQuery,
};
use takusu_util::{parse_date_expression, parse_datetime_to_timestamp, parse_datetime_tz};

use crate::{
    ChangeOperation, InferredField, InvalidArgsError, ProposedChange, Target, TargetKind,
    ToolError, ToolExposure, ToolName, ToolOutput, ToolRegistry, TypedTool,
    deserialize_trimmed_optional, deserialize_trimmed_required, inferred_fields_schema,
};

use super::common::{
    TaskContext, TimeZoneCache, client_error, entry_in_range, format_datetime_for_display,
    format_display_datetime_args, habit_json, habit_summary_json, is_overdue, normalize_status,
    overdue_in_range, schedule_entry_value, server_timezone, strip_leading_hash, task_json,
    transform_preview, transitive_dependencies,
};

/// Registers the read-only planner tools used by the agent.
pub(super) fn register_read_tools(
    registry: &mut ToolRegistry,
    client: Client,
    tz_cache: TimeZoneCache,
) {
    registry.register(Box::new(crate::tool::Typed(ListTasks {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    })));
    registry.register(Box::new(crate::tool::Typed(GetTask {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    })));
    registry.register(Box::new(crate::tool::Typed(ListHabits {
        client: client.clone(),
    })));
    registry.register(Box::new(crate::tool::Typed(GetHabit {
        client: client.clone(),
    })));
    registry.register(Box::new(crate::tool::Typed(GetSchedule {
        client: client.clone(),
        tz_cache: tz_cache.clone(),
    })));
    registry.register(Box::new(crate::tool::Typed(GetSettings {
        client: client.clone(),
    })));
    registry.register(Box::new(crate::tool::Typed(PreviewScheduleTool {
        client,
        tz_cache,
    })));
}

// ── shared types ────────────────────────────────────────────────────────

// Accepts either a single string or an array of strings in tool arguments.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
enum StringOrArray {
    Single(String),
    Multiple(Vec<String>),
}

impl StringOrArray {
    /// Convert to a list of references: trim, strip leading `#`, and dedupe
    /// while preserving order. Empty values are skipped.
    fn to_refs(&self) -> Vec<String> {
        let raw: &[String] = match self {
            StringOrArray::Single(s) => std::slice::from_ref(s),
            StringOrArray::Multiple(v) => v.as_slice(),
        };
        let mut refs = Vec::new();
        let mut seen = HashSet::new();
        for r in raw {
            let r = r.trim();
            if r.is_empty() {
                continue;
            }
            let r = strip_leading_hash(r);
            if r.is_empty() {
                continue;
            }
            if seen.insert(r.to_string()) {
                refs.push(r.to_string());
            }
        }
        refs
    }
}

/// Schema-only enum that generates the task status filter enum in the JSON
/// schema. The actual field is `Option<String>` so any string is accepted at
/// runtime and normalized via `normalize_status`.
#[allow(dead_code)]
#[derive(JsonSchema)]
#[schemars(rename_all = "snake_case")]
enum TaskStatusFilterSchema {
    Pending,
    Scheduled,
    InProgress,
    Completed,
    Skipped,
    Overdue,
}

/// Action enum for `habit_scheduled_spans`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ScheduledSpanAction {
    List,
    Create,
    Delete,
}

// ── ListTasks ───────────────────────────────────────────────────────────

pub(super) struct ListTasks {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

/// Arguments for [`ListTasks`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ListTasksArgs {
    /// Task status filter. Use 'completed' for done tasks, 'overdue' for tasks whose end_at has passed but are not completed or skipped.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "TaskStatusFilterSchema")]
    status: Option<String>,
    /// Start of range; interpreted in server timezone.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    from: Option<String>,
    /// End of range; interpreted in server timezone.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    until: Option<String>,
    /// If true, exclude tasks whose end_at has passed. Do not use together with status='overdue'.
    #[serde(default)]
    no_overdue: Option<bool>,
    /// Habit reference such as h1.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    habit_id: Option<String>,
    /// Free-form search query with qualifiers. Boolean: AND (space/AND), OR, -/NOT. Parentheses supported. Qualifiers: status:<pending|scheduled|in_progress|completed|skipped|overdue>, title:<text>, desc:<text>, start:<date>, end:<date>, scheduled-start:<date>, scheduled-end:<date>, from:<date> (alias end:>=), until:<date> (alias start:<=), habit:<hN|N>, depends:<#N|UUID>, dependents:<#N|UUID>, deps_count:<op>N, is:<overdue|fixed|parallelizable|allows_parallel>, has:<description|completed_at|schedule|depends>. Date: YYYY-MM-DD, today, tomorrow, yesterday, Nd (relative), or operators like >=2026-07-25. Examples: status:pending 買い物, end:today OR end:tomorrow, -habit:h1, depends:#42
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    q: Option<String>,
    /// Maximum number of tasks to return.
    #[serde(default)]
    limit: Option<i64>,
}

#[async_trait]
impl TypedTool for ListTasks {
    type Params = ListTasksArgs;

    fn name(&self) -> &'static str {
        ToolName::ListTasks.into()
    }
    fn description(&self) -> &'static str {
        "List tasks, optionally filtered by status, time range, or habit."
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let habit = match args.habit_id {
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
            status: args
                .status
                .map(|s| {
                    let normalized = normalize_status(&s);
                    normalized
                        .parse::<takusu_util::TaskStatusFilter>()
                        .map_err(|e| {
                            ToolError::InvalidArgs(InvalidArgsError::new(
                                "status",
                                format!("invalid: {e}"),
                            ))
                        })
                })
                .transpose()?,
            from: args
                .from
                .map(|s| parse_datetime_to_timestamp(&s, &tz).map(takusu_util::Timestamp::from))
                .transpose()
                .map_err(|e| {
                    ToolError::InvalidArgs(InvalidArgsError::new("from", format!("invalid: {e}")))
                })?,
            until: args
                .until
                .map(|s| parse_datetime_to_timestamp(&s, &tz).map(takusu_util::Timestamp::from))
                .transpose()
                .map_err(|e| {
                    ToolError::InvalidArgs(InvalidArgsError::new("until", format!("invalid: {e}")))
                })?,
            no_overdue: args.no_overdue,
            habit_id: habit.as_ref().map(|habit| habit.habit.id.clone()),
            ical_uid: None,
            q: args.q,
            limit: args.limit,
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

// ── GetTask ─────────────────────────────────────────────────────────────

pub(super) struct GetTask {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

/// Arguments for [`GetTask`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GetTaskArgs {
    task_ref: StringOrArray,
}

#[async_trait]
impl TypedTool for GetTask {
    type Params = GetTaskArgs;

    fn name(&self) -> &'static str {
        ToolName::GetTask.into()
    }
    fn description(&self) -> &'static str {
        "Get one or more tasks by display reference. task_ref may be a single #id or h<habit>#<task>, or an array of such references. Returns an object with `tasks` (requested), `dependencies` (all transitive dependencies), and `missing_dependencies` (dependency IDs not found in the task list)."
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        if let Some(any_of) = schema
            .get_mut("properties")
            .and_then(|v| v.get_mut("task_ref"))
            .and_then(|v| v.get_mut("anyOf"))
            .and_then(Value::as_array_mut)
        {
            for variant in any_of.iter_mut() {
                if variant.get("type").and_then(Value::as_str) == Some("string") {
                    variant["description"] = json!("#42 or h1#5");
                } else if variant.get("type").and_then(Value::as_str) == Some("array") {
                    variant["description"] =
                        json!("Array of task references such as [\"#1\", \"h1#5\"]");
                }
            }
        }
        schema
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if args.task_ref.to_refs().is_empty() {
            return Err(InvalidArgsError::new("task_ref", "must not be empty"));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_refs = args.task_ref.to_refs();

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

// ── ListHabits ──────────────────────────────────────────────────────────

pub(super) struct ListHabits {
    pub(super) client: Client,
}

/// Arguments for [`ListHabits`] (no parameters).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ListHabitsArgs {}

#[async_trait]
impl TypedTool for ListHabits {
    type Params = ListHabitsArgs;

    fn name(&self) -> &'static str {
        ToolName::ListHabits.into()
    }
    fn description(&self) -> &'static str {
        "List all habits."
    }

    async fn call_typed(&self, _args: Self::Params) -> Result<ToolOutput, ToolError> {
        let habits = self.client.list_habits().await.map_err(client_error)?;
        let content = habits.iter().map(habit_summary_json).collect::<Vec<_>>();
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            ..Default::default()
        })
    }
}

// ── GetHabit ────────────────────────────────────────────────────────────

pub(super) struct GetHabit {
    pub(super) client: Client,
}

/// Arguments for [`GetHabit`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GetHabitArgs {
    habit_ref: StringOrArray,
}

#[async_trait]
impl TypedTool for GetHabit {
    type Params = GetHabitArgs;

    fn name(&self) -> &'static str {
        ToolName::GetHabit.into()
    }
    fn description(&self) -> &'static str {
        "Get one or more habits by display reference. habit_ref may be a single h<id> or an array of such references. Returns an object with `habits`."
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        if let Some(any_of) = schema
            .get_mut("properties")
            .and_then(|v| v.get_mut("habit_ref"))
            .and_then(|v| v.get_mut("anyOf"))
            .and_then(Value::as_array_mut)
        {
            for variant in any_of.iter_mut() {
                if variant.get("type").and_then(Value::as_str) == Some("string") {
                    variant["description"] = json!("Habit reference such as h1");
                } else if variant.get("type").and_then(Value::as_str) == Some("array") {
                    variant["description"] =
                        json!("Array of habit references such as [\"h1\", \"h2\"]");
                }
            }
        }
        schema
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if args.habit_ref.to_refs().is_empty() {
            return Err(InvalidArgsError::new("habit_ref", "must not be empty"));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let habit_refs = args.habit_ref.to_refs();

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

// ── GetSchedule ─────────────────────────────────────────────────────────

pub(super) struct GetSchedule {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

/// Arguments for [`GetSchedule`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GetScheduleArgs {
    /// Start of the range; omitted means unbounded. Accepts absolute date (YYYY-MM-DD), relative days (e.g. '7d' for 7 days from now), 'today', or 'now'.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    from: Option<String>,
    /// End of the range; omitted means unbounded. Accepts absolute date (YYYY-MM-DD), relative days (e.g. '7d' for 7 days from now), 'today', or 'now'.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    to: Option<String>,
    /// If true, omit the overdue tasks section from the response.
    #[serde(default)]
    no_overdue: Option<bool>,
}

#[async_trait]
impl TypedTool for GetSchedule {
    type Params = GetScheduleArgs;

    fn name(&self) -> &'static str {
        ToolName::GetSchedule.into()
    }
    fn description(&self) -> &'static str {
        "Get the current generated schedule with absolute timestamps. Optionally filter by a date range using from/to (e.g. 2026-07-20, 7d, today, now). Includes overdue tasks by default."
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let no_overdue = args.no_overdue.unwrap_or(false);

        let tz = server_timezone(&self.tz_cache).await;
        let from_ts = args
            .from
            .map(|s| parse_date_expression(&s, &tz, false))
            .transpose()
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("from", format!("invalid: {e}")))
            })?;
        let to_ts = args
            .to
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

// ── HabitScheduledSpans ─────────────────────────────────────────────────

pub(super) struct HabitScheduledSpans {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

/// Arguments for [`HabitScheduledSpans`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct HabitScheduledSpansArgs {
    /// Habit reference such as h1.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    habit_ref: String,
    /// Operation to perform.
    action: ScheduledSpanAction,
    /// Start date (YYYY-MM-DD) for action=create.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    start_date: Option<String>,
    /// End date (YYYY-MM-DD) for action=create.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    end_date: Option<String>,
    /// Optional reason for action=create.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    reason: Option<String>,
    /// Span id to delete for action=delete. Use the id returned by action=list.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    span_id: Option<String>,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    inferred_fields: Vec<InferredField>,
}

#[async_trait]
impl TypedTool for HabitScheduledSpans {
    type Params = HabitScheduledSpansArgs;

    fn name(&self) -> &'static str {
        ToolName::HabitScheduledSpans.into()
    }

    fn description(&self) -> &'static str {
        "List, create, or delete scheduled spans for a habit. The effect of a span depends on the habit's active flag: for active habits it is a pause period, for disabled habits it is an activation window. action=list returns existing spans; action=create and action=delete generate approval proposals."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            props.insert(
                "inferred_fields".into(),
                inferred_fields_schema("List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time."),
            );
        }
        schema
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        match args.action {
            ScheduledSpanAction::Create => {
                if args.start_date.is_none() {
                    return Err(InvalidArgsError::new(
                        "start_date",
                        "required for action=create",
                    ));
                }
                if args.end_date.is_none() {
                    return Err(InvalidArgsError::new(
                        "end_date",
                        "required for action=create",
                    ));
                }
                if let (Some(start), Some(end)) = (&args.start_date, &args.end_date) {
                    validate_scheduled_span_dates(start, end).map_err(|e| e.into_invalid_args())?;
                }
            }
            ScheduledSpanAction::Delete => {
                if args.span_id.is_none() {
                    return Err(InvalidArgsError::new(
                        "span_id",
                        "required for action=delete",
                    ));
                }
            }
            ScheduledSpanAction::List => {}
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let habit_ref = strip_leading_hash(&args.habit_ref).to_string();
        let habit = self
            .client
            .get_habit(&habit_ref)
            .await
            .map_err(client_error)?;

        let tz = server_timezone(&self.tz_cache).await;

        match args.action {
            ScheduledSpanAction::List => self.list(&habit, &tz).await,
            ScheduledSpanAction::Create => {
                self.proposal_output(
                    &args,
                    &habit_ref,
                    &habit,
                    ChangeOperation::CreateScheduledSpan,
                    None,
                )
                .await
            }
            ScheduledSpanAction::Delete => {
                let span_id = args.span_id.as_ref().expect("validated in validate_args");
                let spans = self
                    .client
                    .list_habit_scheduled_spans(&habit.habit.id)
                    .await
                    .map_err(client_error)?;
                let span = spans
                    .into_iter()
                    .find(|span| span.id == *span_id)
                    .ok_or_else(|| {
                        ToolError::NotFound(format!(
                            "scheduled span {span_id} not found for habit h{}",
                            habit.habit.display_id
                        ))
                    })?;
                let before = span_json(&span, &tz);
                self.proposal_output(
                    &args,
                    &habit_ref,
                    &habit,
                    ChangeOperation::DeleteScheduledSpan,
                    Some(before),
                )
                .await
            }
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

    async fn proposal_output(
        &self,
        args: &HabitScheduledSpansArgs,
        habit_ref: &str,
        habit: &HabitDetail,
        operation: ChangeOperation,
        before: Option<Value>,
    ) -> Result<ToolOutput, ToolError> {
        // Determine start_date and end_date for the description.
        // For delete, prefer the actual span dates from `before`.
        let (start_date, end_date) = match before.as_ref().and_then(Value::as_object) {
            Some(before_obj) => {
                let sd = before_obj
                    .get("start_date")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let ed = before_obj
                    .get("end_date")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                (sd, ed)
            }
            None => (
                args.start_date.clone().unwrap_or_default(),
                args.end_date.clone().unwrap_or_default(),
            ),
        };

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

        let why = args.why.clone().unwrap_or_default();

        // Build execution_args and display_args from the typed fields.
        let mut execution_args = serde_json::Map::new();
        execution_args.insert("habit_ref".into(), Value::String(habit_ref.to_string()));
        if let Some(sd) = &args.start_date {
            execution_args.insert("start_date".into(), Value::String(sd.clone()));
        }
        if let Some(ed) = &args.end_date {
            execution_args.insert("end_date".into(), Value::String(ed.clone()));
        }
        if let Some(reason) = &args.reason {
            execution_args.insert("reason".into(), Value::String(reason.clone()));
        }
        if let Some(span_id) = &args.span_id {
            execution_args.insert("span_id".into(), Value::String(span_id.clone()));
        }
        if !why.is_empty() {
            execution_args.insert("why".into(), Value::String(why.clone()));
        }
        if !args.warnings.is_empty() {
            execution_args.insert("warnings".into(), json!(args.warnings));
        }
        if !args.inferred_fields.is_empty() {
            execution_args.insert("inferred_fields".into(), json!(args.inferred_fields));
        }

        let mut display_args = execution_args.clone();
        let tz = server_timezone(&self.tz_cache).await;
        format_display_datetime_args(&mut display_args, &tz);

        let proposal = ProposedChange {
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
            warnings: args.warnings.clone(),
            proposed_changes: vec![proposal],
            inferred_fields: args.inferred_fields.clone(),
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

// ── GetSettings ─────────────────────────────────────────────────────────

pub(super) struct GetSettings {
    pub(super) client: Client,
}

/// Arguments for [`GetSettings`] (no parameters).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GetSettingsArgs {}

#[async_trait]
impl TypedTool for GetSettings {
    type Params = GetSettingsArgs;

    fn name(&self) -> &'static str {
        ToolName::GetSettings.into()
    }
    fn description(&self) -> &'static str {
        "Get server timezone and sleep/work settings."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    async fn call_typed(&self, _args: Self::Params) -> Result<ToolOutput, ToolError> {
        let settings = self.client.get_settings().await.map_err(client_error)?;
        Ok(ToolOutput {
            content: serde_json::to_string(&settings).unwrap(),
            ..Default::default()
        })
    }
}

// ── PreviewScheduleTool ─────────────────────────────────────────────────

pub(super) struct PreviewScheduleTool {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
}

/// Arguments for [`PreviewScheduleTool`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PreviewScheduleArgs {
    #[serde(default)]
    mode: Option<String>,
    /// Start of range; interpreted in server timezone.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    from: Option<String>,
    /// End of range; interpreted in server timezone.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    until: Option<String>,
    #[serde(default)]
    task_ids: Option<Vec<String>>,
    #[serde(default)]
    pinned: Option<Vec<String>>,
    #[serde(default)]
    sleep: Option<String>,
}

#[async_trait]
impl TypedTool for PreviewScheduleTool {
    type Params = PreviewScheduleArgs;

    fn name(&self) -> &'static str {
        ToolName::PreviewSchedule.into()
    }
    fn description(&self) -> &'static str {
        "Preview a schedule without replacing the active schedule; reports moved, unscheduled, and sleep impact."
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let tz = server_timezone(&self.tz_cache).await;

        let from = args
            .from
            .map(|s| parse_datetime_tz(&s, &tz))
            .transpose()
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("from", format!("invalid: {e}")))
            })?;
        let until = args
            .until
            .map(|s| parse_datetime_tz(&s, &tz))
            .transpose()
            .map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("until", format!("invalid: {e}")))
            })?;

        let task_ids = args.task_ids.map(|v| {
            v.iter()
                .map(|s| strip_leading_hash(s.trim()).to_string())
                .collect::<Vec<_>>()
        });
        let pinned = args
            .pinned
            .unwrap_or_default()
            .iter()
            .map(|s| strip_leading_hash(s.trim()).to_string())
            .collect::<Vec<_>>();

        let request = SchedulePreviewRequest {
            mode: args
                .mode
                .as_deref()
                .unwrap_or("full")
                .parse()
                .map_err(|e| {
                    ToolError::InvalidArgs(InvalidArgsError::new("mode", format!("invalid: {e}")))
                })?,
            from,
            until,
            task_ids,
            pinned,
            sleep: args
                .sleep
                .as_deref()
                .unwrap_or("recommended")
                .parse()
                .map_err(|e| {
                    ToolError::InvalidArgs(InvalidArgsError::new(
                        "sleep",
                        format!("invalid: {e}"),
                    ))
                })?,
        };

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
        let preview_value = serde_json::to_value(&preview).unwrap();
        Ok(ToolOutput {
            content: serde_json::to_string(&transform_preview(preview_value, &ctx, Some(&tz)))
                .unwrap(),
            ..Default::default()
        })
    }
}
