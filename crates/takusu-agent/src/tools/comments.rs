use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use takusu_client::{Client, CommentRow, CreateComment};

use crate::tools::takusu::strip_leading_hash;
use crate::tools::{ToolContext, ToolModule};
use crate::{
    ChangeOperation, ChangeReceipt, InvalidArgsError, ReceiptTarget, TargetKind, ToolError,
    ToolExposure, ToolName, ToolOutput, ToolRegistry, TypedTool,
};

/// Serialized form of a comment returned by `add_comment`.
#[derive(Debug, Clone, serde::Serialize)]
struct CommentResponse<'a> {
    id: &'a str,
    task_id: &'a str,
    author: &'a takusu_types::CommentAuthor,
    content: &'a str,
    seq: i64,
    created_at: &'a takusu_types::Timestamp,
}

impl<'a> From<&'a CommentRow> for CommentResponse<'a> {
    fn from(row: &'a CommentRow) -> Self {
        Self {
            id: &row.id,
            task_id: &row.task_id,
            author: &row.author,
            content: &row.content,
            seq: row.seq,
            created_at: &row.created_at,
        }
    }
}

/// A task completion that deviated beyond 1σ, awaiting a single check-in
/// question on the next turn. The user's answer is stored as a task comment.
///
/// This is the "next-turn prompt note" delivery mechanism for the overrun
/// check-in (WI-3); the resident-agent event channel is future work.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct PendingCheckIn {
    pub task_id: String,
    pub display_id: i64,
    pub title: String,
    pub avg_minutes: i64,
    pub actual_minutes: i64,
    pub sigma_minutes: i64,
    /// Whether the overrun check-in has ever been surfaced in a system prompt.
    ///
    /// A check-in is only treated as answered when a comment is recorded for
    /// the task *after* it has been delivered; comments added before delivery
    /// (e.g. an unrelated note) do not clear it.
    #[serde(default)]
    pub delivered: bool,
}

impl PendingCheckIn {
    fn overrun(self: &PendingCheckIn) -> bool {
        self.sigma_minutes > 0 && self.actual_minutes - self.avg_minutes > self.sigma_minutes
    }

    fn to_prompt_line(self: &PendingCheckIn) -> String {
        format!(
            "- #{}「{}」: 見積もり {avg} 分 / 実績 {actual} 分（σ={sigma}）",
            self.display_id,
            self.title,
            avg = self.avg_minutes,
            actual = self.actual_minutes,
            sigma = self.sigma_minutes,
        )
    }
}

/// Decide whether a completed task warrants an overrun check-in.
///
/// Returns `Some` only when the task has an `actual_minutes` value, a positive
/// `sigma_minutes`, and the actual duration exceeded the estimate by more than
/// one standard deviation. Returns `None` (no check-in) when `sigma = 0`,
/// actuals are missing, or the task did not overrun.
pub(crate) fn completion_overrun_check_in(
    task_id: &str,
    display_id: i64,
    title: &str,
    avg_minutes: i64,
    actual_minutes: Option<i64>,
    sigma_minutes: i64,
) -> Option<PendingCheckIn> {
    let candidate = PendingCheckIn {
        task_id: task_id.to_owned(),
        display_id,
        title: title.to_owned(),
        avg_minutes,
        actual_minutes: actual_minutes?,
        sigma_minutes,
        delivered: false,
    };
    candidate.overrun().then_some(candidate)
}

/// Which task reads attach comments to their results, how many of the newest
/// comments are included, and the prompt guidance for the `add_comment` tool.
pub(crate) const MAX_ATTACHED_COMMENTS: usize = 5;

/// Attach the task comment timeline onto a task-shaped JSON object.
///
/// Adds `comment_count` (total) and `comments` (the newest
/// [`MAX_ATTACHED_COMMENTS`], ascending `seq`, author-labeled) so the model
/// sees qualitative history without inflating the context window.
pub(crate) fn attach_comments(obj: &mut Value, comments: &[CommentRow]) {
    let total = comments.len();
    let newest: Vec<&CommentRow> = comments
        .iter()
        .rev()
        .take(MAX_ATTACHED_COMMENTS)
        .rev()
        .collect();
    let arr: Vec<Value> = newest
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "author": c.author,
                "content": c.content,
                "seq": c.seq,
                "created_at": c.created_at,
            })
        })
        .collect();
    if let Value::Object(map) = obj {
        map.insert("comment_count".into(), json!(total));
        map.insert("comments".into(), json!(arr));
    }
}

/// Extract a [`PendingCheckIn`] from an executed task-completion receipt.
///
/// The receipt's `after` snapshot carries the task row (approved actuals). No
/// check-in is produced for `sigma = 0` or missing actuals.
pub(crate) fn check_in_from_complete_receipt(receipt: &ChangeReceipt) -> Option<PendingCheckIn> {
    if receipt.target.target_type != TargetKind::Task
        || receipt.operation != ChangeOperation::Complete
    {
        return None;
    }
    let after = receipt.after.as_ref()?;
    #[derive(serde::Deserialize)]
    struct TaskAfter {
        display_id: i64,
        title: String,
        avg_minutes: i64,
        sigma_minutes: i64,
        actual_minutes: Option<i64>,
    }
    let task: TaskAfter = serde_json::from_value(after.clone()).ok()?;
    completion_overrun_check_in(
        &receipt.target.target_id,
        task.display_id,
        &task.title,
        task.avg_minutes,
        task.actual_minutes,
        task.sigma_minutes,
    )
}

/// Enqueue a check-in unless one for the same task is already pending.
pub(crate) fn enqueue_check_in(queue: &mut Vec<PendingCheckIn>, check_in: PendingCheckIn) {
    if !queue.iter().any(|c| c.task_id == check_in.task_id) {
        queue.push(check_in);
    }
}

/// Drop pending check-ins for tasks that were commented on.
///
/// Only check-ins that have already been **delivered** (surfaced in a system
/// prompt, i.e. actually asked) are cleared, so an unrelated comment added for
/// a task whose check-in has not yet been prompted leaves the check-in intact.
pub(crate) fn clear_delivered_check_ins_for_task_ids(
    queue: &mut Vec<PendingCheckIn>,
    task_ids: &[String],
) {
    if task_ids.is_empty() {
        return;
    }
    queue.retain(|c| !(c.delivered && task_ids.iter().any(|id| id == &c.task_id)));
}

/// Drop pending check-ins for tasks that were deleted. Deletion is
/// unconditional (the task no longer exists, so it can never be answered).
pub(crate) fn clear_check_ins_for_task_ids(queue: &mut Vec<PendingCheckIn>, task_ids: &[String]) {
    if task_ids.is_empty() {
        return;
    }
    queue.retain(|c| !task_ids.iter().any(|id| id == &c.task_id));
}

/// Build the system-prompt section for pending overrun check-ins, marking each
/// included check-in as delivered.
pub(crate) fn check_in_prompt_section(queue: &mut [PendingCheckIn]) -> String {
    if queue.is_empty() {
        return String::new();
    }
    let lines = queue
        .iter()
        .map(|c| c.to_prompt_line())
        .collect::<Vec<_>>()
        .join("\n");
    for c in queue.iter_mut() {
        c.delivered = true;
    }
    format!(
        "## 完了タスクの振り返り（確認待ち）
        以下の完了タスクは見積もりを 1σ を超えて超過しました。このターンの初めに、ユーザーに超過理由を 1 件だけ確認してください。
        ユーザーの回答は `add_comment` でそのタスクのコメントとして記録してください。ユーザーが回答するまで、この確認を先送りにせず完了させること。
        {lines}",
    )
}

/// A delay that crossed the short-snooze threshold, awaiting a one-time
/// "why did you postpone this task?" question on a later turn.
///
/// The user's answer is stored as a task comment.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct PendingPostponeReason {
    pub task_id: String,
    pub display_id: i64,
    pub title: String,
    pub snooze_minutes: i64,
    /// Whether the postpone check-in has ever been surfaced in a system prompt.
    #[serde(default)]
    pub delivered: bool,
}

/// Enqueue a postpone reason check-in unless one for the same task is already
/// pending.
pub fn enqueue_postpone_reason(
    queue: &mut Vec<PendingPostponeReason>,
    pending: PendingPostponeReason,
) {
    if !queue.iter().any(|p| p.task_id == pending.task_id) {
        queue.push(pending);
    }
}

/// Drop pending postpone reasons for tasks that were deleted.
pub fn clear_postpone_reasons_for_task_ids(
    queue: &mut Vec<PendingPostponeReason>,
    task_ids: &[String],
) {
    if task_ids.is_empty() {
        return;
    }
    queue.retain(|p| !task_ids.iter().any(|id| id == &p.task_id));
}

/// Build the system-prompt section for pending postpone reasons.
pub fn postpone_reason_prompt_section(queue: &mut [PendingPostponeReason]) -> String {
    if queue.is_empty() {
        return String::new();
    }
    let lines = queue
        .iter()
        .map(|p| {
            let question = crate::contact_policy::postpone_reason_check_in(&p.title);
            format!("{}\n  理由を聞いて、回答を `add_comment` でそのタスクのコメントとして記録してください。", question)
        })
        .collect::<Vec<_>>()
        .join("\n");
    for p in queue.iter_mut() {
        p.delivered = true;
    }
    format!(
        "## タスク先送りの理由確認（確認待ち）
{lines}",
    )
}

struct CommentModule;

impl ToolModule for CommentModule {
    fn register(&self, registry: &mut ToolRegistry, ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(AddComment {
            client: ctx.client.clone(),
        })));
    }
}

static COMMENT_MODULE: &dyn ToolModule = &CommentModule;

inventory::submit!(COMMENT_MODULE);

#[derive(Clone)]
struct AddComment {
    client: Client,
}

/// Arguments for [`AddComment`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddCommentArgs {
    /// Task display reference such as #42 or h1#3.
    #[serde(deserialize_with = "crate::deserialize_trimmed_required")]
    #[schemars(with = "String")]
    task_ref: String,
    /// Comment content (time-series note). Must be non-empty.
    #[serde(deserialize_with = "crate::deserialize_trimmed_required")]
    #[schemars(with = "String")]
    content: String,
}

#[async_trait]
impl TypedTool for AddComment {
    type Params = AddCommentArgs;

    fn name(&self) -> &'static str {
        ToolName::AddComment.into()
    }
    fn description(&self) -> &'static str {
        "Append a task-scoped time-series note/comment to a task timeline. Writes immediately (no approval); the comment is visible to the user and can be deleted by them. Use for overrun reasons and qualitative context, not for the task's current spec (that belongs in `description`)."
    }
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if args.content.chars().count() > 4096 {
            return Err(InvalidArgsError::new(
                "content",
                "must be at most 4096 characters",
            ));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let task_ref = strip_leading_hash(&args.task_ref).to_string();
        let task = self
            .client
            .get_task(&task_ref)
            .await
            .map_err(super::client_error)?;
        let operation_id = uuid::Uuid::now_v7().to_string();
        let row = self
            .client
            .create_agent_comment(
                &task.id,
                &CreateComment {
                    content: args.content.clone(),
                },
                Some(&operation_id),
            )
            .await
            .map_err(super::client_error)?;

        let receipt = ChangeReceipt {
            // The target is the task the comment was appended to, so the
            // turn's change list reads "comment on task #5".
            target: ReceiptTarget {
                target_type: TargetKind::Comment,
                target_id: task.id.clone(),
            },
            operation: ChangeOperation::Create,
            after: Some(serde_json::to_value(CommentResponse::from(&row)).unwrap_or_default()),
            ..Default::default()
        };
        let content = json!({
            "comment": {
                "id": row.id,
                "task_id": row.task_id,
                "author": row.author,
                "content": row.content,
                "seq": row.seq,
                "created_at": row.created_at,
            }
        });
        Ok(ToolOutput {
            content: serde_json::to_string(&content).unwrap(),
            changes: vec![receipt],
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;
    use axum::Json;

    #[test]
    fn overrun_check_in_skips_sigma_zero() {
        let check = completion_overrun_check_in("t", 1, "x", 60, Some(120), 0);
        assert!(check.is_none());
    }

    #[test]
    fn overrun_check_in_skips_missing_actuals() {
        let check = completion_overrun_check_in("t", 1, "x", 60, None, 10);
        assert!(check.is_none());
    }

    #[test]
    fn overrun_check_in_requires_beyond_one_sigma() {
        // actual 65, avg 60, sigma 10 → diff 5 ≤ sigma → no check-in.
        let check = completion_overrun_check_in("t", 1, "x", 60, Some(65), 10);
        assert!(check.is_none());
        // actual 75, avg 60, sigma 10 → diff 15 > sigma → check-in.
        let check = completion_overrun_check_in("t", 1, "x", 60, Some(75), 10).unwrap();
        assert!(check.overrun());
    }

    #[test]
    fn overrun_check_in_prompt_line() {
        let check = completion_overrun_check_in("t", 42, "レポート", 60, Some(90), 10).unwrap();
        assert_eq!(
            check.to_prompt_line(),
            "- #42「レポート」: 見積もり 60 分 / 実績 90 分（σ=10）"
        );
    }

    #[test]
    fn check_in_from_complete_receipt_requires_complete() {
        use crate::{ChangeOperation, ChangeReceipt, ReceiptTarget, TargetKind};
        let task_after = task_json_for_after();
        let complete = ChangeReceipt {
            operation: ChangeOperation::Complete,
            target: ReceiptTarget {
                target_type: TargetKind::Task,
                target_id: "task-uuid".into(),
            },
            after: Some(task_after.clone()),
            ..Default::default()
        };
        assert!(check_in_from_complete_receipt(&complete).is_some());

        let update = ChangeReceipt {
            operation: ChangeOperation::Update,
            ..complete.clone()
        };
        assert!(check_in_from_complete_receipt(&update).is_none());

        let no_after = ChangeReceipt {
            after: None,
            ..complete
        };
        assert!(check_in_from_complete_receipt(&no_after).is_none());
    }

    #[test]
    fn check_in_from_complete_receipt_skips_sigma_zero() {
        use crate::{ChangeOperation, ChangeReceipt, ReceiptTarget, TargetKind};
        let mut after = task_json_for_after();
        after["sigma_minutes"] = json!(0);
        let receipt = ChangeReceipt {
            operation: ChangeOperation::Complete,
            target: ReceiptTarget {
                target_type: TargetKind::Task,
                target_id: "task-uuid".into(),
            },
            after: Some(after),
            ..Default::default()
        };
        assert!(check_in_from_complete_receipt(&receipt).is_none());
    }

    #[test]
    fn check_in_queue_enqueue_and_clear() {
        let one = completion_overrun_check_in("t1", 1, "x", 60, Some(90), 10).unwrap();
        let mut queue = Vec::new();
        enqueue_check_in(&mut queue, one.clone());
        enqueue_check_in(&mut queue, one);
        assert_eq!(queue.len(), 1);
        clear_check_ins_for_task_ids(&mut queue, &["t1".to_string()]);
        assert!(queue.is_empty());
    }

    #[test]
    fn delivered_check_in_is_marked_and_settled_by_comment() {
        let mut check = completion_overrun_check_in("t1", 1, "x", 60, Some(90), 10).unwrap();
        assert!(!check.delivered);

        // Not yet delivered: a comment for the task must NOT clear it.
        let mut queue = vec![check.clone()];
        let section = check_in_prompt_section(&mut queue);
        assert!(!section.is_empty());
        assert!(queue[0].delivered, "prompt delivery marks the check-in");

        // Delivered and answered via comment → cleared.
        clear_delivered_check_ins_for_task_ids(&mut queue, &["t1".to_string()]);
        assert!(queue.is_empty());

        // A comment before delivery leaves the check-in pending.
        check.delivered = false;
        let mut queue = vec![check];
        clear_delivered_check_ins_for_task_ids(&mut queue, &["t1".to_string()]);
        assert_eq!(
            queue.len(),
            1,
            "undelivered check-in survives unrelated comment"
        );
    }

    #[test]
    fn check_in_serde_round_trips_for_snapshot() {
        let mut check = completion_overrun_check_in("t1", 1, "x", 60, Some(90), 10).unwrap();
        check.delivered = true;
        let json = serde_json::to_string(&check).unwrap();
        let back: PendingCheckIn = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, "t1");
        assert!(back.delivered);
    }

    #[test]
    fn attach_comments_adds_count_and_newest() {
        use takusu_types::CommentAuthor;
        let mk = |id: &str, seq: i64| CommentRow {
            id: id.into(),
            task_id: "task-uuid".into(),
            author: CommentAuthor::Agent,
            content: format!("note {id}"),
            seq,
            created_at: "2025-01-01T00:00:00Z".parse().unwrap(),
        };
        let comments: Vec<CommentRow> = (1..=6).map(|seq| mk(&format!("c{seq}"), seq)).collect();
        let mut obj = json!({ "title": "x" });
        attach_comments(&mut obj, &comments);
        assert_eq!(obj["comment_count"], 6);
        let attached = obj["comments"].as_array().unwrap();
        // Newest 5 (c2..c6), ascending seq.
        assert_eq!(attached.len(), MAX_ATTACHED_COMMENTS);
        assert_eq!(attached[0]["seq"], 2);
        assert_eq!(attached[4]["seq"], 6);
    }

    #[tokio::test]
    async fn add_comment_writes_immediately_without_approval() {
        let task = task_row("task-uuid", 42, "買い物");
        let row = CommentRow {
            id: "comment-1".into(),
            task_id: "task-uuid".into(),
            author: takusu_types::CommentAuthor::Agent,
            content: "思ったより手間取った".into(),
            seq: 1,
            created_at: "2025-01-01T00:00:00Z".parse().unwrap(),
        };
        let task_for_get = task.clone();
        let row_for_post = row.clone();
        let router = axum::Router::new()
            .route(
                "/api/tasks/{id}",
                axum::routing::get(move || async move { Json(task_for_get) }),
            )
            .route(
                "/api/tasks/{id}/comments/agent",
                axum::routing::post(move || async move { Json(row_for_post) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

        let client = takusu_client::Client::new(&format!("http://{addr}"), "");
        let tool = crate::tool::Typed(AddComment { client });
        let output = tool
            .call(json!({ "task_ref": "#42", "content": "思ったより手間取った" }))
            .await
            .unwrap();

        // No Proposal is produced; the write happened immediately via a ChangeReceipt.
        assert!(output.proposed_changes.is_empty());
        assert_eq!(output.changes.len(), 1);
        let receipt = &output.changes[0];
        assert_eq!(receipt.target.target_type, TargetKind::Comment);
        assert_eq!(receipt.target.target_id, "task-uuid");
        let content: Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(content["comment"]["seq"], 1);
        assert_eq!(content["comment"]["author"], "agent");
    }

    // ── helpers ──────────────────────────────────────────────

    fn task_json_for_after() -> Value {
        json!({
            "id": "task-uuid",
            "display_id": 7,
            "title": "レポート",
            "avg_minutes": 60,
            "sigma_minutes": 10,
            "actual_minutes": 90,
        })
    }

    fn task_row(id: &str, display_id: i64, title: &str) -> takusu_client::TaskRow {
        takusu_client::TaskRow {
            id: id.to_string(),
            display_id,
            title: title.to_string(),
            description: None,
            start_at: None,
            end_at: "2025-06-05T10:00:00Z".parse().unwrap(),
            avg_minutes: 30,
            sigma_minutes: 5,
            depends: Vec::new().into(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            status: takusu_types::TaskStatus::Pending,
            habit_id: None,
            ical_uid: None,
            user_edited: false,
            fixed: false,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: takusu_types::Quantity::default(),
            quantity_unit: None,
            completed_at: None,
            split_from_task_id: None,
            original_quantity_total: None,
            actual_minutes: None,
            created_at: "2025-06-01T00:00:00Z".parse().unwrap(),
            updated_at: "2025-06-01T00:00:00Z".parse().unwrap(),
        }
    }
}
