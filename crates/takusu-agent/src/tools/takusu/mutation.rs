use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::marker::PhantomData;
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
    optional_string, transform_preview,
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
    fn register_kind<S: MutationSpec>(
        registry: &mut ToolRegistry,
        client: &Client,
        tz_cache: &TimeZoneCache,
    ) {
        registry.register(Box::new(crate::tool::Typed(MutationTool::<S>::new(
            client.clone(),
            tz_cache.clone(),
        ))));
    }

    register_kind::<CreateTask>(registry, &client, &tz_cache);
    register_kind::<UpdateTask>(registry, &client, &tz_cache);
    register_kind::<DeleteTask>(registry, &client, &tz_cache);
    register_kind::<CreateHabit>(registry, &client, &tz_cache);
    register_kind::<UpdateHabit>(registry, &client, &tz_cache);
    register_kind::<DeleteHabit>(registry, &client, &tz_cache);
    register_kind::<GenerateSchedule>(registry, &client, &tz_cache);
    register_kind::<Reschedule>(registry, &client, &tz_cache);
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

/// Proposal metadata shared by every mutation args struct. `why`, `warnings`,
/// `inferred_fields`, and `proposal_id` are stripped from backend execution args
/// and surfaced on the [`ToolOutput`] instead.
pub(super) trait MutationMeta {
    fn why(&self) -> Option<String>;
    fn warnings(&self) -> Vec<String>;
    fn inferred_fields(&self) -> Vec<InferredField>;
    fn proposal_id(&self) -> Option<String>;
}

/// Whether a mutation runs a schedule preview, and how the preview `mode` is
/// determined.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Preview {
    None,
    /// Insert `mode: "full"` before previewing (generate_schedule).
    Full,
    /// Use the `mode` supplied in the args (reschedule).
    FromArgs,
}

/// Per-kind behaviour for a mutation tool. Each implementing type is a
/// zero-sized marker pairing a [`MutationTool`] with its args struct and
/// metadata, replacing the previous `MutationKind` enum and its many `match`
/// sites. Adding a mutation kind is now a matter of adding a new spec type.
#[async_trait]
pub(super) trait MutationSpec: Send + Sync + Sized + 'static {
    type Args: MutationMeta + serde::de::DeserializeOwned + JsonSchema + Serialize + Send + Sync;

    const NAME: ToolName;
    const DESCRIPTION: &'static str;
    const OPERATION: ChangeOperation;
    const TARGET_TYPE: TargetKind;

    fn exposure() -> ToolExposure {
        ToolExposure::Direct
    }

    /// Display target and human-readable description for the proposal.
    fn change_summary(args: &Self::Args) -> (String, String);

    /// Normalize datetime/status fields in place. Kinds with no datetime or
    /// status fields implement this as an explicit no-op, so adding a kind
    /// forces a conscious decision rather than silently skipping normalization.
    fn normalize(args: &mut Self::Args, tz: &jiff::tz::TimeZone) -> Result<(), ToolError>;

    /// Strip leading `#` from reference fields in the backend execution args.
    /// Defaults to a no-op.
    fn normalize_refs(
        _execution_args: &mut serde_json::Map<String, Value>,
    ) -> Result<(), ToolError> {
        Ok(())
    }

    /// Fetch the "before" state for update/delete operations. Defaults to
    /// no before state.
    async fn fetch_before(
        _client: &Client,
        _args: &Self::Args,
        _tz: &jiff::tz::TimeZone,
    ) -> Result<(Option<Value>, Option<String>), ToolError> {
        Ok((None, None))
    }

    fn preview() -> Preview {
        Preview::None
    }
}

/// Generic mutation tool parameterized by a [`MutationSpec`]. The shared
/// proposal-building pipeline lives here once; per-kind differences are
/// supplied by the spec.
pub(super) struct MutationTool<S: MutationSpec> {
    pub(super) client: Client,
    pub(super) tz_cache: TimeZoneCache,
    _spec: PhantomData<S>,
}

impl<S: MutationSpec> MutationTool<S> {
    pub(super) fn new(client: Client, tz_cache: TimeZoneCache) -> Self {
        Self {
            client,
            tz_cache,
            _spec: PhantomData,
        }
    }
}

#[async_trait]
impl<S: MutationSpec> TypedTool for MutationTool<S> {
    type Params = S::Args;

    fn name(&self) -> &'static str {
        S::NAME.into()
    }
    fn description(&self) -> &'static str {
        S::DESCRIPTION
    }
    fn exposure(&self) -> ToolExposure {
        S::exposure()
    }

    fn parameters_schema(&self) -> Value {
        let mut schema = self.default_parameters_schema();
        // Mutation tools historically always advertised a `required` array,
        // even when empty (generate_schedule). schemars omits the key when no
        // field is required, so re-add it to keep the LLM-facing contract
        // stable.
        if let Some(obj) = schema.as_object_mut() {
            obj.entry("required").or_insert(Value::Array(Vec::new()));
        }
        schema
    }

    async fn call_typed(&self, mut args: S::Args) -> Result<ToolOutput, ToolError> {
        let tz = server_timezone(&self.tz_cache).await;

        S::normalize(&mut args, &tz)?;

        // Serialize typed args to a Map for display and execution args.
        let value = serde_json::to_value(&args).map_err(|e| ToolError::Other(Box::new(e)))?;
        let mut display_args = value.as_object().cloned().unwrap_or_default();
        let mut execution_args = display_args.clone();
        // `why`, `warnings`, `inferred_fields`, and `proposal_id` are proposal
        // metadata, not backend arguments (matching MoveTaskTool behavior).
        execution_args.remove("why");
        execution_args.remove("warnings");
        execution_args.remove("inferred_fields");
        execution_args.remove("proposal_id");

        // Strip leading `#` from reference fields in execution_args only.
        S::normalize_refs(&mut execution_args)?;

        // `proposal_id` is internal grouping metadata and should not appear in
        // the diff UI.
        display_args.remove("proposal_id");

        // Convert absolute datetimes back to the configured timezone for the
        // approval UI; execution_args retains the canonical UTC values.
        format_display_datetime_args(&mut display_args, &tz);

        // Fetch "before" state for update/delete operations.
        let (before, observed_updated_at) = S::fetch_before(&self.client, &args, &tz).await?;

        // Run schedule preview and insert results into the args maps.
        match S::preview() {
            Preview::None => {}
            mode => {
                let mut preview_args = execution_args.clone();
                if mode == Preview::Full {
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
        }

        let (target, description) = S::change_summary(&args);
        let why = args.why();
        let warnings = args.warnings();
        let inferred_fields = args.inferred_fields();

        let proposal = ProposedChange {
            operation: S::OPERATION,
            target: Target::new(S::TARGET_TYPE, target),
            description,
            before,
            after: Some(Value::Object(display_args)),
            arguments: Some(Value::Object(execution_args)),
            observed_updated_at,
            proposal_id: args.proposal_id(),
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

/// Normalize an optional datetime field in place, rejecting invalid values.
fn normalize_optional_datetime(
    field: &mut Option<String>,
    name: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<(), ToolError> {
    if let Some(s) = field.take() {
        *field = Some(parse_datetime_tz(&s, tz).map_err(|e| {
            ToolError::InvalidArgs(InvalidArgsError::new(name, format!("invalid: {e}")))
        })?);
    }
    Ok(())
}

/// Normalize a required datetime field in place, rejecting invalid values.
fn normalize_required_datetime(
    field: &mut String,
    name: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<(), ToolError> {
    *field = parse_datetime_tz(field, tz).map_err(|e| {
        ToolError::InvalidArgs(InvalidArgsError::new(name, format!("invalid: {e}")))
    })?;
    Ok(())
}

/// Fetch the "before" state for a task update/delete.
async fn fetch_task_before(
    client: &Client,
    task_ref: &str,
    tz: &jiff::tz::TimeZone,
) -> Result<(Option<Value>, Option<String>), ToolError> {
    let lookup = strip_leading_hash(task_ref);

    let default_query = TaskQuery::default();
    let c1 = client.clone();
    let c2 = client.clone();
    let c3 = client.clone();
    let (task, all_tasks, habits) = tokio::try_join!(
        async { c1.get_task(lookup).await },
        async { c2.list_tasks(&default_query).await },
        async { c3.list_habits().await },
    )
    .map_err(client_error)?;

    let ctx = TaskContext::new(&all_tasks, &habits);
    Ok((
        Some(task_json(&task, &ctx, Some(tz))),
        Some(task.updated_at.to_string()),
    ))
}

/// Fetch the "before" state for a habit update/delete.
async fn fetch_habit_before(
    client: &Client,
    habit_ref: &str,
) -> Result<(Option<Value>, Option<String>), ToolError> {
    let habit = client.get_habit(habit_ref).await.map_err(client_error)?;
    Ok((
        Some(habit_json(&habit)),
        Some(habit.habit.updated_at.to_string()),
    ))
}

// ── create_task ─────────────────────────────────────────────────────────

/// Typed arguments for `create_task`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateTaskArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    title: String,
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
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    end_at: String,
    avg_minutes: i64,
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
    /// List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inferred_fields: Vec<InferredField>,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for CreateTaskArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        self.inferred_fields.clone()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct CreateTask;

#[async_trait]
impl MutationSpec for CreateTask {
    type Args = CreateTaskArgs;

    const NAME: ToolName = ToolName::CreateTask;
    const DESCRIPTION: &'static str = "Create a task proposal. Calling this tool generates a pending approval request; it does not write immediately. For one-utterance capture, fill title, quantity (quantity_total / quantity_unit), estimate (avg_minutes / sigma_minutes), end_at, and optional start_at from similar_tasks, memory, and context. Record the rationale in inferred_fields. For example, \"演習30題追加。金曜まで\".";
    const OPERATION: ChangeOperation = ChangeOperation::Create;
    const TARGET_TYPE: TargetKind = TargetKind::Task;

    fn change_summary(args: &Self::Args) -> (String, String) {
        let t = args.title.clone();
        (t.clone(), format!("「{t}」を作成"))
    }

    fn normalize(args: &mut Self::Args, tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        normalize_optional_datetime(&mut args.start_at, "start_at", tz)?;
        normalize_required_datetime(&mut args.end_at, "end_at", tz)?;
        Ok(())
    }

    fn normalize_refs(
        execution_args: &mut serde_json::Map<String, Value>,
    ) -> Result<(), ToolError> {
        normalize_reference_array(execution_args, "depends")
    }
}

// ── update_task ─────────────────────────────────────────────────────────

/// Typed arguments for `update_task`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateTaskArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    task_ref: String,
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
    /// New task status. 'completed' means done.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<StatusSchema>")]
    status: Option<String>,
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
    /// List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inferred_fields: Vec<InferredField>,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for UpdateTaskArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        self.inferred_fields.clone()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct UpdateTask;

#[async_trait]
impl MutationSpec for UpdateTask {
    type Args = UpdateTaskArgs;

    const NAME: ToolName = ToolName::UpdateTask;
    const DESCRIPTION: &'static str = "Create a task update proposal. Calling this tool generates a pending approval request; it does not write immediately.";
    const OPERATION: ChangeOperation = ChangeOperation::Update;
    const TARGET_TYPE: TargetKind = TargetKind::Task;

    fn change_summary(args: &Self::Args) -> (String, String) {
        let r = args.task_ref.clone();
        let description = args
            .title
            .as_ref()
            .map_or_else(|| format!("{r}を更新"), |t| format!("「{t}」を更新"));
        (r, description)
    }

    fn normalize(args: &mut Self::Args, tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        normalize_optional_datetime(&mut args.start_at, "start_at", tz)?;
        normalize_optional_datetime(&mut args.end_at, "end_at", tz)?;
        if let Some(s) = args.status.take() {
            let status = normalize_status(&s).map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("status", format!("invalid: {e}")))
            })?;
            if status == TaskStatusFilter::Overdue {
                return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                    "status",
                    "overdue is not a valid task status",
                )));
            }
            args.status = Some(status.to_string());
        }
        Ok(())
    }

    fn normalize_refs(
        execution_args: &mut serde_json::Map<String, Value>,
    ) -> Result<(), ToolError> {
        normalize_task_ref(execution_args, "task_ref")?;
        normalize_reference_array(execution_args, "depends")
    }

    async fn fetch_before(
        client: &Client,
        args: &Self::Args,
        tz: &jiff::tz::TimeZone,
    ) -> Result<(Option<Value>, Option<String>), ToolError> {
        fetch_task_before(client, &args.task_ref, tz).await
    }
}

// ── delete_task ─────────────────────────────────────────────────────────

/// Typed arguments for `delete_task`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteTaskArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    task_ref: String,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for DeleteTaskArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        Vec::new()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct DeleteTask;

#[async_trait]
impl MutationSpec for DeleteTask {
    type Args = DeleteTaskArgs;

    const NAME: ToolName = ToolName::DeleteTask;
    const DESCRIPTION: &'static str = "Create a task deletion proposal. Calling this tool generates a pending approval request; it does not write immediately.";
    const OPERATION: ChangeOperation = ChangeOperation::Delete;
    const TARGET_TYPE: TargetKind = TargetKind::Task;

    fn normalize(_args: &mut Self::Args, _tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        Ok(())
    }

    fn change_summary(args: &Self::Args) -> (String, String) {
        let r = args.task_ref.clone();
        (r.clone(), format!("{r}を削除"))
    }

    fn normalize_refs(
        execution_args: &mut serde_json::Map<String, Value>,
    ) -> Result<(), ToolError> {
        normalize_task_ref(execution_args, "task_ref")
    }

    async fn fetch_before(
        client: &Client,
        args: &Self::Args,
        tz: &jiff::tz::TimeZone,
    ) -> Result<(Option<Value>, Option<String>), ToolError> {
        fetch_task_before(client, &args.task_ref, tz).await
    }
}

// ── create_habit ────────────────────────────────────────────────────────

/// Typed arguments for `create_habit`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateHabitArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    title: String,
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    recurrence: String,
    /// Time of day (HH:MM).
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    start_time: String,
    /// Time of day (HH:MM).
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    end_time: String,
    avg_minutes: i64,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    description: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sigma_minutes: Option<i64>,
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
    /// List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inferred_fields: Vec<InferredField>,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for CreateHabitArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        self.inferred_fields.clone()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct CreateHabit;

#[async_trait]
impl MutationSpec for CreateHabit {
    type Args = CreateHabitArgs;

    const NAME: ToolName = ToolName::CreateHabit;
    const DESCRIPTION: &'static str = "Create a recurring habit proposal. Calling this tool generates a pending approval request; it does not write immediately.";
    const OPERATION: ChangeOperation = ChangeOperation::Create;
    const TARGET_TYPE: TargetKind = TargetKind::Habit;

    fn normalize(_args: &mut Self::Args, _tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        Ok(())
    }

    fn change_summary(args: &Self::Args) -> (String, String) {
        let t = args.title.clone();
        (t.clone(), format!("「{t}」を作成"))
    }
}

// ── update_habit ────────────────────────────────────────────────────────

/// Typed arguments for `update_habit`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateHabitArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    habit_ref: String,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    description: Option<String>,
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
    avg_minutes: Option<i64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    sigma_minutes: Option<i64>,
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
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    active: Option<bool>,
    /// If true, the start time is fixed and the scheduler will not move the task.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    fixed: Option<bool>,
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
    /// List of fields that were inferred from ambiguous user input and should be highlighted. Do not include obvious conversions (e.g. '1 hour' -> 60 minutes) or values filled from the current date/time.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inferred_fields: Vec<InferredField>,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for UpdateHabitArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        self.inferred_fields.clone()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct UpdateHabit;

#[async_trait]
impl MutationSpec for UpdateHabit {
    type Args = UpdateHabitArgs;

    const NAME: ToolName = ToolName::UpdateHabit;
    const DESCRIPTION: &'static str = "Create a recurring habit update proposal. Calling this tool generates a pending approval request; it does not write immediately.";
    const OPERATION: ChangeOperation = ChangeOperation::Update;
    const TARGET_TYPE: TargetKind = TargetKind::Habit;

    fn normalize(_args: &mut Self::Args, _tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        Ok(())
    }

    fn change_summary(args: &Self::Args) -> (String, String) {
        let r = args.habit_ref.clone();
        let description = args
            .title
            .as_ref()
            .map_or_else(|| format!("{r}を更新"), |t| format!("「{t}」を更新"));
        (r, description)
    }

    async fn fetch_before(
        client: &Client,
        args: &Self::Args,
        _tz: &jiff::tz::TimeZone,
    ) -> Result<(Option<Value>, Option<String>), ToolError> {
        fetch_habit_before(client, &args.habit_ref).await
    }
}

// ── delete_habit ────────────────────────────────────────────────────────

/// Typed arguments for `delete_habit`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteHabitArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    habit_ref: String,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for DeleteHabitArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        Vec::new()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct DeleteHabit;

#[async_trait]
impl MutationSpec for DeleteHabit {
    type Args = DeleteHabitArgs;

    const NAME: ToolName = ToolName::DeleteHabit;
    const DESCRIPTION: &'static str = "Create a recurring habit deletion proposal. Calling this tool generates a pending approval request; it does not write immediately.";
    const OPERATION: ChangeOperation = ChangeOperation::Delete;
    const TARGET_TYPE: TargetKind = TargetKind::Habit;

    fn normalize(_args: &mut Self::Args, _tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        Ok(())
    }

    fn change_summary(args: &Self::Args) -> (String, String) {
        let r = args.habit_ref.clone();
        (r.clone(), format!("{r}を削除"))
    }

    async fn fetch_before(
        client: &Client,
        args: &Self::Args,
        _tz: &jiff::tz::TimeZone,
    ) -> Result<(Option<Value>, Option<String>), ToolError> {
        fetch_habit_before(client, &args.habit_ref).await
    }
}

// ── generate_schedule ───────────────────────────────────────────────────

/// Typed arguments for `generate_schedule`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GenerateScheduleArgs {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    task_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    sleep: Option<String>,
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for GenerateScheduleArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        Vec::new()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct GenerateSchedule;

#[async_trait]
impl MutationSpec for GenerateSchedule {
    type Args = GenerateScheduleArgs;

    const NAME: ToolName = ToolName::GenerateSchedule;
    const DESCRIPTION: &'static str = "Create a schedule generation proposal. Calling this tool generates a pending approval request; it does not write immediately.";
    const OPERATION: ChangeOperation = ChangeOperation::Generate;
    const TARGET_TYPE: TargetKind = TargetKind::Schedule;

    fn exposure() -> ToolExposure {
        ToolExposure::Deferred
    }

    fn normalize(_args: &mut Self::Args, _tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        Ok(())
    }

    fn change_summary(_args: &Self::Args) -> (String, String) {
        (String::new(), "スケジュールを生成".to_owned())
    }

    fn normalize_refs(
        execution_args: &mut serde_json::Map<String, Value>,
    ) -> Result<(), ToolError> {
        normalize_reference_array(execution_args, "task_ids")
    }

    fn preview() -> Preview {
        Preview::Full
    }
}

// ── reschedule ──────────────────────────────────────────────────────────

/// Typed arguments for `reschedule`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RescheduleArgs {
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    mode: String,
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
    /// Short user-facing reason for the proposed change.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    why: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

impl MutationMeta for RescheduleArgs {
    fn why(&self) -> Option<String> {
        self.why.clone()
    }
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }
    fn inferred_fields(&self) -> Vec<InferredField> {
        Vec::new()
    }
    fn proposal_id(&self) -> Option<String> {
        self.proposal_id.clone()
    }
}

pub(super) struct Reschedule;

#[async_trait]
impl MutationSpec for Reschedule {
    type Args = RescheduleArgs;

    const NAME: ToolName = ToolName::Reschedule;
    const DESCRIPTION: &'static str = "Create a partial reschedule proposal. Calling this tool generates a pending approval request; it does not write immediately.";
    const OPERATION: ChangeOperation = ChangeOperation::Reschedule;
    const TARGET_TYPE: TargetKind = TargetKind::Schedule;

    fn exposure() -> ToolExposure {
        ToolExposure::Deferred
    }

    fn change_summary(_args: &Self::Args) -> (String, String) {
        (String::new(), "スケジュールを再調整".to_owned())
    }

    fn normalize(args: &mut Self::Args, tz: &jiff::tz::TimeZone) -> Result<(), ToolError> {
        normalize_optional_datetime(&mut args.from, "from", tz)?;
        normalize_optional_datetime(&mut args.until, "until", tz)?;
        Ok(())
    }

    fn normalize_refs(
        execution_args: &mut serde_json::Map<String, Value>,
    ) -> Result<(), ToolError> {
        normalize_reference_array(execution_args, "task_ids")?;
        normalize_reference_array(execution_args, "pinned")
    }

    fn preview() -> Preview {
        Preview::FromArgs
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
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
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
        display_args.remove("proposal_id");
        format_display_datetime_args(&mut display_args, &tz);

        let mut execution_args = base;
        execution_args.remove("why");
        execution_args.remove("warnings");
        execution_args.remove("inferred_fields");
        execution_args.remove("proposal_id");

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
            proposal_id: args.proposal_id,
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
