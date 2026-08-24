use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use takusu_client::{Client, HabitRow, TaskQuery, TaskRow, WorkSessionRow};
use takusu_types::estimator::{DurationDistribution, effective_distribution, progress_posterior};
use takusu_types::{Quantity, TaskStatus};

use crate::tools::takusu::{
    TaskContext, TimeZoneCache, client_error, server_timezone, strip_leading_hash, task_json,
};
use crate::tools::{ToolContext, ToolModule};
use crate::{
    ChangeOperation, InvalidArgsError, ProposedChange, Target, TargetKind, ToolError, ToolExposure,
    ToolName, ToolOutput, ToolRegistry, TypedTool, deserialize_trimmed_optional,
};

struct ProgressModule;

impl ToolModule for ProgressModule {
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(TaskStart {
            client: ctx.client.clone(),
            tz_cache: ctx.tz_cache.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(TaskPause {
            client: ctx.client.clone(),
            tz_cache: ctx.tz_cache.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(TaskProgress {
            client: ctx.client.clone(),
            tz_cache: ctx.tz_cache.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(TaskComplete {
            client: ctx.client.clone(),
            tz_cache: ctx.tz_cache.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(TaskSplit {
            client: ctx.client.clone(),
            tz_cache: ctx.tz_cache.clone(),
        })));
        registry.register(Box::new(crate::tool::Typed(TaskUndo {
            client: ctx.client.clone(),
            tz_cache: ctx.tz_cache.clone(),
        })));
    }
}

static PROGRESS_MODULE: &dyn ToolModule = &ProgressModule;

inventory::submit!(PROGRESS_MODULE);

async fn load_task_context(
    client: &Client,
) -> Result<(Vec<TaskRow>, Vec<HabitRow>, TaskContext), ToolError> {
    let (tasks, habits) = tokio::try_join!(
        async {
            client
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(client_error)
        },
        async { client.list_habits().await.map_err(client_error) },
    )?;
    let ctx = TaskContext::new(&tasks, &habits);
    Ok((tasks, habits, ctx))
}

async fn resolve_task(
    client: &Client,
    task_ref: &str,
) -> Result<(TaskRow, TaskContext), ToolError> {
    let (tasks, habits, mut ctx) = load_task_context(client).await?;
    let task_ref = strip_leading_hash(task_ref);
    if let Some(t) = tasks
        .iter()
        .find(|t| ctx.reference(t).trim_start_matches('#') == task_ref)
    {
        return Ok((t.clone(), ctx));
    }
    let task = client.get_task(task_ref).await.map_err(client_error)?;
    let mut tasks = tasks;
    if !tasks.iter().any(|t| t.id == task.id) {
        tasks.push(task.clone());
    }
    ctx = TaskContext::new(&tasks, &habits);
    Ok((task, ctx))
}

fn clarification_output(message: &str) -> ToolOutput {
    ToolOutput {
        content: json!({ "focused_clarification": message }).to_string(),
        why: Some(message.to_string()),
        warnings: Vec::new(),
        proposed_changes: Vec::new(),
        inferred_fields: Vec::new(),
        changes: Vec::new(),
        discovered_tools: Vec::new(),
        schedule_dirty: false,
        is_error: false,
        intake_state: None,
    }
}

fn focused_clarification(
    action: &str,
    status_hint: &str,
    candidates: &[TaskRow],
    ctx: &TaskContext,
) -> ToolOutput {
    let message = if candidates.is_empty() {
        format!(
            "{}する対象のタスクが見つかりません。{}のタスクを #番号 や h#番号 で指定してください。",
            action, status_hint
        )
    } else {
        let lines: Vec<String> = candidates
            .iter()
            .map(|t| format!("- {}: {}", ctx.reference(t), t.title))
            .collect();
        format!(
            "「{}」の対象となるタスクが複数あります。どれですか？\n{}",
            action,
            lines.join("\n")
        )
    };
    clarification_output(&message)
}

fn estimate_preview(
    avg_minutes: i64,
    sigma_minutes: i64,
    task_kind_prior: Option<DurationDistribution>,
    quantity_total: Option<i64>,
    active_minutes: i64,
    quantity_done: i64,
) -> (i64, i64) {
    let Some(total) = quantity_total.filter(|total| *total > 0) else {
        return (avg_minutes, sigma_minutes);
    };
    if quantity_done <= 0 || active_minutes <= 0 {
        return (avg_minutes, sigma_minutes);
    }
    let fraction = (quantity_done as f64 / total as f64).clamp(f64::EPSILON, 1.0);
    let prior = effective_distribution(avg_minutes as f64, sigma_minutes as f64, task_kind_prior);
    let Ok(posterior) = progress_posterior(prior, active_minutes as f64, fraction) else {
        return (avg_minutes, sigma_minutes);
    };
    (
        posterior.posterior.mean_minutes().round() as i64,
        posterior.posterior.stddev_minutes().round() as i64,
    )
}

fn active_minutes_for_sessions(sessions: &[WorkSessionRow]) -> i64 {
    sessions
        .iter()
        .map(|session| {
            let end = session
                .ended_at
                .map(|timestamp| timestamp.to_string())
                .unwrap_or_else(takusu_types::now_rfc3339);
            takusu_types::minutes_between(&session.started_at.to_string(), &end)
        })
        .sum()
}

fn apply_estimate_preview(
    after: &mut Value,
    content_extra: &mut serde_json::Map<String, Value>,
    avg: i64,
    sigma: i64,
    task: &TaskRow,
) {
    let mut changed = false;
    if let Some(obj) = after.as_object_mut() {
        if avg != task.avg_minutes {
            obj.insert("avg_minutes".to_string(), Value::Number(avg.into()));
            changed = true;
        }
        if sigma != task.sigma_minutes {
            obj.insert("sigma_minutes".to_string(), Value::Number(sigma.into()));
            changed = true;
        }
    }
    if changed {
        content_extra.insert(
            "estimate_preview".to_string(),
            json!({"avg_minutes": avg, "sigma_minutes": sigma}),
        );
    }
}

/// Content payload for progress tools, extending [`ProposalContent`] with
/// tool-specific extra fields (e.g. `estimate_preview`, `choices`).
#[derive(Debug, Serialize)]
struct ProgressContent {
    approval_required: bool,
    target: String,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}

#[allow(clippy::too_many_arguments)]
fn progress_output(
    operation: ChangeOperation,
    target: &Target,
    description: &str,
    why: &str,
    before: Value,
    after: Value,
    execution_args: Value,
    warnings: Vec<String>,
    content_extra: serde_json::Map<String, Value>,
    observed_updated_at: Option<String>,
    schedule_dirty: bool,
    proposal_id: Option<String>,
) -> ToolOutput {
    let content = ProgressContent {
        approval_required: true,
        target: target.to_string(),
        extra: content_extra,
    };
    ToolOutput {
        content: serde_json::to_string(&content).unwrap(),
        why: Some(why.to_string()),
        warnings,
        proposed_changes: vec![ProposedChange {
            operation,
            target: target.clone(),
            description: description.to_string(),
            before: Some(before),
            after: Some(after),
            arguments: Some(execution_args),
            observed_updated_at,
            proposal_id,
        }],
        inferred_fields: Vec::new(),
        changes: Vec::new(),
        discovered_tools: Vec::new(),
        schedule_dirty,
        is_error: false,
        intake_state: None,
    }
}

struct TaskStart {
    client: Client,
    tz_cache: TimeZoneCache,
}

/// Arguments for [`TaskStart`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskStartArgs {
    /// Task reference such as #42 or h1#3. Omit if the user did not specify a task; a focused clarification will be returned.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    task_ref: Option<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

#[async_trait]
impl TypedTool for TaskStart {
    type Params = TaskStartArgs;

    fn name(&self) -> &'static str {
        ToolName::TaskStart.into()
    }

    fn description(&self) -> &'static str {
        "Propose starting work on a task. Creates an open work session and sets the task status to in_progress. If task_ref is omitted, asks for clarification. Requires approval before writing."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_ref = args.task_ref.map(|s| strip_leading_hash(&s).to_string());
        let tz = server_timezone(&self.tz_cache).await;

        let (task, ctx) = match task_ref {
            Some(ref r) => resolve_task(&self.client, r).await?,
            None => {
                let (tasks, _habits, ctx) = load_task_context(&self.client).await?;
                let candidates: Vec<TaskRow> = tasks
                    .into_iter()
                    .filter(|t| {
                        t.status == TaskStatus::Scheduled || t.status == TaskStatus::Pending
                    })
                    .collect();
                return Ok(focused_clarification(
                    "着手",
                    "予定または未スケジュール（pending）",
                    &candidates,
                    &ctx,
                ));
            }
        };
        let display_ref = ctx.reference(&task);
        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                format!("cannot start work on a {} task", task.status),
            )));
        }

        let before = task_json(&task, &ctx, Some(&tz));
        let mut after = before.clone();
        if let Some(obj) = after.as_object_mut() {
            obj.insert(
                "status".to_string(),
                Value::String("in_progress".to_string()),
            );
        }

        let execution_args = json!({
            "task_ref": display_ref.trim_start_matches('#'),
        });
        let observed = Some(task.updated_at.to_string());

        Ok(progress_output(
            ChangeOperation::Start,
            &Target::new(TargetKind::Task, display_ref.as_str()),
            &format!("「{}」の作業を開始", task.title),
            &format!("「{}」の作業を開始します", task.title),
            before,
            after,
            execution_args,
            Vec::new(),
            serde_json::Map::new(),
            observed,
            true,
            args.proposal_id,
        ))
    }
}

struct TaskPause {
    client: Client,
    tz_cache: TimeZoneCache,
}

/// Arguments for [`TaskPause`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskPauseArgs {
    /// Task reference such as #42 or h1#3. Omit if the user did not specify a task; a focused clarification will be returned.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    task_ref: Option<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

#[async_trait]
impl TypedTool for TaskPause {
    type Params = TaskPauseArgs;

    fn name(&self) -> &'static str {
        ToolName::TaskPause.into()
    }

    fn description(&self) -> &'static str {
        "Propose pausing work on a task. Closes the open work session and records active minutes. If task_ref is omitted, asks for clarification. Requires approval before writing."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_ref = args.task_ref.map(|s| strip_leading_hash(&s).to_string());
        let tz = server_timezone(&self.tz_cache).await;

        let (task, ctx) = match task_ref {
            Some(ref r) => resolve_task(&self.client, r).await?,
            None => {
                let (tasks, _habits, ctx) = load_task_context(&self.client).await?;
                let candidates: Vec<TaskRow> = tasks
                    .into_iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .collect();
                return Ok(focused_clarification(
                    "一時停止",
                    "作業中",
                    &candidates,
                    &ctx,
                ));
            }
        };
        let display_ref = ctx.reference(&task);
        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                format!("cannot pause work on a {} task", task.status),
            )));
        }

        let before = task_json(&task, &ctx, Some(&tz));
        let mut after = before.clone();
        if let Some(obj) = after.as_object_mut() {
            obj.insert("status".to_string(), Value::String("scheduled".to_string()));
        }

        let execution_args = json!({
            "task_ref": display_ref.trim_start_matches('#'),
        });
        let observed = Some(task.updated_at.to_string());

        Ok(progress_output(
            ChangeOperation::Pause,
            &Target::new(TargetKind::Task, display_ref.as_str()),
            &format!("「{}」の作業を一時停止", task.title),
            &format!("「{}」の作業を一時停止します", task.title),
            before,
            after,
            execution_args,
            Vec::new(),
            serde_json::Map::new(),
            observed,
            true,
            args.proposal_id,
        ))
    }
}

struct TaskProgress {
    client: Client,
    tz_cache: TimeZoneCache,
}

/// Arguments for [`TaskProgress`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskProgressArgs {
    /// Task reference such as #42 or h1#3. Omit if the user did not specify a task; a focused clarification will be returned.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    task_ref: Option<String>,
    /// Cumulative quantity completed.
    quantity_done: i64,
    /// Optional note for this progress event.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    note: Option<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

#[async_trait]
impl TypedTool for TaskProgress {
    type Params = TaskProgressArgs;

    fn name(&self) -> &'static str {
        ToolName::TaskProgress.into()
    }

    fn description(&self) -> &'static str {
        "Propose recording cumulative progress on a task. Updates quantity_done and may update the estimate. A lower quantity is treated as a correction, not a speed observation. Does not implicitly close the work session; use task_complete to finish. If task_ref is omitted, asks for clarification. Requires approval before writing."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        Quantity::new(args.quantity_done)
            .map_err(|_| InvalidArgsError::new("quantity_done", "cannot be negative"))?;
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_ref = args.task_ref.map(|s| strip_leading_hash(&s).to_string());
        let quantity_done =
            Quantity::new(args.quantity_done).expect("validate_args ensures quantity_done >= 0");
        let note = args.note;

        let tz = server_timezone(&self.tz_cache).await;
        let (task, ctx) = match task_ref {
            Some(ref r) => resolve_task(&self.client, r).await?,
            None => {
                let (tasks, _habits, ctx) = load_task_context(&self.client).await?;
                let candidates: Vec<TaskRow> = tasks
                    .into_iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .collect();
                return Ok(focused_clarification(
                    "進捗記録",
                    "作業中",
                    &candidates,
                    &ctx,
                ));
            }
        };
        let display_ref = ctx.reference(&task);

        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                format!("cannot record progress on a {} task", task.status),
            )));
        }
        if let Some(total) = task.quantity_total
            && quantity_done > total
        {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "quantity_done",
                format!(
                    "cannot exceed quantity_total ({} > {})",
                    quantity_done, total
                ),
            )));
        }

        let before = task_json(&task, &ctx, Some(&tz));
        let mut after = before.clone();
        if let Some(obj) = after.as_object_mut() {
            obj.insert(
                "quantity_done".to_string(),
                Value::Number(quantity_done.get().into()),
            );
        }

        let mut warnings = Vec::new();
        let mut content_extra = serde_json::Map::new();
        let suggests_completion = task
            .quantity_total
            .map(|total| quantity_done >= total)
            .unwrap_or(false);
        if suggests_completion {
            warnings.push(
                "quantity_done reached the task total. Call task_complete explicitly to finish and record actual time.".to_string(),
            );
            content_extra.insert("suggests_completion".to_string(), Value::Bool(true));
        }
        if quantity_done < task.quantity_done {
            warnings.push(
                "quantity_done decreased; this will be recorded as a correction, not a new speed observation.".to_string(),
            );
            content_extra.insert("is_correction".to_string(), Value::Bool(true));
        }

        let delta_quantity = quantity_done.get() - task.quantity_done.get();
        if delta_quantity > 0 && !task.fixed {
            match self.client.list_work_sessions(Some(&task.id)).await {
                Ok(sessions) => {
                    let open_count = sessions.iter().filter(|s| s.ended_at.is_none()).count();
                    if open_count == 0 {
                        warnings.push(
                            "作業セッションが開いていないため推定値のプレビューは更新されません"
                                .to_string(),
                        );
                    } else if open_count > 1 {
                        warnings.push(
                            "複数の作業セッションが開いているため推定値のプレビューは更新されません"
                                .to_string(),
                        );
                    } else {
                        let active_minutes = active_minutes_for_sessions(&sessions);
                        let task_kind_prior = ctx.task_kind_prior(&task);
                        let (new_avg, new_sigma) = estimate_preview(
                            task.avg_minutes,
                            task.sigma_minutes,
                            task_kind_prior,
                            task.quantity_total.map(|q| q.get()),
                            active_minutes,
                            task.quantity_done.get() + delta_quantity,
                        );
                        apply_estimate_preview(
                            &mut after,
                            &mut content_extra,
                            new_avg,
                            new_sigma,
                            &task,
                        );
                    }
                }
                Err(e) => {
                    warnings.push(format!(
                        "作業時間の取得に失敗したため推定値のプレビューが無効です: {e}"
                    ));
                }
            }
        }

        let mut execution_args = serde_json::Map::from_iter([(
            "quantity_done".to_string(),
            Value::Number(quantity_done.get().into()),
        )]);
        if let Some(note) = &note {
            execution_args.insert("note".to_string(), Value::String(note.clone()));
        }

        let why = if quantity_done < task.quantity_done {
            format!(
                "「{}」の進捗を {} から {} に訂正します",
                task.title, task.quantity_done, quantity_done
            )
        } else {
            format!(
                "「{}」の進捗を {} / {} に更新します",
                task.title,
                quantity_done,
                task.quantity_total
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "?".to_string())
            )
        };

        Ok(progress_output(
            ChangeOperation::Progress,
            &Target::new(TargetKind::Task, display_ref.as_str()),
            &why,
            &why,
            before,
            after,
            Value::Object(execution_args),
            warnings,
            content_extra,
            Some(task.updated_at.to_string()),
            false,
            args.proposal_id,
        ))
    }
}

struct TaskComplete {
    client: Client,
    tz_cache: TimeZoneCache,
}

/// Arguments for [`TaskComplete`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskCompleteArgs {
    /// Task reference such as #42 or h1#3. Omit if the user did not specify a task; a focused clarification will be returned.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    task_ref: Option<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

#[async_trait]
impl TypedTool for TaskComplete {
    type Params = TaskCompleteArgs;

    fn name(&self) -> &'static str {
        ToolName::TaskComplete.into()
    }

    fn description(&self) -> &'static str {
        "Propose completing a task. Closes the open work session and records the total active time. If task_ref is omitted, asks for clarification. Requires approval before writing."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_ref = args.task_ref.map(|s| strip_leading_hash(&s).to_string());
        let tz = server_timezone(&self.tz_cache).await;

        let (task, ctx) = match task_ref {
            Some(ref r) => resolve_task(&self.client, r).await?,
            None => {
                let (tasks, _habits, ctx) = load_task_context(&self.client).await?;
                let candidates: Vec<TaskRow> = tasks
                    .into_iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .collect();
                return Ok(focused_clarification("完了", "作業中", &candidates, &ctx));
            }
        };
        let display_ref = ctx.reference(&task);
        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                format!("cannot complete a {} task", task.status),
            )));
        }

        let before = task_json(&task, &ctx, Some(&tz));
        let mut after = before.clone();
        if let Some(obj) = after.as_object_mut() {
            obj.insert("status".to_string(), Value::String("completed".to_string()));
            if let Some(total) = task.quantity_total {
                obj.insert(
                    "quantity_done".to_string(),
                    Value::Number(total.get().into()),
                );
            }
        }

        let mut content_extra = serde_json::Map::new();
        let mut warnings = Vec::new();
        match self.client.list_work_sessions(Some(&task.id)).await {
            Ok(sessions) => {
                let total_active = active_minutes_for_sessions(&sessions);
                let task_kind_prior = ctx.task_kind_prior(&task);
                if let Some(total) = task.quantity_total
                    && !task.fixed
                {
                    let (new_avg, new_sigma) = estimate_preview(
                        task.avg_minutes,
                        task.sigma_minutes,
                        task_kind_prior,
                        Some(total.get()),
                        total_active,
                        total.get(),
                    );
                    apply_estimate_preview(
                        &mut after,
                        &mut content_extra,
                        new_avg,
                        new_sigma,
                        &task,
                    );
                } else if total_active > 0 && !task.fixed {
                    let (new_avg, new_sigma) = estimate_preview(
                        task.avg_minutes,
                        task.sigma_minutes,
                        task_kind_prior,
                        Some(1),
                        total_active,
                        1,
                    );
                    apply_estimate_preview(
                        &mut after,
                        &mut content_extra,
                        new_avg,
                        new_sigma,
                        &task,
                    );
                }
                content_extra.insert(
                    "total_active_minutes".to_string(),
                    Value::Number(total_active.into()),
                );
            }
            Err(e) => {
                warnings.push(format!(
                    "作業時間の取得に失敗したため推定値のプレビューが無効です: {e}"
                ));
            }
        }

        let execution_args = json!({
            "task_ref": display_ref.trim_start_matches('#'),
        });

        Ok(progress_output(
            ChangeOperation::Complete,
            &Target::new(TargetKind::Task, display_ref.as_str()),
            &format!("「{}」の作業を完了", task.title),
            &format!("「{}」の作業を完了し、実績時間を記録します", task.title),
            before,
            after,
            execution_args,
            warnings,
            content_extra,
            Some(task.updated_at.to_string()),
            true,
            args.proposal_id,
        ))
    }
}

struct TaskSplit {
    client: Client,
    tz_cache: TimeZoneCache,
}

/// Arguments for [`TaskSplit`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskSplitArgs {
    /// Task reference such as #42 or h1#3. Omit if the user did not specify a task; a focused clarification will be returned.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    task_ref: Option<String>,
    /// Quantity to keep on the original task.
    retained_quantity: i64,
    /// If true, the remainder depends on the original task. Defaults to true.
    #[serde(default)]
    set_dependency: Option<bool>,
    /// Optional title for the remainder task.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    title: Option<String>,
    /// Optional description for the remainder task.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    description: Option<String>,
    /// Optional deadline for the remainder task; interpreted in server timezone.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    end_at: Option<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

#[async_trait]
impl TypedTool for TaskSplit {
    type Params = TaskSplitArgs;

    fn name(&self) -> &'static str {
        ToolName::TaskSplit.into()
    }

    fn description(&self) -> &'static str {
        "Propose splitting a task into an original (retained quantity) and a new remainder task. Preserves history and optionally sets a dependency. If task_ref is omitted, asks for clarification. Requires approval before writing."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        Quantity::new(args.retained_quantity)
            .map_err(|_| InvalidArgsError::new("retained_quantity", "cannot be negative"))?;
        if args.retained_quantity <= 0 {
            return Err(InvalidArgsError::new(
                "retained_quantity",
                "must be greater than 0",
            ));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_ref = args.task_ref.map(|s| strip_leading_hash(&s).to_string());
        let retained_quantity = Quantity::new(args.retained_quantity)
            .expect("validate_args ensures retained_quantity > 0");
        let set_dependency = args.set_dependency.unwrap_or(true);
        let title = args.title;
        let description = args.description;
        let end_at = args.end_at;

        let tz = server_timezone(&self.tz_cache).await;
        let (task, ctx) = match task_ref {
            Some(ref r) => resolve_task(&self.client, r).await?,
            None => {
                let (tasks, _habits, ctx) = load_task_context(&self.client).await?;
                let candidates: Vec<TaskRow> = tasks
                    .into_iter()
                    .filter(|t| {
                        t.status != TaskStatus::Completed
                            && t.status != TaskStatus::Skipped
                            && t.quantity_total.is_some()
                    })
                    .collect();
                return Ok(focused_clarification(
                    "分割",
                    "作業中・予定・未スケジュール",
                    &candidates,
                    &ctx,
                ));
            }
        };
        let display_ref = ctx.reference(&task);

        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                format!("cannot split a {} task", task.status),
            )));
        }
        let total = task.quantity_total.ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new(
                "quantity_total",
                "missing: cannot split a task with no quantity_total",
            ))
        })?;
        if retained_quantity >= total {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "retained_quantity",
                "must be less than quantity_total",
            )));
        }
        if retained_quantity < task.quantity_done {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "retained_quantity",
                "cannot be less than quantity_done",
            )));
        }

        let before = task_json(&task, &ctx, Some(&tz));
        let mut after = before.clone();
        if let Some(obj) = after.as_object_mut() {
            obj.insert(
                "quantity_total".to_string(),
                Value::Number(retained_quantity.get().into()),
            );
            obj.insert(
                "quantity_done".to_string(),
                Value::Number(task.quantity_done.min(retained_quantity).get().into()),
            );
        }

        let mut execution_args = serde_json::Map::new();
        execution_args.insert(
            "task_ref".to_string(),
            Value::String(display_ref.trim_start_matches('#').to_string()),
        );
        execution_args.insert(
            "retained_quantity".to_string(),
            Value::Number(retained_quantity.get().into()),
        );
        execution_args.insert("set_dependency".to_string(), Value::Bool(set_dependency));
        if let Some(v) = &title {
            execution_args.insert("title".to_string(), Value::String(v.clone()));
        }
        if let Some(v) = &description {
            execution_args.insert("description".to_string(), Value::String(v.clone()));
        }
        let end_at_normalized = if let Some(v) = &end_at {
            let normalized = takusu_types::parse_datetime_tz(v, &tz).map_err(|e| {
                ToolError::InvalidArgs(InvalidArgsError::new("end_at", format!("invalid: {e}")))
            })?;
            execution_args.insert("end_at".to_string(), Value::String(normalized.clone()));
            Some(normalized)
        } else {
            None
        };

        let remainder_quantity = total.get() - retained_quantity.get();
        let remainder_title = title.unwrap_or_else(|| format!("{}（残り）", task.title));
        let mut remainder = serde_json::Map::new();
        remainder.insert("title".to_string(), Value::String(remainder_title.clone()));
        remainder.insert(
            "quantity_total".to_string(),
            Value::Number(remainder_quantity.into()),
        );
        remainder.insert("quantity_done".to_string(), Value::Number(0.into()));
        if let Some(unit) = &task.quantity_unit {
            remainder.insert("quantity_unit".to_string(), Value::String(unit.clone()));
        }
        if let Some(desc) = description.as_ref().or(task.description.as_ref()) {
            remainder.insert("description".to_string(), Value::String(desc.clone()));
        }
        if let Some(end) = end_at_normalized.as_ref() {
            remainder.insert("end_at".to_string(), Value::String(end.clone()));
        } else if task.end_at != takusu_types::Timestamp::default() {
            remainder.insert("end_at".to_string(), Value::String(task.end_at.to_string()));
        }
        if set_dependency {
            remainder.insert(
                "depends".to_string(),
                Value::Array(vec![Value::String(display_ref.clone())]),
            );
        }

        let mut content_extra = serde_json::Map::new();
        content_extra.insert("remainder".to_string(), Value::Object(remainder));
        if let Some(obj) = after.as_object_mut() {
            obj.insert("remainder".to_string(), content_extra["remainder"].clone());
        }

        let why = format!(
            "「{}」を {} + {} に分割します（残り: {}）",
            task.title, retained_quantity, remainder_quantity, remainder_title
        );

        Ok(progress_output(
            ChangeOperation::Split,
            &Target::new(TargetKind::Task, display_ref.as_str()),
            &why,
            &why,
            before,
            after,
            Value::Object(execution_args),
            Vec::new(),
            content_extra,
            Some(task.updated_at.to_string()),
            true,
            args.proposal_id,
        ))
    }
}

struct TaskUndo {
    client: Client,
    tz_cache: TimeZoneCache,
}

/// Arguments for [`TaskUndo`].
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskUndoArgs {
    /// Task reference such as #42 or h1#3. Omit if the user did not specify a task; a focused clarification will be returned.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    #[schemars(with = "Option<String>")]
    task_ref: Option<String>,
    /// Optional proposal id. Set the same value across multiple related tool calls to group them into a single proposal for review.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    proposal_id: Option<String>,
}

#[async_trait]
impl TypedTool for TaskUndo {
    type Params = TaskUndoArgs;

    fn name(&self) -> &'static str {
        ToolName::TaskUndo.into()
    }

    fn description(&self) -> &'static str {
        "Propose undoing the most recent start/pause work session for a task. If task_ref is omitted, asks for clarification. Requires approval before writing."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Deferred
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_ref = args.task_ref.map(|s| strip_leading_hash(&s).to_string());
        let tz = server_timezone(&self.tz_cache).await;

        let (task, ctx) = match task_ref {
            Some(ref r) => resolve_task(&self.client, r).await?,
            None => {
                let (tasks, _habits, ctx) = load_task_context(&self.client).await?;
                let candidates: Vec<TaskRow> = tasks
                    .into_iter()
                    .filter(|t| {
                        t.status == TaskStatus::InProgress || t.status == TaskStatus::Scheduled
                    })
                    .collect();
                return Ok(focused_clarification(
                    "元に戻す",
                    "作業中・予定",
                    &candidates,
                    &ctx,
                ));
            }
        };
        let display_ref = ctx.reference(&task);
        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "task_ref",
                format!("cannot undo a {} task", task.status),
            )));
        }

        let before = task_json(&task, &ctx, Some(&tz));
        let after = before.clone();

        let execution_args = json!({
            "task_ref": display_ref.trim_start_matches('#'),
        });
        let observed = Some(task.updated_at.to_string());

        Ok(progress_output(
            ChangeOperation::Undo,
            &Target::new(TargetKind::Task, display_ref.as_str()),
            &format!("「{}」の作業を元に戻す", task.title),
            &format!("「{}」の作業状態を元に戻します", task.title),
            before,
            after,
            execution_args,
            Vec::new(),
            serde_json::Map::new(),
            observed,
            true,
            args.proposal_id,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn task_start_schema_allows_optional_task_ref() {
        let client = Client::new("http://localhost", "");
        let tool = TaskStart {
            client: client.clone(),
            tz_cache: TimeZoneCache::new(client),
        };
        let schema = tool.parameters_schema();
        assert_ne!(schema["required"], json!(["task_ref"]));
        assert!(schema["properties"].get("task_ref").is_some());
    }

    #[test]
    fn task_progress_schema_requires_quantity_done() {
        let client = Client::new("http://localhost", "");
        let tool = TaskProgress {
            client: client.clone(),
            tz_cache: TimeZoneCache::new(client),
        };
        let schema = tool.parameters_schema();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("quantity_done"))
        );
        assert!(schema["properties"].get("quantity_done").is_some());
        assert!(schema["properties"].get("note").is_some());
    }

    #[test]
    fn estimate_preview_without_total_returns_original() {
        assert_eq!(estimate_preview(60, 10, None, None, 30, 5), (60, 10));
    }

    #[test]
    fn estimate_preview_with_zero_quantity_returns_original() {
        assert_eq!(estimate_preview(60, 10, None, Some(10), 30, 0), (60, 10));
    }

    #[test]
    fn estimate_preview_uses_the_truncated_normal_posterior() {
        let (avg, sigma) = estimate_preview(60, 5, None, Some(20), 60, 10);
        assert_eq!((avg, sigma), (90, 4));
    }

    #[test]
    fn estimate_preview_complete_is_degenerate() {
        assert_eq!(estimate_preview(60, 5, None, Some(1), 1, 1), (1, 0));
    }

    #[test]
    fn estimate_preview_without_active_time_returns_original() {
        assert_eq!(estimate_preview(60, 5, None, Some(1), 0, 1), (60, 5));
    }

    #[test]
    fn estimate_preview_for_unquantified_completion_is_degenerate() {
        assert_eq!(estimate_preview(60, 20, None, Some(1), 45, 1), (45, 0));
    }

    #[test]
    fn estimate_preview_falls_back_to_task_kind_prior_when_sigma_is_zero() {
        let prior = DurationDistribution::new(90.0, 15.0);
        let (avg, sigma) = estimate_preview(0, 0, Some(prior), Some(2), 45, 1);
        // The preview should be based on the prior (90, 15) rather than the
        // wide fallback used when no prior is supplied.
        assert!(avg > 60);
        assert!(sigma > 5);
    }

    #[test]
    fn apply_estimate_preview_updates_after_and_content_extra() {
        let mut after = json!({"avg_minutes": 60, "sigma_minutes": 10});
        let mut content_extra = serde_json::Map::new();
        let task = base_task();
        apply_estimate_preview(&mut after, &mut content_extra, 90, 15, &task);
        assert_eq!(after["avg_minutes"], 90);
        assert_eq!(after["sigma_minutes"], 15);
        assert_eq!(
            content_extra["estimate_preview"],
            json!({"avg_minutes": 90, "sigma_minutes": 15})
        );
    }

    fn base_task() -> TaskRow {
        TaskRow {
            id: "t1".to_string(),
            display_id: 1,
            title: "task".to_string(),
            description: None,
            start_at: None,
            end_at: "2025-01-02T00:00:00Z".parse().unwrap(),
            avg_minutes: 60,
            sigma_minutes: 10,
            depends: Vec::new().into(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            status: TaskStatus::Scheduled,
            habit_id: None,
            ical_uid: None,
            user_edited: false,
            fixed: false,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: Quantity::default(),
            quantity_unit: None,
            completed_at: None,
            split_from_task_id: None,
            original_quantity_total: None,
            actual_minutes: None,
            created_at: "2025-01-01T00:00:00Z".parse().unwrap(),
            updated_at: "2025-01-01T00:00:00Z".parse().unwrap(),
        }
    }
}
