//! Approval flow for proposed changes.
//!
//! When the agent produces [`ProposedChange`]s that are not auto-approved by
//! the session or provider permissions, an [`ApprovalRequest`] is built and
//! stored as the session's pending approval. The user (via the transport
//! layer) resolves the request through [`AgentSession::resolve_approval`],
//! which either executes the changes or records the denial in history.

use serde_json::Value;

use crate::change_executor;
use crate::llm;
use crate::tool::{
    ChangeOperation, ChangeReceipt, InferredField, InvalidArgsError, ProposedChange, TargetKind,
    ToolError,
};
use crate::{AgentError, AgentSession, ApprovalRequest, ApprovalResult};

impl AgentSession {
    pub(crate) fn is_auto_approved(&self, target: &str, operation: &str) -> Result<bool, AgentError> {
        let session = self.session_permissions.lock()?;
        if let Some(allowed) = session.resolve(target, operation) {
            return Ok(allowed);
        }
        Ok(self
            .config
            .read()?
            .llm
            .permissions
            .is_allowed(target, operation))
    }

    pub(crate) fn make_approval_request(
        &self,
        changes: Vec<ProposedChange>,
        inferred_fields: Vec<InferredField>,
        why: Option<String>,
        warnings: Vec<String>,
    ) -> Result<Option<ApprovalRequest>, AgentError> {
        if changes.is_empty() {
            return Ok(None);
        }
        let mut sequence = self.approval_sequence.lock()?;
        *sequence += 1;
        let id = format!("{}-approval-{}", self.session_id, *sequence);
        tracing::info!(session_id = %self.session_id, approval_id = %id, changes = changes.len(), "approval requested");
        let request = ApprovalRequest {
            id,
            why: why.unwrap_or_else(|| "ユーザーの承認が必要な変更です".to_owned()),
            changes,
            inferred_fields,
            warnings,
            expires_at: jiff::Timestamp::now()
                .checked_add(jiff::Span::new().minutes(5))
                .expect("valid approval expiry"),
        };
        *self.pending_approval.lock()? = Some(request.clone());
        Ok(Some(request))
    }

    pub fn build_approval_resolution_message(
        approved: bool,
        changes: &[ProposedChange],
        auto: bool,
    ) -> String {
        let header = match (approved, auto) {
            (true, true) => "以下の変更は自動承認され、適用されました。",
            (true, false) => "ユーザーは以下の提案を承認し、変更を適用しました。",
            (false, _) => "ユーザーは以下の提案を拒否しました。",
        };
        let mut lines = vec![header.to_string()];
        for change in changes {
            lines.push(format!("- {}", change.description));
        }
        lines.join("\n")
    }

    pub fn pending_approval(&self) -> Option<ApprovalRequest> {
        self.pending_approval.lock().ok()?.clone()
    }

    pub async fn resolve_approval(
        &self,
        id: &str,
        approve: bool,
    ) -> Result<ApprovalResult, AgentError> {
        let _guard = self.turn_lock.lock().await;
        tracing::info!(session_id = %self.session_id, approval_id = %id, approved = approve, "resolving approval");
        let request = {
            let mut pending = self.pending_approval.lock()?;
            let current = pending.as_ref().ok_or_else(|| {
                AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::new(
                    "approval_id",
                    "not found",
                )))
            })?;
            if current.id != id {
                return Err(AgentError::Tool(ToolError::InvalidArgs(
                    InvalidArgsError::new("approval_id", "mismatch"),
                )));
            }
            pending.take().expect("approval was present")
        };
        if jiff::Timestamp::now() >= request.expires_at {
            return Err(AgentError::Tool(ToolError::Cancelled));
        }
        if !approve {
            tracing::info!(session_id = %self.session_id, approval_id = %id, "approval denied");
            let system_estimate = self.last_system_estimate.lock()?.unwrap_or(0);
            let resolution_message =
                Self::build_approval_resolution_message(false, &request.changes, false);
            let mut local = self.history.lock()?.clone();
            local.push(llm::Message::User(resolution_message));
            self.replace_history(local, None, system_estimate)?;
            return Ok(ApprovalResult {
                id: id.to_owned(),
                approved: false,
                changes: Vec::new(),
                schedule_dirty: *self.schedule_dirty.lock()?,
            });
        }
        self.execute_approved_changes(request, false).await
    }

    pub(crate) async fn execute_approved_changes(
        &self,
        request: ApprovalRequest,
        auto: bool,
    ) -> Result<ApprovalResult, AgentError> {
        tracing::info!(session_id = %self.session_id, approval_id = %request.id, count = request.changes.len(), auto, "executing approved changes");
        let changes_for_message = request.changes.clone();
        let schedule_commit = request.changes.iter().any(|change| {
            change.target.kind == TargetKind::Schedule
                && matches!(
                    change.operation,
                    ChangeOperation::Generate | ChangeOperation::Reschedule
                )
        });
        let mut receipts = Vec::new();
        let mut schedule_dirty = *self.schedule_dirty.lock()?;
        let mut execution_error = None;
        for (idx, change) in request.changes.into_iter().enumerate() {
            let args = change.arguments.clone().unwrap_or_default();
            let operation_id = format!("{}:{idx}", request.id);
            match self
                .execute_proposed_change(&change, args, Some(&operation_id))
                .await
            {
                Ok(receipt) => {
                    schedule_dirty |=
                        matches!(change.target.kind, TargetKind::Task | TargetKind::Habit);
                    receipts.push(receipt);
                }
                Err(e) => {
                    execution_error = Some((change, e));
                    break;
                }
            }
        }
        if schedule_commit && execution_error.is_none() {
            schedule_dirty = false;
        }
        *self.schedule_dirty.lock()? = schedule_dirty;
        let system_estimate = self.last_system_estimate.lock()?.unwrap_or(0);
        if let Some((change, e)) = execution_error {
            let error_message = if auto {
                format!(
                    "自動承認された変更の適用中にエラーが発生しました: {}\n- {}",
                    e, change.description
                )
            } else {
                format!(
                    "ユーザーは以下の提案を承認しましたが、変更の適用中にエラーが発生しました: {}\n- {}",
                    e, change.description
                )
            };
            let mut local = self.history.lock()?.clone();
            local.push(llm::Message::User(error_message));
            self.replace_history(local, None, system_estimate)?;
            tracing::error!(session_id = %self.session_id, approval_id = %request.id, error = %e, "approved change failed");
            return Err(e);
        }
        let resolution_message =
            Self::build_approval_resolution_message(true, &changes_for_message, auto);
        let mut local = self.history.lock()?.clone();
        local.push(llm::Message::User(resolution_message));
        self.replace_history(local, None, system_estimate)?;
        tracing::info!(session_id = %self.session_id, approval_id = %request.id, count = receipts.len(), "approved changes executed");
        Ok(ApprovalResult {
            id: request.id,
            approved: true,
            changes: receipts,
            schedule_dirty,
        })
    }

    pub(crate) async fn execute_proposed_change(
        &self,
        change: &ProposedChange,
        args: Value,
        operation_id: Option<&str>,
    ) -> Result<ChangeReceipt, AgentError> {
        let mut args = args;
        let steps_value = args.get("steps").cloned();
        if let Some(obj) = args.as_object_mut() {
            obj.remove("steps");
        }
        let executor =
            change_executor::dispatch(change.target.kind, change.operation).ok_or_else(|| {
                AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::no_field(
                    "unsupported proposal",
                )))
            })?;
        let target = executor
            .fetch_target(&change_executor::FetchContext {
                session: self,
                change,
            })
            .await?;
        if let Some(observed) = &change.observed_updated_at
            && target
                .current_updated_at
                .as_ref()
                .map(|t| t.to_string())
                .as_deref()
                != Some(observed.as_str())
        {
            return Err(AgentError::Tool(ToolError::Conflict(
                "target changed after proposal".into(),
            )));
        }
        let ctx = change_executor::ChangeContext {
            session: self,
            target_id: target.target_id,
            args,
            steps_value,
            existing_habit: target.existing_habit,
            operation_id,
            change,
        };
        let outcome = executor.execute(&ctx).await?;
        let result = outcome.result_id;
        let before = outcome.before;
        let after = outcome.after;
        let target_revision = outcome.target_revision;
        if change.target.kind == TargetKind::Skill {
            self.clear_skills_index()?;
        }
        Ok(ChangeReceipt {
            operation: change.operation,
            target: crate::tool::ReceiptTarget {
                target_type: change.target.kind,
                target_id: result,
            },
            before,
            after,
            target_revision,
            ..Default::default()
        })
    }
}
