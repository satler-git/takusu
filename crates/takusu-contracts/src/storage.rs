//! Async storage backend trait.
//!
//! Implemented by `SqliteStorage` (direct `sqlx`) and `WorkersStorage`
//! (reqwest against the Cloudflare Worker + D1). The local server injects
//! the chosen backend into its axum router.

use async_trait::async_trait;

use crate::error::StorageError;
use crate::model::*;
use takusu_types::Timestamp;
use takusu_types::TokenClaims;

pub type StorageResult<T> = Result<T, StorageError>;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Storage: Send + Sync + 'static {
    async fn verify_token(&self, token: &str) -> StorageResult<Option<TokenClaims>>;

    async fn list_tasks(&self, query: &TaskQuery) -> StorageResult<Vec<TaskRow>>;
    async fn get_task(&self, id: &str) -> StorageResult<TaskRow>;
    async fn create_task(&self, body: &CreateTask) -> StorageResult<TaskRow>;
    async fn update_task(&self, id: &str, body: &UpdateTask) -> StorageResult<TaskRow>;
    async fn replace_task(&self, id: &str, body: &CreateTask) -> StorageResult<TaskRow>;
    async fn delete_task(&self, id: &str) -> StorageResult<()>;

    // ── Task comments (WI-1) ────────────────────────────────
    /// List a task's comments in ascending `seq` order.
    async fn list_comments(&self, task_id: &str) -> StorageResult<Vec<CommentRow>>;
    /// Append a comment to a task. `author` is decided by the caller (the
    /// server), never by the HTTP body.
    async fn create_comment(
        &self,
        task_id: &str,
        author: takusu_types::CommentAuthor,
        content: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<CommentRow>;
    /// Delete a comment (user-only operation).
    async fn delete_comment(&self, id: &str) -> StorageResult<()>;

    /// Check whether a task with the given iCal UID already exists.
    async fn task_exists_by_ical_uid(&self, uid: &str) -> StorageResult<bool>;

    async fn list_habits(&self) -> StorageResult<Vec<HabitRow>>;
    async fn get_habit(&self, id: &str) -> StorageResult<HabitRow>;
    async fn create_habit(&self, body: &CreateHabit) -> StorageResult<HabitRow>;
    async fn update_habit(&self, id: &str, body: &UpdateHabit) -> StorageResult<HabitRow>;
    async fn replace_habit(&self, id: &str, body: &CreateHabit) -> StorageResult<HabitRow>;
    async fn delete_habit(&self, id: &str) -> StorageResult<()>;

    // ── Habit scheduled spans (#303 / #503) ─────────────────
    /// List scheduled spans for a single habit.
    async fn list_habit_scheduled_spans(
        &self,
        habit_id: &str,
    ) -> StorageResult<Vec<HabitScheduledSpanRow>>;
    /// List scheduled spans for all habits (used by sync_habit_tasks).
    async fn list_all_habit_scheduled_spans(&self) -> StorageResult<Vec<HabitScheduledSpanRow>>;
    /// Create a scheduled span for a habit.
    async fn create_habit_scheduled_span(
        &self,
        habit_id: &str,
        body: &CreateHabitScheduledSpan,
    ) -> StorageResult<HabitScheduledSpanRow>;
    /// Delete a scheduled span by its id.
    async fn delete_habit_scheduled_span(&self, habit_id: &str, span_id: &str)
    -> StorageResult<()>;

    // ── Habit steps (#95) ─────────────────────────────────
    /// List steps for a single habit, ordered by position.
    async fn list_habit_steps(&self, habit_id: &str) -> StorageResult<Vec<HabitStepRow>>;
    /// List steps for all habits (used by sync_habit_tasks).
    async fn list_all_habit_steps(&self) -> StorageResult<Vec<HabitStepRow>>;
    /// Bulk-replace a habit's steps. Steps with an `id` matching an existing
    /// row are updated; steps without a matching `id` are created; existing
    /// steps absent from `steps` are deleted. Runs atomically. DAG validation
    /// (cycle detection, intra-habit references) is the caller's
    /// responsibility.
    async fn replace_habit_steps(
        &self,
        habit_id: &str,
        steps: &[HabitStepInput],
    ) -> StorageResult<Vec<HabitStepRow>>;

    /// Apply a habit estimate atomically: update the habit's avg/sigma and
    /// update only the estimate fields of the given steps. Steps not listed
    /// are left untouched. Backends that cannot provide atomic updates should
    /// implement this as a best-effort sequence of updates.
    async fn apply_habit_estimate(
        &self,
        habit_id: &str,
        avg_minutes: i64,
        sigma_minutes: i64,
        step_estimates: &[HabitStepEstimateInput],
    ) -> StorageResult<()> {
        let _ = (habit_id, avg_minutes, sigma_minutes, step_estimates);
        Err(StorageError::Internal(
            "apply_habit_estimate not implemented".into(),
        ))
    }

    async fn get_schedule(&self) -> StorageResult<Option<ScheduleRow>>;
    async fn save_schedule(&self, req: &SaveScheduleRequest) -> StorageResult<ScheduleRow>;
    async fn clear_schedule(&self) -> StorageResult<()>;

    async fn create_token(&self, label: Option<&str>) -> StorageResult<TokenCreateResponse>;
    async fn list_tokens(&self) -> StorageResult<Vec<TokenRow>>;
    async fn revoke_token(&self, id: i64) -> StorageResult<()>;

    async fn get_settings(&self) -> StorageResult<SettingsRow>;
    async fn update_settings(&self, body: &UpdateSettings) -> StorageResult<SettingsRow>;

    // ── Skills (#WI-6) ────────────────────────────────────
    async fn list_skills(&self) -> StorageResult<Vec<SkillRow>>;
    async fn get_skill(&self, slug: &str) -> StorageResult<SkillRow>;
    async fn create_skill(&self, body: &CreateSkill) -> StorageResult<SkillRow>;
    async fn update_skill(&self, slug: &str, body: &UpdateSkill) -> StorageResult<SkillRow>;
    async fn delete_skill(&self, slug: &str) -> StorageResult<()>;

    // ── Memory (#WI-7) ────────────────────────────────────
    async fn get_memory(&self, id: &str) -> StorageResult<MemoryRow>;
    async fn create_memory(
        &self,
        body: &CreateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow>;
    async fn update_memory(
        &self,
        id: &str,
        body: &UpdateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow>;
    async fn delete_memory(
        &self,
        id: &str,
        observed_revision: i64,
        operation_id: Option<&str>,
    ) -> StorageResult<()>;
    async fn search_memories(&self, query: &MemoryQuery) -> StorageResult<Vec<MemoryRow>>;
    async fn injectable_memories(
        &self,
        query: &MemoryInjectionQuery,
    ) -> StorageResult<MemoryInjectionResult>;
    async fn find_similar_tasks(
        &self,
        query: &SimilarTaskQuery,
    ) -> StorageResult<Vec<SimilarTaskRow>>;

    // ── Work sessions (#WI-9 / #1393) ─────────────────────
    async fn start_work_session(
        &self,
        body: &StartWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow>;
    async fn pause_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow>;
    async fn complete_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow>;
    async fn record_work_session_progress(
        &self,
        id: &str,
        body: &RecordWorkSessionProgress,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionProgressResult>;
    async fn get_work_session(&self, id: &str) -> StorageResult<WorkSessionRow>;
    async fn list_work_sessions(&self, task_id: Option<&str>)
    -> StorageResult<Vec<WorkSessionRow>>;
    async fn attach_work_session(
        &self,
        id: &str,
        body: &AttachWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow>;
    async fn convert_work_session(
        &self,
        id: &str,
        body: &ConvertWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<TaskRow>;
    async fn undo_work_session(
        &self,
        body: &UndoWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<UndoWorkSessionResult>;
    async fn get_task_progress(&self, id: &str) -> StorageResult<TaskProgress>;
    async fn get_estimator_state(&self, task_id: &str) -> StorageResult<Option<EstimatorStateRow>> {
        let _ = task_id;
        Ok(None)
    }

    // ── Planner event ledger (WI-9) ─────────────────────────
    /// Return the monotonic active-schedule revision used in event IDs.
    async fn get_schedule_revision(&self) -> StorageResult<i64> {
        Ok(0)
    }

    /// Read all inputs for a consistent planner-event evaluation in one
    /// backend call. The result must be a snapshot as atomic as the backend
    /// allows: the schedule revision, task list, active schedule, event
    /// ledger, and per-task progress must all come from the same logical
    /// point-in-time.
    async fn get_evaluation_inputs(&self) -> StorageResult<crate::model::EvaluationInputs>;

    /// List immutable ledger rows in creation order. `device_id` filters rows
    /// that have already been claimed by that device when provided.
    async fn list_event_ledger(
        &self,
        device_id: Option<&str>,
    ) -> StorageResult<Vec<EventLedgerRow>> {
        let _ = device_id;
        Ok(Vec::new())
    }

    /// Insert an event idempotently after the caller has validated its
    /// snapshot. Implementations must not replace an existing payload.
    async fn insert_event_ledger(
        &self,
        event: &EventLedgerInsert,
    ) -> StorageResult<EventLedgerRow> {
        let _ = event;
        Err(StorageError::Internal(
            "event ledger is not supported".into(),
        ))
    }

    /// Atomically commit the result of one planner evaluation. Implementations
    /// should verify that `schedule_revision` still matches the active schedule
    /// and insert all `events` in a single backend transaction (or as close to
    /// one as the backend supports). The default implementation checks the
    /// revision and falls back to inserting events one by one.
    async fn commit_event_evaluation(
        &self,
        schedule_revision: i64,
        events: &[EventLedgerInsert],
    ) -> StorageResult<()> {
        if self.get_schedule_revision().await? != schedule_revision {
            return Err(StorageError::Conflict("schedule revision changed".into()));
        }
        for event in events {
            self.insert_event_ledger(event).await?;
        }
        Ok(())
    }

    /// Claim delivery for one device. Repeating the same claim is a no-op.
    async fn claim_event_delivery(&self, device_id: &str, event_id: &str) -> StorageResult<bool> {
        let _ = (device_id, event_id);
        Err(StorageError::Internal(
            "event ledger is not supported".into(),
        ))
    }

    /// Transition delivery state without changing the immutable event payload.
    async fn update_event_delivery_state(
        &self,
        event_id: &str,
        state: EventDeliveryState,
    ) -> StorageResult<EventLedgerRow> {
        let _ = (event_id, state);
        Err(StorageError::Internal(
            "event ledger is not supported".into(),
        ))
    }

    async fn split_task(
        &self,
        id: &str,
        body: &SplitTask,
        operation_id: Option<&str>,
    ) -> StorageResult<SplitResult>;

    async fn get_gcal_settings(&self) -> StorageResult<GoogleCalSettingsRow>;
    async fn update_gcal_settings(
        &self,
        body: &UpdateGoogleCalSettings,
    ) -> StorageResult<GoogleCalSettingsRow>;
    async fn list_gcal_mappings(&self) -> StorageResult<Vec<GoogleCalEventRow>>;
    async fn upsert_gcal_mappings(&self, mappings: &[(String, String)]) -> StorageResult<()>;
    async fn delete_gcal_mappings(&self, task_ids: &[String]) -> StorageResult<()>;
    async fn clear_gcal_mappings(&self) -> StorageResult<()>;

    /// Update the Cloudflare Worker endpoint and token at runtime.
    /// The default implementation is a no-op for backends that do not use
    /// worker credentials (e.g. SQLite).
    async fn update_workers_credentials(&self, _url: &str, _token: &str) -> StorageResult<()> {
        Ok(())
    }

    // ── Coverage trust state (WI-10) ──────────────────────────────────────
    /// Read the coverage confirmation and unsettled-interval state used by
    /// planner-event evaluation. Backends should return the most recent
    /// confirmation for the current local day and any unsettled intervals that
    /// have not been resolved.
    async fn get_coverage_evaluation(&self) -> StorageResult<CoverageEvaluation> {
        Ok(CoverageEvaluation::default())
    }

    /// Record a coverage confirmation (e.g. after an intake or capture flow).
    async fn create_coverage_confirmation(
        &self,
        _body: &CreateCoverageConfirmation,
    ) -> StorageResult<CoverageConfirmationRow> {
        Err(StorageError::Internal(
            "coverage confirmations are not supported".into(),
        ))
    }

    /// Record an unsettled interval to be settled later (WI-18).
    async fn create_unsettled_interval(
        &self,
        _body: &CreateUnsettledInterval,
    ) -> StorageResult<UnsettledIntervalRow> {
        Err(StorageError::Internal(
            "unsettled intervals are not supported".into(),
        ))
    }

    /// Mark an unsettled interval as settled by a stable operation ID.
    async fn settle_unsettled_interval(
        &self,
        _id: &str,
        _operation_id: &str,
    ) -> StorageResult<UnsettledIntervalRow> {
        Err(StorageError::Internal(
            "unsettled intervals are not supported".into(),
        ))
    }

    /// Atomically settle an interval and save the replanned schedule (WI-18).
    async fn settle(&self, request: &SettleRequest) -> StorageResult<SettleResponse>;

    // ── Schedule move idempotency (WI-4) ─────────────────────────────────
    /// Check whether a `move_entry` response for the same `operation_id` and
    /// `request_hash` has already been persisted. Default backends that do not
    /// implement idempotency return `Ok(None)`.
    async fn get_move_idempotency(
        &self,
        _operation_id: &str,
        _request_hash: &str,
    ) -> StorageResult<Option<MoveEntryResponse>> {
        Ok(None)
    }

    /// Persist a `move_entry` response so retries with the same
    /// `operation_id` and `request_hash` can be replayed.
    async fn record_move_idempotency(
        &self,
        _operation_id: &str,
        _request_hash: &str,
        _response: &MoveEntryResponse,
    ) -> StorageResult<()> {
        Ok(())
    }

    // ── Multi-device arbitration (WI-11) ─────────────────────────────────
    /// Register or upsert a device. The platform and id together form the
    /// unique identity; re-registering the same id updates `name` and leaves
    /// heartbeat/lease state untouched unless `UpdateDevice` is used.
    async fn register_device(&self, body: &CreateDevice) -> StorageResult<DeviceRow> {
        let _ = body;
        Err(StorageError::Internal(
            "device registry is not supported".into(),
        ))
    }

    async fn get_device(&self, id: &str) -> StorageResult<DeviceRow> {
        let _ = id;
        Err(StorageError::Internal(
            "device registry is not supported".into(),
        ))
    }

    async fn list_devices(&self) -> StorageResult<Vec<DeviceRow>> {
        Ok(Vec::new())
    }

    async fn update_device(&self, id: &str, body: &UpdateDevice) -> StorageResult<DeviceRow> {
        let _ = (id, body);
        Err(StorageError::Internal(
            "device registry is not supported".into(),
        ))
    }

    async fn delete_device(&self, id: &str) -> StorageResult<()> {
        let _ = id;
        Err(StorageError::Internal(
            "device registry is not supported".into(),
        ))
    }

    /// Refresh a desktop evaluator heartbeat. `until` is the wall-clock time
    /// through which the host claims to remain alive and resident-eligible.
    async fn refresh_evaluator_heartbeat(
        &self,
        device_id: &str,
        until: Timestamp,
    ) -> StorageResult<DeviceRow> {
        let _ = (device_id, until);
        Err(StorageError::Internal(
            "device registry is not supported".into(),
        ))
    }

    /// Reserve or renew an Android evaluator lease. `lease_until` covers the
    /// next exact alarm plus grace period; `next_eval_at` is the scheduled
    /// evaluation wall-clock time.
    async fn refresh_evaluator_lease(
        &self,
        device_id: &str,
        lease_until: Timestamp,
        next_eval_at: Option<Timestamp>,
    ) -> StorageResult<DeviceRow> {
        let _ = (device_id, lease_until, next_eval_at);
        Err(StorageError::Internal(
            "device registry is not supported".into(),
        ))
    }

    /// Compute the current resident authority from the priority list and
    /// alive devices. Returns the resident device and whether `candidate_id`
    /// currently holds that role.
    async fn resolve_resident_authority(
        &self,
        candidate_id: &str,
    ) -> StorageResult<ResidentAuthority> {
        let _ = candidate_id;
        Ok(ResidentAuthority {
            device_id: None,
            is_resident: false,
            next_eval_at: None,
        })
    }

    /// Backend health check. Returns a short human-readable status string.
    /// For `WorkersStorage` this pings the Cloudflare Worker `/health`;
    /// for `SqliteStorage` it reports the local DB is reachable.
    async fn health_check(&self) -> StorageResult<String>;
}

/// Compute the current resident authority from the priority list and alive
/// devices. Returns the resident device and whether `candidate_id` currently
/// holds that role. Shared between SQLite and D1 storage backends.
pub fn resolve_resident_authority_from_rows(
    devices: &[DeviceRow],
    priority_list: &[String],
    candidate_id: &str,
    now: Timestamp,
) -> ResidentAuthority {
    let now_sec = now.as_second();
    let mut sorted = devices.to_vec();
    sorted.sort_by(|a, b| {
        let a_pos = priority_list
            .iter()
            .position(|p| p == a.platform.as_str())
            .unwrap_or(usize::MAX);
        let b_pos = priority_list
            .iter()
            .position(|p| p == b.platform.as_str())
            .unwrap_or(usize::MAX);
        a_pos
            .cmp(&b_pos)
            .then_with(|| a.priority.cmp(&b.priority))
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    let alive = sorted.iter().find(|d| {
        d.evaluator_heartbeat_until
            .is_some_and(|t| t.as_second() > now_sec)
            || d.evaluator_lease_until
                .is_some_and(|t| t.as_second() > now_sec)
    });
    match alive {
        Some(d) => ResidentAuthority {
            device_id: Some(d.id.clone()),
            is_resident: d.id == candidate_id,
            next_eval_at: d.next_eval_at,
        },
        None => ResidentAuthority {
            device_id: None,
            is_resident: false,
            next_eval_at: None,
        },
    }
}
