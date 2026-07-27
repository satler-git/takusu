use std::sync::Weak;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::tools::takusu::{object, optional_i64, required_string};
use crate::{InvalidArgsError, Tool, ToolError, ToolExposure, ToolOutput, ToolRegistry};

/// Search tool for discovering deferred tools.
///
/// The agent keeps a small set of Direct tools in each request. When the model
/// needs a less-common tool, it calls `tool_search` with keywords; the result
/// includes matching tool definitions and the discovered tool names are added
/// to the active set for the current turn.
pub(crate) struct ToolSearch {
    registry: Weak<ToolRegistry>,
}

impl ToolSearch {
    pub(crate) fn from_registry(registry: Weak<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for ToolSearch {
    fn name(&self) -> &'static str {
        "tool_search"
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn description(&self) -> &'static str {
        "Search for less-frequently used tools by name, description, or parameters. \
         When a task needs a tool not in the current list, call this first with relevant \
         keywords (e.g. 'memory', 'skill', 'progress', 'reschedule') to discover it."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords describing the tool or task. Include tool names, nouns, or verbs. Examples: 'skill list', 'memory search', 'task progress', 'reschedule schedule'."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of tools to return (default 5).",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let registry = self.registry.upgrade().ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new("tool_search", "registry unavailable"))
        })?;
        let args = object(args)?;
        let query = required_string(&args, "query")?;
        let limit = optional_i64(&args, "limit")?
            .map(|n| (n as usize).clamp(1, 20))
            .unwrap_or(5);

        let entries = registry.search(&query, Some(limit));
        let definitions: Vec<Value> = entries.iter().map(|e| e.definition.clone()).collect();
        let discovered: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();

        Ok(ToolOutput {
            content: serde_json::to_string(&json!({
                "results": definitions,
                "count": definitions.len(),
            }))
            .unwrap_or_default(),
            discovered_tools: discovered,
            ..Default::default()
        })
    }
}
