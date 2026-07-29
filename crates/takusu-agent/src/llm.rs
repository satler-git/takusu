use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{Stream, StreamExt};
use rand::random;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::Permissions;
use crate::tool::OpenAITool;

pub use crate::compact::CompactionSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProviderKind {
    Openai,
    Openrouter,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProviderConfig {
    pub id: String,
    pub name: String,
    pub provider: LlmProviderKind,
    pub base_url: String,
    pub selected_model: String,
    #[serde(default)]
    pub cached_models: Vec<String>,
    pub models_fetched_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    #[serde(default = "default_llm_base_url")]
    pub base_url: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(default = "default_llm_api_key_env")]
    pub api_key_env: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default = "default_max_history")]
    pub max_history: usize,
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: usize,
    #[serde(
        default = "default_request_timeout",
        with = "takusu_util::duration_seconds",
        alias = "request_timeout_seconds"
    )]
    pub request_timeout: Duration,
    #[serde(default)]
    pub compaction: CompactionSettings,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: default_llm_base_url(),
            model: default_llm_model(),
            api_key_env: default_llm_api_key_env(),
            api_key: String::new(),
            permissions: Permissions::default(),
            max_history: default_max_history(),
            max_context_tokens: default_max_context_tokens(),
            max_tool_calls: default_max_tool_calls(),
            request_timeout: default_request_timeout(),
            compaction: CompactionSettings::default(),
        }
    }
}

fn default_llm_base_url() -> String {
    "https://api.openai.com/v1".into()
}

fn default_llm_model() -> String {
    "gpt-4.1-mini".into()
}

fn default_llm_api_key_env() -> String {
    "TAKUSU_LLM_API_KEY".into()
}

fn default_max_history() -> usize {
    64
}

fn default_max_context_tokens() -> usize {
    32000
}

fn default_max_tool_calls() -> usize {
    64
}

fn default_request_timeout() -> Duration {
    Duration::from_secs(60)
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("http error {status}: {message}")]
    Http { status: u16, message: String },
    #[error("rate limited")]
    RateLimited,
    #[error("request timed out")]
    Timeout,
    #[error("response parse error: {0}")]
    Parse(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited
                | LlmError::Timeout
                | LlmError::Http {
                    status: 429 | 502 | 503 | 504 | 524,
                    ..
                }
                | LlmError::Request(_)
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// OpenAI tool-call format (what we send to the API).
#[derive(Debug, Clone, Serialize)]
pub struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub function: OpenAIToolCallFunction,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAIToolCallFunction {
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn to_openai(&self) -> OpenAIToolCall {
        OpenAIToolCall {
            id: self.id.clone(),
            type_: "function",
            function: OpenAIToolCallFunction {
                name: self.name.clone(),
                arguments: self.arguments.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    ContentFilter,
    ToolCalls,
    Other(String),
}

impl From<&str> for FinishReason {
    fn from(s: &str) -> Self {
        match s {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            "content_filter" => FinishReason::ContentFilter,
            "tool_calls" | "function_call" => FinishReason::ToolCalls,
            other => FinishReason::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: LlmResponseContent,
    pub prompt_tokens: Option<usize>,
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone)]
pub enum LlmResponseContent {
    Text(String),
    ToolCalls {
        /// Assistant text produced alongside the tool calls, if any.
        text: Option<String>,
        calls: Vec<ToolCall>,
    },
}

/// A single chunk emitted by a streaming chat completion.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmStreamEvent {
    Text(String),
    Thinking(String),
    ToolCall(ToolCall),
    Done {
        finish_reason: Option<FinishReason>,
        prompt_tokens: Option<usize>,
    },
}

#[derive(Debug, Clone)]
pub enum AssistantContent {
    Text(String),
    ToolCalls {
        /// Assistant text produced alongside the tool calls, if any.
        text: Option<String>,
        calls: Vec<ToolCall>,
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    System(String),
    User(String),
    Assistant(AssistantContent),
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
}

/// OpenAI message format (what we send to the API).
///
/// `content` is always serialized (as `null` when absent) to match the
/// behavior of the previous `json!`-based implementation and to stay
/// compatible with OpenAI-compatible providers that expect the field.
#[derive(Debug, Clone, Serialize)]
pub struct OpenAIMessage {
    pub role: &'static str,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl Message {
    pub fn to_openai(&self) -> OpenAIMessage {
        match self {
            Message::System(c) => OpenAIMessage {
                role: "system",
                content: Some(c.clone()),
                tool_calls: None,
                tool_call_id: None,
                is_error: None,
            },
            Message::User(c) => OpenAIMessage {
                role: "user",
                content: Some(c.clone()),
                tool_calls: None,
                tool_call_id: None,
                is_error: None,
            },
            Message::Assistant(AssistantContent::Text(c)) => OpenAIMessage {
                role: "assistant",
                content: Some(c.clone()),
                tool_calls: None,
                tool_call_id: None,
                is_error: None,
            },
            Message::Assistant(AssistantContent::ToolCalls { text, calls }) => OpenAIMessage {
                role: "assistant",
                content: text.clone(),
                tool_calls: Some(calls.iter().map(ToolCall::to_openai).collect()),
                tool_call_id: None,
                is_error: None,
            },
            Message::ToolResult {
                call_id,
                content,
                is_error,
            } => OpenAIMessage {
                role: "tool",
                content: Some(content.clone()),
                tool_calls: None,
                tool_call_id: Some(call_id.clone()),
                is_error: if *is_error { Some(true) } else { None },
            },
        }
    }

    pub fn role(&self) -> &'static str {
        match self {
            Message::System(_) => "system",
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult { .. } => "tool",
        }
    }
}

pub(crate) const TOKEN_ESTIMATE_OVERHEAD: usize = 4;
pub(crate) const TOKEN_ESTIMATE_CHARS_PER_TOKEN: usize = 4;

pub(crate) fn estimate_text_tokens(text: &str) -> usize {
    text.chars()
        .count()
        .div_ceil(TOKEN_ESTIMATE_CHARS_PER_TOKEN)
        + TOKEN_ESTIMATE_OVERHEAD
}

impl Message {
    /// Very rough token estimate for history trimming. Treats ~4 characters as one token
    /// plus a small per-message overhead, which is enough to preserve context limits.
    pub fn estimate_tokens(&self) -> usize {
        let content_len = match self {
            Message::System(c) | Message::User(c) => c.chars().count(),
            Message::Assistant(AssistantContent::Text(c)) => c.chars().count(),
            Message::Assistant(AssistantContent::ToolCalls { text, calls }) => {
                let calls_tokens: usize = calls
                    .iter()
                    .map(|c| {
                        c.name.chars().count()
                            + c.arguments.to_string().chars().count()
                            + c.id.chars().count()
                    })
                    .sum();
                text.as_deref().map(|t| t.chars().count()).unwrap_or(0) + calls_tokens
            }
            Message::ToolResult {
                call_id, content, ..
            } => call_id.chars().count() + content.chars().count(),
        };
        content_len.div_ceil(TOKEN_ESTIMATE_CHARS_PER_TOKEN) + TOKEN_ESTIMATE_OVERHEAD
    }
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[OpenAITool],
    ) -> Result<LlmResponse, LlmError>;

    async fn chat_stream(
        &self,
        _messages: &[Message],
        _tools: &[OpenAITool],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, LlmError>> + Send>>, LlmError>
    {
        Err(LlmError::Request(
            "streaming not supported by this client".into(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct OpenAIClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    request_timeout: Duration,
    max_retries: usize,
    initial_backoff: Duration,
}

impl OpenAIClient {
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        let client = takusu_client::default_http_client(Some(config.request_timeout.as_secs()))
            .map_err(|e| LlmError::Request(e.to_string()))?;

        let api_key = if config.api_key.is_empty() {
            std::env::var(&config.api_key_env).unwrap_or_default()
        } else {
            config.api_key
        };

        Ok(Self {
            client,
            base_url: config.base_url,
            api_key,
            model: config.model,
            request_timeout: config.request_timeout,
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
        })
    }

    pub fn with_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Fetch model IDs from an OpenAI-compatible `/models` endpoint.
    ///
    /// Providers may expose additional fields, but only the stable `id` is
    /// surfaced to the UI so the dropdown stays provider-neutral.
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let response = tokio::time::timeout(
            self.request_timeout,
            self.client.get(url).bearer_auth(&self.api_key).send(),
        )
        .await
        .map_err(|_| LlmError::Timeout)?
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::Timeout
            } else if e.is_request() {
                LlmError::Request(e.to_string())
            } else {
                LlmError::Other(e.into())
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(if status == 429 {
                LlmError::RateLimited
            } else {
                LlmError::Http {
                    status: status.as_u16(),
                    message: extract_error_message(&text),
                }
            });
        }
        let body = response
            .json::<ModelsResponse>()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;
        let mut models: Vec<String> = body
            .data
            .into_iter()
            .map(|model| model.id)
            .filter(|id| !id.trim().is_empty())
            .collect();
        models.sort_unstable();
        models.dedup();
        Ok(models)
    }

    async fn send_request(
        &self,
        messages: &[Message],
        tools: &[OpenAITool],
        stream: bool,
    ) -> Result<reqwest::Response, LlmError> {
        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: messages.iter().map(Message::to_openai).collect(),
            tools: tools.to_vec(),
            stream: if stream { Some(true) } else { None },
            stream_options: if stream {
                Some(json!({"include_usage": true}))
            } else {
                None
            },
        };
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let response = tokio::time::timeout(
            self.request_timeout,
            self.client
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&request)
                .send(),
        )
        .await
        .map_err(|_| LlmError::Timeout)?
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::Timeout
            } else if e.is_request() {
                LlmError::Request(e.to_string())
            } else {
                LlmError::Other(e.into())
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(if status == 429 {
                LlmError::RateLimited
            } else {
                LlmError::Http {
                    status: status.as_u16(),
                    message: extract_error_message(&text),
                }
            });
        }

        Ok(response)
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<LlmResponse, LlmError> {
        let body = response
            .json::<ChatCompletionResponse>()
            .await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        let choice = body
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::Parse("no choices in response".into()))?;

        let prompt_tokens = body.usage.as_ref().map(|u| u.prompt_tokens as usize);
        let finish_reason = choice.finish_reason.as_deref().map(FinishReason::from);

        let content = if let Some(tool_calls) = choice.message.tool_calls {
            let calls = tool_calls
                .into_iter()
                .map(|tc| ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments: serde_json::from_str(&tc.function.arguments).unwrap_or(Value::Null),
                })
                .collect();
            let text = choice.message.content.filter(|c| !c.is_empty());
            LlmResponseContent::ToolCalls { text, calls }
        } else {
            LlmResponseContent::Text(choice.message.content.unwrap_or_default())
        };

        Ok(LlmResponse {
            content,
            prompt_tokens,
            finish_reason,
        })
    }

    fn backoff(&self, attempt: usize) -> Duration {
        let base = self.initial_backoff.as_millis() as f64 * 2f64.powi(attempt as i32);
        let jitter = base * random::<f64>();
        Duration::from_millis((base + jitter).clamp(0.0, u64::MAX as f64) as u64)
    }
}

#[async_trait]
impl LlmClient for OpenAIClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[OpenAITool],
    ) -> Result<LlmResponse, LlmError> {
        let mut attempt = 0;
        loop {
            let response = self.send_request(messages, tools, false).await;
            match response {
                Ok(resp) => return self.parse_response(resp).await,
                Err(e) if e.is_retryable() && attempt < self.max_retries => {
                    tokio::time::sleep(self.backoff(attempt)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[OpenAITool],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, LlmError>> + Send>>, LlmError>
    {
        let mut attempt = 0;
        loop {
            let response = self.send_request(messages, tools, true).await;
            match response {
                Ok(resp) => {
                    return Ok(Box::pin(parse_sse_stream(resp.bytes_stream())));
                }
                Err(e) if e.is_retryable() && attempt < self.max_retries => {
                    tokio::time::sleep(self.backoff(attempt)).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

#[derive(Deserialize, Debug)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelResponse>,
}

#[derive(Deserialize, Debug)]
struct ModelResponse {
    id: String,
}

#[derive(Serialize, Debug)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAITool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<Value>,
}

#[derive(Deserialize, Debug)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize, Debug)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
}

#[derive(Deserialize, Debug)]
struct Choice {
    message: ResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ResponseMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallResponse>>,
}

#[derive(Deserialize, Debug)]
struct ToolCallResponse {
    id: String,
    function: ToolCallFunction,
}

#[derive(Deserialize, Debug)]
struct ToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize, Debug)]
struct ErrorResponse {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize, Debug)]
struct ErrorDetail {
    message: String,
}

fn extract_error_message(text: &str) -> String {
    serde_json::from_str::<ErrorResponse>(text)
        .ok()
        .and_then(|r| r.error.map(|e| e.message))
        .unwrap_or_else(|| text.to_string())
}

#[derive(Deserialize, Debug)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    usage: Option<Usage>,
}

#[derive(Deserialize, Debug)]
struct ChunkChoice {
    delta: ChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct ChunkDelta {
    content: Option<String>,
    #[serde(rename = "reasoning_content")]
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ChunkToolCall>>,
}

#[derive(Deserialize, Debug, Default)]
struct ChunkToolCall {
    index: usize,
    id: Option<String>,
    function: Option<ChunkToolCallFunction>,
}

#[derive(Deserialize, Debug, Default)]
struct ChunkToolCallFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PartialToolCall {
    fn into_tool_call(self) -> Option<ToolCall> {
        let id = self.id?;
        let name = self.name?;
        let arguments = if self.arguments.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&self.arguments).unwrap_or(Value::Null)
        };
        Some(ToolCall {
            id,
            name,
            arguments,
        })
    }
}

fn drain_complete_sse_block(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    for i in 0..buffer.len().saturating_sub(1) {
        let delim_len = if &buffer[i..i + 2] == b"\n\n" {
            2
        } else if &buffer[i..i + 2] == b"\r\n" && buffer.get(i + 2..i + 4) == Some(b"\r\n") {
            4
        } else {
            continue;
        };
        let mut rest = buffer.split_off(i + delim_len);
        std::mem::swap(buffer, &mut rest);
        rest.truncate(i);
        return Some(rest);
    }
    None
}

fn process_sse_block(
    block: &str,
    tool_calls: &mut BTreeMap<usize, PartialToolCall>,
    pending_finish: &mut Option<FinishReason>,
) -> Vec<Result<LlmStreamEvent, LlmError>> {
    let mut data_lines = Vec::new();
    for line in block.split('\n') {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim_start_matches(' ').trim_end_matches('\r');
            data_lines.push(data);
        }
    }
    if data_lines.is_empty() {
        return Vec::new();
    }
    let data = data_lines.join("\n");
    if data == "[DONE]" {
        return vec![Ok(LlmStreamEvent::Done {
            finish_reason: pending_finish.take(),
            prompt_tokens: None,
        })];
    }
    let chunk: ChatCompletionChunk = match serde_json::from_str(&data) {
        Ok(c) => c,
        Err(e) => return vec![Err(LlmError::Parse(e.to_string()))],
    };

    let mut events = Vec::new();
    let mut finished = false;
    let mut finish_reason = None;

    for choice in &chunk.choices {
        if let Some(content) = choice.delta.content.as_ref()
            && !content.is_empty()
        {
            events.push(Ok(LlmStreamEvent::Text(content.clone())));
        }
        if let Some(reasoning) = choice.delta.reasoning_content.as_ref()
            && !reasoning.is_empty()
        {
            events.push(Ok(LlmStreamEvent::Thinking(reasoning.clone())));
        }
        if let Some(calls) = choice.delta.tool_calls.as_ref() {
            for call in calls {
                let entry = tool_calls.entry(call.index).or_default();
                if let Some(id) = call.id.as_ref() {
                    entry.id = Some(id.clone());
                }
                if let Some(function) = call.function.as_ref() {
                    if let Some(name) = function.name.as_ref() {
                        entry.name = Some(name.clone());
                    }
                    if let Some(args) = function.arguments.as_ref() {
                        entry.arguments.push_str(args);
                    }
                }
            }
        }
        if let Some(fr) = choice.finish_reason.as_deref() {
            finished = true;
            finish_reason = Some(FinishReason::from(fr));
        }
    }

    if finished {
        if !tool_calls.is_empty() {
            for (_, partial) in std::mem::take(tool_calls) {
                if let Some(tc) = partial.into_tool_call() {
                    events.push(Ok(LlmStreamEvent::ToolCall(tc)));
                }
            }
        }
        let prompt_tokens = chunk.usage.as_ref().map(|u| u.prompt_tokens as usize);
        if prompt_tokens.is_some() {
            events.push(Ok(LlmStreamEvent::Done {
                finish_reason,
                prompt_tokens,
            }));
        } else {
            *pending_finish = finish_reason;
        }
    } else if chunk.usage.is_some() && chunk.choices.is_empty() {
        let prompt_tokens = chunk.usage.as_ref().map(|u| u.prompt_tokens as usize);
        if let Some(finish_reason) = pending_finish.take() {
            events.push(Ok(LlmStreamEvent::Done {
                finish_reason: Some(finish_reason),
                prompt_tokens,
            }));
        } else {
            events.push(Ok(LlmStreamEvent::Done {
                finish_reason: None,
                prompt_tokens,
            }));
        }
    }

    events
}

fn parse_sse_stream<S, B, E>(
    bytes_stream: S,
) -> Pin<Box<dyn Stream<Item = Result<LlmStreamEvent, LlmError>> + Send>>
where
    S: Stream<Item = Result<B, E>> + Send + Unpin + 'static,
    B: AsRef<[u8]> + Send,
    E: std::error::Error + Send + Sync + 'static,
{
    use std::collections::VecDeque;

    Box::pin(futures_util::stream::unfold(
        (
            bytes_stream,
            Vec::<u8>::new(),
            BTreeMap::<usize, PartialToolCall>::new(),
            VecDeque::<Result<LlmStreamEvent, LlmError>>::new(),
            Option::<FinishReason>::None,
        ),
        async move |(mut stream, mut buffer, mut tool_calls, mut pending, mut pending_finish)| {
            if let Some(event) = pending.pop_front() {
                return Some((event, (stream, buffer, tool_calls, pending, pending_finish)));
            }

            loop {
                while let Some(block) = drain_complete_sse_block(&mut buffer) {
                    let block = match String::from_utf8(block) {
                        Ok(s) => s,
                        Err(_) => {
                            return Some((
                                Err(LlmError::Parse("invalid UTF-8 in SSE data".into())),
                                (stream, buffer, tool_calls, pending, pending_finish),
                            ));
                        }
                    };
                    pending.extend(process_sse_block(
                        &block,
                        &mut tool_calls,
                        &mut pending_finish,
                    ));
                    if let Some(event) = pending.pop_front() {
                        return Some((
                            event,
                            (stream, buffer, tool_calls, pending, pending_finish),
                        ));
                    }
                }

                match stream.next().await {
                    Some(Ok(bytes)) => {
                        buffer.extend_from_slice(bytes.as_ref());
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(LlmError::Request(e.to_string())),
                            (stream, buffer, tool_calls, pending, pending_finish),
                        ));
                    }
                    None => {
                        // Stream ended; flush any final, undelimited block.
                        if !buffer.is_empty() {
                            let block = match String::from_utf8(std::mem::take(&mut buffer)) {
                                Ok(s) => s,
                                Err(_) => {
                                    return Some((
                                        Err(LlmError::Parse("invalid UTF-8 in SSE data".into())),
                                        (stream, buffer, tool_calls, pending, pending_finish),
                                    ));
                                }
                            };
                            if !block.is_empty() {
                                pending.extend(process_sse_block(
                                    &block,
                                    &mut tool_calls,
                                    &mut pending_finish,
                                ));
                            }
                        }

                        if !tool_calls.is_empty() {
                            for (_, partial) in std::mem::take(&mut tool_calls) {
                                if let Some(tc) = partial.into_tool_call() {
                                    pending.push_back(Ok(LlmStreamEvent::ToolCall(tc)));
                                }
                            }
                            pending.push_back(Ok(LlmStreamEvent::Done {
                                finish_reason: Some(FinishReason::ToolCalls),
                                prompt_tokens: None,
                            }));
                        } else if let Some(finish_reason) = pending_finish.take() {
                            pending.push_back(Ok(LlmStreamEvent::Done {
                                finish_reason: Some(finish_reason),
                                prompt_tokens: None,
                            }));
                        } else if pending.is_empty() {
                            // Nothing to emit.
                            return None;
                        }
                        if let Some(event) = pending.pop_front() {
                            return Some((
                                event,
                                (stream, buffer, tool_calls, pending, pending_finish),
                            ));
                        }
                        return None;
                    }
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_error_retryable_detection() {
        assert!(LlmError::RateLimited.is_retryable());
        assert!(LlmError::Timeout.is_retryable());
        assert!(LlmError::Request("connection refused".into()).is_retryable());
        assert!(
            LlmError::Http {
                status: 429,
                message: "rate limited".into(),
            }
            .is_retryable()
        );
        assert!(
            LlmError::Http {
                status: 503,
                message: "unavailable".into(),
            }
            .is_retryable()
        );
        assert!(
            !LlmError::Http {
                status: 400,
                message: "bad request".into(),
            }
            .is_retryable()
        );
        assert!(!LlmError::Parse("broken json".into()).is_retryable());
    }

    #[test]
    fn message_serializes_to_openai_format() {
        let msg = Message::User("hello".into());
        let value: Value = serde_json::to_value(msg.to_openai()).unwrap();
        assert_eq!(value["role"], "user");
        assert_eq!(value["content"], "hello");

        let tool_call = ToolCall {
            id: "call_1".into(),
            name: "echo".into(),
            arguments: json!({"message": "hi"}),
        };
        let msg = Message::Assistant(AssistantContent::ToolCalls {
            text: None,
            calls: vec![tool_call],
        });
        let value: Value = serde_json::to_value(msg.to_openai()).unwrap();
        assert_eq!(value["role"], "assistant");
        // content must be explicitly present as null (not omitted) for
        // compatibility with OpenAI-compatible providers that expect the
        // field on assistant tool_calls messages.
        assert_eq!(value.get("content"), Some(&Value::Null));
        assert_eq!(value["tool_calls"][0]["id"], "call_1");
        assert_eq!(value["tool_calls"][0]["function"]["name"], "echo");

        // When text is present alongside tool calls, both fields serialize.
        let msg = Message::Assistant(AssistantContent::ToolCalls {
            text: Some("thinking…".into()),
            calls: vec![ToolCall {
                id: "call_2".into(),
                name: "echo".into(),
                arguments: json!({"message": "hi"}),
            }],
        });
        let value: Value = serde_json::to_value(msg.to_openai()).unwrap();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"], "thinking…");
        assert_eq!(value["tool_calls"][0]["id"], "call_2");
    }

    #[test]
    fn fixture_text_response_deserializes() {
        let text = include_str!("fixtures/text_response.json");
        let body: ChatCompletionResponse = serde_json::from_str(text).unwrap();
        let choice = body.choices.into_iter().next().unwrap();
        assert_eq!(
            choice.message.content.as_deref(),
            Some("今日は会議が2つあります")
        );
        assert!(choice.message.tool_calls.is_none());
    }

    #[test]
    fn fixture_tool_calls_response_deserializes() {
        let text = include_str!("fixtures/tool_calls_response.json");
        let body: ChatCompletionResponse = serde_json::from_str(text).unwrap();
        let choice = body.choices.into_iter().next().unwrap();
        assert!(choice.message.content.is_none());
        let calls = choice.message.tool_calls.unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "list_tasks");
        assert_eq!(calls[1].function.name, "get_schedule");
    }

    #[test]
    fn fixture_error_response_parses_message() {
        let text = include_str!("fixtures/error_response.json");
        let msg = extract_error_message(text);
        assert!(msg.contains("quota"));
    }

    #[test]
    fn response_with_null_content_parses_as_empty_text() {
        let json = json!({
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": null },
                "finish_reason": "stop"
            }]
        });
        let body: ChatCompletionResponse = serde_json::from_value(json).unwrap();
        let choice = body.choices.into_iter().next().unwrap();
        assert_eq!(choice.message.content.unwrap_or_default(), "");
    }

    #[tokio::test]
    async fn parse_sse_stream_buffers_bytes_for_multibyte_utf8() {
        let event = json!({
            "choices": [{"index": 0, "delta": {"content": "こんにちは"}, "finish_reason": null}]
        });
        let payload = format!("data: {}\n\n", serde_json::to_string(&event).unwrap());
        let bytes = payload.into_bytes();
        // Split somewhere inside the multibyte UTF-8 sequence.
        let split = bytes.len() / 2;
        let chunks = vec![
            Ok::<_, std::io::Error>(bytes[..split].to_vec()),
            Ok(bytes[split..].to_vec()),
        ];
        let events = parse_sse_stream(futures_util::stream::iter(chunks))
            .collect::<Vec<_>>()
            .await;
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            Ok(LlmStreamEvent::Text(t)) if t == "こんにちは"
        ));
    }

    #[tokio::test]
    async fn parse_sse_stream_merges_usage_chunk_with_finish_reason() {
        let text_chunk = json!({
            "choices": [{"index": 0, "delta": {"content": "Hello"}, "finish_reason": "stop"}],
            "usage": null
        });
        let usage_chunk = json!({
            "choices": [],
            "usage": {"prompt_tokens": 42}
        });
        let payload = format!(
            "data: {}\r\n\r\ndata: {}\n\n",
            serde_json::to_string(&text_chunk).unwrap(),
            serde_json::to_string(&usage_chunk).unwrap()
        );
        let bytes = payload.into_bytes();
        let stream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(bytes)]);
        let events = parse_sse_stream(stream).collect::<Vec<_>>().await;
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], Ok(LlmStreamEvent::Text(t)) if t == "Hello"));
        assert!(matches!(
            &events[1],
            Ok(LlmStreamEvent::Done {
                finish_reason: Some(FinishReason::Stop),
                prompt_tokens: Some(42),
            })
        ));
    }

    #[tokio::test]
    async fn parse_sse_stream_emits_tool_call_then_done_with_usage() {
        let tool_chunk = json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"name": "echo", "arguments": "{\"message\":\"hi\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        let usage_chunk = json!({
            "choices": [],
            "usage": {"prompt_tokens": 7}
        });
        let payload = format!(
            "data: {}\n\ndata: {}\n\n",
            serde_json::to_string(&tool_chunk).unwrap(),
            serde_json::to_string(&usage_chunk).unwrap()
        );
        let bytes = payload.into_bytes();
        let stream = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(bytes)]);
        let events = parse_sse_stream(stream).collect::<Vec<_>>().await;
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            Ok(LlmStreamEvent::ToolCall(tc))
                if tc.id == "call_1" && tc.name == "echo" && tc.arguments == json!({"message": "hi"})
        ));
        assert!(matches!(
            &events[1],
            Ok(LlmStreamEvent::Done {
                finish_reason: Some(FinishReason::ToolCalls),
                prompt_tokens: Some(7),
            })
        ));
    }

    #[tokio::test]
    async fn openai_client_parses_text_response_from_mock_server() {
        use axum::Router;
        use axum::extract::Json;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        use axum::routing::post;

        async fn handler(Json(_): Json<Value>) -> impl IntoResponse {
            let fixture = include_str!("fixtures/text_response.json");
            let body: Value = serde_json::from_str(fixture).unwrap();
            (StatusCode::OK, axum::Json(body))
        }

        let app = Router::new().route("/chat/completions", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = LlmConfig {
            base_url: format!("http://{addr}/"),
            request_timeout: Duration::from_secs(5),
            api_key_env: "UNUSED".into(),
            ..Default::default()
        };
        let client = OpenAIClient::new(cfg).unwrap();

        let response = client
            .chat(&[Message::User("hello".into())], &[])
            .await
            .unwrap();

        assert!(
            matches!(response.content, LlmResponseContent::Text(text) if text == "今日は会議が2つあります")
        );
    }

    #[tokio::test]
    async fn openai_client_parses_multiple_tool_calls_from_mock_server() {
        use axum::Router;
        use axum::extract::Json;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        use axum::routing::post;

        async fn handler(Json(_): Json<Value>) -> impl IntoResponse {
            let fixture = include_str!("fixtures/tool_calls_response.json");
            let body: Value = serde_json::from_str(fixture).unwrap();
            (StatusCode::OK, axum::Json(body))
        }

        let app = Router::new().route("/chat/completions", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = LlmConfig {
            base_url: format!("http://{addr}/"),
            request_timeout: Duration::from_secs(5),
            api_key_env: "UNUSED".into(),
            ..Default::default()
        };
        let client = OpenAIClient::new(cfg).unwrap();

        let response = client
            .chat(&[Message::User("予定を教えて".into())], &[])
            .await
            .unwrap();

        if let LlmResponseContent::ToolCalls { calls, .. } = response.content {
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].name, "list_tasks");
            assert_eq!(calls[1].name, "get_schedule");
        } else {
            panic!("expected tool calls, got {response:?}");
        }
    }

    #[tokio::test]
    async fn openai_client_retries_429_and_then_succeeds() {
        use axum::Router;
        use axum::extract::{Json, State};
        use axum::http::StatusCode;
        use axum::routing::post;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone)]
        struct AppState {
            count: Arc<AtomicUsize>,
        }

        async fn handler(
            State(state): State<AppState>,
            Json(_): Json<Value>,
        ) -> Result<axum::Json<Value>, StatusCode> {
            let count = state.count.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
            let fixture = include_str!("fixtures/text_response.json");
            let body: Value = serde_json::from_str(fixture).unwrap();
            Ok(axum::Json(body))
        }

        let state = AppState {
            count: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/chat/completions", post(handler))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = LlmConfig {
            base_url: format!("http://{addr}/"),
            request_timeout: Duration::from_secs(5),
            api_key_env: "UNUSED".into(),
            ..Default::default()
        };
        let client = OpenAIClient::new(cfg).unwrap();

        let response = client
            .chat(&[Message::User("hello".into())], &[])
            .await
            .unwrap();

        assert!(matches!(response.content, LlmResponseContent::Text(_)));
        assert_eq!(state.count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn openai_client_times_out_against_slow_server() {
        use axum::Router;
        use axum::extract::Json;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        use axum::routing::post;

        async fn handler(Json(_): Json<Value>) -> impl IntoResponse {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let fixture = include_str!("fixtures/text_response.json");
            let body: Value = serde_json::from_str(fixture).unwrap();
            (StatusCode::OK, axum::Json(body))
        }

        let app = Router::new().route("/chat/completions", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = LlmConfig {
            base_url: format!("http://{addr}/"),
            request_timeout: Duration::from_secs(1),
            api_key_env: "UNUSED".into(),
            ..Default::default()
        };
        let client = OpenAIClient::new(cfg).unwrap();

        let response = client.chat(&[Message::User("hello".into())], &[]).await;

        assert!(matches!(response, Err(LlmError::Timeout)));
    }

    #[tokio::test]
    #[ignore = "requires a real OpenAI-compatible API key"]
    async fn real_endpoint_smoke_test() {
        let cfg = LlmConfig {
            api_key_env: "TAKUSU_LLM_API_KEY".into(),
            request_timeout: Duration::from_secs(30),
            ..Default::default()
        };
        let client = OpenAIClient::new(cfg).unwrap();
        let response = client.chat(&[Message::User("hello".into())], &[]).await;
        assert!(response.is_ok(), "{response:?}");
    }
}
