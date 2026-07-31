use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};
use std::str::FromStr;
use takusu_types::{UnknownLabel, enum_label};

/// Structured recoverable argument error passed back to the LLM.
///
/// Carrying the field separately from the reason lets the agent format
/// clearer retry guidance without exposing the entire argument object.
#[derive(Debug, thiserror::Error)]
pub enum InvalidArgsError {
    #[error("field '{field}': {reason}")]
    Field { field: String, reason: String },
    #[error("{reason}")]
    NoField { reason: String },
}

impl InvalidArgsError {
    /// Create an error for a specific field.
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Field {
            field: field.into(),
            reason: reason.into(),
        }
    }

    /// Create an error without naming a specific field.
    pub fn no_field(reason: impl Into<String>) -> Self {
        Self::NoField {
            reason: reason.into(),
        }
    }

    /// Name of the invalid argument/field, if known.
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::Field { field, .. } => Some(field),
            Self::NoField { .. } => None,
        }
    }

    /// Human-readable explanation of what is wrong.
    pub fn reason(&self) -> &str {
        match self {
            Self::Field { reason, .. } | Self::NoField { reason } => reason,
        }
    }
}

/// Converts a `String` into an `InvalidArgsError` without a field name.
///
/// Prefer `InvalidArgsError::new` when the problematic argument/field is known,
/// because this conversion drops field information and produces a generic error.
impl From<String> for InvalidArgsError {
    fn from(reason: String) -> Self {
        Self::no_field(reason)
    }
}

/// Converts a `&str` into an `InvalidArgsError` without a field name.
///
/// Prefer `InvalidArgsError::new` when the problematic argument/field is known,
/// because this conversion drops field information and produces a generic error.
impl From<&str> for InvalidArgsError {
    fn from(reason: &str) -> Self {
        Self::no_field(reason)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArgs(InvalidArgsError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("optimistic conflict: {0}")]
    Conflict(String),
    #[error("operation cancelled by user")]
    Cancelled,
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl From<UnknownLabel> for ToolError {
    fn from(err: UnknownLabel) -> Self {
        ToolError::InvalidArgs(InvalidArgsError::no_field(err.to_string()))
    }
}

impl From<InvalidArgsError> for ToolError {
    fn from(err: InvalidArgsError) -> Self {
        ToolError::InvalidArgs(err)
    }
}

impl ToolError {
    /// Extract the `InvalidArgsError` from a `ToolError::InvalidArgs`,
    /// converting any other variant into `InvalidArgsError::no_field`.
    pub fn into_invalid_args(self) -> InvalidArgsError {
        match self {
            ToolError::InvalidArgs(e) => e,
            other => InvalidArgsError::no_field(other.to_string()),
        }
    }
}

/// Serde helper: deserialize an optional string, trim whitespace, and return
/// `None` for empty/whitespace-only values. Mirrors the behavior of the
/// hand-written `optional_string` helper used by the legacy tools.
pub fn deserialize_trimmed_optional<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    Ok(raw.and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }))
}

/// Serde helper: deserialize a required string and trim whitespace. Returns
/// an error if the result is empty. Mirrors the behavior of the hand-written
/// `required_string` helper.
pub fn deserialize_trimmed_required<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: String = String::deserialize(deserializer)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err(serde::de::Error::custom("missing or empty"))
    } else {
        Ok(trimmed.to_string())
    }
}

/// How a tool is exposed to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    /// Always included in the tool list sent to the model.
    Direct,
    /// Not sent by default; discoverable through `tool_search`.
    Deferred,
    /// Never sent to the model and rejected if called.
    Hidden,
}

/// Type-safe name for every tool registered with [`ToolRegistry`].
///
/// Replaces the scattered `&'static str` literals that were previously returned
/// by `Tool::name()`. The string representation
/// (`into()`, provided by strum's `IntoStaticStr`) is the wire format sent to
/// the LLM; the enum variants give compile-time protection against typos at
/// registration and call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::IntoStaticStr)]
pub enum ToolName {
    #[strum(serialize = "correct_asr")]
    CorrectAsr,
    #[strum(serialize = "tool_search")]
    ToolSearch,

    #[strum(serialize = "skills_list")]
    SkillsList,
    #[strum(serialize = "skills_read")]
    SkillsRead,
    #[strum(serialize = "skills_propose_add")]
    SkillsProposeAdd,
    #[strum(serialize = "skills_propose_edit")]
    SkillsProposeEdit,

    #[strum(serialize = "expand_rrule")]
    ExpandRrule,

    #[strum(serialize = "task_start")]
    TaskStart,
    #[strum(serialize = "task_pause")]
    TaskPause,
    #[strum(serialize = "task_progress")]
    TaskProgress,
    #[strum(serialize = "task_complete")]
    TaskComplete,
    #[strum(serialize = "task_split")]
    TaskSplit,

    #[strum(serialize = "memory_search")]
    MemorySearch,
    #[strum(serialize = "similar_tasks")]
    SimilarTasks,
    #[strum(serialize = "memory_save")]
    MemorySave,
    #[strum(serialize = "memory_update")]
    MemoryUpdate,
    #[strum(serialize = "memory_delete")]
    MemoryDelete,

    #[strum(serialize = "day_details")]
    DayDetails,

    #[strum(serialize = "list_tasks")]
    ListTasks,
    #[strum(serialize = "get_task")]
    GetTask,
    #[strum(serialize = "list_habits")]
    ListHabits,
    #[strum(serialize = "get_habit")]
    GetHabit,
    #[strum(serialize = "get_schedule")]
    GetSchedule,
    #[strum(serialize = "habit_scheduled_spans")]
    HabitScheduledSpans,
    #[strum(serialize = "get_settings")]
    GetSettings,
    #[strum(serialize = "preview_schedule")]
    PreviewSchedule,

    #[strum(serialize = "create_task")]
    CreateTask,
    #[strum(serialize = "update_task")]
    UpdateTask,
    #[strum(serialize = "delete_task")]
    DeleteTask,
    #[strum(serialize = "create_habit")]
    CreateHabit,
    #[strum(serialize = "update_habit")]
    UpdateHabit,
    #[strum(serialize = "delete_habit")]
    DeleteHabit,
    #[strum(serialize = "generate_schedule")]
    GenerateSchedule,
    #[strum(serialize = "reschedule")]
    Reschedule,

    #[strum(serialize = "move_task")]
    MoveTask,
}

enum_label! {
    /// Operation kind for a proposed or applied change.
    pub enum ChangeOperation {
        #[default] Create = "create",
        Update = "update",
        Delete = "delete",
        Generate = "generate",
        Reschedule = "reschedule",
        Move = "move",
        Start = "start",
        Pause = "pause",
        Progress = "progress",
        Complete = "complete",
        Split = "split",
        CreateScheduledSpan = "create_scheduled_span",
        DeleteScheduledSpan = "delete_scheduled_span",
    }
}

enum_label! {
    /// Target kind for a proposed or applied change.
    pub enum TargetKind {
        #[default] Task = "task",
        Habit = "habit",
        Skill = "skill",
        Memory = "memory",
        Schedule = "schedule",
    }
}

/// A typed target identifier, preserving the JSON representation `"task #42"`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Target {
    pub kind: TargetKind,
    pub display_id: String,
}

impl Target {
    pub fn new(kind: TargetKind, display_id: impl Into<String>) -> Self {
        Self {
            kind,
            display_id: display_id.into(),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.display_id.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{} {}", self.kind, self.display_id)
        }
    }
}

impl FromStr for Target {
    type Err = UnknownLabel;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim_start();
        if let Some((kind, rest)) = s.split_once(char::is_whitespace) {
            Ok(Self::new(kind.parse()?, rest.trim_start().to_owned()))
        } else {
            Ok(Self::new(s.parse()?, String::new()))
        }
    }
}

impl Serialize for Target {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Flattened target fields inside `ChangeReceipt`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiptTarget {
    #[serde(with = "takusu_types::enum_serde")]
    pub target_type: TargetKind,
    pub target_id: String,
}

impl ToolError {
    /// Errors that the LLM can correct by adjusting its request.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            ToolError::InvalidArgs(_)
                | ToolError::NotFound(_)
                | ToolError::Conflict(_)
                | ToolError::Cancelled
        )
    }

    /// Returns a compact, LLM-facing description of the error.
    ///
    /// Includes the tool name and a short retry hint while keeping the
    /// message small so it does not dominate the context window.
    pub fn to_llm_content(&self, tool_name: &str) -> String {
        let hint = match self {
            ToolError::InvalidArgs(_) => "check arguments",
            ToolError::NotFound(_) => "verify id",
            ToolError::Conflict(_) => "retry with latest",
            ToolError::Cancelled => "ask user",
            ToolError::Other(_) => "unexpected error",
        };
        format!("{tool_name}: {self} [{hint}]")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProposedChange {
    #[serde(with = "takusu_types::enum_serde")]
    pub operation: ChangeOperation,
    #[serde(rename = "target_label")]
    pub target: Target,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalDecision {
    pub proposal_id: String,
    pub approve: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InferredField {
    /// Name of the inferred field.
    pub field: String,
    /// Inferred value for the field.
    pub value: Value,
    /// Reason the field was inferred.
    pub reason: String,
}

pub fn inferred_field_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "field": {"type": "string", "description": "Name of the inferred field."},
            "value": {"description": "Inferred value for the field."},
            "reason": {"type": "string", "description": "Reason the field was inferred."}
        },
        "required": ["field", "value", "reason"],
        "additionalProperties": false
    })
}

pub fn inferred_fields_schema(description: &str) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": inferred_field_schema()
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChangeReceipt {
    #[serde(with = "takusu_types::enum_serde")]
    pub operation: ChangeOperation,
    #[serde(flatten)]
    pub target: ReceiptTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_revision: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_fields: Option<Value>,
}

/// Content payload for proposal-generating tools, serialized into
/// [`ToolOutput::content`].
///
/// Replaces the ad-hoc `json!({ "approval_required": true, "target": ... })`
/// blocks that were duplicated across mutation, memory, skill, and progress
/// tools. The struct makes the wire shape visible at the type level and
/// catches field-name typos at compile time.
#[derive(Debug, Serialize)]
pub struct ProposalContent {
    pub approval_required: bool,
    pub target: String,
}

impl ProposalContent {
    /// Build the standard proposal content for `target`.
    pub fn new(target: impl fmt::Display) -> Self {
        Self {
            approval_required: true,
            target: target.to_string(),
        }
    }

    /// Serialize to the JSON string stored in [`ToolOutput::content`].
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    /// JSON or text returned to the LLM.
    pub content: String,
    /// Short user-facing explanation supplied by the model for an approval request.
    pub why: Option<String>,
    pub warnings: Vec<String>,
    /// Planner writes proposed for application-level approval.
    pub proposed_changes: Vec<ProposedChange>,
    pub inferred_fields: Vec<InferredField>,
    /// Change receipts collected for the application UI.
    pub changes: Vec<ChangeReceipt>,
    /// Tool names discovered by `tool_search` that should become active this turn.
    pub discovered_tools: Vec<String>,
    pub schedule_dirty: bool,
    /// Whether this result represents an error the LLM should correct.
    pub is_error: bool,
}

/// OpenAI function-calling tool definition (what we send to the API).
#[derive(Debug, Clone, Serialize)]
pub struct OpenAITool {
    pub function: OpenAIToolFunction,
    #[serde(rename = "type")]
    pub type_: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAIToolFunction {
    pub description: String,
    pub name: String,
    pub parameters: Value,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON Schema for the arguments object (OpenAI function-calling format).
    fn parameters_schema(&self) -> Value;
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError>;

    /// How this tool should be exposed to the model.
    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    /// Call with the tool-call id from the LLM provider.
    ///
    /// Tools that need to correlate with host UI events (e.g. `correct_asr`)
    /// should override this. The default delegates to `call`.
    async fn call_with_id(&self, _id: &str, args: Value) -> Result<ToolOutput, ToolError> {
        self.call(args).await
    }

    /// Returns the tool definition in the OpenAI function-calling format.
    fn to_openai_definition(&self) -> OpenAITool {
        OpenAITool {
            type_: "function",
            function: OpenAIToolFunction {
                name: self.name().to_string(),
                description: self.description().to_string(),
                parameters: self.parameters_schema(),
            },
        }
    }
}

/// Type-safe tool trait with a compile-time-known argument shape.
///
/// `TypedTool` is **not** object-safe (it has an associated type), so the
/// [`ToolRegistry`] cannot hold `Box<dyn TypedTool>`. Instead, wrap a
/// `TypedTool` in [`Typed`] and register that as `Box<dyn Tool>`:
///
/// ```ignore
/// registry.register(Box::new(Typed(MyTool { client })));
/// ```
///
/// The wrapper deserializes the incoming `Value` into `Self::Params`, runs
/// [`TypedTool::validate_args`], and forwards the typed value to `call_typed`.
#[async_trait::async_trait]
pub trait TypedTool: Send + Sync {
    type Params: serde::de::DeserializeOwned + schemars::JsonSchema + Send + Sync;

    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    /// JSON Schema for the arguments object. Defaults to a schemars-generated
    /// schema for [`Self::Params`], normalized to match the hand-written
    /// schema style used by the existing tools (no `$schema`, no `title`,
    /// no `format`, `Option<T>` rendered as `T`).
    ///
    /// Override to post-process further (e.g. attach descriptions that cannot
    /// be expressed via doc comments). When overriding, call
    /// [`TypedTool::default_parameters_schema`] to get the generated base
    /// schema instead of calling `parameters_schema` (which would recurse).
    fn parameters_schema(&self) -> Value {
        self.default_parameters_schema()
    }

    /// The schemars-generated, normalized schema for [`Self::Params`].
    /// Intended to be called from `parameters_schema` overrides.
    fn default_parameters_schema(&self) -> Value {
        use schemars::generate::{SchemaGenerator, SchemaSettings};
        let mut settings = SchemaSettings::default();
        settings.inline_subschemas = true;
        let mut generator = SchemaGenerator::new(settings);
        let schema = <Self::Params as schemars::JsonSchema>::json_schema(&mut generator);
        normalize_schema(schema.to_value())
    }

    /// Cross-field validation that serde cannot express (e.g. non-empty
    /// strings, value ranges). Defaults to no extra checks. Runs after
    /// deserialization and before `call_typed`.
    fn validate_args(&self, _args: &Self::Params) -> Result<(), InvalidArgsError> {
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError>;

    async fn call_typed_with_id(
        &self,
        _id: &str,
        args: Self::Params,
    ) -> Result<ToolOutput, ToolError> {
        self.call_typed(args).await
    }
}

/// Normalize a schemars-generated schema to match the hand-written style.
///
/// Strips:
/// - `$schema` (the agent doesn't use a JSON Schema dialect URL)
/// - `title` (struct name — not useful to the LLM and adds tokens)
/// - `format` (e.g. `int64` — OpenAI function-calling ignores it)
/// - `assertionLine` / `description` on the root object when it is just the
///   doc-comment of the params struct (the per-field descriptions are kept)
/// - `default` (schemars emits it for `#[serde(default)]` fields; the
///   hand-written schemas never carried it)
///
/// Converts `Option<T>` rendered as `["T", "null"]` back to just `"T"`,
/// matching the previous hand-written schemas where optional fields were
/// typed as their inner type.
pub fn normalize_schema(mut schema: Value) -> Value {
    // Remove root-level keys that schemars adds but the hand-written schemas
    // never carried. Field-level descriptions inside `properties` are kept.
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("description");
        obj.remove("title");
        // schemars omits `properties` for structs with no fields; the
        // hand-written schemas always included `"properties": {}`.
        if !obj.contains_key("properties") {
            obj.insert("properties".into(), Value::Object(Default::default()));
        }
    }
    normalize_value(&mut schema);
    schema
}

fn normalize_value(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        // Strip keys that are not present in the hand-written schemas.
        obj.remove("$schema");
        obj.remove("format");
        obj.remove("assertionLine");
        // schemars emits `default` for `#[serde(default)]` fields; the
        // hand-written schemas never carried it.
        obj.remove("default");

        // Collapse `["integer", "null"]` / `["string", "null"]` etc. back to
        // the non-null variant so the LLM sees the same shape as before.
        // schemars renders `Option<T>` as a `type` array containing the
        // string `"null"`, not `Value::Null`. We do NOT add `default: null`
        // because the hand-written schemas never had it.
        if let Some(t) = obj.get_mut("type").and_then(Value::as_array_mut) {
            let has_null = t.iter().any(|v| v.as_str() == Some("null"));
            if t.len() == 2 && has_null {
                let non_null = t.iter().find(|v| v.as_str() != Some("null")).cloned();
                if let Some(inner) = non_null {
                    obj.insert("type".into(), inner);
                }
            }
        }

        // schemars includes `null` in the `enum` array for `Option<Enum>`
        // fields. After collapsing the type above, strip `null` from `enum`
        // so the LLM sees only the valid non-null variants.
        if let Some(e) = obj.get_mut("enum").and_then(Value::as_array_mut) {
            e.retain(|v| !v.is_null());
        }

        // Recurse into every child value. `properties` is a map of
        // field-name → schema, so we must walk each field's schema too.
        for child in obj.values_mut() {
            normalize_value(child);
        }
    } else if let Some(arr) = value.as_array_mut() {
        for item in arr.iter_mut() {
            normalize_value(item);
        }
    }
}

/// Wrapper that adapts a [`TypedTool`] into the object-safe [`Tool`] trait.
///
/// Registered tools should be `Box::new(Typed(tool))` so the registry can keep
/// using `Box<dyn Tool>`. This allows migrating tools one at a time without a
/// blanket `impl` that would collide with hand-written `impl Tool` blocks.
pub struct Typed<T: TypedTool>(pub T);

#[async_trait::async_trait]
impl<T: TypedTool> Tool for Typed<T> {
    fn name(&self) -> &'static str {
        TypedTool::name(&self.0)
    }

    fn description(&self) -> &'static str {
        TypedTool::description(&self.0)
    }

    fn exposure(&self) -> ToolExposure {
        TypedTool::exposure(&self.0)
    }

    fn parameters_schema(&self) -> Value {
        TypedTool::parameters_schema(&self.0)
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let params = self.parse_params(args)?;
        self.0.call_typed(params).await
    }

    async fn call_with_id(&self, id: &str, args: Value) -> Result<ToolOutput, ToolError> {
        let params = self.parse_params(args)?;
        self.0.call_typed_with_id(id, params).await
    }
}

impl<T: TypedTool> Typed<T> {
    fn parse_params(&self, args: Value) -> Result<T::Params, ToolError> {
        let params: T::Params = serde_path_to_error::deserialize(args).map_err(|e| {
            let path = e.path().to_string();
            let reason = e.into_inner().to_string();
            if path.is_empty() {
                ToolError::InvalidArgs(InvalidArgsError::no_field(reason))
            } else {
                ToolError::InvalidArgs(InvalidArgsError::new(path, reason))
            }
        })?;
        self.0.validate_args(&params)?;
        Ok(params)
    }
}

fn estimate_tool_tokens(defs: &[OpenAITool]) -> usize {
    defs.iter()
        .map(|d| crate::llm::estimate_text_tokens(&serde_json::to_string(d).unwrap_or_default()))
        .sum()
}

/// Search index entry for a deferred tool.
#[derive(Clone)]
pub(crate) struct SearchEntry {
    pub(crate) name: String,
    pub(crate) name_lower: String,
    pub(crate) description_lower: String,
    pub(crate) param_names: Vec<String>,
    pub(crate) definition: OpenAITool,
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    definitions_cache: Mutex<Option<(Vec<OpenAITool>, usize)>>,
    search_index: Mutex<Option<Vec<SearchEntry>>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: HashMap::new(),
            definitions_cache: Mutex::new(None),
            search_index: Mutex::new(None),
        }
    }
}

/// Score an entry against a list of lowercased query words.
///
/// Returns `None` if any word does not match. Matches are evaluated at the
/// token level (split by non-alphanumeric characters) so that a query like
/// "art" does not spuriously match the middle of "start".
fn score_entry(entry: &SearchEntry, words: &[&str]) -> Option<i64> {
    let mut total = 0i64;
    for word in words {
        let word_score = score_word(entry, word);
        if word_score == 0 {
            return None;
        }
        total += word_score;
    }
    Some(total)
}

fn score_word(entry: &SearchEntry, word: &str) -> i64 {
    let name_score = if entry.name_lower == word {
        100
    } else {
        token_score(&entry.name_lower, word, 100, 50)
    };
    name_score
        .max(token_score(&entry.description_lower, word, 40, 20))
        .max(
            entry
                .param_names
                .iter()
                .map(|p| token_score(p, word, 30, 15))
                .max()
                .unwrap_or(0),
        )
}

/// Score `word` against the tokens in `haystack`.
///
/// `haystack` is lowercased. Tokens are split by non-alphanumeric characters,
/// which treats underscores in snake_case names as word boundaries.
/// Returns the best of:
/// - `exact_score` if a token equals `word`
/// - `prefix_score` if a token starts with `word`
fn token_score(haystack: &str, word: &str, exact_score: i64, prefix_score: i64) -> i64 {
    if haystack.is_empty() || word.is_empty() {
        return 0;
    }

    let mut best = 0i64;
    for token in haystack.split(|c: char| !c.is_alphanumeric()) {
        if token == word {
            return exact_score;
        }
        if best < prefix_score && token.starts_with(word) {
            best = prefix_score;
        }
    }
    best
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
        // Cache invalidation: a poisoned guard still holds the cache value,
        // so recovering via `into_inner()` and resetting to `None` is safe
        // (worst case the cache is rebuilt on next access).
        *self
            .definitions_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self.search_index.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Names of all registered tools, sorted alphabetically (including
    /// `Hidden` tools). Use [`ToolRegistry::exposed_tool_names`] when you
    /// need only the tools visible to the model.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.tools.keys().cloned().collect();
        names.sort();
        names
    }

    /// Names of tools that are exposed to the model (i.e. not `Hidden`),
    /// sorted alphabetically.
    pub fn exposed_tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tools
            .iter()
            .filter(|(_, t)| !matches!(t.exposure(), ToolExposure::Hidden))
            .map(|(n, _)| n.clone())
            .collect();
        names.sort();
        names
    }

    /// OpenAI function-calling definition for a single tool by name.
    ///
    /// Returns `None` for unknown names and for `Hidden` tools (which are never
    /// exposed to the model).
    pub fn definition_for_name(&self, name: &str) -> Option<OpenAITool> {
        self.tools
            .get(name)
            .filter(|t| !matches!(t.exposure(), ToolExposure::Hidden))
            .map(|t| t.to_openai_definition())
    }

    fn build_definitions(&self, active_names: Option<&BTreeSet<String>>) -> Vec<OpenAITool> {
        match active_names {
            Some(names) => names
                .iter()
                .filter_map(|n| {
                    self.tools
                        .get(n)
                        .filter(|t| !matches!(t.exposure(), ToolExposure::Hidden))
                        .map(|t| t.to_openai_definition())
                })
                .collect(),
            None => {
                let mut tools: Vec<_> = self.tools.values().collect();
                tools.sort_by(|a, b| a.name().cmp(b.name()));
                tools
                    .into_iter()
                    .filter(|t| !matches!(t.exposure(), ToolExposure::Hidden))
                    .map(|t| t.to_openai_definition())
                    .collect()
            }
        }
    }

    pub fn schemas(&self) -> Vec<Value> {
        self.tools.values().map(|t| t.parameters_schema()).collect()
    }

    /// Names of tools that should always be visible to the model.
    pub fn direct_tool_names(&self) -> BTreeSet<String> {
        self.tools
            .values()
            .filter(|t| matches!(t.exposure(), ToolExposure::Direct))
            .map(|t| t.name().to_string())
            .collect()
    }

    /// Tool definitions in OpenAI function-calling format for the given active set.
    pub fn definitions_for(&self, active_names: &BTreeSet<String>) -> Vec<OpenAITool> {
        self.build_definitions(Some(active_names))
    }

    /// Tool definitions in OpenAI function-calling format for all registered tools.
    pub fn definitions(&self) -> Vec<OpenAITool> {
        {
            let guard = self
                .definitions_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((defs, _)) = guard.as_ref() {
                return defs.clone();
            }
        }
        let defs = self.build_definitions(None);
        let tokens = estimate_tool_tokens(&defs);
        let mut guard = self
            .definitions_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some((defs.clone(), tokens));
        }
        guard.as_ref().unwrap().0.clone()
    }

    /// Rough token estimate for an active tool set.
    pub fn definitions_estimate_tokens_for(&self, active_names: &BTreeSet<String>) -> usize {
        estimate_tool_tokens(&self.build_definitions(Some(active_names)))
    }

    /// Rough token estimate for all tool definitions, using the same heuristic
    /// as `llm::Message::estimate_tokens` (4 chars per token + overhead).
    pub fn definitions_estimate_tokens(&self) -> usize {
        {
            let guard = self
                .definitions_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((_, tokens)) = guard.as_ref() {
                return *tokens;
            }
        }
        let defs = self.build_definitions(None);
        let tokens = estimate_tool_tokens(&defs);
        let mut guard = self
            .definitions_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some((defs, tokens));
        }
        guard.as_ref().unwrap().1
    }

    pub(crate) fn build_search_index(&self) -> Vec<SearchEntry> {
        {
            let guard = self.search_index.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(index) = guard.as_ref() {
                return index.clone();
            }
        }

        let mut index = Vec::new();
        for tool in self.tools.values() {
            if !matches!(tool.exposure(), ToolExposure::Deferred) {
                continue;
            }
            let name = tool.name().to_string();
            let description = tool.description().to_string();
            let mut param_names = Vec::new();
            if let Some(properties) = tool
                .parameters_schema()
                .get("properties")
                .and_then(Value::as_object)
            {
                for name in properties.keys() {
                    param_names.push(name.to_lowercase());
                }
            }
            index.push(SearchEntry {
                name_lower: name.to_lowercase(),
                name,
                description_lower: description.to_lowercase(),
                param_names,
                definition: tool.to_openai_definition(),
            });
        }
        index.sort_by(|a, b| a.name.cmp(&b.name));

        let mut guard = self.search_index.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(index.clone());
        }
        guard.as_ref().unwrap().clone()
    }

    /// Search deferred tools by name, description, and parameter names.
    ///
    /// Results are ranked by match quality: exact/prefix/word matches in the
    /// tool name are preferred, followed by matches in the description and
    /// parameter names. The `limit` is applied after ranking, not after sorting
    /// alphabetically.
    pub(crate) fn search(&self, query: &str, limit: Option<usize>) -> Vec<SearchEntry> {
        let index = self.build_search_index();
        let query = query.to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(i64, &SearchEntry)> = Vec::new();
        for entry in &index {
            if let Some(score) = score_entry(entry, &words) {
                scored.push((score, entry));
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

        let limit = limit.unwrap_or(usize::MAX);
        scored
            .into_iter()
            .take(limit)
            .map(|(_, e)| e.clone())
            .collect()
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<ToolOutput, ToolError> {
        let tool = self.tools.get(name).ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new("tool", format!("unknown: {name}")))
        })?;
        if matches!(tool.exposure(), ToolExposure::Hidden) {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "tool",
                format!("{name} is not available"),
            )));
        }
        tool.call(args).await
    }

    pub async fn call_with_id(
        &self,
        name: &str,
        call_id: &str,
        args: Value,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self.tools.get(name).ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new("tool", format!("unknown: {name}")))
        })?;
        if matches!(tool.exposure(), ToolExposure::Hidden) {
            return Err(ToolError::InvalidArgs(InvalidArgsError::new(
                "tool",
                format!("{name} is not available"),
            )));
        }
        tool.call_with_id(call_id, args).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::tools::tool_search::ToolSearch;

    struct DummyTool;

    #[async_trait::async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &'static str {
            "dummy"
        }

        fn description(&self) -> &'static str {
            "A dummy tool for testing"
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::default())
        }
    }

    struct TestTool {
        name: &'static str,
        exposure: ToolExposure,
    }

    #[async_trait::async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn description(&self) -> &'static str {
            "a test tool"
        }

        fn exposure(&self) -> ToolExposure {
            self.exposure
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {}
            })
        }

        async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::default())
        }
    }

    #[test]
    fn definitions_estimate_tokens_is_non_zero_and_consistent() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DummyTool));

        let first = registry.definitions_estimate_tokens();
        assert!(first > 0);

        let defs = registry.definitions();
        assert_eq!(defs.len(), 1);

        let second = registry.definitions_estimate_tokens();
        assert_eq!(second, first);
    }

    #[test]
    fn empty_registry_has_zero_tool_tokens() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.definitions_estimate_tokens(), 0);
        assert!(registry.definitions().is_empty());
    }

    #[test]
    fn invalid_args_error_display_includes_field_and_reason() {
        let with_field = InvalidArgsError::new("message", "missing");
        assert_eq!(with_field.to_string(), "field 'message': missing");

        let no_field = InvalidArgsError::no_field("bad args");
        assert_eq!(no_field.to_string(), "bad args");
    }

    #[test]
    fn tool_error_to_llm_content_is_compact_and_includes_hint() {
        let err = ToolError::InvalidArgs(InvalidArgsError::new("message", "missing"));
        assert_eq!(
            err.to_llm_content("echo"),
            "echo: invalid arguments: field 'message': missing [check arguments]"
        );

        let not_found = ToolError::NotFound("task #42".into());
        assert!(not_found.to_llm_content("get_task").contains("verify id"));

        let conflict = ToolError::Conflict("task #42".into());
        assert!(
            conflict
                .to_llm_content("update_task")
                .contains("retry with latest")
        );

        let cancelled = ToolError::Cancelled;
        assert!(cancelled.to_llm_content("create_task").contains("ask user"));
    }

    #[test]
    fn default_tool_exposure_is_direct() {
        let tool = DummyTool;
        assert_eq!(tool.exposure(), ToolExposure::Direct);
    }

    #[test]
    fn direct_tool_names_only_includes_direct() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            name: "direct_tool",
            exposure: ToolExposure::Direct,
        }));
        registry.register(Box::new(TestTool {
            name: "deferred_tool",
            exposure: ToolExposure::Deferred,
        }));
        registry.register(Box::new(TestTool {
            name: "hidden_tool",
            exposure: ToolExposure::Hidden,
        }));

        let names = registry.direct_tool_names();
        assert!(names.contains("direct_tool"));
        assert!(!names.contains("deferred_tool"));
        assert!(!names.contains("hidden_tool"));
    }

    #[test]
    fn definitions_are_sorted_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            name: "zzz_tool",
            exposure: ToolExposure::Direct,
        }));
        registry.register(Box::new(TestTool {
            name: "aaa_tool",
            exposure: ToolExposure::Direct,
        }));
        registry.register(Box::new(TestTool {
            name: "mmm_tool",
            exposure: ToolExposure::Deferred,
        }));

        let defs = registry.definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert_eq!(names, vec!["aaa_tool", "mmm_tool", "zzz_tool"]);
    }

    #[test]
    fn definitions_for_filters_active_names_and_hides_hidden() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            name: "direct_tool",
            exposure: ToolExposure::Direct,
        }));
        registry.register(Box::new(TestTool {
            name: "deferred_tool",
            exposure: ToolExposure::Deferred,
        }));
        registry.register(Box::new(TestTool {
            name: "hidden_tool",
            exposure: ToolExposure::Hidden,
        }));

        let active = BTreeSet::from([
            "direct_tool".to_string(),
            "deferred_tool".to_string(),
            "hidden_tool".to_string(),
        ]);
        let defs = registry.definitions_for(&active);
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        assert_eq!(names, vec!["deferred_tool", "direct_tool"]);
    }

    #[test]
    fn search_returns_deferred_tools_matching_keywords() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TestTool {
            name: "similar_tasks",
            exposure: ToolExposure::Deferred,
        }));
        registry.register(Box::new(TestTool {
            name: "memory_search",
            exposure: ToolExposure::Deferred,
        }));
        registry.register(Box::new(TestTool {
            name: "direct_tool",
            exposure: ToolExposure::Direct,
        }));

        let similar = registry.search("similar", None);
        assert_eq!(similar.len(), 1);
        assert_eq!(similar[0].name, "similar_tasks");

        let memory = registry.search("memory", None);
        assert_eq!(memory.len(), 1);
        assert_eq!(memory[0].name, "memory_search");

        let all = registry.search("tool", None);
        assert_eq!(all.len(), 2);

        let empty = registry.search("xyz", None);
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn tool_search_default_limit_is_five() {
        let registry = Arc::new_cyclic(|weak| {
            let mut registry = ToolRegistry::new();
            for i in 0..10 {
                registry.register(Box::new(TestTool {
                    name: Box::leak(format!("tool_{i}").into_boxed_str()),
                    exposure: ToolExposure::Deferred,
                }));
            }
            registry.register(Box::new(Typed(ToolSearch::from_registry(weak.clone()))));
            registry
        });

        let tool_search = Typed(ToolSearch::from_registry(Arc::downgrade(&registry)));
        let output = tool_search.call(json!({"query": "tool"})).await.unwrap();

        assert_eq!(output.discovered_tools.len(), 5);
        let parsed: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(parsed["count"].as_u64().unwrap(), 5);
    }

    #[tokio::test]
    async fn tool_search_limit_can_be_overridden() {
        let registry = Arc::new_cyclic(|weak| {
            let mut registry = ToolRegistry::new();
            for i in 0..10 {
                registry.register(Box::new(TestTool {
                    name: Box::leak(format!("tool_{i}").into_boxed_str()),
                    exposure: ToolExposure::Deferred,
                }));
            }
            registry.register(Box::new(Typed(ToolSearch::from_registry(weak.clone()))));
            registry
        });

        let tool_search = Typed(ToolSearch::from_registry(Arc::downgrade(&registry)));
        let output = tool_search
            .call(json!({"query": "tool", "limit": 3}))
            .await
            .unwrap();

        assert_eq!(output.discovered_tools.len(), 3);
    }

    #[test]
    fn proposed_change_serialization_preserves_existing_json() {
        let change = ProposedChange {
            operation: ChangeOperation::Update,
            target: Target::new(TargetKind::Task, "#42"),
            description: "update task".to_string(),
            before: None,
            after: Some(json!({"title": "new title"})),
            arguments: Some(json!({"task_ref": "42"})),
            observed_updated_at: Some("2025-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };
        let expected = r#"{"operation":"update","target_label":"task #42","description":"update task","after":{"title":"new title"},"arguments":{"task_ref":"42"},"observed_updated_at":"2025-01-01T00:00:00Z"}"#;
        assert_eq!(serde_json::to_string(&change).unwrap(), expected);

        let parsed: ProposedChange = serde_json::from_str(expected).unwrap();
        assert_eq!(parsed.operation, ChangeOperation::Update);
        assert_eq!(parsed.target, Target::new(TargetKind::Task, "#42"));

        let schedule = ProposedChange {
            operation: ChangeOperation::Generate,
            target: Target::new(TargetKind::Schedule, ""),
            description: "generate schedule".to_string(),
            before: None,
            after: None,
            arguments: None,
            observed_updated_at: None,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&schedule).unwrap(),
            r#"{"operation":"generate","target_label":"schedule","description":"generate schedule"}"#
        );
    }

    #[test]
    fn change_receipt_serialization_preserves_existing_json() {
        let receipt = ChangeReceipt {
            operation: ChangeOperation::Update,
            target: ReceiptTarget {
                target_type: TargetKind::Task,
                target_id: "task-uuid".to_string(),
            },
            before: None,
            after: Some(json!({"title": "new title"})),
            target_revision: Some(3),
            inferred_fields: None,
        };
        let expected = r#"{"operation":"update","target_type":"task","target_id":"task-uuid","after":{"title":"new title"},"target_revision":3}"#;
        assert_eq!(serde_json::to_string(&receipt).unwrap(), expected);

        let parsed: ChangeReceipt = serde_json::from_str(expected).unwrap();
        assert_eq!(parsed.operation, ChangeOperation::Update);
        assert_eq!(parsed.target.target_type, TargetKind::Task);
        assert_eq!(parsed.target.target_id, "task-uuid");
    }

    #[test]
    fn target_round_trips_display_id_with_spaces() {
        let target = Target::new(TargetKind::Task, "some long title");
        let s = target.to_string();
        assert_eq!(s, "task some long title");
        let parsed: Target = s.parse().unwrap();
        assert_eq!(parsed, target);
    }
}
