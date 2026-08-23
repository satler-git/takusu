//! Typed presentation model for the resident planner (WI-1).
//!
//! Clients must not render free-form LLM-generated JSON. This module is the
//! fixed, closed set of presentation payloads that Rust code builds from tool
//! results and schedule state; the LLM chooses *which* tool to call, never the
//! rendering. Every variant carries a deterministic voice/notification
//! template so later phases can drive TTS and notification bodies from the
//! same data.
//!
//! The wire encoding is internally tagged by a top-level `type` field and is
//! version tolerant: a client that receives an unknown tag decodes it as
//! [`Presentation::Text`] (using the accompanying `text` fallback when
//! present) rather than failing. The Rust [`Presentation::deserialize`] here
//! implements that same rule so fixtures are shared with the mobile client.

use std::borrow::Cow;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::str::FromStr;

use crate::ApprovalRequest;
use crate::UserInputAnswer;
use crate::approval::DEFAULT_APPROVAL_WHY;
use crate::tool::{ChangeOperation, TargetKind, ToolOutput};

/// A non-empty wrapper for a group of choices or actions.
///
/// Emptiness is rejected both at construction time and at deserialization, so
/// a [`CheckInCard`] can never be built (or received) with an empty 「行動」 or
/// 「ズラす」 group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmptyVec<T>(Vec<T>);

impl<T> NonEmptyVec<T> {
    /// Build a non-empty collection; fails if `items` is empty.
    pub fn new(items: Vec<T>) -> Result<Self, &'static str> {
        if items.is_empty() {
            return Err("collection must not be empty");
        }
        Ok(Self(items))
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Serialize> Serialize for NonEmptyVec<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for NonEmptyVec<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let items = Vec::<T>::deserialize(deserializer)?;
        Self::new(items).map_err(D::Error::custom)
    }
}

impl<T: schemars::JsonSchema> schemars::JsonSchema for NonEmptyVec<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("NonEmptyVec{}", T::schema_name()).into()
    }

    fn schema_id() -> Cow<'static, str> {
        format!("{}::NonEmptyVec<{}>", module_path!(), T::schema_id()).into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // The non-empty invariant is part of the JSON Schema contract.
        let value = serde_json::json!({
            "type": "array",
            "minItems": 1,
            "items": generator.subschema_for::<T>().to_value(),
        });
        schemars::Schema::try_from(value).expect("NonEmptyVec schema is a valid JSON Schema")
    }
}

/// Coverage authority attached to a current-task card.
///
/// Until WI-10 derives it from observed coverage state, cards default to
/// [`TaskAuthority::Candidate`] (the safe side of the coverage invariant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskAuthority {
    /// Present as a candidate for the user to accept (bootstrap).
    Candidate,
    /// Authoritative 「今やること」 because today is covered.
    TodayCovered,
}

/// State of a task's work session shown on a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    NotStarted,
    InProgress,
    Overdue,
}

/// Settlement prompt shown ahead of the current task when coverage is stale.
///
/// Mirrors a one-round-trip check-in so the same action rendering code can
/// handle it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SettlementPrompt {
    pub question: String,
    pub act: ActionGroup,
    pub shift: ActionGroup,
}

/// Current + next task card with quick actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskCard {
    pub title: String,
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
    pub work_state: WorkState,
    pub authority: TaskAuthority,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_task: Option<String>,
    /// Settlement prompt shown before this task when coverage is stale (WI-10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement: Option<SettlementPrompt>,
}

/// Kind of a recorded work transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkTransitionKind {
    Start,
    Pause,
    Progress,
    Complete,
    /// Emitted by the `ChangeOperation::Snooze` tool for short snoozes
    /// (WI-2 delay) and by quick-action capabilities that perform a short delay.
    Delay,
    Split,
    Undo,
}

/// Result of a start / pause / progress / complete / delay / split mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkTransition {
    pub kind: WorkTransitionKind,
    pub reference: String,
    pub title: String,
    /// Human-readable detail (e.g. new quantity, total active minutes).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl WorkTransition {
    fn voice_template(&self) -> String {
        let prefix = match self.kind {
            WorkTransitionKind::Start => "「{}」の作業を開始しました",
            WorkTransitionKind::Pause => "「{}」の作業を一時停止しました",
            WorkTransitionKind::Progress => "「{}」の進捗を記録しました",
            WorkTransitionKind::Complete => "「{}」の作業を完了しました",
            WorkTransitionKind::Delay => "「{}」の開始をずらしました",
            WorkTransitionKind::Split => "「{}」を分割しました",
            WorkTransitionKind::Undo => "「{}」を元に戻しました",
        };
        let base = strfmt(prefix, &self.title);
        if self.detail.is_empty() {
            base
        } else {
            format!("{base} {}", self.detail)
        }
    }
}

/// A scheduled slot in a schedule summary.
///
/// `schemars` is implemented manually (non-referenceable, inline) so the OpenAPI
/// schema is inlined into `ScheduleSummary` instead of emitting a top-level
/// `ScheduleEntry` definition, which would collide with the existing planner
/// `ScheduleEntry` when the local and agent specs are merged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub reference: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
}

impl schemars::JsonSchema for ScheduleEntry {
    fn schema_name() -> Cow<'static, str> {
        "AgentScheduleEntry".into()
    }

    fn schema_id() -> Cow<'static, str> {
        "AgentScheduleEntry".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let value = serde_json::json!({
            "type": "object",
            "properties": {
                "reference": { "type": "string" },
                "title": { "type": "string" },
                "start_at": { "type": "string" },
                "end_at": { "type": "string" }
            },
            "required": ["reference", "title"]
        });
        schemars::Schema::try_from(value).expect("AgentScheduleEntry is a valid JSON Schema")
    }

    fn inline_schema() -> bool {
        true
    }
}

/// Concise summary of the active schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScheduleSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<ScheduleEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ScheduleEntry>,
}

impl ScheduleSummary {
    fn voice_template(&self) -> String {
        match &self.next {
            Some(next) => format!(
                "スケジュールを確認しました。次の予定は {}、{} からです",
                next.title,
                next.start_at.as_deref().unwrap_or("未定")
            ),
            None => "スケジュールを確認しました。次の予定はありません".to_string(),
        }
    }
}

/// Aggregated progress counts from a task read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProgressSummary {
    pub done: usize,
    pub in_progress: usize,
    pub scheduled: usize,
}

impl ProgressSummary {
    fn voice_template(&self) -> String {
        format!(
            "現在の状況です。完了 {} 件、作業中 {} 件、予定 {} 件です",
            self.done, self.in_progress, self.scheduled
        )
    }
}

/// Kind of a schedule alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAlertKind {
    Conflict,
    Overdue,
    GenerationFailure,
}

/// A planner error surfaced to the user (never a check-in).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScheduleAlert {
    pub kind: ScheduleAlertKind,
    pub message: String,
}

impl ScheduleAlert {
    fn voice_template(&self) -> String {
        self.message.clone()
    }
}

/// Kind of a quick action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Immediate,
    Approval,
    Panel,
}

/// A single quick action on a check-in or card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub kind: ActionKind,
    /// The server-issued one-shot capability for this action, if it is an
    /// immediate capability-authorized action. `Panel` and `Approval` actions
    /// do not carry a capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<crate::capability::ActionCapability>,
}

/// One labelled group of actions. Never empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActionGroup {
    pub title: String,
    pub actions: NonEmptyVec<Action>,
}

impl ActionGroup {
    pub fn new(title: impl Into<String>, actions: Vec<Action>) -> Result<Self, &'static str> {
        Ok(Self {
            title: title.into(),
            actions: NonEmptyVec::new(actions)?,
        })
    }
}

/// A one-round-trip check-in that always offers 「行動」 and 「ズラす」.
///
/// The non-empty wrapper on both action groups makes a card without either
/// group unrepresentable, which enforces the product invariant that every
/// proactive contact offers both options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckInCard {
    pub question: String,
    /// 「行動」 group.
    pub act: ActionGroup,
    /// 「ズラす」 group.
    pub shift: ActionGroup,
}

impl CheckInCard {
    pub fn new(
        question: impl Into<String>,
        act: ActionGroup,
        shift: ActionGroup,
    ) -> Result<Self, &'static str> {
        if act.actions.is_empty() || shift.actions.is_empty() {
            return Err("check-in card requires both 行動 and ズラす action groups");
        }
        Ok(Self {
            question: question.into(),
            act,
            shift,
        })
    }

    fn voice_template(&self) -> String {
        format!(
            "{} どうしますか。行動するか、ずらすかを選んでください",
            self.question
        )
    }
}

/// A focused clarification question instead of a full interview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FocusedQuestion {
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

impl FocusedQuestion {
    fn voice_template(&self) -> String {
        self.message.clone()
    }
}

/// The typed presentation payload carried on a turn result or event.
///
/// Wire form is internally tagged by `type`; unknown tags decode as
/// [`Presentation::Text`].
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Presentation {
    /// Current + next task with quick actions.
    CurrentTask(TaskCard),
    /// Start / pause / progress / complete / delay / split result.
    WorkTransition(WorkTransition),
    /// Schedule read summary.
    ScheduleSummary(ScheduleSummary),
    /// Task-count read summary.
    ProgressSummary(ProgressSummary),
    /// Conflict / overdue / generation failure.
    ScheduleAlert(ScheduleAlert),
    /// One-round-trip check-in with 行動 + ズラす.
    CheckIn(CheckInCard),
    /// An approval request, rendered from the existing type.
    ChangeProposal(ApprovalRequest),
    /// A focused clarification.
    Clarification(FocusedQuestion),
    /// Free-form fallback text.
    ///
    /// Represented as a struct variant so internally-tagged serialization works
    /// (serde cannot tag a primitive newtype like `Text(String)`), and so an
    /// unknown tag can degrade to `Text { text }`.
    Text { text: String },
}

impl Presentation {
    /// The deterministic voice/notification template for this presentation.
    ///
    /// Fixed sentence structures with interpolated values; this is what TTS
    /// and notification bodies (and the voice readback layer) are generated
    /// from. The same data renders identically every time.
    pub fn voice_template(&self) -> String {
        match self {
            Presentation::CurrentTask(card) => {
                if let Some(settlement) = &card.settlement {
                    return format!(
                        "{} どうしますか。行動するか、ずらすかを選んでください",
                        settlement.question
                    );
                }
                let action = if card.work_state == WorkState::InProgress {
                    "作業中"
                } else if card.work_state == WorkState::Overdue {
                    "期限超過"
                } else {
                    "未着手"
                };
                let candidate = if card.authority == TaskAuthority::Candidate {
                    "（候補）"
                } else {
                    ""
                };
                format!(
                    "今やるのは「{}」{candidate} {}です。{}～{}",
                    card.title,
                    action,
                    card.start_at.as_deref().unwrap_or("未定"),
                    card.end_at.as_deref().unwrap_or("未定"),
                )
            }
            Presentation::WorkTransition(t) => t.voice_template(),
            Presentation::ScheduleSummary(s) => s.voice_template(),
            Presentation::ProgressSummary(p) => p.voice_template(),
            Presentation::ScheduleAlert(a) => a.voice_template(),
            Presentation::CheckIn(c) => c.voice_template(),
            Presentation::ChangeProposal(r) => build_change_proposal_readback(r),
            Presentation::Clarification(q) => q.voice_template(),
            Presentation::Text { text } => text.clone(),
        }
    }

    /// Build a presentation from a tool call's output, mapping the known
    /// tool shapes onto the typed model.
    ///
    /// Returns `None` for tools without a corresponding presentation kind so
    /// callers can fall back to the switch-on-approval rule (a pending
    /// [`ApprovalRequest`] becomes [`Presentation::ChangeProposal`]).
    pub fn from_tool_output(name: &str, output: &ToolOutput) -> Option<Presentation> {
        match name {
            "task_start" | "task_pause" | "task_progress" | "task_complete" | "task_split" => {
                // The progress mutation tools produce a WorkTransition when a
                // task is given; when task_ref is omitted they instead return a
                // focused_clarification with an empty proposed_changes (WI-1).
                if let Some(change) = output.proposed_changes.first() {
                    let (title, reference) = title_and_reference(change.after.as_ref());
                    let kind = match change.operation {
                        ChangeOperation::Start => WorkTransitionKind::Start,
                        ChangeOperation::Pause => WorkTransitionKind::Pause,
                        ChangeOperation::Snooze => WorkTransitionKind::Delay,
                        ChangeOperation::Progress => WorkTransitionKind::Progress,
                        ChangeOperation::Complete => WorkTransitionKind::Complete,
                        ChangeOperation::Split => WorkTransitionKind::Split,
                        _ => return None,
                    };
                    Some(Presentation::WorkTransition(WorkTransition {
                        kind,
                        title,
                        reference,
                        detail: change.description.clone(),
                    }))
                } else {
                    focused_clarification(output)
                }
            }
            "get_schedule" | "preview_schedule" => schedule_summary(output),
            "list_tasks" | "get_task" => progress_summary_from_tasks(output),
            "correct_asr" => correct_asr_clarification(output),
            _ => None,
        }
    }
}

/// Map a progress tool's `focused_clarification` output (task_ref omitted) to
/// a [`Clarification`] so the client can present the question as a card.
fn focused_clarification(output: &ToolOutput) -> Option<Presentation> {
    let value: Value = serde_json::from_str(&output.content).ok()?;
    let message = value.get("focused_clarification")?.as_str()?.to_string();
    if message.is_empty() {
        return None;
    }
    Some(Presentation::Clarification(FocusedQuestion {
        message,
        choices: Vec::new(),
    }))
}

/// Map the `correct_asr` tool output to a [`Clarification`].
///
/// The CorrectAsr tool returns `Vec<UserInputAnswer>` (the user's corrected
/// texts) as a JSON array in `ToolOutput::content` (see `user_input.rs:61`).
/// Present the corrected texts as the focused message instead of the raw JSON
/// so the voice/notification template never reads `[{"text": ...}]` aloud.
fn correct_asr_clarification(output: &ToolOutput) -> Option<Presentation> {
    let answers: Vec<UserInputAnswer> = serde_json::from_str(&output.content).ok()?;
    if answers.is_empty() {
        return None;
    }
    let texts: Vec<&str> = answers.iter().map(|a| a.text.as_str()).collect();
    let message = match texts.len() {
        1 => format!("認識を修正しました：{}", texts[0]),
        _ => format!("認識を修正しました：{}", texts.join("、")),
    };
    Some(Presentation::Clarification(FocusedQuestion {
        message,
        choices: Vec::new(),
    }))
}

fn title_and_reference(after: Option<&Value>) -> (String, String) {
    let title = after
        .and_then(|v| v.get("title"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "タスク".to_string());
    let reference = after
        .and_then(|v| v.get("reference"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_default();
    (title, reference)
}

/// Map get_schedule / preview_schedule output to a [`ScheduleSummary`]. Only
/// the first entry becomes the `next` slot so the presentation stays compact.
fn schedule_summary(output: &ToolOutput) -> Option<Presentation> {
    let value: Value = serde_json::from_str(&output.content).ok()?;
    let entries_raw = value.get("entries")?.as_array()?;
    let entries: Vec<ScheduleEntry> = entries_raw
        .iter()
        .filter_map(parse_schedule_entry)
        .collect();
    let next = entries.first().cloned();
    Some(Presentation::ScheduleSummary(ScheduleSummary {
        next,
        entries,
    }))
}

fn parse_schedule_entry(v: &Value) -> Option<ScheduleEntry> {
    let reference = v.get("reference")?.as_str()?.to_string();
    let title = v.get("title")?.as_str()?.to_string();
    Some(ScheduleEntry {
        reference,
        title,
        start_at: v.get("start_at").and_then(Value::as_str).map(Into::into),
        end_at: v.get("end_at").and_then(Value::as_str).map(Into::into),
    })
}

/// Count task statuses from `list_tasks` (bare JSON array of task objects) and
/// `get_task` (`{ "tasks": [...] }`) output.
fn progress_summary_from_tasks(output: &ToolOutput) -> Option<Presentation> {
    let value: Value = serde_json::from_str(&output.content).ok()?;
    let arr = match value.get("tasks") {
        Some(Value::Array(a)) => a,
        Some(_) | None => value.as_array()?,
    };
    let mut done = 0usize;
    let mut in_progress = 0usize;
    let mut scheduled = 0usize;
    for task in arr {
        match task.get("status").and_then(Value::as_str) {
            Some("completed") | Some("skipped") => done += 1,
            Some("in_progress") => in_progress += 1,
            _ => scheduled += 1,
        }
    }
    Some(Presentation::ProgressSummary(ProgressSummary {
        done,
        in_progress,
        scheduled,
    }))
}

/// Version-tolerant decode: a valid tag decodes to its typed variant; an
/// unknown (or malformed) tag degrades to [`Presentation::Text`] using the
/// accompanying `text` field as the fallback when present.
impl<'de> Deserialize<'de> for Presentation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;
        let fallback = value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let tag = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .to_string();
        // The `type` discriminator is not a field of any inner variant; strip
        // it so deny_unknown_fields structs such as CheckInCard still decode.
        if let Value::Object(map) = &mut value {
            map.remove("type");
        }
        // A malformed known kind also degrades to the fallback text (version
        // tolerant), so a forward-extension or a parse surprise never fails the
        // whole turn.
        let downgrade =
            |r: Result<Option<Presentation>, serde_json::Error>| -> Result<Self, D::Error> {
                match r {
                    Ok(Some(p)) => Ok(p),
                    _ => Ok(Presentation::Text {
                        text: fallback.clone(),
                    }),
                }
            };
        match tag.as_str() {
            "current_task" => downgrade(
                serde_json::from_value(value).map(|c: TaskCard| Some(Presentation::CurrentTask(c))),
            ),
            "work_transition" => downgrade(
                serde_json::from_value(value)
                    .map(|w: WorkTransition| Some(Presentation::WorkTransition(w))),
            ),
            "schedule_summary" => downgrade(
                serde_json::from_value(value)
                    .map(|s: ScheduleSummary| Some(Presentation::ScheduleSummary(s))),
            ),
            "progress_summary" => downgrade(
                serde_json::from_value(value)
                    .map(|p: ProgressSummary| Some(Presentation::ProgressSummary(p))),
            ),
            "schedule_alert" => downgrade(
                serde_json::from_value(value)
                    .map(|a: ScheduleAlert| Some(Presentation::ScheduleAlert(a))),
            ),
            "check_in" => downgrade(
                serde_json::from_value(value).map(|c: CheckInCard| Some(Presentation::CheckIn(c))),
            ),
            "change_proposal" => downgrade(
                serde_json::from_value(value)
                    .map(|r: ApprovalRequest| Some(Presentation::ChangeProposal(r))),
            ),
            "clarification" => downgrade(
                serde_json::from_value(value)
                    .map(|q: FocusedQuestion| Some(Presentation::Clarification(q))),
            ),
            "text" => Ok(Presentation::Text { text: fallback }),
            _ => Ok(Presentation::Text { text: fallback }),
        }
    }
}

/// Build a deterministic Japanese readback for a change proposal.
///
/// Reads the concrete fields out of each `ProposedChange.after` value so the
/// user hears what will actually be written: task title, estimate, deadline,
/// quantity, and any schedule knock-on effects (displaced tasks from a
/// `_preview` block). The result always ends with the confirmation prompt
/// expected by the voice confirmation loop.
fn build_change_proposal_readback(request: &ApprovalRequest) -> String {
    let mut task_parts = Vec::new();
    let mut schedule_parts = Vec::new();
    let mut other_parts = Vec::new();

    for change in &request.changes {
        match (change.target.kind, change.operation) {
            (TargetKind::Task, ChangeOperation::Create) => {
                if let Some(after) = change.after.as_ref().filter(|v| !v.is_null()) {
                    task_parts.push(describe_task_create(after));
                } else {
                    task_parts.push(change.description.clone());
                }
            }
            (TargetKind::Task, ChangeOperation::Update) => {
                if let Some(after) = change.after.as_ref().filter(|v| !v.is_null()) {
                    task_parts.push(describe_task_update(after, &change.description));
                } else {
                    task_parts.push(change.description.clone());
                }
            }
            (TargetKind::Task, ChangeOperation::Delete) => {
                task_parts.push(change.description.clone());
            }
            (TargetKind::Schedule, ChangeOperation::Generate)
            | (TargetKind::Schedule, ChangeOperation::Reschedule) => {
                schedule_parts.push(describe_schedule_impact(
                    change.after.as_ref(),
                    change.operation,
                    &change.description,
                ));
            }
            _ => other_parts.push(change.description.clone()),
        }
    }

    let mut body = String::new();
    if !task_parts.is_empty() {
        body.push_str(&task_parts.join("。"));
        body.push('。');
    }
    if !schedule_parts.is_empty() {
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(&schedule_parts.join("。"));
        body.push('。');
    }
    if !other_parts.is_empty() {
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(&other_parts.join("。"));
        body.push('。');
    }

    if !request.why.is_empty() && request.why != DEFAULT_APPROVAL_WHY {
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(&request.why);
        if !body.ends_with('。') {
            body.push('。');
        }
    }

    if body.is_empty() {
        body = request.why.clone();
    }
    if body.is_empty() {
        body = "変更を提案しました".to_string();
    }
    if body.ends_with('。') {
        body.pop();
    }
    format!("{}。よろしいですか", body)
}

/// Format a stored/absolute datetime string for voice readback.
///
/// Preserves the original offset when possible and produces a Japanese
/// phrase such as "2026年8月28日 23時59分". Falls back to the original
/// string when parsing fails.
fn format_datetime_for_voice(s: &str) -> String {
    if s.is_empty() {
        return s.to_string();
    }
    if let Ok(zdt) = jiff::Zoned::from_str(s) {
        return zdt.strftime("%Y年%m月%d日 %H時%M分").to_string();
    }
    if let Ok(ts) = jiff::Timestamp::from_str(s) {
        return ts
            .to_zoned(jiff::tz::TimeZone::UTC)
            .strftime("%Y年%m月%d日 %H時%M分")
            .to_string();
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = jiff::civil::DateTime::strptime(fmt, s)
            && let Ok(zdt) = dt.to_zoned(jiff::tz::TimeZone::UTC)
        {
            return zdt.strftime("%Y年%m月%d日 %H時%M分").to_string();
        }
    }
    s.to_string()
}

/// Describe a task-create proposal in spoken Japanese.
///
/// Falls back gracefully when the LLM did not fill optional fields.
fn describe_task_create(after: &Value) -> String {
    let title = after
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("タスク");

    let mut parts = vec![format!("「{title}」を作成")];

    if let Some(avg) = after.get("avg_minutes").and_then(Value::as_i64) {
        parts.push(format!("見積もり{avg}分"));
    }
    if let Some(q) = after.get("quantity_total").and_then(Value::as_i64)
        && q > 0
    {
        let unit = after
            .get("quantity_unit")
            .and_then(Value::as_str)
            .unwrap_or("");
        if unit.is_empty() {
            parts.push(format!("数量{q}"));
        } else {
            parts.push(format!("数量{q}{unit}"));
        }
    }
    if let Some(end) = after.get("end_at").and_then(Value::as_str) {
        parts.push(format!("期限{}まで", format_datetime_for_voice(end)));
    } else if let Some(end) = after.get("end_at") {
        // The field may be JSON null; ignore it.
        let _ = end;
    }
    if let Some(start) = after.get("start_at").and_then(Value::as_str)
        && !start.is_empty()
    {
        parts.push(format!("開始{}から", format_datetime_for_voice(start)));
    }

    parts.join("、")
}

/// Describe a task-update proposal, including fields that actually changed.
fn describe_task_update(after: &Value, description: &str) -> String {
    let title = after.get("title").and_then(Value::as_str).unwrap_or("");
    if title.is_empty() {
        return description.to_string();
    }
    let mut parts = vec![format!("「{title}」を更新")];
    if let Some(end) = after.get("end_at").and_then(Value::as_str) {
        parts.push(format!("期限{}まで", format_datetime_for_voice(end)));
    }
    if let Some(start) = after.get("start_at").and_then(Value::as_str)
        && !start.is_empty()
    {
        parts.push(format!("開始{}から", format_datetime_for_voice(start)));
    }
    if let Some(avg) = after.get("avg_minutes").and_then(Value::as_i64) {
        parts.push(format!("見積もり{avg}分"));
    }
    if let Some(q) = after.get("quantity_total").and_then(Value::as_i64)
        && q > 0
    {
        let unit = after
            .get("quantity_unit")
            .and_then(Value::as_str)
            .unwrap_or("");
        if unit.is_empty() {
            parts.push(format!("数量{q}"));
        } else {
            parts.push(format!("数量{q}{unit}"));
        }
    }
    parts.join("、")
}

/// Describe the knock-on schedule impact from a `_preview` block.
///
/// Only the displaced tasks are read aloud; unscheduled tasks and full entry
/// lists would be too long for a voice readback.
fn describe_schedule_impact(
    after: Option<&Value>,
    operation: ChangeOperation,
    description: &str,
) -> String {
    let mut summary = match operation {
        ChangeOperation::Reschedule => "スケジュールを再調整".to_string(),
        _ => "スケジュールを生成".to_string(),
    };

    let Some(after) = after else {
        return description.to_string();
    };
    let preview = after
        .get("_preview")
        .or_else(|| after.get("_preview_entries"));
    if let Some(Value::Array(entries)) = preview.as_ref().and_then(|p| p.get("entries"))
        && !entries.is_empty()
    {
        let first = entries
            .iter()
            .next()
            .and_then(|e| e.get("title").and_then(Value::as_str))
            .unwrap_or("");
        if !first.is_empty() {
            summary = format!("{summary}。直近は{first}から");
        }
    }

    if let Some(Value::Array(displaced)) =
        preview.as_ref().and_then(|p| p.get("displaced_task_ids"))
    {
        let titles: Vec<&str> = displaced
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.is_empty() && *s != "unknown" && !s.starts_with("unknown "))
            .collect();
        if !titles.is_empty() {
            summary = format!("{summary}。{}がずれます", titles.join("、"));
        }
    }

    summary
}

/// Minimal `{}` placeholder formatter. Only the first placeholder is replaced
/// so template constants stay deterministic and free of panics on stray
/// braces elsewhere in the literal.
fn strfmt(template: &str, arg: &str) -> String {
    template.replacen("{}", arg, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ChangeOperation, ProposedChange, Target, TargetKind};
    use serde_json::json;

    fn action(label: &str) -> Action {
        Action {
            id: format!("id-{label}").to_string(),
            label: label.to_string(),
            kind: ActionKind::Immediate,
            capability: None,
        }
    }

    fn check_in_card() -> CheckInCard {
        CheckInCard::new(
            "続けますか?",
            ActionGroup::new("行動", vec![action("着手")]).unwrap(),
            ActionGroup::new("ズラす", vec![action("10分後")]).unwrap(),
        )
        .unwrap()
    }

    fn task_output(operation: ChangeOperation, content: serde_json::Value) -> ToolOutput {
        ToolOutput {
            content: content.to_string(),
            why: Some("テスト".into()),
            proposed_changes: vec![ProposedChange {
                operation,
                target: Target::new(TargetKind::Task, "#7"),
                description: "「レポート」に変更".into(),
                before: Some(
                    json!({"title": "レポート", "reference": "#7", "status": "scheduled"}),
                ),
                after: Some(
                    json!({"title": "レポート", "reference": "#7", "status": "in_progress"}),
                ),
                arguments: None,
                observed_updated_at: None,
                proposal_id: None,
            }],
            ..Default::default()
        }
    }

    // ── CheckInCard cannot omit 行動 or ズラす ─────────────────────────

    #[test]
    fn check_in_requires_both_action_groups() {
        let act = ActionGroup::new("行動", vec![action("着手")]).unwrap();
        let shift = ActionGroup::new("ズラす", vec![action("10分後")]).unwrap();
        assert!(CheckInCard::new("続けますか?", act.clone(), shift.clone()).is_ok());

        // Removing either group must be rejected.
        let empty = ActionGroup::new("行動", vec![]);
        assert!(empty.is_err());
    }

    #[test]
    fn non_empty_vec_serializes_as_plain_array_and_rejects_empty_on_decode() {
        let v = NonEmptyVec::new(vec![1, 2, 3]).unwrap();
        assert_eq!(serde_json::to_value(&v).unwrap(), json!([1, 2, 3]));
        assert!(serde_json::from_str::<NonEmptyVec<i32>>("[]").is_err());
        assert!(serde_json::from_str::<NonEmptyVec<i32>>("[1]").is_ok());
    }

    #[test]
    fn check_in_with_empty_group_decode_is_rejected() {
        let card = check_in_card();
        let mut v = serde_json::to_value(&card).unwrap();
        v["act"]["actions"] = json!([]);
        assert!(serde_json::from_value::<CheckInCard>(v).is_err());
    }

    // ── tool-output → presentation mapping ────────────────────────────

    #[test]
    fn progress_tools_map_to_work_transition() {
        for (tool, kind, op) in [
            (
                "task_start",
                WorkTransitionKind::Start,
                ChangeOperation::Start,
            ),
            (
                "task_pause",
                WorkTransitionKind::Pause,
                ChangeOperation::Pause,
            ),
            (
                "task_progress",
                WorkTransitionKind::Progress,
                ChangeOperation::Progress,
            ),
            (
                "task_complete",
                WorkTransitionKind::Complete,
                ChangeOperation::Complete,
            ),
            (
                "task_split",
                WorkTransitionKind::Split,
                ChangeOperation::Split,
            ),
        ] {
            let out = task_output(op, json!({}));
            let p = Presentation::from_tool_output(tool, &out).expect("maps to a presentation");
            assert!(
                matches!(&p, Presentation::WorkTransition(t) if t.kind == kind && t.title == "レポート"),
                "tool {tool} produced {p:?}"
            );
        }
    }

    #[test]
    fn unknown_tool_maps_to_none() {
        let out = ToolOutput::default();
        assert!(Presentation::from_tool_output("memory_search", &out).is_none());
    }

    #[test]
    fn schedule_read_maps_to_schedule_summary() {
        let out = task_output(
            ChangeOperation::Complete,
            json!({
                "id": "s",
                "entries": [
                    {"reference": "#1", "title": "朝会", "start_at": "09:00", "end_at": "09:30"},
                    {"reference": "#2", "title": "出社", "start_at": "10:00", "end_at": "11:00"}
                ]
            }),
        );
        let p = Presentation::from_tool_output("get_schedule", &out).expect("maps");
        let Presentation::ScheduleSummary(s) = p else {
            panic!("expected ScheduleSummary");
        };
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.next.as_ref().unwrap().title, "朝会");
    }

    #[test]
    fn task_read_counts_progress() {
        let out = task_output(
            ChangeOperation::Complete,
            json!([
                {"title": "a", "status": "completed"},
                {"title": "b", "status": "in_progress"},
                {"title": "c", "status": "scheduled"}
            ]),
        );
        let p = Presentation::from_tool_output("list_tasks", &out).expect("maps");
        let Presentation::ProgressSummary(s) = p else {
            panic!("expected ProgressSummary");
        };
        assert_eq!((s.done, s.in_progress, s.scheduled), (1, 1, 1));
    }

    #[test]
    fn get_task_read_counts_tasks_object() {
        let out = task_output(
            ChangeOperation::Complete,
            json!({"tasks": [{"status": "completed"}, {"status": "scheduled"}]}),
        );
        let p = Presentation::from_tool_output("get_task", &out).expect("maps");
        let Presentation::ProgressSummary(s) = p else {
            panic!("expected ProgressSummary");
        };
        assert_eq!((s.done, s.in_progress, s.scheduled), (1, 0, 1));
    }

    #[test]
    fn correct_asr_maps_to_clarification_from_answers() {
        // The real CorrectAsr tool returns a JSON array of UserInputAnswer
        // (user_input.rs); the message must reflect the corrected texts, not
        // raw JSON.
        let out = ToolOutput {
            content: json!([{"text": "出社"}]).to_string(),
            ..Default::default()
        };
        let p = Presentation::from_tool_output("correct_asr", &out).expect("maps");
        assert!(
            matches!(
                &p,
                Presentation::Clarification(q)
                    if q.message == "認識を修正しました：出社"
            ),
            "unexpected: {p:?}"
        );
    }

    #[test]
    fn correct_asr_with_multiple_answers_joins_texts() {
        let out = ToolOutput {
            content: json!([{"text": "出社"}, {"text": "会議"}]).to_string(),
            ..Default::default()
        };
        let p = Presentation::from_tool_output("correct_asr", &out).expect("maps");
        assert!(
            matches!(
                &p,
                Presentation::Clarification(q)
                    if q.message == "認識を修正しました：出社、会議"
            ),
            "unexpected: {p:?}"
        );
    }

    #[test]
    fn correct_asr_with_non_answer_content_maps_to_none() {
        // Content that is not the expected Vec<UserInputAnswer> must not be
        // read aloud as raw JSON.
        let out = ToolOutput {
            content: "どちらの会話ですか？".into(),
            ..Default::default()
        };
        assert!(Presentation::from_tool_output("correct_asr", &out).is_none());
    }

    #[test]
    fn progress_tool_focused_clarification_maps_to_clarification() {
        // A progress tool with task_ref omitted returns an empty
        // proposed_changes and content["focused_clarification"].
        let out = ToolOutput {
            content: json!({"focused_clarification": "「着手」の対象となるタスクが複数あります。どれですか？"}).to_string(),
            ..Default::default()
        };
        let p = Presentation::from_tool_output("task_start", &out).expect("maps");
        assert!(
            matches!(
                &p,
                Presentation::Clarification(q)
                    if q.message.contains("どれですか")
            ),
            "unexpected: {p:?}"
        );
    }

    // ── voice templates ───────────────────────────────────────────────

    #[test]
    fn progress_summary_template_is_deterministic() {
        let summary = ProgressSummary {
            done: 1,
            in_progress: 2,
            scheduled: 3,
        };
        let a = Presentation::ProgressSummary(summary.clone()).voice_template();
        let b = Presentation::ProgressSummary(summary).voice_template();
        assert_eq!(a, b);
        assert_eq!(a, "現在の状況です。完了 1 件、作業中 2 件、予定 3 件です");
    }

    #[test]
    fn work_transition_template_renders_start() {
        let p = Presentation::WorkTransition(WorkTransition {
            kind: WorkTransitionKind::Start,
            reference: "#7".into(),
            title: "レポート".into(),
            detail: String::new(),
        });
        assert_eq!(p.voice_template(), "「レポート」の作業を開始しました");
    }

    #[test]
    fn check_in_template_includes_question() {
        let p = Presentation::CheckIn(check_in_card());
        assert!(p.voice_template().contains("続けますか?"));
    }

    #[test]
    fn change_proposal_template_uses_approval_why() {
        let r = ApprovalRequest {
            id: "x".into(),
            why: "スケジュールを再生成します".into(),
            changes: vec![],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let p = Presentation::ChangeProposal(r);
        assert_eq!(
            p.voice_template(),
            "スケジュールを再生成します。よろしいですか"
        );
    }

    #[test]
    fn current_task_template_marks_candidate() {
        let card = TaskCard {
            title: "レポート".into(),
            reference: "#7".into(),
            start_at: Some("09:00".into()),
            end_at: Some("10:00".into()),
            work_state: WorkState::NotStarted,
            authority: TaskAuthority::Candidate,
            next_task: None,
            settlement: None,
        };
        assert_eq!(
            Presentation::CurrentTask(card).voice_template(),
            "今やるのは「レポート」（候補） 未着手です。09:00～10:00"
        );
    }

    #[test]
    fn current_task_with_settlement_prompt_leads_with_settlement() {
        let card = TaskCard {
            title: "レポート".into(),
            reference: "#7".into(),
            start_at: Some("09:00".into()),
            end_at: Some("10:00".into()),
            work_state: WorkState::NotStarted,
            authority: TaskAuthority::Candidate,
            next_task: None,
            settlement: Some(SettlementPrompt {
                question: "09:00〜09:30 の未確定時間を整理してください".into(),
                act: ActionGroup::new("行動", vec![action("この時間で作業")]).unwrap(),
                shift: ActionGroup::new("ズラす", vec![action("無視")]).unwrap(),
            }),
        };
        let template = Presentation::CurrentTask(card).voice_template();
        assert!(template.contains("未確定時間"));
        assert!(template.contains("どうしますか"));
    }

    // ── version-tolerant round trips ──────────────────────────────────

    #[test]
    fn unknown_tag_decodes_to_text() {
        let json = json!({"type": "future_kind", "title": "x", "text": "fallback"});
        let p: Presentation = serde_json::from_value(json).unwrap();
        assert!(matches!(p, Presentation::Text { text } if text == "fallback"));
    }

    #[test]
    fn missing_type_decodes_to_text() {
        let p: Presentation = serde_json::from_str(r#"{"text":"hi"}"#).unwrap();
        assert!(matches!(p, Presentation::Text { text } if text == "hi"));
    }

    #[test]
    fn recognized_kind_round_trips() {
        let p = Presentation::CheckIn(check_in_card());
        let encoded = serde_json::to_value(&p).unwrap();
        // The wire form is internally tagged.
        assert_eq!(encoded["type"], "check_in");
        let decoded: Presentation = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            serde_json::to_value(&p).unwrap()
        );
    }

    #[test]
    fn text_presentation_round_trips() {
        let p = Presentation::Text {
            text: "こんにちは".into(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v, json!({"type": "text", "text": "こんにちは"}));
        let d: Presentation = serde_json::from_value(v).unwrap();
        assert!(matches!(d, Presentation::Text { text } if text == "こんにちは"));
    }

    /// The canonical wire fixtures in `src/fixtures/presentations.json` are
    /// shared with the mobile client; this round-trips each one so Rust and
    /// TypeScript parse the exact same encoding.
    #[test]
    fn shared_fixtures_round_trip() {
        let raw = include_str!("fixtures/presentations.json");
        let fixtures: Vec<Value> = serde_json::from_str(raw).expect("fixtures parse");
        assert_eq!(fixtures.len(), 6, "fixtures should contain 6 presentations");
        for fixture in fixtures {
            let decoded: Presentation = serde_json::from_value(fixture.clone())
                .unwrap_or_else(|e| panic!("fixture {fixture} failed to decode: {e}"));
            // Re-serializing must be byte-identical to the fixture.
            let reencoded = serde_json::to_value(&decoded).expect("re-encode");
            assert_eq!(
                reencoded, fixture,
                "fixture did not round-trip byte-identically"
            );
            // Every presentation must be able to render a voice template.
            assert!(!decoded.voice_template().is_empty());
        }
    }

    /// An unknown future-presentation tag must decode to `Text` and not fail,
    /// which is the forward-compatibility contract for the shared fixtures.
    #[test]
    fn shared_fixture_unknown_tag_is_version_tolerant() {
        let v: Presentation =
            serde_json::from_str(r#"{"type":"future_kind","text":"fallback"}"#).unwrap();
        assert!(matches!(v, Presentation::Text { text } if text == "fallback"));
    }

    // ── one-utterance capture readback (WI-15) ─────────────────────────

    #[test]
    fn capture_readback_includes_estimate_quantity_deadline() {
        let request = ApprovalRequest {
            id: "approval-1".into(),
            why: "過去の演習実績から推定".into(),
            changes: vec![ProposedChange {
                operation: ChangeOperation::Create,
                target: Target::new(TargetKind::Task, "#next"),
                description: "「演習30題追加」を作成".into(),
                after: Some(json!({
                    "title": "演習30題追加",
                    "avg_minutes": 45,
                    "sigma_minutes": 10,
                    "quantity_total": 30,
                    "quantity_unit": "題",
                    "end_at": "2026-08-28T23:59",
                    "start_at": "2026-08-26T14:00",
                })),
                ..Default::default()
            }],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let template = Presentation::ChangeProposal(request).voice_template();
        assert!(template.contains("演習30題追加"));
        assert!(template.contains("見積もり45分"));
        assert!(template.contains("数量30題"));
        assert!(template.contains("期限"));
        assert!(template.contains("よろしいですか"));
    }

    #[test]
    fn capture_readback_includes_knock_on_schedule_impact() {
        let request = ApprovalRequest {
            id: "approval-2".into(),
            why: String::new(),
            changes: vec![
                ProposedChange {
                    operation: ChangeOperation::Create,
                    target: Target::new(TargetKind::Task, "#next"),
                    description: "「歯医者」を作成".into(),
                    after: Some(json!({
                        "title": "歯医者",
                        "avg_minutes": 60,
                        "end_at": "2026-08-26T13:40",
                    })),
                    ..Default::default()
                },
                ProposedChange {
                    operation: ChangeOperation::Generate,
                    target: Target::new(TargetKind::Schedule, ""),
                    description: "スケジュールを生成".into(),
                    after: Some(json!({
                        "_preview": {
                            "entries": [
                                {"title": "歯医者", "start_at": "12:40", "end_at": "13:40"},
                            ],
                            "displaced_task_ids": ["#3 昼食"],
                        },
                    })),
                    ..Default::default()
                },
            ],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let template = Presentation::ChangeProposal(request).voice_template();
        assert!(template.contains("歯医者"));
        assert!(template.contains("スケジュールを生成"));
        assert!(template.contains("昼食"));
        assert!(template.contains("ずれます"));
        assert!(template.contains("よろしいですか"));
    }

    #[test]
    fn capture_readback_formats_deadline_for_voice() {
        let request = ApprovalRequest {
            id: "approval-3".into(),
            why: String::new(),
            changes: vec![ProposedChange {
                operation: ChangeOperation::Create,
                target: Target::new(TargetKind::Task, "#next"),
                description: "「演習」を作成".into(),
                after: Some(json!({
                    "title": "演習",
                    "avg_minutes": 30,
                    "end_at": "2026-08-28T23:59",
                })),
                ..Default::default()
            }],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let template = Presentation::ChangeProposal(request).voice_template();
        assert!(template.contains("2026年08月28日 23時59分"), "deadline should be voice-formatted: {template}");
        assert!(!template.contains("T23:59"), "raw ISO string should not appear: {template}");
    }

    #[test]
    fn capture_readback_omits_zero_quantity() {
        let request = ApprovalRequest {
            id: "approval-4".into(),
            why: String::new(),
            changes: vec![ProposedChange {
                operation: ChangeOperation::Create,
                target: Target::new(TargetKind::Task, "#next"),
                description: "「雑務」を作成".into(),
                after: Some(json!({
                    "title": "雑務",
                    "avg_minutes": 15,
                    "quantity_total": 0,
                    "quantity_unit": "題",
                })),
                ..Default::default()
            }],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let template = Presentation::ChangeProposal(request).voice_template();
        assert!(!template.contains("数量"), "zero quantity should be omitted: {template}");
    }

    #[test]
    fn capture_readback_falls_back_to_description_when_after_is_null() {
        let request = ApprovalRequest {
            id: "approval-5".into(),
            why: String::new(),
            changes: vec![ProposedChange {
                operation: ChangeOperation::Create,
                target: Target::new(TargetKind::Task, "#next"),
                description: "「メール確認」を作成".into(),
                after: Some(Value::Null),
                ..Default::default()
            }],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let template = Presentation::ChangeProposal(request).voice_template();
        assert!(template.contains("メール確認"), "description should be read when after is null: {template}");
        assert!(template.contains("よろしいですか"));
    }

    #[test]
    fn reschedule_readback_says_reschedule_not_generate() {
        let request = ApprovalRequest {
            id: "approval-6".into(),
            why: String::new(),
            changes: vec![ProposedChange {
                operation: ChangeOperation::Reschedule,
                target: Target::new(TargetKind::Schedule, ""),
                description: "スケジュールを再調整".into(),
                after: Some(json!({
                    "_preview": {
                        "entries": [{"title": "会議", "start_at": "14:00", "end_at": "15:00"}],
                        "displaced_task_ids": [],
                    },
                })),
                ..Default::default()
            }],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let template = Presentation::ChangeProposal(request).voice_template();
        assert!(template.contains("スケジュールを再調整"), "reschedule should say 再調整: {template}");
        assert!(!template.contains("スケジュールを生成"), "reschedule should not say 生成: {template}");
    }

    #[test]
    fn capture_readback_ignores_unknown_displaced_tasks() {
        let request = ApprovalRequest {
            id: "approval-7".into(),
            why: String::new(),
            changes: vec![
                ProposedChange {
                    operation: ChangeOperation::Create,
                    target: Target::new(TargetKind::Task, "#next"),
                    description: "「歯医者」を作成".into(),
                    after: Some(json!({
                        "title": "歯医者",
                        "avg_minutes": 60,
                        "end_at": "2026-08-26T13:40",
                    })),
                    ..Default::default()
                },
                ProposedChange {
                    operation: ChangeOperation::Generate,
                    target: Target::new(TargetKind::Schedule, ""),
                    description: "スケジュールを生成".into(),
                    after: Some(json!({
                        "_preview": {
                            "entries": [{"title": "歯医者"}],
                            "displaced_task_ids": ["unknown", "#3 昼食", "unknown task"],
                        },
                    })),
                    ..Default::default()
                },
            ],
            inferred_fields: vec![],
            warnings: vec![],
            expires_at: jiff::Timestamp::now(),
        };
        let template = Presentation::ChangeProposal(request).voice_template();
        assert!(template.contains("昼食"), "known displaced task should be read: {template}");
        assert!(!template.contains("unknown"), "unknown displaced task should be omitted: {template}");
    }
}
