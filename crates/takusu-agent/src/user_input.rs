use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ToolError;

/// A single ambiguous ASR snippet the agent wants the user to correct.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserInputQuestion {
    /// The ASR-recognized text that may be wrong
    pub text: String,
    /// What this text is used for and why it is ambiguous
    #[serde(rename = "for")]
    pub purpose: String,
}

/// A user-supplied correction for one `UserInputQuestion`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserInputAnswer {
    /// The corrected text.
    pub text: String,
}

/// Bridge between a tool that needs human input and the host application.
///
/// The tool blocks on `request` until the application calls `resolve` with
/// the same `call_id`. This lets the agent turn keep its normal tool-call
/// loop while the mobile client shows an input sheet.
#[async_trait]
pub trait UserInputProvider: Send + Sync {
    async fn request(
        &self,
        call_id: &str,
        questions: Vec<UserInputQuestion>,
    ) -> Result<Vec<UserInputAnswer>, ToolError>;

    async fn resolve(&self, call_id: &str, answers: Vec<UserInputAnswer>) -> Result<(), ToolError>;
}

/// Provider that returns the original ASR text unchanged.
///
/// Useful for headless or testing contexts where no interactive UI is available.
#[derive(Debug, Clone, Default)]
pub struct StubUserInputProvider;

#[async_trait]
impl UserInputProvider for StubUserInputProvider {
    async fn request(
        &self,
        _call_id: &str,
        questions: Vec<UserInputQuestion>,
    ) -> Result<Vec<UserInputAnswer>, ToolError> {
        Ok(questions
            .into_iter()
            .map(|q| UserInputAnswer { text: q.text })
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
