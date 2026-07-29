//! History trimming and compaction for [`AgentSession`].
//!
//! The agent keeps a running conversation history that must fit within the
//! model's context window. Two mechanisms cooperate:
//!
//! * **Trimming** ([`AgentSession::trim_messages`] /
//!   [`AgentSession::replace_history`]) drops the oldest complete turns when
//!   the token budget is exceeded. It uses the provider-reported prompt token
//!   count to calibrate per-message estimates.
//! * **Compaction** ([`AgentSession::maybe_compact`]) summarizes older turns
//!   into a compact summary that is injected into the system prompt, keeping
//!   recent turns verbatim.
//!
//! [`AgentSession::truncate_history`] is the user-facing entry point for
//! discarding turns by index (e.g. after an edit).

use std::collections::BTreeSet;

use crate::{AgentError, AgentSession, InvalidArgsError, ToolError, llm};

impl AgentSession {
    pub(crate) fn active_tool_names(&self) -> BTreeSet<String> {
        let mut names = self.registry.direct_tool_names();
        names.extend(self.discovered_tools.lock().unwrap().iter().cloned());
        names
    }

    /// Compacts older conversation history when it exceeds the configured
    /// context-window budget. The compacted summary is stored and injected
    /// into the system prompt on subsequent turns.
    pub(crate) async fn maybe_compact(&self) -> Result<(), AgentError> {
        let (max_context_tokens, settings) = {
            let cfg = self.config.read().unwrap();
            let settings = cfg.llm.compaction;
            let reserve = settings.reserve_tokens;
            let keep_recent = settings.keep_recent_tokens;
            if !settings.enabled
                || cfg.llm.max_context_tokens.saturating_sub(reserve) <= keep_recent
            {
                return Ok(());
            }
            (cfg.llm.max_context_tokens, settings)
        };

        let history = self.history.lock().unwrap().clone();
        if history.is_empty() {
            return Ok(());
        }

        let system_estimate = {
            let last = *self.last_system_estimate.lock().unwrap();
            match last {
                Some(est) => est,
                None => llm::Message::System(self.build_system_prompt().await).estimate_tokens(),
            }
        };
        let active_names = self.active_tool_names();
        let tools_estimate = self.registry.definitions_estimate_tokens_for(&active_names);
        let system_and_tools = system_estimate + tools_estimate;

        if !crate::compact::should_compact(
            &history,
            system_and_tools,
            max_context_tokens,
            &settings,
        ) {
            return Ok(());
        }

        let previous_summary = self.compaction_summary.lock().unwrap().clone();
        let llm = self.llm.read().unwrap().clone();
        let keep_recent = settings.keep_recent_tokens;
        // Reserve response tokens for the summary itself.
        let max_prompt_tokens = max_context_tokens.saturating_sub(settings.reserve_tokens);

        match crate::compact::compact_history(
            &history,
            previous_summary.as_deref(),
            &llm,
            keep_recent,
            system_and_tools,
            max_prompt_tokens,
        )
        .await
        {
            Ok(Some(result)) => {
                if result.dropped_before > 0 {
                    tracing::warn!(
                        session_id = %self.session_id,
                        dropped_before = result.dropped_before,
                        "compaction dropped oldest messages that did not fit in the summarization prompt"
                    );
                }
                tracing::info!(
                    session_id = %self.session_id,
                    first_kept_index = result.first_kept_index,
                    dropped_before = result.dropped_before,
                    tokens_before = result.tokens_before,
                    "context compacted"
                );
                let kept: Vec<_> = history.into_iter().skip(result.first_kept_index).collect();
                *self.history.lock().unwrap() = kept;
                *self.compaction_summary.lock().unwrap() = Some(result.summary);
                // The system prompt now includes the new summary; force a
                // re-estimate on the next turn so compaction decisions remain
                // accurate.
                *self.last_system_estimate.lock().unwrap() = None;
                *self.last_prompt_tokens.lock().unwrap() = None;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    session_id = %self.session_id,
                    error = %e,
                    "context compaction failed; falling back to truncation"
                );
            }
        }

        Ok(())
    }

    pub async fn truncate_history(
        &self,
        turn_index: usize,
        after_user: bool,
    ) -> Result<(), AgentError> {
        let _guard = self.turn_lock.lock().await;

        let mut history = self.history.lock().unwrap();
        let user_positions: Vec<usize> = history
            .iter()
            .enumerate()
            .filter(|(_, m)| matches!(m, llm::Message::User(_)))
            .map(|(i, _)| i)
            .collect();
        let &user_position = user_positions.get(turn_index).ok_or_else(|| {
            AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::new(
                "turn_index",
                format!("out of range: {turn_index}"),
            )))
        })?;

        if after_user {
            history.truncate(user_position + 1);
        } else if let Some(&next) = user_positions.get(turn_index + 1) {
            history.truncate(next);
        }
        // If this is the latest turn and we want after_user=false, nothing to truncate.

        *self.pending_approval.lock().unwrap() = None;
        // Truncated history may have a different token count and schedule state.
        *self.last_prompt_tokens.lock().unwrap() = None;
        *self.schedule_dirty.lock().unwrap() = false;
        Ok(())
    }

    pub(crate) fn trim_messages(&self, mut messages: Vec<llm::Message>) -> Vec<llm::Message> {
        let system_message = if messages
            .first()
            .map(|m| matches!(m, llm::Message::System(_)))
            == Some(true)
        {
            Some(messages.remove(0))
        } else {
            None
        };

        let system_estimate = system_message
            .as_ref()
            .map(|m| m.estimate_tokens())
            .unwrap_or(0);
        let active_names = self.active_tool_names();
        let tools_estimate = self.registry.definitions_estimate_tokens_for(&active_names);
        let last_estimate = messages.last().map_or(0, |m| m.estimate_tokens());
        let config = self.config.read().unwrap();
        let target = config
            .llm
            .max_context_tokens
            .saturating_sub(system_estimate)
            .saturating_sub(tools_estimate)
            .max(last_estimate);

        let current = messages.iter().map(|m| m.estimate_tokens()).sum::<usize>();
        let actual_local = {
            let last = *self.last_prompt_tokens.lock().unwrap();
            last.map(|p| {
                p.saturating_sub(system_estimate)
                    .saturating_sub(tools_estimate)
            })
            .unwrap_or(current)
        };

        let mut messages = trim_to_target(messages, target, current, actual_local, last_estimate);

        if let Some(system) = system_message {
            messages.insert(0, system);
        }
        messages
    }

    pub(crate) fn replace_history(
        &self,
        local: Vec<llm::Message>,
        prompt_tokens: Option<usize>,
        system_estimate: usize,
    ) {
        let active_names = self.active_tool_names();
        let tools_estimate = self.registry.definitions_estimate_tokens_for(&active_names);
        let last_estimate = local.last().map_or(0, |m| m.estimate_tokens());
        let config = self.config.read().unwrap();
        let target = config
            .llm
            .max_context_tokens
            .saturating_sub(system_estimate)
            .saturating_sub(tools_estimate)
            .max(last_estimate);
        let current = local.iter().map(|m| m.estimate_tokens()).sum::<usize>();
        let actual_local = prompt_tokens
            .map(|p| {
                p.saturating_sub(system_estimate)
                    .saturating_sub(tools_estimate)
            })
            .unwrap_or(current);

        // Both trim_messages and replace_history delegate the trimming loop to
        // trim_to_target so the two paths stay consistent. When the provider
        // reports we are within budget (actual_local > 0 && actual_local <=
        // target), trim_to_target computes adjusted_target >= current and
        // returns the messages untouched. When actual_local == 0 (degenerate
        // provider count from saturating_sub), trim_to_target falls back to
        // `target` and trims conservatively if current > target — matching
        // trim_messages' behavior for the same case.
        let local = trim_to_target(local, target, current, actual_local, last_estimate);

        let mut guard = self.history.lock().unwrap();
        *guard = local;
    }
}

/// Trim `messages` (without the system message) to fit within a token budget.
///
/// `target` is the local token budget (max context minus system + tools).
/// `current` is the estimated token count of `messages`, and `actual_local` is
/// the provider-reported count for the same span when available (otherwise
/// equal to `current`). The provider count calibrates the per-message estimate:
/// when the estimate overshoots the provider count, the adjusted target is
/// scaled up proportionally so we don't over-trim.
///
/// `last_estimate` is the token estimate of the final message; the adjusted
/// target is clamped to at least this so the current turn's last message is
/// never dropped.
///
/// Drops the oldest complete turns first. The last user message and the
/// assistant/tool-result messages that belong to the current turn are always
/// preserved.
fn trim_to_target(
    mut messages: Vec<llm::Message>,
    target: usize,
    current: usize,
    actual_local: usize,
    last_estimate: usize,
) -> Vec<llm::Message> {
    let adjusted_target = {
        let base = if actual_local > 0 {
            (target as f64 * current as f64 / actual_local as f64) as usize
        } else {
            target
        };
        base.max(last_estimate)
    };

    let mut estimate = current;
    while estimate > adjusted_target && !messages.is_empty() {
        // Never remove the last user message or the assistant/tool-result
        // messages that belong to the current turn.
        let last_user_start = messages
            .iter()
            .enumerate()
            .rfind(|(_, m)| matches!(m, llm::Message::User(_)))
            .map(|(i, _)| i)
            .unwrap_or(0);

        let drain_end = if messages.len() > 1 {
            let start = messages
                .iter()
                .enumerate()
                .find(|(_, m)| matches!(m, llm::Message::User(_)))
                .map(|(i, _)| i)
                .unwrap_or(0);
            let next = if start == 0 {
                messages
                    .iter()
                    .enumerate()
                    .skip(1)
                    .find(|(_, m)| matches!(m, llm::Message::User(_)))
                    .map(|(i, _)| i)
                    .unwrap_or(messages.len())
            } else {
                start
            };
            next.min(last_user_start)
        } else {
            1.min(last_user_start)
        };

        if messages.drain(0..drain_end).count() == 0 {
            break;
        }
        estimate = messages.iter().map(|m| m.estimate_tokens()).sum();
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::trim_to_target;
    use crate::llm::Message;

    // estimate_tokens = chars.div_ceil(4) + 4.
    // "aaaa" (4 chars)  → 5 tokens
    // "aaaaaaaa" (8 chars) → 6 tokens
    fn user(s: &str) -> Message {
        Message::User(s.to_string())
    }

    #[test]
    fn trim_to_target_no_trim_when_within_budget() {
        // current (10) <= adjusted_target (25): no trimming.
        let msgs = vec![user("aaaa"), user("aaaa")];
        let result = trim_to_target(msgs, 20, 10, 8, 5);
        assert_eq!(result.len(), 2, "should not trim when within budget");
    }

    #[test]
    fn trim_to_target_trims_when_actual_local_equals_current() {
        // adjusted_target = target (10) = current (15) → trim one turn.
        let msgs = vec![user("aaaa"), user("aaaa"), user("aaaa")];
        let result = trim_to_target(msgs, 10, 15, 15, 5);
        assert_eq!(result.len(), 2, "should trim one oldest turn");
    }

    #[test]
    fn trim_to_target_falls_back_to_target_when_actual_local_is_zero() {
        // Degenerate provider count (saturating_sub → 0). adjusted_target
        // falls back to `target` (10), and current (15) > 10 → trim.
        let msgs = vec![user("aaaa"), user("aaaa"), user("aaaa")];
        let result = trim_to_target(msgs, 10, 15, 0, 5);
        assert_eq!(
            result.len(),
            2,
            "actual_local == 0 should fall back to target and trim conservatively"
        );
    }

    #[test]
    fn trim_to_target_no_trim_when_actual_local_zero_and_current_within_target() {
        // actual_local == 0, but current (10) <= target (20) → no trim.
        let msgs = vec![user("aaaa"), user("aaaa")];
        let result = trim_to_target(msgs, 20, 10, 0, 5);
        assert_eq!(result.len(), 2, "should not trim when current <= target");
    }

    #[test]
    fn trim_to_target_trims_more_when_estimate_overcounts_provider() {
        // actual_local (20) > current (15): provider says we used more than
        // our estimate predicts. adjusted_target = 10*15/20 = 7, so we trim
        // aggressively down to the last message.
        let msgs = vec![user("aaaa"), user("aaaa"), user("aaaa")];
        let result = trim_to_target(msgs, 10, 15, 20, 5);
        assert_eq!(result.len(), 1, "should trim down to last message");
    }

    #[test]
    fn trim_to_target_trims_less_when_estimate_undercounts_provider() {
        // actual_local (12) < current (15): estimate overcounts, so
        // adjusted_target = 10*15/12 = 12, only one turn trimmed.
        let msgs = vec![user("aaaa"), user("aaaa"), user("aaaa")];
        let result = trim_to_target(msgs, 10, 15, 12, 5);
        assert_eq!(result.len(), 2, "should trim only one turn");
    }

    #[test]
    fn trim_to_target_preserves_last_message_when_target_below_last_estimate() {
        // target (3) < last_estimate (6): adjusted_target clamped to 6,
        // so the last message is never dropped.
        let msgs = vec![user("aaaa"), user("aaaaaaaa")];
        let result = trim_to_target(msgs, 3, 11, 11, 6);
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], Message::User(t) if t == "aaaaaaaa"));
    }
}
