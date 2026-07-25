use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    pub operation: String,
    pub target_label: String,
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
    pub operation: String,
    pub target_type: String,
    pub target_id: String,
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

fn estimate_tool_tokens(defs: &[Value]) -> usize {
    defs.iter()
        .map(|d| crate::llm::estimate_text_tokens(&serde_json::to_string(d).unwrap_or_default()))
        .sum()
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    definitions_cache: Mutex<Option<(Vec<Value>, usize)>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: HashMap::new(),
            definitions_cache: Mutex::new(None),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
        *self.definitions_cache.lock().unwrap() = None;
    }

    fn build_definitions(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.parameters_schema(),
                    }
                })
            })
            .collect()
    }

    pub fn schemas(&self) -> Vec<Value> {
        self.tools.values().map(|t| t.parameters_schema()).collect()
    }

    /// Tool definitions in OpenAI function-calling format.
    pub fn definitions(&self) -> Vec<Value> {
        {
            let guard = self.definitions_cache.lock().unwrap();
            if let Some((defs, _)) = guard.as_ref() {
                return defs.clone();
            }
        }
        let defs = self.build_definitions();
        let tokens = estimate_tool_tokens(&defs);
        let mut guard = self.definitions_cache.lock().unwrap();
        if guard.is_none() {
            *guard = Some((defs.clone(), tokens));
        }
        guard.as_ref().unwrap().0.clone()
    }

    /// Rough token estimate for the tool definitions, using the same heuristic
    /// as `llm::Message::estimate_tokens` (4 chars per token + overhead).
    pub fn definitions_estimate_tokens(&self) -> usize {
        {
            let guard = self.definitions_cache.lock().unwrap();
            if let Some((_, tokens)) = guard.as_ref() {
                return *tokens;
            }
        }
        let defs = self.build_definitions();
        let tokens = estimate_tool_tokens(&defs);
        let mut guard = self.definitions_cache.lock().unwrap();
        if guard.is_none() {
            *guard = Some((defs, tokens));
        }
        guard.as_ref().unwrap().1
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<ToolOutput, ToolError> {
        let tool = self.tools.get(name).ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new("tool", format!("unknown: {name}")))
        })?;
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
        tool.call_with_id(call_id, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
