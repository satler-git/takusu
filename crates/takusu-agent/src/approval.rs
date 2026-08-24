//! Approval flow for proposed changes.
//!
//! When the agent produces [`ProposedChange`]s that are not auto-approved by
//! the session or provider permissions, an [`ApprovalRequest`] is built and
//! stored as the session's pending approval. The user (via the transport
//! layer) resolves the request through [`AgentSession::resolve_approval`],
//! which either executes the changes or records the denial in history.

use serde_json::Value;
use takusu_client::{SaveScheduleRequest, ScheduleEntry, ScheduleRow};

use crate::change_executor;
use crate::llm;
use crate::tool::{
    ChangeOperation, ChangeReceipt, InferredField, InvalidArgsError, ProposalDecision,
    ProposedChange, TargetKind, ToolError,
};
use crate::{AgentError, AgentSession, ApprovalRequest, ApprovalResult};

/// Default `why` text for an approval request. Kept in one place so the readback
/// layer can reliably filter out the generic boilerplate.
pub(crate) const DEFAULT_APPROVAL_WHY: &str = "ユーザーの承認が必要な変更です";

impl AgentSession {
    pub(crate) fn is_auto_approved(
        &self,
        target: TargetKind,
        operation: ChangeOperation,
    ) -> Result<bool, AgentError> {
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
        let mut changes = changes;
        self.fill_proposal_ids(&mut changes);
        let mut sequence = self.approval_sequence.lock()?;
        *sequence += 1;
        let id = format!("{}-approval-{}", self.session_id, *sequence);
        tracing::info!(session_id = %self.session_id, approval_id = %id, changes = changes.len(), "approval requested");
        let request = ApprovalRequest {
            id,
            why: why.unwrap_or_else(|| DEFAULT_APPROVAL_WHY.to_owned()),
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
        approved: &[ProposedChange],
        denied: &[ProposedChange],
        auto: bool,
    ) -> String {
        let mut lines = Vec::new();
        if auto && !approved.is_empty() && denied.is_empty() {
            lines.push("以下の変更は自動承認され、適用されました。".to_string());
            for change in approved {
                lines.push(format!("- {}", change.description));
            }
        } else if !approved.is_empty() && !denied.is_empty() {
            lines.push("ユーザーは以下の提案を一部承認し、一部を拒否しました。".to_string());
            lines.push("承認:".to_string());
            for change in approved {
                lines.push(format!("- {}", change.description));
            }
            lines.push("拒否:".to_string());
            for change in denied {
                lines.push(format!("- {}", change.description));
            }
        } else if !approved.is_empty() {
            lines.push("ユーザーは以下の提案を承認し、変更を適用しました。".to_string());
            for change in approved {
                lines.push(format!("- {}", change.description));
            }
        } else if !denied.is_empty() {
            lines.push("ユーザーは以下の提案を拒否しました。".to_string());
            for change in denied {
                lines.push(format!("- {}", change.description));
            }
        } else {
            lines.push("提案はすべて拒否されました。".to_string());
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
        proposals: Option<Vec<ProposalDecision>>,
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

        let (approved_changes, denied_changes) = match proposals {
            None => {
                if approve {
                    (request.changes.clone(), Vec::new())
                } else {
                    (Vec::new(), request.changes.clone())
                }
            }
            Some(decisions) => {
                let mut decision_map = std::collections::HashMap::new();
                let mut seen_ids = std::collections::HashSet::new();
                for d in &decisions {
                    if !seen_ids.insert(d.proposal_id.clone()) {
                        return Err(AgentError::Tool(ToolError::InvalidArgs(
                            InvalidArgsError::new("proposals", "duplicate decision for a proposal"),
                        )));
                    }
                    decision_map.insert(d.proposal_id.clone(), d.approve);
                }
                let request_ids: std::collections::HashSet<String> = request
                    .changes
                    .iter()
                    .filter_map(|c| c.proposal_id.clone())
                    .collect();
                for change in &request.changes {
                    if change.proposal_id.is_none()
                        || !decision_map.contains_key(change.proposal_id.as_ref().unwrap())
                    {
                        return Err(AgentError::Tool(ToolError::InvalidArgs(
                            InvalidArgsError::new("proposals", "missing decision for a proposal"),
                        )));
                    }
                }
                for id in decision_map.keys() {
                    if !request_ids.contains(id) {
                        return Err(AgentError::Tool(ToolError::InvalidArgs(
                            InvalidArgsError::new("proposals", "unknown proposal_id in decisions"),
                        )));
                    }
                }
                let mut approved = Vec::new();
                let mut denied = Vec::new();
                for change in request.changes.iter() {
                    let id = change.proposal_id.as_ref().unwrap();
                    if decision_map.get(id).copied().unwrap_or(false) {
                        approved.push(change.clone());
                    } else {
                        denied.push(change.clone());
                    }
                }
                (approved, denied)
            }
        };

        if approved_changes.is_empty() {
            tracing::info!(session_id = %self.session_id, approval_id = %id, "approval denied");
            let system_estimate = self.last_system_estimate.lock()?.unwrap_or(0);
            let resolution_message =
                Self::build_approval_resolution_message(&[], &denied_changes, false);
            let mut local = self.history.lock()?.clone();
            local.push(llm::Message::User(resolution_message));
            self.replace_history(local, None, system_estimate)?;
            return Ok(ApprovalResult {
                id: id.to_owned(),
                approved: false,
                changes: Vec::new(),
                schedule_dirty: *self.schedule_dirty.lock()?,
                presentation: None,
            });
        }

        let mut request = request;
        request.changes = approved_changes;
        self.execute_approved_changes(request, denied_changes, false)
            .await
    }

    pub(crate) async fn execute_approved_changes(
        &self,
        mut request: ApprovalRequest,
        denied_changes: Vec<ProposedChange>,
        auto: bool,
    ) -> Result<ApprovalResult, AgentError> {
        tracing::info!(session_id = %self.session_id, approval_id = %request.id, count = request.changes.len(), auto, "executing approved changes");

        // Reorder so coverage confirmations execute last. This guarantees that
        // a batch containing `generate_schedule` / `reschedule` commits the
        // schedule before `coverage_confirm` records a confirmation, so the
        // confirmation is stamped with the new schedule revision (WI-16).
        {
            let (non_coverage, coverage): (Vec<_>, Vec<_>) = request
                .changes
                .into_iter()
                .partition(|c| c.target.kind != TargetKind::Coverage);
            request.changes = non_coverage.into_iter().chain(coverage).collect();
        }

        let changes_for_message = request.changes.clone();
        let schedule_commit = request.changes.iter().any(|change| {
            change.target.kind == TargetKind::Schedule
                && matches!(
                    change.operation,
                    ChangeOperation::Generate | ChangeOperation::Reschedule | ChangeOperation::Settle
                )
        });

        // If the batch may replace the schedule, snapshot it so we can restore
        // on partial failure (WI-16).
        let before_schedule: Option<ScheduleRow> = if schedule_commit {
            match self.client().get_schedule().await {
                Ok(row) => Some(row),
                Err(e) => {
                    tracing::warn!(session_id = %self.session_id, %e, "failed to snapshot schedule before batch; rollback will not be able to restore it");
                    None
                }
            }
        } else {
            None
        };

        let mut receipts = Vec::new();
        let mut schedule_dirty = *self.schedule_dirty.lock()?;
        let mut execution_error = None;
        for (idx, change) in request.changes.iter().enumerate() {
            let args = change.arguments.clone().unwrap_or_default();
            let operation_id = format!("{}:{idx}", request.id);
            match self
                .execute_proposed_change(change, args, Some(&operation_id))
                .await
            {
                Ok(receipt) => {
                    schedule_dirty |=
                        matches!(change.target.kind, TargetKind::Task | TargetKind::Habit);
                    receipts.push(receipt);
                }
                Err(e) => {
                    execution_error = Some((change.clone(), e));
                    break;
                }
            }
        }

        if let Some((change, e)) = execution_error {
            let executed = &request.changes[..receipts.len()];
            // WI-3: completed tasks before the failure are already saved, so
            // their overrun check-ins must not be lost. Do this before rolling
            // back the partially committed creates.
            self.record_completion_check_ins(&receipts)?;
            // Drop check-ins for tasks deleted before the failure.
            self.clear_check_ins_for_deleted_tasks(&receipts)?;
            // WI-17: long snooze/move reasons for operations that succeeded.
            if let Err(error) = self.record_postpone_reasons_from_changes(executed).await {
                tracing::warn!(session_id = %self.session_id, %error, "failed to record postpone reasons from approved changes");
            }
            if let Err(rollback_err) = self
                .compensate_failed_batch(&request, &receipts, before_schedule.as_ref())
                .await
            {
                tracing::error!(session_id = %self.session_id, %rollback_err, "rollback after partial approval failure failed");
            }

            let system_estimate = self.last_system_estimate.lock()?.unwrap_or(0);
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

        // Success path: post-processing and intake state updates.
        self.record_completion_check_ins(&receipts)?;
        self.clear_check_ins_for_deleted_tasks(&receipts)?;
        let executed = &request.changes[..receipts.len()];
        if let Err(error) = self.record_postpone_reasons_from_changes(executed).await {
            tracing::warn!(session_id = %self.session_id, %error, "failed to record postpone reasons from approved changes");
        }
        if schedule_commit {
            schedule_dirty = false;
        }
        *self.schedule_dirty.lock()? = schedule_dirty;
        self.update_intake_state_from_batch(&request, &receipts).await?;

        let system_estimate = self.last_system_estimate.lock()?.unwrap_or(0);
        let resolution_message =
            Self::build_approval_resolution_message(&changes_for_message, &denied_changes, auto);
        let mut local = self.history.lock()?.clone();
        local.push(llm::Message::User(resolution_message));
        self.replace_history(local, None, system_estimate)?;
        tracing::info!(session_id = %self.session_id, approval_id = %request.id, count = receipts.len(), "approved changes executed");
        let presentation = crate::Presentation::from_change_receipts(&receipts)
            .or_else(|| Some(crate::Presentation::Text { text: "承認しました。".into() }));
        Ok(ApprovalResult {
            id: request.id,
            approved: true,
            changes: receipts,
            schedule_dirty,
            presentation,
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

    /// Best-effort rollback for a partially failed approval batch (WI-16).
    ///
    /// Deletes newly created tasks/habits and, if the batch contained a schedule
    /// replacement, restores the previous schedule. Coverage confirmations and
    /// operations other than creates cannot be rolled back by this path.
    async fn compensate_failed_batch(
        &self,
        request: &ApprovalRequest,
        receipts: &[ChangeReceipt],
        before_schedule: Option<&ScheduleRow>,
    ) -> Result<(), AgentError> {
        for (change, receipt) in request.changes.iter().zip(receipts) {
            match (change.target.kind, change.operation) {
                (TargetKind::Task, ChangeOperation::Create) => {
                    if let Err(e) = self.client().delete_task(&receipt.target.target_id).await {
                        tracing::warn!(
                            session_id = %self.session_id,
                            %e,
                            "rollback: failed to delete created task {} (may already be gone)",
                            receipt.target.target_id
                        );
                    }
                }
                (TargetKind::Habit, ChangeOperation::Create) => {
                    if let Err(e) = self.client().delete_habit(&receipt.target.target_id).await {
                        tracing::warn!(
                            session_id = %self.session_id,
                            %e,
                            "rollback: failed to delete created habit {} (may already be gone)",
                            receipt.target.target_id
                        );
                    }
                }
                _ => {}
            }
        }

        if let Some(before) = before_schedule {
            let entries = before.schedule.0.clone();
            let mark_scheduled_task_ids = entries
                .iter()
                .map(|e: &ScheduleEntry| e.task_id.clone())
                .collect();
            let horizon_task_ids = before.horizon_task_ids.0.clone();
            let restore = SaveScheduleRequest {
                entries,
                mark_scheduled_task_ids,
                horizon_task_ids,
            };
            if let Err(e) = self.client().replace_schedule(&restore).await {
                tracing::warn!(
                    session_id = %self.session_id,
                    %e,
                    "rollback: failed to restore previous schedule"
                );
            }
        }

        Ok(())
    }

    /// Update `IntakeState` after a successful batch (WI-16).
    ///
    /// - Adds display ids of newly created tasks/habits that belong to the
    ///   current intake proposal to `collected_ids`.
    /// - Clears `coverage_pending` if a coverage confirmation in the same batch
    ///   was committed.
    async fn update_intake_state_from_batch(
        &self,
        request: &ApprovalRequest,
        receipts: &[ChangeReceipt],
    ) -> Result<(), AgentError> {
        let mut state = self.get_intake_state()?;
        let mut coverage_committed = false;

        for (change, receipt) in request.changes.iter().zip(receipts) {
            if change.target.kind == TargetKind::Coverage
                && change.operation == ChangeOperation::Confirm
            {
                coverage_committed = true;
                continue;
            }

            let matches_intake = state
                .proposal_id
                .as_deref()
                .zip(change.proposal_id.as_deref())
                .is_some_and(|(a, b)| a == b);
            if !matches_intake {
                continue;
            }
            if change.operation != ChangeOperation::Create {
                continue;
            }

            let display_id = receipt
                .after
                .as_ref()
                .and_then(|v| v.as_object())
                .and_then(|obj| obj.get("display_id"))
                .and_then(|v| v.as_i64());
            if let Some(id) = display_id {
                let id_str = match change.target.kind {
                    TargetKind::Task => format!("#{id}"),
                    TargetKind::Habit => format!("h{id}"),
                    _ => id.to_string(),
                };
                if !state.collected_ids.contains(&id_str) {
                    state.collected_ids.push(id_str);
                }
            }
        }

        if coverage_committed {
            state.coverage_pending = false;
        }

        self.set_intake_state(state)?;
        Ok(())
    }

    /// Queue one-time postpone-reason check-ins for approved snoozes/moves that
    /// exceed the short-snooze threshold.
    pub(crate) async fn record_postpone_reasons_from_changes(
        &self,
        changes: &[ProposedChange],
    ) -> Result<(), AgentError> {
        for change in changes {
            if change.target.kind != TargetKind::Task {
                continue;
            }
            if !matches!(change.operation, ChangeOperation::Move | ChangeOperation::Snooze) {
                continue;
            }
            let Some(args) = &change.arguments else {
                continue;
            };

            let minutes = if let Some(minutes) = args.get("snooze_minutes").and_then(|v| v.as_i64())
            {
                minutes
            } else if let Some(target) = args
                .get("snooze_target")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<takusu_types::Timestamp>().ok())
            {
                let now = takusu_types::Timestamp::from(jiff::Timestamp::now());
                ((target.as_second() - now.as_second()) / 60).max(0)
            } else if let Some(start_at) = args
                .get("start_at")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<takusu_types::Timestamp>().ok())
            {
                let now = takusu_types::Timestamp::from(jiff::Timestamp::now());
                ((start_at.as_second() - now.as_second()) / 60).max(0)
            } else {
                continue;
            };

            if !crate::contact_policy::should_ask_postpone_reason(minutes) {
                continue;
            }

            let task = self.client().get_task(&change.target.display_id).await?;
            let pending = crate::tools::comments::PendingPostponeReason {
                task_id: task.id,
                display_id: task.display_id,
                title: task.title,
                snooze_minutes: minutes,
                delivered: false,
            };
            self.enqueue_pending_postpone_reason(pending)?;
        }
        Ok(())
    }
}
