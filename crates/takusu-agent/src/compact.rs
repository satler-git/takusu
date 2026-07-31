//! Context compaction for long agent sessions.
//!
//! Inspired by pi's compaction mechanism: when the conversation exceeds a
//! context-window threshold, older turns are summarized by an LLM and replaced
//! by a compact summary. The most recent `keep_recent_tokens` worth of turns are
//! kept intact so the model still sees the current conversation.

use crate::llm::{AssistantContent, LlmClient, LlmError, LlmResponseContent, Message};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const TOOL_RESULT_MAX_CHARS: usize = 2000;

const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"あなたは会話の要約アシスタントです。ユーザーとAIアシスタントの会話を読み、構造化された要約を出力してください。会話を続けたり、会話内の質問に答えたりしないでください。"#;

const SUMMARIZATION_PROMPT: &str = r#"上記の会話を要約してください。別のLLMがこの要約を使って作業を継続できるよう、構造化されたコンテキストチェックポイントを作成してください。

## 目標
[ユーザーが何を達成しようとしているか。複数あれば列挙]

## 制約と偏好
- [ユーザーが述べた制約、偏好、要件]
- [なければ「なし」]

## 進捗
### 完了
- [x] [完了したタスクや変更]

### 進行中
- [ ] [現在の作業]

### 阻害
- [進捗を妨げる問題があれば]

## 重要な決定
- **[決定]**: [簡潔な理由]

## 次にやるべきこと
1. [次に行うべきことを箇条書き。不明・未確定なら「不明」とする]
2. [特になければ「なし」とする]

## 重要なコンテキスト
- [作業を継続するために必要なデータ、参照、エラーメッセージ]
- [該当しない場合は「なし」]

各セクションを簡潔に保ち、正確なタスク名、日時、数値、エラーメッセージを保持してください。"#;

const UPDATE_SUMMARIZATION_PROMPT: &str = r#"上記は、以前の要約に追加すべき新しい会話メッセージです。<previous-summary>タグ内の既存の要約に新しい情報を統合してください。

ルール:
- 既存の情報はすべて保持する
- 新しい進捗、決定、コンテキストを追加する
- 「進捗」セクションを更新する: 完了した項目を「進行中」から「完了」へ移動する
- 新しい状況に応じて「次にやるべきこと」を更新する
- 正確なタスク名、日時、数値、エラーメッセージを保持する
- もう関係なくなった情報は削除してもよい

上記のフォーマットに従って、更新された要約全体を出力してください。"#;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CompactionSettings {
    /// Whether automatic context compaction is enabled.
    pub enabled: bool,
    /// Tokens reserved for the model's response.
    pub reserve_tokens: usize,
    /// How many of the most recent tokens to keep verbatim.
    pub keep_recent_tokens: usize,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 4096,
            keep_recent_tokens: 12000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_index: usize,
    /// Number of oldest messages that were dropped entirely because they did
    /// not fit in the summarization prompt.
    pub dropped_before: usize,
    pub tokens_before: usize,
}

/// Returns true when the estimated context size exceeds the compaction threshold.
pub fn should_compact(
    history: &[Message],
    system_and_tools_tokens: usize,
    max_context_tokens: usize,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled || max_context_tokens == 0 {
        return false;
    }
    let available = max_context_tokens.saturating_sub(settings.reserve_tokens);
    if available <= settings.keep_recent_tokens {
        return false;
    }
    let total: usize = history.iter().map(|m| m.estimate_tokens()).sum();
    total.saturating_add(system_and_tools_tokens) > available
}

/// Find the oldest user message such that keeping from that message to the end
/// preserves roughly `keep_recent_tokens` worth of the conversation.
///
/// Cut points are always at user messages so that complete assistant/tool-result
/// turns are kept together.
pub fn find_cut_point(history: &[Message], keep_recent_tokens: usize) -> Option<usize> {
    if history.is_empty() || keep_recent_tokens == 0 {
        return None;
    }

    let mut accumulated = 0usize;
    let mut target_reached = false;
    let mut cut: Option<usize> = None;

    for (i, msg) in history.iter().enumerate().rev() {
        accumulated += msg.estimate_tokens();
        if !target_reached && accumulated >= keep_recent_tokens {
            target_reached = true;
        }
        if target_reached && matches!(msg, Message::User(_)) {
            cut = Some(i);
            break;
        }
    }

    if !target_reached {
        return None;
    }

    // If the whole history is one continuous turn, keep everything.
    if cut.is_none() {
        cut = history.iter().position(|m| matches!(m, Message::User(_)));
    }

    cut.filter(|c| *c > 0)
}

/// Summarize older messages and return a result describing the new summary
/// and the first index that should be kept.
///
/// The summarization prompt itself is kept under `max_prompt_tokens` by
/// dropping the oldest messages from the summarization slice. Those dropped
/// messages are lost rather than summarized. The summary slice always starts
/// at a user message so the conversation context is coherent.
pub async fn compact_history(
    history: &[Message],
    previous_summary: Option<&str>,
    llm: &Arc<dyn LlmClient + Send + Sync>,
    keep_recent_tokens: usize,
    system_and_tools_tokens: usize,
    max_prompt_tokens: usize,
) -> Result<Option<CompactionResult>, LlmError> {
    let Some(cut) = find_cut_point(history, keep_recent_tokens) else {
        return Ok(None);
    };
    if cut == 0 {
        return Ok(None);
    }

    // Build the empty summary prompt once to get the constant character counts.
    let template_messages = build_summary_messages(&[], previous_summary);
    let (system_chars, user_template_chars) = match &template_messages[..] {
        [Message::System(system), Message::User(user)] => {
            (system.chars().count(), user.chars().count())
        }
        _ => (0, 0),
    };

    // Pre-compute the serialized text size of each message so we can estimate
    // the summary prompt token count for any candidate slice in O(1).
    let serialized_parts: Vec<String> = history[..cut].iter().map(serialize_message).collect();
    let mut prefix_chars = Vec::with_capacity(cut + 1);
    prefix_chars.push(0);
    for part in &serialized_parts {
        prefix_chars.push(prefix_chars.last().unwrap() + part.chars().count());
    }

    // Candidate start positions must be user messages.
    let user_indices: Vec<usize> = history[..cut]
        .iter()
        .enumerate()
        .filter(|(_, m)| matches!(m, Message::User(_)))
        .map(|(i, _)| i)
        .collect();
    if user_indices.is_empty() {
        return Ok(None);
    }

    fn token_estimate_from_chars(chars: usize) -> usize {
        chars.div_ceil(crate::llm::TOKEN_ESTIMATE_CHARS_PER_TOKEN)
            + crate::llm::TOKEN_ESTIMATE_OVERHEAD
    }

    let serialized_chars = |start: usize| {
        let count = cut - start;
        prefix_chars[cut] - prefix_chars[start] + 2 * count.saturating_sub(1)
    };

    // Pick the earliest user index whose summarization prompt fits.
    let mut start = None;
    for &u in &user_indices {
        let user_chars = user_template_chars + serialized_chars(u);
        let prompt_tokens =
            token_estimate_from_chars(system_chars) + token_estimate_from_chars(user_chars);
        if prompt_tokens <= max_prompt_tokens {
            start = Some(u);
            break;
        }
    }
    let Some(start) = start else {
        return Ok(None);
    };

    let messages_to_summarize = &history[start..cut];
    let summary_messages = build_summary_messages(messages_to_summarize, previous_summary);
    let response = llm.chat(&summary_messages, &[]).await?;

    let summary = match response.content {
        LlmResponseContent::Text(text) => text,
        LlmResponseContent::ToolCalls { text, calls } => {
            let text_part = text.unwrap_or_default();
            return Err(LlmError::Parse(format!(
                "unexpected tool calls in compaction summary: text={text_part:?}, calls={calls:?}"
            )));
        }
    };

    let history_tokens: usize = history.iter().map(|m| m.estimate_tokens()).sum();
    Ok(Some(CompactionResult {
        summary,
        first_kept_index: cut,
        dropped_before: start,
        tokens_before: history_tokens + system_and_tools_tokens,
    }))
}

fn build_summary_messages(messages: &[Message], previous_summary: Option<&str>) -> Vec<Message> {
    let serialized = serialize_messages(messages);
    let conversation = format!("<conversation>\n{serialized}\n</conversation>\n\n");
    let user_content = if let Some(prev) = previous_summary {
        format!(
            "{conversation}<previous-summary>\n{prev}\n</previous-summary>\n\n{UPDATE_SUMMARIZATION_PROMPT}"
        )
    } else {
        format!("{conversation}{SUMMARIZATION_PROMPT}")
    };

    vec![
        Message::System(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        Message::User(user_content),
    ]
}

fn serialize_messages(messages: &[Message]) -> String {
    let mut parts = Vec::with_capacity(messages.len());
    for msg in messages {
        parts.push(serialize_message(msg));
    }
    parts.join("\n\n")
}

fn serialize_message(msg: &Message) -> String {
    match msg {
        Message::System(text) => format!("[System]: {text}"),
        Message::User(text) => format!("[User]: {text}"),
        Message::Assistant(AssistantContent::Text(text)) => format!("[Assistant]: {text}"),
        Message::Assistant(AssistantContent::ToolCalls { text, calls }) => {
            let calls_str = calls
                .iter()
                .map(|c| format!("{}({})", c.name, c.arguments))
                .collect::<Vec<_>>()
                .join("; ");
            let prefix = text
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(|t| format!("[Assistant]: {t}\n\n"))
                .unwrap_or_default();
            format!("{prefix}[Assistant tool calls]: {calls_str}")
        }
        Message::ToolResult {
            call_id,
            content,
            is_error,
        } => {
            let mut text = format!(
                "[Tool result (call_id={call_id})]: {content}",
                content = truncate(content)
            );
            if *is_error {
                text.push_str(" (error)");
            }
            text
        }
    }
}

fn truncate(text: &str) -> String {
    let char_count = text.chars().count();
    if char_count <= TOOL_RESULT_MAX_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(TOOL_RESULT_MAX_CHARS).collect();
    format!(
        "{kept}...\n\n[... {remaining} more characters truncated]",
        remaining = char_count - TOOL_RESULT_MAX_CHARS
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[test]
    fn find_cut_point_keeps_recent_turns() {
        let history: Vec<Message> = (0..6)
            .flat_map(|i| {
                [
                    Message::User(format!("user {i}")),
                    Message::Assistant(AssistantContent::Text(format!("assistant {i}"))),
                ]
            })
            .collect();

        // Each message is > 4 chars, so each costs at least 2 tokens (4 chars per token + overhead).
        // With keep_recent_tokens=1 we should cut somewhere before the last turn.
        let cut = find_cut_point(&history, 1).expect("should cut");
        assert!(cut > 0);
        assert!(matches!(history[cut], Message::User(_)));
    }

    #[test]
    fn find_cut_point_returns_none_when_under_budget() {
        let history = vec![
            Message::User("hi".into()),
            Message::Assistant(AssistantContent::Text("hello".into())),
        ];
        assert!(find_cut_point(&history, 1000).is_none());
    }

    #[test]
    fn find_cut_point_returns_none_for_single_turn() {
        let history = vec![
            Message::User("start".into()),
            Message::Assistant(AssistantContent::Text("a".repeat(400))),
            Message::ToolResult {
                call_id: "1".into(),
                content: "result".into(),
                is_error: false,
            },
        ];
        // A single huge turn has nothing older to summarize.
        assert!(find_cut_point(&history, 10).is_none());
    }

    #[test]
    fn serialize_messages_includes_all_roles() {
        let messages = vec![
            Message::User("hello".into()),
            Message::Assistant(AssistantContent::Text("hi".into())),
            Message::Assistant(AssistantContent::ToolCalls {
                text: None,
                calls: vec![crate::llm::ToolCall {
                    id: "call_1".into(),
                    name: "echo".into(),
                    arguments: serde_json::json!({"message": "hi"}),
                }],
            }),
            Message::ToolResult {
                call_id: "call_1".into(),
                content: "long result".into(),
                is_error: true,
            },
        ];
        let serialized = serialize_messages(&messages);
        assert!(serialized.contains("[User]: hello"));
        assert!(serialized.contains("[Assistant]: hi"));
        assert!(serialized.contains("[Assistant tool calls]: echo"));
        assert!(serialized.contains("[Tool result (call_id=call_1)]: long result (error)"));
    }

    #[test]
    fn should_compact_respects_available_budget() {
        let history = vec![Message::User("x".repeat(2000))];
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 100,
            keep_recent_tokens: 50,
        };
        // Context (500) minus reserve (100) leaves 400; history is ~504 tokens, over budget.
        assert!(should_compact(&history, 0, 500, &settings));
        // Context too small: available (0) is not greater than keep_recent (50) -> disabled.
        assert!(!should_compact(&history, 0, 50, &settings));
    }

    #[derive(Clone)]
    struct FixedLlm(String);

    #[async_trait]
    impl crate::llm::LlmClient for FixedLlm {
        async fn chat(
            &self,
            _messages: &[Message],
            _tools: &[crate::tool::OpenAITool],
        ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
            Ok(crate::llm::LlmResponse {
                content: crate::llm::LlmResponseContent::Text(self.0.clone()),
                prompt_tokens: None,
                finish_reason: None,
            })
        }
    }

    #[tokio::test]
    async fn compact_history_drops_oldest_messages_to_fit_context() {
        let mut history = Vec::new();
        for i in 0..20 {
            let filler = "x".repeat(500);
            history.push(Message::User(format!("user {i} {filler}")));
            history.push(Message::Assistant(AssistantContent::Text(format!(
                "assistant {i} {filler}"
            ))));
        }

        let llm: Arc<dyn crate::llm::LlmClient + Send + Sync> =
            Arc::new(FixedLlm("summary".into()));
        let result = compact_history(&history, None, &llm, 100, 0, 3000)
            .await
            .unwrap()
            .expect("should compact");

        assert_eq!(result.summary, "summary");
        // The summarization prompt was too large for the whole prefix, so some
        // oldest messages were dropped.
        assert!(result.dropped_before > 0);
        // The summarization slice always starts at a user message.
        assert!(matches!(history[result.dropped_before], Message::User(_)));
    }
}
