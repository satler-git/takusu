use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::tools::{ToolContext, ToolModule};
use crate::{
    ToolError, ToolName, ToolOutput, ToolRegistry, TypedTool, deserialize_trimmed_required,
};

pub struct CaptureModule;

impl ToolModule for CaptureModule {
    fn register(&self, registry: &mut ToolRegistry, _ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(GapCaptureCheckIn)));
    }
}

static CAPTURE_MODULE: &dyn ToolModule = &CaptureModule;

inventory::submit!(CAPTURE_MODULE);

/// Present a classification check-in for an unclassified gap capture.
///
/// The user has already answered the "今なにしてる？" check-in with a
/// free-form activity. When the answer does not determine whether the activity
/// is one-off, recurring, free time, or routine, this tool returns a
/// `CheckInCard` that offers those four outcomes. The next user answer is then
/// routed to the matching `create_task`, `create_habit`, or `coverage_confirm`
/// call in a single turn.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GapCaptureCheckInArgs {
    /// The user's free-form description of the activity during the unclassified gap.
    #[serde(deserialize_with = "deserialize_trimmed_required")]
    #[schemars(with = "String")]
    activity: String,
}

struct GapCaptureCheckIn;

#[async_trait]
impl TypedTool for GapCaptureCheckIn {
    type Params = GapCaptureCheckInArgs;

    fn name(&self) -> &'static str {
        ToolName::GapCaptureCheckIn.into()
    }

    fn description(&self) -> &'static str {
        "Present a CheckInCard with one-off, recurring, free-time, and routine outcomes for an unclassified gap capture. Use when the user has described an activity during an unclassified gap and the classification is not yet clear. The `activity` is the user's free-form description. After this tool, wait for the user to choose one of the four options and then call the matching capture tool in the next turn."
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let content = json!({"activity": args.activity}).to_string();
        Ok(ToolOutput {
            content,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_capture_check_in_name_matches_tool_name() {
        let tool = GapCaptureCheckIn;
        assert_eq!(tool.name(), "gap_capture_check_in");
    }

    #[tokio::test]
    async fn call_typed_returns_activity_payload() {
        let tool = GapCaptureCheckIn;
        let args = GapCaptureCheckInArgs {
            activity: "バイトの引き継ぎ資料を作っている".into(),
        };
        let output = tool.call_typed(args).await.unwrap();
        assert!(output.content.contains("バイトの引き継ぎ資料"));
        assert!(output.proposed_changes.is_empty());
        assert!(!output.schedule_dirty);
    }

    #[test]
    fn deserialize_rejects_empty_activity() {
        assert!(serde_json::from_str::<GapCaptureCheckInArgs>(r#"{"activity":"   "}"#).is_err());
    }
}
