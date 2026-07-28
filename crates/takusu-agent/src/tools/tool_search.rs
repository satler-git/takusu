use std::sync::Weak;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::{InvalidArgsError, ToolError, ToolExposure, ToolOutput, ToolRegistry, TypedTool};

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

/// Arguments for [`ToolSearch`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolSearchParams {
    /// Keywords describing the tool or task. Include tool names, nouns, or verbs. Examples: 'skill list', 'memory search', 'task progress', 'reschedule schedule'.
    pub query: String,
    /// Maximum number of tools to return (default 5).
    #[serde(default)]
    #[schemars(range(min = 1, max = 20))]
    pub limit: Option<i64>,
}

#[async_trait]
impl TypedTool for ToolSearch {
    type Params = ToolSearchParams;

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

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let registry = self.registry.upgrade().ok_or_else(|| {
            ToolError::InvalidArgs(InvalidArgsError::new("tool_search", "registry unavailable"))
        })?;
        let limit = args.limit.map(|n| (n as usize).clamp(1, 20)).unwrap_or(5);

        let entries = registry.search(&args.query, Some(limit));
        let definitions: Vec<crate::tool::OpenAITool> =
            entries.iter().map(|e| e.definition.clone()).collect();
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
