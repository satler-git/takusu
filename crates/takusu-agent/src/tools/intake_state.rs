use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tools::{ToolContext, ToolModule, ToolRegistry};
use crate::{
    IntakeStage, IntakeState, InvalidArgsError, ToolError, ToolExposure, ToolName, ToolOutput,
    TypedTool, deserialize_trimmed_optional, deserialize_trimmed_vec,
};

pub struct SetIntakeStateModule;

impl ToolModule for SetIntakeStateModule {
    fn register(&self, registry: &mut ToolRegistry, _ctx: &ToolContext) {
        registry.register(Box::new(crate::tool::Typed(SetIntakeState)));
    }
}

static SET_INTAKE_STATE_MODULE: &dyn ToolModule = &SetIntakeStateModule;

inventory::submit!(SET_INTAKE_STATE_MODULE);

/// Update the resumable intake interview state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetIntakeStateArgs {
    /// Current stage in the fixed-order interview.
    stage: IntakeStage,
    /// `proposal_id` used to batch the current set of intake proposals.
    #[serde(default, deserialize_with = "deserialize_trimmed_optional")]
    proposal_id: Option<String>,
    /// Whether a coverage confirmation is waiting for the current batch to commit.
    #[serde(default)]
    coverage_pending: bool,
    /// Display ids of tasks/habits created so far in this intake.
    #[serde(default, deserialize_with = "deserialize_trimmed_vec")]
    collected_ids: Vec<String>,
}

struct SetIntakeState;

#[async_trait]
impl TypedTool for SetIntakeState {
    type Params = SetIntakeStateArgs;

    fn name(&self) -> &'static str {
        ToolName::SetIntakeState.into()
    }

    fn description(&self) -> &'static str {
        "Update the resumable intake interview state. The agent uses this to record the current stage, the proposal id used for the current batch, and the ids collected so far. No approval is required."
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if let Some(proposal_id) = args.proposal_id.as_ref()
            && proposal_id.is_empty()
        {
            return Err(InvalidArgsError::new(
                "proposal_id",
                "proposal_id must not be empty",
            ));
        }
        Ok(())
    }

    async fn call_typed(&self, args: Self::Params) -> Result<ToolOutput, ToolError> {
        let state = IntakeState {
            stage: args.stage,
            proposal_id: args.proposal_id,
            coverage_pending: args.coverage_pending,
            collected_ids: args.collected_ids,
        };
        Ok(ToolOutput {
            content: serde_json::to_string(&state).unwrap_or_default(),
            intake_state: Some(state),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_intake_state_name_matches_tool_name() {
        let tool = SetIntakeState;
        assert_eq!(tool.name(), "set_intake_state");
    }

    #[test]
    fn validate_args_rejects_empty_proposal_id() {
        let tool = SetIntakeState;
        let args = SetIntakeStateArgs {
            stage: IntakeStage::Deadlines,
            proposal_id: Some("".into()),
            coverage_pending: false,
            collected_ids: vec![],
        };
        assert!(tool.validate_args(&args).is_err());
    }

    #[tokio::test]
    async fn call_typed_returns_intake_state_in_output() {
        let tool = SetIntakeState;
        let output = tool
            .call_typed(SetIntakeStateArgs {
                stage: IntakeStage::Recurring,
                proposal_id: Some("p1".into()),
                coverage_pending: true,
                collected_ids: vec!["t1".into()],
            })
            .await
            .unwrap();
        let state = output.intake_state.unwrap();
        assert_eq!(state.stage, IntakeStage::Recurring);
        assert_eq!(state.proposal_id, Some("p1".into()));
        assert!(state.coverage_pending);
        assert_eq!(state.collected_ids, vec!["t1".to_string()]);
    }
}
