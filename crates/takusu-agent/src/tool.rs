use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};
use std::str::FromStr;
use takusu_util::{UnknownLabel, enum_label};

/// Structured recoverable argument error passed back to the LLM.
///
/// Carrying the field separately from the reason lets the agent format
/// clearer retry guidance without exposing the entire argument object.
#[derive(Debug)]
pub struct InvalidArgsError {
    /// Name of the argument or field that is invalid, if known.
    pub field: Option<String>,
    /// Human-readable explanation of what is wrong.
    pub reason: String,
}

impl InvalidArgsError {
    /// Create an error for a specific field.
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: Some(field.into()),
            reason: reason.into(),
        }
    }

    /// Create an error without naming a specific field.
    pub fn no_field(reason: impl Into<String>) -> Self {
        Self {
            field: None,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for InvalidArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.field {
            Some(field) => write!(f, "field '{field}': {reason}", reason = self.reason),
            None => write!(f, "{}", self.reason),
        }
    }
}

impl std::error::Error for InvalidArgsError {}

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    #[serde(with = "takusu_util::enum_serde")]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedChange {
    #[serde(with = "takusu_util::enum_serde")]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferredField {
    pub field: String,
    pub value: Value,
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
    #[serde(with = "takusu_util::enum_serde")]
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

    /// Returns the tool name in the OpenAI function-calling format.
    fn to_openai_definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": self.description(),
                "parameters": self.parameters_schema(),
            }
        })
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
    /// schema for [`Self::Params`]. Override to post-process the generated
    /// schema (e.g. strip `title`/`$schema`, attach descriptions).
    fn parameters_schema(&self) -> Value {
        schemars::schema_for!(Self::Params).to_value()
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
        let params: T::Params = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArgs(InvalidArgsError::no_field(e.to_string())))?;
        self.0.validate_args(&params)?;
        Ok(params)
    }
}

fn estimate_tool_tokens(defs: &[Value]) -> usize {
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
    pub(crate) definition: Value,
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    definitions_cache: Mutex<Option<(Vec<Value>, usize)>>,
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
        *self.definitions_cache.lock().unwrap() = None;
        *self.search_index.lock().unwrap() = None;
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
    pub fn definition_for_name(&self, name: &str) -> Option<Value> {
        self.tools
            .get(name)
            .filter(|t| !matches!(t.exposure(), ToolExposure::Hidden))
            .map(|t| t.to_openai_definition())
    }

    fn build_definitions(&self, active_names: Option<&BTreeSet<String>>) -> Vec<Value> {
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
    pub fn definitions_for(&self, active_names: &BTreeSet<String>) -> Vec<Value> {
        self.build_definitions(Some(active_names))
    }

    /// Tool definitions in OpenAI function-calling format for all registered tools.
    pub fn definitions(&self) -> Vec<Value> {
        {
            let guard = self.definitions_cache.lock().unwrap();
            if let Some((defs, _)) = guard.as_ref() {
                return defs.clone();
            }
        }
        let defs = self.build_definitions(None);
        let tokens = estimate_tool_tokens(&defs);
        let mut guard = self.definitions_cache.lock().unwrap();
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
            let guard = self.definitions_cache.lock().unwrap();
            if let Some((_, tokens)) = guard.as_ref() {
                return *tokens;
            }
        }
        let defs = self.build_definitions(None);
        let tokens = estimate_tool_tokens(&defs);
        let mut guard = self.definitions_cache.lock().unwrap();
        if guard.is_none() {
            *guard = Some((defs, tokens));
        }
        guard.as_ref().unwrap().1
    }

    pub(crate) fn build_search_index(&self) -> Vec<SearchEntry> {
        {
            let guard = self.search_index.lock().unwrap();
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

        let mut guard = self.search_index.lock().unwrap();
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
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
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
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
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
            registry.register(Box::new(ToolSearch::from_registry(weak.clone())));
            registry
        });

        let tool_search = ToolSearch::from_registry(Arc::downgrade(&registry));
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
            registry.register(Box::new(ToolSearch::from_registry(weak.clone())));
            registry
        });

        let tool_search = ToolSearch::from_registry(Arc::downgrade(&registry));
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
