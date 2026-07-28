use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    InvalidArgsError, ToolError, ToolOutput, ToolRegistry, TypedTool, UserInputProvider,
    UserInputQuestion,
};

/// Ask the user to correct ambiguous ASR text.
///
/// The LLM should only call this when it cannot infer the correct text from
/// context (e.g. unknown proper nouns or homonyms). Group multiple questions
/// into a single call whenever possible.
pub struct CorrectAsr {
    provider: Arc<dyn UserInputProvider>,
}

impl CorrectAsr {
    pub fn new(provider: Arc<dyn UserInputProvider>) -> Self {
        Self { provider }
    }
}

/// Arguments for [`CorrectAsr`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorrectAsrArgs {
    /// List of ambiguous ASR snippets to ask the user to correct. Group multiple questions into one call.
    pub questions: Vec<UserInputQuestion>,
}

#[async_trait]
impl TypedTool for CorrectAsr {
    type Params = CorrectAsrArgs;

    fn name(&self) -> &'static str {
        "correct_asr"
    }

    fn description(&self) -> &'static str {
        "Ask the user to correct ambiguous ASR text. First state your interpretation. Infer obvious ASR errors from context without asking; use this tool only when context is insufficient to disambiguate proper nouns, homonyms, numbers, dates, days of the week, or the action target. Group multiple questions into one call."
    }

    fn validate_args(&self, args: &Self::Params) -> Result<(), InvalidArgsError> {
        if args.questions.is_empty() {
            return Err(InvalidArgsError::new("questions", "must not be empty"));
        }
        Ok(())
    }

    async fn call_typed(&self, _args: Self::Params) -> Result<ToolOutput, ToolError> {
        Err(ToolError::InvalidArgs(InvalidArgsError::new(
            "call_id",
            "correct_asr requires a tool-call id; use call_with_id",
        )))
    }

    async fn call_typed_with_id(
        &self,
        call_id: &str,
        args: Self::Params,
    ) -> Result<ToolOutput, ToolError> {
        let answers = self.provider.request(call_id, args.questions).await?;
        let content = serde_json::to_string(&answers).map_err(|e| ToolError::Other(Box::new(e)))?;

        Ok(ToolOutput {
            content,
            ..Default::default()
        })
    }
}

/// Registers the `correct_asr` user-input tool.
pub fn register_user_input_tool(registry: &mut ToolRegistry, provider: Arc<dyn UserInputProvider>) {
    registry.register(Box::new(crate::tool::Typed(CorrectAsr::new(provider))));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Typed;
    use crate::{StubUserInputProvider, Tool, UserInputAnswer, UserInputProvider};
    use serde_json::json;

    #[derive(Debug)]
    struct TestProvider {
        suffix: String,
    }

    #[async_trait]
    impl UserInputProvider for TestProvider {
        async fn request(
            &self,
            _call_id: &str,
            questions: Vec<UserInputQuestion>,
        ) -> Result<Vec<UserInputAnswer>, ToolError> {
            Ok(questions
                .into_iter()
                .map(|q| UserInputAnswer {
                    text: format!("{}-{}", q.text, self.suffix),
                })
                .collect())
        }

        async fn resolve(
            &self,
            _call_id: &str,
            _answers: Vec<UserInputAnswer>,
        ) -> Result<(), ToolError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn correct_asr_returns_provider_answers_as_json() {
        let tool = Typed(CorrectAsr::new(Arc::new(TestProvider {
            suffix: "fixed".into(),
        })));
        let args = json!({
            "questions": [
                { "text": "kore", "for": "test" },
            ]
        });
        let output = tool.call_with_id("call-1", args).await.unwrap();
        let parsed: Vec<UserInputAnswer> = serde_json::from_str(&output.content).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].text, "kore-fixed");
    }

    #[tokio::test]
    async fn correct_asr_rejects_empty_questions() {
        let tool = Typed(CorrectAsr::new(Arc::new(TestProvider {
            suffix: "fixed".into(),
        })));
        let args = json!({ "questions": [] });
        let result = tool.call_with_id("call-1", args).await;
        assert!(matches!(result, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn correct_asr_rejects_missing_questions() {
        let tool = Typed(CorrectAsr::new(Arc::new(TestProvider {
            suffix: "fixed".into(),
        })));
        let result = tool.call_with_id("call-1", json!({})).await;
        assert!(matches!(result, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn stub_provider_returns_original_text() {
        let provider = StubUserInputProvider;
        let answers = provider
            .request(
                "call-1",
                vec![UserInputQuestion {
                    text: "kore".into(),
                    purpose: "test".into(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(answers[0].text, "kore");
    }
}
