//! Storage trait implementation for D1Storage — split out for readability.

use async_trait::async_trait;
use takusu_contracts::storage::StorageResult;
use takusu_contracts::validate::validate_task_datetimes;
use takusu_contracts::{
    AttachWorkSession, CommentRow, ConvertWorkSession, CoverageConfirmationRow, CoverageEvaluation,
    CoverageState, CreateCoverageConfirmation, CreateDevice, CreateHabit, CreateHabitScheduledSpan,
    CreateMemory, CreateSkill, CreateTask, CreateUnsettledInterval, DeviceRow, EstimatorStateRow,
    EvaluationInputs, EventDeliveryState, EventLedgerInsert, EventLedgerRow, GoogleCalEventRow,
    GoogleCalSettingsRow, HabitRow, HabitScheduledSpanRow, HabitStepEstimateInput, HabitStepInput,
    HabitStepRow, MemoryInjectionQuery, MemoryInjectionResult, MemoryQuery, MemoryRow,
    MoveEntryResponse, ProgressEventRow, RecordWorkSessionProgress, ResidentAuthority,
    SaveScheduleRequest, ScheduleRow, SettingsRow, SimilarTaskQuery, SimilarTaskRow, SkillRow,
    SplitResult, SplitTask, StartWorkSession, Storage, StorageError, TaskProgress, TaskQuery,
    TaskRow, TokenCreateResponse, TokenRow, UnsettledIntervalRow, UpdateDevice,
    UpdateGoogleCalSettings, UpdateHabit, UpdateMemory, UpdateSettings, UpdateSkill, UpdateTask,
    WorkSessionProgressResult, WorkSessionRow,
};
use takusu_types::estimator::effective_distribution;
use takusu_types::jwt::{DEFAULT_TOKEN_TTL_SECONDS, generate_token_jwt};
use takusu_types::{
    CommentAuthor, DependencyList, EnumLabel, Minutes, Quantity, TaskStatus, TaskStatusFilter,
    Timestamp, TokenClaims, WindowMode,
};
use wasm_bindgen::JsValue;

use super::storage_d1::*;

fn valid_event_transition(from: EventDeliveryState, to: EventDeliveryState) -> bool {
    from == to
        || matches!(
            (from, to),
            (
                EventDeliveryState::PendingDelivery,
                EventDeliveryState::Delivered
            ) | (
                EventDeliveryState::PendingDelivery,
                EventDeliveryState::DeferredQuietHours
            ) | (
                EventDeliveryState::DeferredQuietHours,
                EventDeliveryState::Delivered
            ) | (
                EventDeliveryState::Delivered,
                EventDeliveryState::Acknowledged
            ) | (EventDeliveryState::Delivered, EventDeliveryState::Ignored)
                | (EventDeliveryState::Delivered, EventDeliveryState::Resolved)
                | (
                    EventDeliveryState::Acknowledged,
                    EventDeliveryState::Resolved
                )
                | (EventDeliveryState::Ignored, EventDeliveryState::Resolved)
        )
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Storage for D1Storage {
    async fn verify_token(&self, token: &str) -> StorageResult<Option<TokenClaims>> {
        let claims =
            match takusu_types::jwt::verify(&self.jwt_secret, token, takusu_types::DEFAULT_AUD) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
        if claims.is_root() {
            return Ok(Some(claims));
        }
        let stmt = self
            .db
            .prepare("SELECT COUNT(*) AS c FROM tokens WHERE jti = ?1 AND revoked_at IS NULL");
        let row: Option<CountRow> = stmt
            .bind(&[JsValue::from_str(&claims.jti)])
            .map_err(d1_err)?
            .first_t()
            .await?;
        Ok(if row.map(|r| r.c > 0).unwrap_or(false) {
            Some(claims)
        } else {
            None
        })
    }

    async fn list_tasks(&self, query: &TaskQuery) -> StorageResult<Vec<TaskRow>> {
        let mut sql = format!("{} WHERE 1=1", select_tasks());
        let mut bindings: Vec<JsValue> = Vec::new();
        if let Some(status) = query.status {
            if status == TaskStatusFilter::Overdue {
                sql.push_str(" AND ");
                sql.push_str(OVERDUE_SQL);
            } else if status == TaskStatusFilter::Actionable {
                sql.push_str(" AND ");
                sql.push_str(ACTIONABLE_SQL);
            } else {
                sql.push_str(" AND status = ?");
                bindings.push(JsValue::from_str(status.as_str()));
            }
        }
        if let Some(v) = query.from {
            sql.push_str(" AND end_at >= ?");
            bindings.push(JsValue::from_str(&v.to_string()));
        }
        if let Some(v) = query.until {
            sql.push_str(" AND (start_at IS NULL OR start_at <= ?)");
            bindings.push(JsValue::from_str(&v.to_string()));
        }
        if query.no_overdue == Some(true) {
            sql.push_str(" AND ");
            sql.push_str(NOT_OVERDUE_SQL);
        }
        if let Some(ref v) = query.habit_id {
            sql.push_str(" AND habit_id = ?");
            bindings.push(JsValue::from_str(v));
        }
        if let Some(ref v) = query.ical_uid {
            sql.push_str(" AND ical_uid = ?");
            bindings.push(JsValue::from_str(v));
        }
        sql.push_str(" ORDER BY created_at DESC");

        let post_filter_limit = if query.q.is_some() {
            query.limit
        } else {
            if let Some(n) = query.limit {
                sql.push_str(" LIMIT ?");
                bindings.push(JsValue::from_f64(n as f64));
            }
            None
        };

        let stmt = if bindings.is_empty() {
            self.db.prepare(&sql)
        } else {
            self.db.prepare(&sql).bind(&bindings).map_err(d1_err)?
        };
        let mut rows: Vec<TaskRow> = d1_all(&stmt).await?;

        if let Some(ref qstr) = query.q {
            rows = filter_rows_with_query(&self.db, rows, qstr).await?;
        }
        if let Some(n) = post_filter_limit {
            rows.truncate(n as usize);
        }
        Ok(rows)
    }

    async fn get_task(&self, id: &str) -> StorageResult<TaskRow> {
        let full = resolve_task_id(&self.db, id).await?;
        select_one_task(&self.db, &full).await
    }

    async fn create_task(&self, body: &CreateTask) -> StorageResult<TaskRow> {
        let quantity_total = body.quantity_total.filter(|t| *t != 0);
        let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
        validate_quantity(quantity_total, body.quantity_done, original_quantity_total)?;
        validate_task_datetimes(
            body.start_at.as_ref().map(Some),
            Some(&body.end_at),
            None,
            None,
        )?;
        let id = uuid::Uuid::now_v7().to_string();
        let resolved_depends = resolve_depends(&self.db, body.depends.as_deref()).await?;
        let depends = DependencyList::new(resolved_depends);
        let depends_json = depends.to_json_string();
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let display_id = allocate_display_id(&self.db, body.habit_id.as_deref()).await?;
        let quantity_done = body.quantity_done.unwrap_or_default();
        let normalized_title = takusu_search::memory::normalize_text(
            &body.title,
            Some(takusu_search::memory::MAX_CONTENT_SCALARS),
        )
        .ok();
        let stmt = self.db.prepare(
            "INSERT INTO tasks (id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, normalized_title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'pending', ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        );
        stmt.bind(&[
            JsValue::from_str(&id),
            JsValue::from_f64(display_id as f64),
            JsValue::from_str(&body.title),
            body.description
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.start_at
                .map(|t| JsValue::from_str(&t.to_string()))
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&body.end_at.to_string()),
            JsValue::from_f64(body.avg_minutes as f64),
            JsValue::from_f64(sigma as f64),
            JsValue::from_str(&depends_json),
            JsValue::from_bool(parallelizable),
            JsValue::from_bool(allows_parallel),
            JsValue::from_f64(abandonability.into()),
            body.ical_uid
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.habit_id
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::from_bool(body.fixed.unwrap_or(false)),
            body.habit_step_id
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            quantity_total
                .map(|n| JsValue::from_f64(f64::from(n)))
                .unwrap_or(JsValue::NULL),
            JsValue::from_f64(f64::from(quantity_done)),
            body.quantity_unit
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::NULL,
            JsValue::NULL,
            original_quantity_total
                .map(|n| JsValue::from_f64(f64::from(n)))
                .unwrap_or(JsValue::NULL),
            normalized_title
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        select_one_task(&self.db, &id).await
    }

    async fn update_task(&self, id: &str, body: &UpdateTask) -> StorageResult<TaskRow> {
        let depends_json = if let Some(ref deps) = body.depends {
            let resolved = resolve_depends(&self.db, Some(deps)).await?;
            Some(DependencyList::new(resolved).to_json_string())
        } else {
            None
        };

        let full = resolve_task_id(&self.db, id).await?;
        let existing = select_one_task(&self.db, &full).await?;

        let existing_total = existing.quantity_total.filter(|t| *t != 0);
        let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
        validate_quantity(
            body.quantity_total.or(existing_total),
            body.quantity_done.or(Some(existing.quantity_done)),
            original_quantity_total,
        )?;
        if body.start_at.is_some() || body.end_at.is_some() {
            validate_task_datetimes(
                body.start_at.as_ref().map(|o| o.as_ref()),
                body.end_at.as_ref(),
                existing.start_at.as_ref(),
                Some(&existing.end_at),
            )?;
        }

        let status = body.status.unwrap_or(existing.status);
        let normalized_title = body.title.as_deref().and_then(|t| {
            takusu_search::memory::normalize_text(
                t,
                Some(takusu_search::memory::MAX_CONTENT_SCALARS),
            )
            .ok()
        });

        let (upd_start, start_val) = match body.start_at {
            None => (0i32, JsValue::NULL),
            Some(None) => (1i32, JsValue::NULL),
            Some(Some(ref ts)) => (1i32, JsValue::from_str(&ts.to_string())),
        };

        let main_stmt = self.db.prepare(
            "UPDATE tasks SET title=COALESCE(?1,title), description=CASE WHEN ?2='' THEN NULL ELSE COALESCE(?2,description) END, start_at=CASE WHEN ?3=0 THEN start_at ELSE ?4 END, end_at=COALESCE(?5,end_at), avg_minutes=COALESCE(?6,avg_minutes), sigma_minutes=COALESCE(?7,sigma_minutes), depends=COALESCE(?8,depends), parallelizable=COALESCE(?9,parallelizable), allows_parallel=COALESCE(?10,allows_parallel), abandonability=COALESCE(?11,abandonability), status=?12, habit_id=COALESCE(?14,habit_id), user_edited=COALESCE(?15,user_edited), fixed=COALESCE(?16,fixed), habit_step_id=COALESCE(?17,habit_step_id), quantity_total=CASE WHEN ?18=0 THEN NULL ELSE COALESCE(?18,quantity_total) END, quantity_done=COALESCE(?19,quantity_done), quantity_unit=CASE WHEN ?20='' THEN NULL ELSE COALESCE(?20,quantity_unit) END, original_quantity_total=COALESCE(?21,original_quantity_total), normalized_title=COALESCE(?22,normalized_title), updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?13",
        );
        let main_stmt = main_stmt
            .bind(&[
                body.title
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
                body.description
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
                JsValue::from_f64(upd_start as f64),
                start_val,
                body.end_at
                    .map(|t| JsValue::from_str(&t.to_string()))
                    .unwrap_or(JsValue::NULL),
                body.avg_minutes
                    .map(|n| JsValue::from_f64(n as f64))
                    .unwrap_or(JsValue::NULL),
                body.sigma_minutes
                    .map(|n| JsValue::from_f64(n as f64))
                    .unwrap_or(JsValue::NULL),
                depends_json
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
                body.parallelizable
                    .map(JsValue::from_bool)
                    .unwrap_or(JsValue::NULL),
                body.allows_parallel
                    .map(JsValue::from_bool)
                    .unwrap_or(JsValue::NULL),
                body.abandonability
                    .map(|a| JsValue::from_f64(a.into()))
                    .unwrap_or(JsValue::NULL),
                JsValue::from_str(&status.to_string()),
                JsValue::from_str(&full),
                body.habit_id
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
                body.user_edited
                    .map(JsValue::from_bool)
                    .unwrap_or(JsValue::NULL),
                body.fixed.map(JsValue::from_bool).unwrap_or(JsValue::NULL),
                body.habit_step_id
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
                body.quantity_total
                    .map(|n| JsValue::from_f64(f64::from(n)))
                    .unwrap_or(JsValue::NULL),
                body.quantity_done
                    .map(|n| JsValue::from_f64(f64::from(n)))
                    .unwrap_or(JsValue::NULL),
                body.quantity_unit
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
                original_quantity_total
                    .map(|n| JsValue::from_f64(f64::from(n)))
                    .unwrap_or(JsValue::NULL),
                normalized_title
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
            ])
            .map_err(d1_err)?;

        let mut stmts = vec![main_stmt];
        if body.status.is_some() {
            let completed_stmt = self.db.prepare(
                "UPDATE tasks SET completed_at = CASE WHEN ?1 = 'completed' AND completed_at IS NULL THEN strftime('%Y-%m-%dT%H:%M:%SZ','now') WHEN ?1 != 'completed' AND completed_at IS NOT NULL THEN NULL ELSE completed_at END WHERE id = ?2",
            );
            stmts.push(
                completed_stmt
                    .bind(&[
                        JsValue::from_str(&status.to_string()),
                        JsValue::from_str(&full),
                    ])
                    .map_err(d1_err)?,
            );

            if status == TaskStatus::Skipped || status == TaskStatus::Completed {
                let now = takusu_types::now_rfc3339();
                let session_stmt = self.db.prepare(
                    "UPDATE work_sessions SET ended_at = ?1 WHERE task_id = ?2 AND ended_at IS NULL",
                );
                stmts.push(
                    session_stmt
                        .bind(&[JsValue::from_str(&now), JsValue::from_str(&full)])
                        .map_err(d1_err)?,
                );
            }
        }

        self.db.batch(stmts).await.map_err(d1_err)?;
        select_one_task(&self.db, &full).await
    }

    async fn replace_task(&self, id: &str, body: &CreateTask) -> StorageResult<TaskRow> {
        let quantity_total = body.quantity_total.filter(|t| *t != 0);
        let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
        validate_quantity(
            quantity_total,
            Some(Quantity::default()),
            original_quantity_total,
        )?;
        validate_task_datetimes(
            body.start_at.as_ref().map(Some),
            Some(&body.end_at),
            None,
            None,
        )?;
        let full = resolve_task_id(&self.db, id).await?;
        let resolved_depends = resolve_depends(&self.db, body.depends.as_deref()).await?;
        let depends_json = DependencyList::new(resolved_depends).to_json_string();
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let normalized_title = takusu_search::memory::normalize_text(
            &body.title,
            Some(takusu_search::memory::MAX_CONTENT_SCALARS),
        )
        .ok();
        let stmt = self.db.prepare(
            "UPDATE tasks SET title=?1, description=?2, start_at=?3, end_at=?4, avg_minutes=?5, sigma_minutes=?6, depends=?7, parallelizable=?8, allows_parallel=?9, abandonability=?10, status='pending', habit_id=COALESCE(?11,habit_id), fixed=?12, habit_step_id=?13, quantity_total=COALESCE(?14, quantity_total), quantity_done=0, quantity_unit=COALESCE(?15, quantity_unit), completed_at=?16, split_from_task_id=COALESCE(?17, split_from_task_id), original_quantity_total=COALESCE(?18, original_quantity_total), user_edited=CASE WHEN habit_id IS NOT NULL THEN 1 ELSE user_edited END, normalized_title=?19, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?20",
        );
        stmt.bind(&[
            JsValue::from_str(&body.title),
            body.description
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.start_at
                .map(|t| JsValue::from_str(&t.to_string()))
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&body.end_at.to_string()),
            JsValue::from_f64(body.avg_minutes as f64),
            JsValue::from_f64(sigma as f64),
            JsValue::from_str(&depends_json),
            JsValue::from_bool(parallelizable),
            JsValue::from_bool(allows_parallel),
            JsValue::from_f64(abandonability.into()),
            body.habit_id
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::from_bool(body.fixed.unwrap_or(false)),
            body.habit_step_id
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            quantity_total
                .map(|n| JsValue::from_f64(f64::from(n)))
                .unwrap_or(JsValue::NULL),
            body.quantity_unit
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::NULL,
            JsValue::NULL,
            original_quantity_total
                .map(|n| JsValue::from_f64(f64::from(n)))
                .unwrap_or(JsValue::NULL),
            normalized_title
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&full),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        select_one_task(&self.db, &full).await
    }

    async fn delete_task(&self, id: &str) -> StorageResult<()> {
        let full = resolve_task_id(&self.db, id).await?;
        let stmts = vec![
            self.db
                .prepare("UPDATE tasks SET split_from_task_id = NULL WHERE split_from_task_id = ?1")
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
            self.db
                .prepare("DELETE FROM google_cal_events WHERE task_id = ?1")
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
            self.db
                .prepare("DELETE FROM task_comments WHERE task_id = ?1")
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
            self.db
                .prepare("UPDATE work_sessions SET ended_at = ?1 WHERE task_id = ?2 AND ended_at IS NULL")
                .bind(&[
                    JsValue::from_str(&takusu_types::now_rfc3339()),
                    JsValue::from_str(&full),
                ])
                .map_err(d1_err)?,
            self.db
                .prepare("UPDATE work_sessions SET task_id = NULL WHERE task_id = ?1")
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
            self.db
                .prepare("UPDATE progress_events SET task_id = NULL WHERE task_id = ?1")
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
            self.db
                .prepare("DELETE FROM tasks WHERE id = ?1")
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
        ];
        self.db.batch(stmts).await.map_err(d1_err)?;
        Ok(())
    }

    // ── Task comments (WI-1) ──────────────────────────────────────────

    async fn list_comments(&self, task_id: &str) -> StorageResult<Vec<CommentRow>> {
        let full = resolve_task_id(&self.db, task_id).await?;
        let stmt = self.db.prepare(format!(
            "{} WHERE task_id = ?1 ORDER BY seq ASC",
            comment_select()
        ));
        d1_all(&stmt.bind(&[JsValue::from_str(&full)]).map_err(d1_err)?).await
    }

    async fn create_comment(
        &self,
        task_id: &str,
        author: CommentAuthor,
        content: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<CommentRow> {
        let full = resolve_task_id(&self.db, task_id).await?;

        let payload = format!("create:{full}:{}:{content}", author.as_str());
        let hash = comment_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(json) = check_comment_idempotency(&self.db, op_id, &hash).await?
        {
            let row: CommentRow = serde_json::from_str(&json).map_err(|e| {
                StorageError::Internal(format!("corrupt idempotency response: {e}"))
            })?;
            return Ok(row);
        }

        let id = uuid::Uuid::now_v7().to_string();
        let seq_stmt = self
            .db
            .prepare("SELECT COALESCE(MAX(seq), 0) + 1 AS c FROM task_comments WHERE task_id = ?1");
        let rows: Vec<CountRow> =
            d1_all(&seq_stmt.bind(&[JsValue::from_str(&full)]).map_err(d1_err)?).await?;
        let seq = rows.into_iter().next().map(|r| r.c).unwrap_or(1);

        // The comment insert and the receipt insert are issued together in a
        // single D1 batch, which runs atomically (a transaction). This closes
        // the check-then-insert race: under concurrent same-key requests, both
        // compute the same seq, so the loser's comment insert collides on the
        // unique (task_id, seq) index and the whole batch — including its
        // receipt insert — rolls back. The loser then replays the winner's
        // stored receipt below.
        let now = takusu_types::now_rfc3339();
        let created_at: takusu_types::Timestamp = now.parse().map_err(|e| {
            StorageError::Internal(format!("invalid generated timestamp {now}: {e}"))
        })?;
        let row = CommentRow {
            id: id.clone(),
            task_id: full.clone(),
            author,
            content: content.to_string(),
            seq,
            created_at,
        };

        let mut stmts = vec![self
            .db
            .prepare(
                "INSERT INTO task_comments (id, task_id, author, content, seq, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&[
                JsValue::from_str(&id),
                JsValue::from_str(&full),
                JsValue::from_str(author.as_str()),
                JsValue::from_str(content),
                JsValue::from_f64(seq as f64),
                JsValue::from_str(&now),
            ])
            .map_err(d1_err)?];

        if let Some(op_id) = operation_id {
            let response_json = serde_json::to_string(&row)
                .map_err(|e| StorageError::Internal(format!("serialize response: {e}")))?;
            stmts.push(
                self.db
                    .prepare(
                        "INSERT INTO comment_operations (operation_id, request_hash, response_json, created_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                    )
                    .bind(&[
                        JsValue::from_str(op_id),
                        JsValue::from_str(&hash),
                        JsValue::from_str(&response_json),
                    ])
                    .map_err(d1_err)?,
            );
        }

        match self.db.batch(stmts).await {
            Ok(_) => Ok(row),
            Err(e) if operation_id.is_some() => {
                // A concurrent request with the same Idempotency-Key likely won
                // the race and committed its comment + receipt. Replay its
                // stored response; if no receipt exists this was a genuine
                // failure (e.g. an unrelated seq conflict), so surface it.
                let op_id = operation_id.expect("guarded by match arm");
                if let Some(json) = check_comment_idempotency(&self.db, op_id, &hash).await? {
                    let replay: CommentRow = serde_json::from_str(&json).map_err(|e| {
                        StorageError::Internal(format!("corrupt idempotency response: {e}"))
                    })?;
                    return Ok(replay);
                }
                Err(d1_err(e))
            }
            Err(e) => Err(d1_err(e)),
        }
    }

    async fn delete_comment(&self, id: &str) -> StorageResult<()> {
        let stmt = self.db.prepare("DELETE FROM task_comments WHERE id = ?1");
        let result = stmt
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        let affected = result
            .meta()
            .map_err(d1_err)?
            .and_then(|m| m.rows_written)
            .unwrap_or(0);
        if affected == 0 {
            return Err(not_found(format!("comment {id} not found")));
        }
        Ok(())
    }

    async fn task_exists_by_ical_uid(&self, uid: &str) -> StorageResult<bool> {
        let stmt = self
            .db
            .prepare("SELECT 1 FROM tasks WHERE ical_uid = ?1 LIMIT 1");
        let rows: Vec<IdRow> =
            d1_all(&stmt.bind(&[JsValue::from_str(uid)]).map_err(d1_err)?).await?;
        Ok(!rows.is_empty())
    }

    // ── Habits ──────────────────────────────────────────────────────────

    async fn list_habits(&self) -> StorageResult<Vec<HabitRow>> {
        let stmt = self
            .db
            .prepare(format!("{} ORDER BY created_at DESC", select_habits()));
        d1_all(&stmt).await
    }

    async fn get_habit(&self, id: &str) -> StorageResult<HabitRow> {
        let full = resolve_habit_id(&self.db, id).await?;
        select_one_habit(&self.db, &full).await
    }

    async fn create_habit(&self, body: &CreateHabit) -> StorageResult<HabitRow> {
        let id = uuid::Uuid::now_v7().to_string();
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let fixed = body.fixed.unwrap_or(false);
        let window_mode = body.window_mode.unwrap_or(WindowMode::Day);

        let seq_stmt = self.db.prepare(
            "UPDATE habit_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1 AS display_id",
        );
        let seq_row: Option<DisplayIdRow> = seq_stmt.first_t().await?;
        let display_id = seq_row
            .ok_or_else(|| StorageError::Internal("habit display_id sequence is empty".into()))?
            .display_id;

        let stmt = self.db.prepare(
            "INSERT INTO habits (id, display_id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed, window_mode, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13, ?14, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        );
        stmt.bind(&[
            JsValue::from_str(&id),
            JsValue::from_f64(display_id as f64),
            JsValue::from_str(&body.title),
            body.description
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&body.recurrence),
            JsValue::from_str(&body.start_time.to_string()),
            JsValue::from_str(&body.end_time.to_string()),
            JsValue::from_f64(body.avg_minutes as f64),
            JsValue::from_f64(sigma as f64),
            JsValue::from_bool(parallelizable),
            JsValue::from_bool(allows_parallel),
            JsValue::from_f64(abandonability.into()),
            JsValue::from_bool(fixed),
            JsValue::from_str(&window_mode.to_string()),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        select_one_habit(&self.db, &id).await
    }

    async fn update_habit(&self, id: &str, body: &UpdateHabit) -> StorageResult<HabitRow> {
        let full = resolve_habit_id(&self.db, id).await?;
        let stmt = self.db.prepare(
            "UPDATE habits SET title=COALESCE(?1,title), description=COALESCE(?2,description), recurrence=COALESCE(?3,recurrence), start_time=COALESCE(?4,start_time), end_time=COALESCE(?5,end_time), avg_minutes=COALESCE(?6,avg_minutes), sigma_minutes=COALESCE(?7,sigma_minutes), parallelizable=COALESCE(?8,parallelizable), allows_parallel=COALESCE(?9,allows_parallel), abandonability=COALESCE(?10,abandonability), active=COALESCE(?11,active), fixed=COALESCE(?12,fixed), window_mode=COALESCE(?13,window_mode), updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?14",
        );
        stmt.bind(&[
            body.title
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.description
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.recurrence
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.start_time
                .map(|t| JsValue::from_str(&t.to_string()))
                .unwrap_or(JsValue::NULL),
            body.end_time
                .map(|t| JsValue::from_str(&t.to_string()))
                .unwrap_or(JsValue::NULL),
            body.avg_minutes
                .map(|n| JsValue::from_f64(n as f64))
                .unwrap_or(JsValue::NULL),
            body.sigma_minutes
                .map(|n| JsValue::from_f64(n as f64))
                .unwrap_or(JsValue::NULL),
            body.parallelizable
                .map(JsValue::from_bool)
                .unwrap_or(JsValue::NULL),
            body.allows_parallel
                .map(JsValue::from_bool)
                .unwrap_or(JsValue::NULL),
            body.abandonability
                .map(|a| JsValue::from_f64(a.into()))
                .unwrap_or(JsValue::NULL),
            body.active.map(JsValue::from_bool).unwrap_or(JsValue::NULL),
            body.fixed.map(JsValue::from_bool).unwrap_or(JsValue::NULL),
            body.window_mode
                .as_ref()
                .map(|w| JsValue::from_str(&w.to_string()))
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&full),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        select_one_habit(&self.db, &full).await
    }

    async fn replace_habit(&self, id: &str, body: &CreateHabit) -> StorageResult<HabitRow> {
        let full = resolve_habit_id(&self.db, id).await?;
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let fixed = body.fixed.unwrap_or(false);
        let window_mode = body.window_mode.unwrap_or(WindowMode::Day);
        let stmt = self.db.prepare(
            "UPDATE habits SET title=?1, description=?2, recurrence=?3, start_time=?4, end_time=?5, avg_minutes=?6, sigma_minutes=?7, parallelizable=?8, allows_parallel=?9, abandonability=?10, fixed=?11, window_mode=?12, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?13",
        );
        stmt.bind(&[
            JsValue::from_str(&body.title),
            body.description
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&body.recurrence),
            JsValue::from_str(&body.start_time.to_string()),
            JsValue::from_str(&body.end_time.to_string()),
            JsValue::from_f64(body.avg_minutes as f64),
            JsValue::from_f64(sigma as f64),
            JsValue::from_bool(parallelizable),
            JsValue::from_bool(allows_parallel),
            JsValue::from_f64(abandonability.into()),
            JsValue::from_bool(fixed),
            JsValue::from_str(&window_mode.to_string()),
            JsValue::from_str(&full),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        select_one_habit(&self.db, &full).await
    }

    async fn delete_habit(&self, id: &str) -> StorageResult<()> {
        let full = resolve_habit_id(&self.db, id).await?;
        let stmts = vec![
            self.db.prepare("UPDATE tasks SET split_from_task_id = NULL WHERE split_from_task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("DELETE FROM google_cal_events WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("UPDATE work_sessions SET ended_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?1) AND ended_at IS NULL").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("UPDATE work_sessions SET task_id = NULL WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("UPDATE progress_events SET task_id = NULL WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?1)").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("DELETE FROM tasks WHERE habit_id = ?1").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("DELETE FROM habit_scheduled_spans WHERE habit_id = ?1").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("DELETE FROM habit_steps WHERE habit_id = ?1").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("DELETE FROM habit_task_display_id_seq WHERE habit_id = ?1").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
            self.db.prepare("DELETE FROM habits WHERE id = ?1").bind(&[JsValue::from_str(&full)]).map_err(d1_err)?,
        ];
        self.db.batch(stmts).await.map_err(d1_err)?;
        Ok(())
    }

    // ── Habit scheduled spans ───────────────────────────────────────────

    async fn list_habit_scheduled_spans(
        &self,
        habit_id: &str,
    ) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        let full = resolve_habit_id(&self.db, habit_id).await?;
        let stmt = self.db.prepare(format!(
            "SELECT {SCHEDULED_SPAN_COLS} FROM habit_scheduled_spans WHERE habit_id = ?1 ORDER BY start_date ASC, created_at ASC"
        ));
        d1_all(&stmt.bind(&[JsValue::from_str(&full)]).map_err(d1_err)?).await
    }

    async fn list_all_habit_scheduled_spans(&self) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        let stmt = self.db.prepare(format!(
            "SELECT {SCHEDULED_SPAN_COLS} FROM habit_scheduled_spans ORDER BY habit_id, start_date ASC, created_at ASC"
        ));
        d1_all(&stmt).await
    }

    async fn create_habit_scheduled_span(
        &self,
        habit_id: &str,
        body: &CreateHabitScheduledSpan,
    ) -> StorageResult<HabitScheduledSpanRow> {
        let full = resolve_habit_id(&self.db, habit_id).await?;
        let span_id = uuid::Uuid::now_v7().to_string();
        let stmt = self.db.prepare(
            "INSERT INTO habit_scheduled_spans (id, habit_id, start_date, end_date, reason, created_at) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        );
        stmt.bind(&[
            JsValue::from_str(&span_id),
            JsValue::from_str(&full),
            JsValue::from_str(&body.start_date.to_string()),
            JsValue::from_str(&body.end_date.to_string()),
            body.reason
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        let sel_stmt = self.db.prepare(format!(
            "SELECT {SCHEDULED_SPAN_COLS} FROM habit_scheduled_spans WHERE id = ?1"
        ));
        let row: Option<HabitScheduledSpanRow> = sel_stmt
            .bind(&[JsValue::from_str(&span_id)])
            .map_err(d1_err)?
            .first_t()
            .await?;
        row.ok_or_else(|| StorageError::Internal("inserted scheduled span not found".into()))
    }

    async fn delete_habit_scheduled_span(
        &self,
        habit_id: &str,
        span_id: &str,
    ) -> StorageResult<()> {
        let full = resolve_habit_id(&self.db, habit_id).await?;
        let stmt = self
            .db
            .prepare("DELETE FROM habit_scheduled_spans WHERE id = ?1 AND habit_id = ?2");
        let result = stmt
            .bind(&[JsValue::from_str(span_id), JsValue::from_str(&full)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        let affected = result
            .meta()
            .map_err(d1_err)?
            .and_then(|m| m.rows_written)
            .unwrap_or(0);
        if affected == 0 {
            return Err(not_found(format!(
                "scheduled span {span_id} not found for habit {habit_id}"
            )));
        }
        Ok(())
    }

    // ── Habit steps ─────────────────────────────────────────────────────

    async fn list_habit_steps(&self, habit_id: &str) -> StorageResult<Vec<HabitStepRow>> {
        let full = resolve_habit_id(&self.db, habit_id).await?;
        select_steps_for_habit(&self.db, &full).await
    }

    async fn list_all_habit_steps(&self) -> StorageResult<Vec<HabitStepRow>> {
        let stmt = self.db.prepare(format!(
            "SELECT {STEP_COLS} FROM habit_steps ORDER BY habit_id, position ASC, created_at ASC"
        ));
        d1_all(&stmt).await
    }

    async fn replace_habit_steps(
        &self,
        habit_id: &str,
        steps: &[HabitStepInput],
    ) -> StorageResult<Vec<HabitStepRow>> {
        let full = resolve_habit_id(&self.db, habit_id).await?;

        let id_stmt = self
            .db
            .prepare("SELECT id FROM habit_steps WHERE habit_id = ?1");
        let existing: Vec<IdRow> =
            d1_all(&id_stmt.bind(&[JsValue::from_str(&full)]).map_err(d1_err)?).await?;
        let existing_ids: std::collections::HashSet<String> =
            existing.into_iter().map(|r| r.id).collect();

        let mut input_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();

        for s in steps {
            let id =
                s.id.clone()
                    .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
            input_ids.insert(id.clone());
            let sigma = s
                .sigma_minutes
                .unwrap_or(Minutes(s.avg_minutes).to_slots().0.max(1));
            let parallelizable = s.parallelizable.unwrap_or(false);
            let allows_parallel = s.allows_parallel.unwrap_or(false);
            let abandonability = s.abandonability.unwrap_or(0.5.into());
            let fixed = s.fixed.unwrap_or(false);
            let depends_json = DependencyList::new(s.depends_on.clone()).to_json_string();

            if existing_ids.contains(&id) {
                let stmt = self.db.prepare(
                    "UPDATE habit_steps SET position=?1, title=?2, description=?3, start_time=?4, end_time=?5, avg_minutes=?6, sigma_minutes=?7, parallelizable=?8, allows_parallel=?9, abandonability=?10, fixed=?11, depends_on=?12 WHERE id = ?13 AND habit_id = ?14",
                );
                stmts.push(
                    stmt.bind(&[
                        JsValue::from_f64(s.position as f64),
                        JsValue::from_str(&s.title),
                        s.description
                            .as_deref()
                            .map(JsValue::from_str)
                            .unwrap_or(JsValue::NULL),
                        JsValue::from_str(&s.start_time.to_string()),
                        JsValue::from_str(&s.end_time.to_string()),
                        JsValue::from_f64(s.avg_minutes as f64),
                        JsValue::from_f64(sigma as f64),
                        JsValue::from_bool(parallelizable),
                        JsValue::from_bool(allows_parallel),
                        JsValue::from_f64(abandonability.into()),
                        JsValue::from_bool(fixed),
                        JsValue::from_str(&depends_json),
                        JsValue::from_str(&id),
                        JsValue::from_str(&full),
                    ])
                    .map_err(d1_err)?,
                );
            } else {
                let stmt = self.db.prepare(
                    "INSERT INTO habit_steps (id, habit_id, position, title, description, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, fixed, depends_on, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                );
                stmts.push(
                    stmt.bind(&[
                        JsValue::from_str(&id),
                        JsValue::from_str(&full),
                        JsValue::from_f64(s.position as f64),
                        JsValue::from_str(&s.title),
                        s.description
                            .as_deref()
                            .map(JsValue::from_str)
                            .unwrap_or(JsValue::NULL),
                        JsValue::from_str(&s.start_time.to_string()),
                        JsValue::from_str(&s.end_time.to_string()),
                        JsValue::from_f64(s.avg_minutes as f64),
                        JsValue::from_f64(sigma as f64),
                        JsValue::from_bool(parallelizable),
                        JsValue::from_bool(allows_parallel),
                        JsValue::from_f64(abandonability.into()),
                        JsValue::from_bool(fixed),
                        JsValue::from_str(&depends_json),
                    ])
                    .map_err(d1_err)?,
                );
            }
        }

        for old_id in &existing_ids {
            if !input_ids.contains(old_id) {
                let stmt = self
                    .db
                    .prepare("DELETE FROM habit_steps WHERE id = ?1 AND habit_id = ?2");
                stmts.push(
                    stmt.bind(&[JsValue::from_str(old_id), JsValue::from_str(&full)])
                        .map_err(d1_err)?,
                );
            }
        }

        if !stmts.is_empty() {
            self.db.batch(stmts).await.map_err(d1_err)?;
        }
        select_steps_for_habit(&self.db, &full).await
    }

    async fn apply_habit_estimate(
        &self,
        habit_id: &str,
        avg_minutes: i64,
        sigma_minutes: i64,
        step_estimates: &[HabitStepEstimateInput],
    ) -> StorageResult<()> {
        let full = resolve_habit_id(&self.db, habit_id).await?;
        let habit = select_one_habit(&self.db, &full).await?;
        if habit.fixed {
            return Err(StorageError::BadRequest(
                "cannot apply estimate to fixed habit".into(),
            ));
        }
        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        for step in step_estimates {
            let stmt = self.db.prepare(
                "UPDATE habit_steps SET avg_minutes = ?1, sigma_minutes = ?2 WHERE id = ?3 AND habit_id = ?4 AND fixed = 0",
            );
            stmts.push(
                stmt.bind(&[
                    JsValue::from_f64(step.avg_minutes as f64),
                    JsValue::from_f64(step.sigma_minutes as f64),
                    JsValue::from_str(&step.step_id),
                    JsValue::from_str(&full),
                ])
                .map_err(d1_err)?,
            );
        }
        let habit_stmt = self
            .db
            .prepare("UPDATE habits SET avg_minutes = ?1, sigma_minutes = ?2 WHERE id = ?3");
        stmts.push(
            habit_stmt
                .bind(&[
                    JsValue::from_f64(avg_minutes as f64),
                    JsValue::from_f64(sigma_minutes as f64),
                    JsValue::from_str(&full),
                ])
                .map_err(d1_err)?,
        );
        self.db.batch(stmts).await.map_err(d1_err)?;
        Ok(())
    }

    // ── Schedule ────────────────────────────────────────────────────────

    async fn get_schedule(&self) -> StorageResult<Option<ScheduleRow>> {
        let stmt = self.db.prepare(
            "SELECT id, created_at, updated_at, schedule, horizon_task_ids FROM schedules WHERE id = 'active'",
        );
        let rows: Vec<ScheduleRow> = d1_all(&stmt).await?;
        Ok(rows.into_iter().next())
    }

    async fn save_schedule(&self, req: &SaveScheduleRequest) -> StorageResult<ScheduleRow> {
        let schedule_json =
            takusu_contracts::ScheduleData::new(req.entries.clone()).to_json_string();
        let horizon_json =
            takusu_types::JsonString::new(req.horizon_task_ids.clone()).to_json_string();
        let mut stmts: Vec<worker::D1PreparedStatement> =
            Vec::with_capacity(1 + req.mark_scheduled_task_ids.len());
        let upsert = self.db.prepare(
            "INSERT INTO schedules (id, created_at, updated_at, schedule, horizon_task_ids) VALUES ('active', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?1, ?2) ON CONFLICT(id) DO UPDATE SET schedule=excluded.schedule, horizon_task_ids=excluded.horizon_task_ids, updated_at=excluded.updated_at",
        )
        .bind(&[JsValue::from_str(&schedule_json), JsValue::from_str(&horizon_json)])
        .map_err(d1_err)?;
        stmts.push(upsert);
        for id in &req.mark_scheduled_task_ids {
            let stmt = self.db.prepare("UPDATE tasks SET status = 'scheduled', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1")
                .bind(&[JsValue::from_str(id)])
                .map_err(d1_err)?;
            stmts.push(stmt);
        }
        stmts.push(
            self.db.prepare(
                "UPDATE schedule_revisions SET revision = revision + 1 WHERE id = 'active'",
            ),
        );
        self.db.batch(stmts).await.map_err(d1_err)?;
        let stmt = self.db.prepare(
            "SELECT id, created_at, updated_at, schedule, horizon_task_ids FROM schedules WHERE id = 'active'",
        );
        let rows: Vec<ScheduleRow> = d1_all(&stmt).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| StorageError::Internal("schedule not found after save".into()))
    }

    async fn clear_schedule(&self) -> StorageResult<()> {
        let stmts = vec![
            self.db.prepare("DELETE FROM schedules WHERE id = 'active'"),
            self.db.prepare(
                "UPDATE schedule_revisions SET revision = revision + 1 WHERE id = 'active'",
            ),
        ];
        self.db.batch(stmts).await.map_err(d1_err)?;
        Ok(())
    }

    // ── Tokens ──────────────────────────────────────────────────────────

    async fn create_token(&self, label: Option<&str>) -> StorageResult<TokenCreateResponse> {
        let label_opt: Option<String> = label.map(|s| s.to_string());
        let (new_token, jti) = generate_token_jwt(
            &self.jwt_secret,
            takusu_types::SCOPE_READ_WRITE,
            label_opt.as_deref(),
            None,
        )
        .map_err(|e| StorageError::Internal(e.to_string()))?;
        let expires_at = token_expires_at(DEFAULT_TOKEN_TTL_SECONDS);
        let stmt = self.db.prepare(
            "INSERT INTO tokens (jti, scope, label, created_by, created_at, expires_at) VALUES (?1, ?2, ?3, 'authenticated', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?4)",
        );
        stmt.bind(&[
            JsValue::from_str(&jti),
            JsValue::from_str(takusu_types::SCOPE_READ_WRITE),
            label_opt
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            expires_at
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        let lookup = self.db.prepare("SELECT id, jti, scope, label, created_by, created_at, revoked_at, expires_at FROM tokens WHERE jti = ?1");
        let row: Option<TokenRow> = lookup
            .bind(&[JsValue::from_str(&jti)])
            .map_err(d1_err)?
            .first_t()
            .await?;
        let row = row.ok_or_else(|| StorageError::Internal("inserted token not found".into()))?;
        Ok(TokenCreateResponse {
            id: row.id,
            token: new_token,
            scope: row.scope,
            label: row.label,
            created_at: row.created_at,
            expires_at: row.expires_at,
        })
    }

    async fn list_tokens(&self) -> StorageResult<Vec<TokenRow>> {
        let stmt = self.db.prepare("SELECT id, jti, scope, label, created_by, created_at, revoked_at, expires_at FROM tokens ORDER BY created_at DESC");
        d1_all(&stmt).await
    }

    async fn revoke_token(&self, id: i64) -> StorageResult<()> {
        let stmt = self.db.prepare("UPDATE tokens SET revoked_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1 AND revoked_at IS NULL");
        let result = stmt
            .bind(&[JsValue::from_f64(id as f64)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        let affected = result
            .meta()
            .map_err(d1_err)?
            .and_then(|m| m.rows_written)
            .unwrap_or(0);
        if affected == 0 {
            return Err(not_found(format!(
                "token {id} not found or already revoked"
            )));
        }
        Ok(())
    }

    // ── Settings ────────────────────────────────────────────────────────

    async fn get_settings(&self) -> StorageResult<SettingsRow> {
        let stmt = self.db.prepare("SELECT id, tz, sleep_start, sleep_end, comfortable_minutes, maximum_minutes, solver, time_budget_ms, seed, warm_start, plan_length_days, device_priority, created_at, updated_at FROM settings WHERE id = 'active'");
        let rows: Vec<SettingsRow> = d1_all(&stmt).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| not_found("settings not found"))
    }

    async fn update_settings(&self, body: &UpdateSettings) -> StorageResult<SettingsRow> {
        let existing = self.get_settings().await?;
        let tz = body.tz.clone().unwrap_or(existing.tz);
        let sleep_start = body.sleep_start.unwrap_or(existing.sleep_start);
        let sleep_end = body.sleep_end.unwrap_or(existing.sleep_end);
        let comfortable_minutes = body.comfortable_minutes.or(existing.comfortable_minutes);
        let maximum_minutes = body.maximum_minutes.or(existing.maximum_minutes);
        let solver = body.solver.unwrap_or(existing.solver);
        let solver = solver.to_string();
        let time_budget_ms = body
            .time_budget_ms
            .filter(|&v| v > 0)
            .or(existing.time_budget_ms);
        let seed = body.seed.filter(|&v| v >= 0).or(existing.seed);
        let warm_start = body.warm_start.unwrap_or(existing.warm_start);
        let plan_length_days = body.plan_length_days.unwrap_or(existing.plan_length_days);
        let device_priority = body
            .device_priority
            .as_ref()
            .map(|list| takusu_types::JsonString::new(list.clone()).to_json_string())
            .unwrap_or_else(|| existing.device_priority.to_json_string());
        let stmt = self.db.prepare(
            "UPDATE settings SET tz = ?1, sleep_start = ?2, sleep_end = ?3, comfortable_minutes = ?4, maximum_minutes = ?5, solver = ?6, time_budget_ms = ?7, seed = ?8, warm_start = ?9, plan_length_days = ?10, device_priority = ?11, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = 'active'",
        );
        stmt.bind(&[
            JsValue::from_str(&tz),
            JsValue::from_str(&sleep_start.to_string()),
            JsValue::from_str(&sleep_end.to_string()),
            comfortable_minutes
                .map(|v| JsValue::from_f64(v as f64))
                .unwrap_or(JsValue::NULL),
            maximum_minutes
                .map(|v| JsValue::from_f64(v as f64))
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(&solver),
            time_budget_ms
                .map(|v| JsValue::from_f64(v as f64))
                .unwrap_or(JsValue::NULL),
            seed.map(|v| JsValue::from_f64(v as f64))
                .unwrap_or(JsValue::NULL),
            JsValue::from_bool(warm_start),
            JsValue::from_f64(plan_length_days as f64),
            JsValue::from_str(&device_priority),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        self.get_settings().await
    }

    // ── Skills ──────────────────────────────────────────────────────────

    async fn list_skills(&self) -> StorageResult<Vec<SkillRow>> {
        let stmt = self
            .db
            .prepare(format!("{} ORDER BY created_at DESC", select_skills()));
        d1_all(&stmt).await
    }

    async fn get_skill(&self, slug: &str) -> StorageResult<SkillRow> {
        select_one_skill(&self.db, slug).await
    }

    async fn create_skill(&self, body: &CreateSkill) -> StorageResult<SkillRow> {
        let built_in = body.built_in.unwrap_or(false);
        let stmt = self.db.prepare(
            "INSERT INTO skills (slug, name, description, body, built_in, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        );
        stmt.bind(&[
            JsValue::from_str(&body.slug),
            JsValue::from_str(&body.name),
            JsValue::from_str(&body.description),
            JsValue::from_str(&body.body),
            JsValue::from_bool(built_in),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        select_one_skill(&self.db, &body.slug).await
    }

    async fn update_skill(&self, slug: &str, body: &UpdateSkill) -> StorageResult<SkillRow> {
        let stmt = self.db.prepare(
            "UPDATE skills SET name=COALESCE(?1,name), description=COALESCE(?2,description), body=COALESCE(?3,body), updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE slug = ?4",
        );
        stmt.bind(&[
            body.name
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.description
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            body.body
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(slug),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        select_one_skill(&self.db, slug).await
    }

    async fn delete_skill(&self, slug: &str) -> StorageResult<()> {
        let stmt = self.db.prepare("DELETE FROM skills WHERE slug = ?1");
        stmt.bind(&[JsValue::from_str(slug)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        Ok(())
    }

    // ── Memory ──────────────────────────────────────────────────────────

    async fn get_memory(&self, id: &str) -> StorageResult<MemoryRow> {
        select_one_memory(&self.db, id).await
    }

    async fn create_memory(
        &self,
        body: &CreateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow> {
        let normalized_key = takusu_search::memory::normalize_key(&body.key)
            .map_err(|e| StorageError::BadRequest(format!("invalid key: {e}")))?;
        let normalized_content = takusu_search::memory::normalize_content(&body.content)
            .map_err(|e| StorageError::BadRequest(format!("invalid content: {e}")))?;
        let subject_type = body.subject_type.unwrap_or_default();
        let subject_id = body.subject_id.clone().unwrap_or_default();

        let payload = serde_json::to_string(body).unwrap_or_default();
        let hash = memory_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(json) = check_memory_idempotency(&self.db, op_id, &hash).await?
        {
            let row: MemoryRow = serde_json::from_str(&json).map_err(|e| {
                StorageError::Internal(format!("corrupt idempotency response: {e}"))
            })?;
            return Ok(row);
        }

        let find_stmt = self.db.prepare(format!(
            "{} WHERE kind = ?1 AND normalized_key = ?2 AND subject_type = ?3 AND subject_id = ?4",
            memory_select()
        ));
        let existing: Vec<MemoryRow> = d1_all(
            &find_stmt
                .bind(&[
                    JsValue::from_str(body.kind.as_str()),
                    JsValue::from_str(&normalized_key),
                    JsValue::from_str(subject_type.as_str()),
                    JsValue::from_str(&subject_id),
                ])
                .map_err(d1_err)?,
        )
        .await?;

        if let Some(existing) = existing.into_iter().next() {
            if body.upsert {
                let new_revision = existing.revision + 1;
                let result = self.db.prepare(
                    "UPDATE memories SET content = ?1, normalized_content = ?2, revision = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?4 AND revision = ?5",
                )
                .bind(&[
                    JsValue::from_str(&body.content),
                    JsValue::from_str(&normalized_content),
                    JsValue::from_f64(new_revision as f64),
                    JsValue::from_str(&existing.id),
                    JsValue::from_f64(existing.revision as f64),
                ]).map_err(d1_err)?
                .run()
                .await
                .map_err(d1_err)?;
                let meta = result.meta().map_err(d1_err)?;
                if meta.and_then(|m| m.rows_written).unwrap_or(0) == 0 {
                    return Err(StorageError::Conflict(
                        "memory changed after proposal".into(),
                    ));
                }
                let row = select_one_memory(&self.db, &existing.id).await?;
                if let Some(op_id) = operation_id {
                    let response_json = serde_json::to_string(&row)
                        .map_err(|e| StorageError::Internal(format!("serialize response: {e}")))?;
                    record_memory_operation(&self.db, op_id, &hash, &response_json).await?;
                }
                return Ok(row);
            }
            return Err(StorageError::Conflict(format!(
                "memory {} already exists",
                body.key
            )));
        }

        let id = uuid::Uuid::now_v7().to_string();
        let insert = self.db.prepare(
            "INSERT INTO memories (id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'user_confirmed', 1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        );
        let result = insert
            .bind(&[
                JsValue::from_str(&id),
                JsValue::from_str(body.kind.as_str()),
                JsValue::from_str(&body.key),
                JsValue::from_str(&normalized_key),
                JsValue::from_str(&body.content),
                JsValue::from_str(&normalized_content),
                JsValue::from_str(subject_type.as_str()),
                JsValue::from_str(&subject_id),
            ])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        let meta = result.meta().map_err(d1_err)?;
        if meta.and_then(|m| m.rows_written).unwrap_or(0) == 0 {
            return Err(StorageError::Internal(
                "memory insert did not write a row".into(),
            ));
        }
        let row = select_one_memory(&self.db, &id).await?;
        if let Some(op_id) = operation_id {
            let response_json = serde_json::to_string(&row)
                .map_err(|e| StorageError::Internal(format!("serialize response: {e}")))?;
            record_memory_operation(&self.db, op_id, &hash, &response_json).await?;
        }
        Ok(row)
    }

    async fn update_memory(
        &self,
        id: &str,
        body: &UpdateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow> {
        let content = body
            .content
            .as_ref()
            .ok_or_else(|| StorageError::BadRequest("content is required".into()))?;
        if content.is_empty() {
            return Err(StorageError::BadRequest("content is required".into()));
        }
        let normalized_content = takusu_search::memory::normalize_content(content)
            .map_err(|e| StorageError::BadRequest(format!("invalid content: {e}")))?;

        let existing = select_one_memory(&self.db, id).await?;
        if existing.revision != body.observed_revision {
            return Err(StorageError::Conflict(
                "memory changed after proposal".into(),
            ));
        }

        let payload = serde_json::to_string(body).unwrap_or_default();
        let hash = memory_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(json) = check_memory_idempotency(&self.db, op_id, &hash).await?
        {
            let row: MemoryRow = serde_json::from_str(&json).map_err(|e| {
                StorageError::Internal(format!("corrupt idempotency response: {e}"))
            })?;
            return Ok(row);
        }

        let new_revision = existing.revision + 1;
        let result = self.db.prepare(
            "UPDATE memories SET content = ?1, normalized_content = ?2, revision = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?4 AND revision = ?5",
        )
        .bind(&[
            JsValue::from_str(content),
            JsValue::from_str(&normalized_content),
            JsValue::from_f64(new_revision as f64),
            JsValue::from_str(id),
            JsValue::from_f64(body.observed_revision as f64),
        ]).map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        let meta = result.meta().map_err(d1_err)?;
        if meta.and_then(|m| m.rows_written).unwrap_or(0) == 0 {
            return Err(StorageError::Conflict(
                "memory changed after proposal".into(),
            ));
        }
        let row = select_one_memory(&self.db, id).await?;
        if let Some(op_id) = operation_id {
            let response_json = serde_json::to_string(&row)
                .map_err(|e| StorageError::Internal(format!("serialize response: {e}")))?;
            record_memory_operation(&self.db, op_id, &hash, &response_json).await?;
        }
        Ok(row)
    }

    async fn delete_memory(
        &self,
        id: &str,
        observed_revision: i64,
        operation_id: Option<&str>,
    ) -> StorageResult<()> {
        let existing = select_one_memory(&self.db, id).await?;
        if existing.revision != observed_revision {
            return Err(StorageError::Conflict(
                "memory changed after proposal".into(),
            ));
        }
        let hash = memory_request_hash(&format!("delete:{id}:{observed_revision}"), operation_id);
        if let Some(op_id) = operation_id
            && let Some(_) = check_memory_idempotency(&self.db, op_id, &hash).await?
        {
            return Ok(());
        }
        let stmt = self
            .db
            .prepare("DELETE FROM memories WHERE id = ?1 AND revision = ?2");
        let result = stmt
            .bind(&[
                JsValue::from_str(id),
                JsValue::from_f64(observed_revision as f64),
            ])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        let affected = result
            .meta()
            .map_err(d1_err)?
            .and_then(|m| m.rows_written)
            .unwrap_or(0);
        if affected == 0 {
            let current = select_one_memory(&self.db, id).await?;
            if current.revision != observed_revision {
                return Err(StorageError::Conflict(
                    "memory changed after proposal".into(),
                ));
            }
            return Err(not_found(format!("memory {id} not found")));
        }
        if let Some(op_id) = operation_id {
            record_memory_operation(&self.db, op_id, &hash, "null").await?;
        }
        Ok(())
    }

    async fn search_memories(&self, query: &MemoryQuery) -> StorageResult<Vec<MemoryRow>> {
        let terms = takusu_search::memory::tokenize_query(&query.q)
            .map_err(|e| StorageError::BadRequest(format!("invalid query: {e}")))?;
        let patterns = takusu_search::memory::memory_like_patterns(&terms);
        let mut sql = String::from("SELECT * FROM memories WHERE ");
        let mut bindings: Vec<JsValue> = Vec::new();
        let mut idx = 1;
        for (i, pat) in patterns.iter().enumerate() {
            if i > 0 {
                sql.push_str(" AND ");
            }
            sql.push_str(&format!("(normalized_key LIKE ?{idx} ESCAPE '\\' OR normalized_content LIKE ?{} ESCAPE '\\')", idx + 1));
            bindings.push(JsValue::from_str(pat));
            bindings.push(JsValue::from_str(pat));
            idx += 2;
        }
        if let Some(k) = query.kind {
            sql.push_str(&format!(" AND kind = ?{idx}"));
            bindings.push(JsValue::from_str(k.as_str()));
            idx += 1;
        }
        if let Some(st) = query.subject_type {
            sql.push_str(&format!(" AND subject_type = ?{idx}"));
            bindings.push(JsValue::from_str(st.as_str()));
            idx += 1;
        }
        if let Some(ref sid) = query.subject_id {
            sql.push_str(&format!(" AND subject_id = ?{idx}"));
            bindings.push(JsValue::from_str(sid));
            idx += 1;
        }
        sql.push_str(&format!(" LIMIT ?{idx}"));
        bindings.push(JsValue::from_f64(1000.0));
        let stmt = self.db.prepare(sql).bind(&bindings).map_err(d1_err)?;
        let mut rows: Vec<MemoryRow> = d1_all(&stmt).await?;
        takusu_search::memory::sort_memories(&query.q, &mut rows);
        let limit = query.limit.unwrap_or(10).clamp(1, 50);
        rows.truncate(limit as usize);
        Ok(rows)
    }

    async fn injectable_memories(
        &self,
        query: &MemoryInjectionQuery,
    ) -> StorageResult<MemoryInjectionResult> {
        let normalized = takusu_search::memory::normalize_text(
            &query.text,
            Some(takusu_search::memory::MAX_INJECTION_UTTERANCE_SCALARS),
        )
        .map_err(|e| StorageError::BadRequest(format!("invalid text: {e}")))?;

        let counts = self.memory_counts().await?;
        if normalized.is_empty() {
            return Ok(MemoryInjectionResult {
                memories: Vec::new(),
                counts,
            });
        }
        // Bound rows read per turn: order by the injection ranking and LIMIT up
        // front instead of loading every `instr` match (a ubiquitous short key
        // could otherwise match an unbounded set).
        let sql_limit = query
            .limit
            .unwrap_or(5)
            .clamp(1, takusu_search::memory::MAX_INJECTION_RESULTS as u32);
        let stmt = self.db.prepare(
            "SELECT * FROM memories WHERE kind IN ('proper_noun', 'fact') AND instr(?1, normalized_key) > 0 ORDER BY length(normalized_key) DESC, updated_at DESC, id ASC LIMIT ?2",
        );
        let bindings = vec![
            JsValue::from_str(&normalized),
            JsValue::from_f64(sql_limit.into()),
        ];
        let mut rows: Vec<MemoryRow> = d1_all(&stmt.bind(&bindings).map_err(d1_err)?).await?;
        let matched = takusu_search::memory::rank_memories_for_injection(&normalized, &mut rows);
        let limit = sql_limit as usize;
        rows.truncate(matched.min(limit));
        Ok(MemoryInjectionResult {
            memories: rows,
            counts,
        })
    }

    async fn find_similar_tasks(
        &self,
        query: &SimilarTaskQuery,
    ) -> StorageResult<Vec<SimilarTaskRow>> {
        let normalized_title = takusu_search::memory::normalize_text(
            &query.title,
            Some(takusu_search::memory::MAX_QUERY_SCALARS),
        )
        .map_err(|e| StorageError::BadRequest(format!("invalid title: {e}")))?;
        let patterns = takusu_search::memory::similar_task_filter_patterns(&normalized_title);
        if patterns.is_empty() {
            return Ok(Vec::new());
        }
        let mut clauses = Vec::with_capacity(patterns.len());
        let mut bindings: Vec<JsValue> = Vec::with_capacity(patterns.len());
        for (i, p) in patterns.iter().enumerate() {
            clauses.push(format!("t.normalized_title LIKE ?{} ESCAPE '\\'", i + 1));
            bindings.push(JsValue::from_str(p));
        }
        let filter = clauses.join(" OR ");
        let cap = takusu_search::memory::SIMILAR_TASK_CANDIDATE_CAP;
        let sql = format!(
            "SELECT t.id AS task_id, t.display_id, t.title, t.avg_minutes, t.sigma_minutes, tam.actual_minutes, t.completed_at, t.updated_at FROM tasks t LEFT JOIN task_actual_minutes tam ON tam.task_id = t.id WHERE t.status = 'completed' AND ((t.normalized_title IS NOT NULL AND ({filter})) OR t.normalized_title IS NULL) ORDER BY t.updated_at DESC LIMIT {cap}"
        );
        let stmt = self.db.prepare(sql).bind(&bindings).map_err(d1_err)?;
        let rows: Vec<SimilarTaskRow> = d1_all(&stmt).await?;
        let mut scored: Vec<(f64, SimilarTaskRow)> = rows
            .into_iter()
            .filter_map(|row| {
                takusu_search::memory::similar_task_score_pre_normalized(
                    &normalized_title,
                    &row.title,
                )
                .map(|score| (score, row))
            })
            .collect();
        scored.sort_by(|(sa, a), (sb, b)| {
            sa.total_cmp(sb)
                .reverse()
                .then_with(|| {
                    let a_str = a.completed_at.map(|t| t.to_string());
                    let b_str = b.completed_at.map(|t| t.to_string());
                    takusu_search::memory::compare_optional_desc(&a_str, &b_str)
                })
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.task_id.cmp(&b.task_id))
        });
        let limit = query.limit.unwrap_or(10).clamp(1, 50);
        let mut out: Vec<SimilarTaskRow> = scored
            .into_iter()
            .map(|(score, mut row)| {
                row.similarity = takusu_types::Similarity::dice(score);
                row
            })
            .collect();
        out.truncate(limit as usize);
        Ok(out)
    }

    // ── Work sessions ───────────────────────────────────────────────────

    async fn start_work_session(
        &self,
        body: &StartWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        let payload = serde_json::json!({"op": "start_work_session", "body": body}).to_string();
        let request_hash = progress_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) =
                check_progress_idempotency::<WorkSessionRow>(&self.db, op_id, &request_hash).await?
        {
            return Ok(stored);
        }

        let mut linked_task: Option<TaskRow> = None;
        let mut task_id: Option<String> = None;

        if let Some(ref id) = body.task_id {
            let full = resolve_task_id(&self.db, id).await?;
            let task = select_one_task(&self.db, &full).await?;
            if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
                return Err(StorageError::BadRequest(format!(
                    "cannot start work session for a {} task",
                    task.status
                )));
            }
            // At most one open work session per task; concurrent sessions are
            // only allowed across different tasks.
            let existing: Option<WorkSessionRow> = self
                .db
                .prepare(format!(
                    "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE task_id = ?1 AND ended_at IS NULL",
                ))
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?
                .first_t()
                .await?;
            if existing.is_some() {
                return Err(StorageError::BadRequest(format!(
                    "task {id} already has an open work session"
                )));
            }
            task_id = Some(full);
            linked_task = Some(task);
        }

        let now = takusu_types::now_rfc3339();
        let id = uuid::Uuid::now_v7().to_string();
        let title = body
            .title
            .clone()
            .or_else(|| linked_task.as_ref().map(|t| t.title.clone()));
        let note = body.note.clone();
        let quantity_total = body
            .quantity_total
            .or(linked_task.as_ref().and_then(|t| t.quantity_total))
            .filter(|q| *q != 0);
        let quantity_unit = body
            .quantity_unit
            .clone()
            .or_else(|| linked_task.as_ref().and_then(|t| t.quantity_unit.clone()));

        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        let insert = self.db.prepare(
            "INSERT INTO work_sessions (id, task_id, title, note, quantity_total, quantity_done, quantity_unit, started_at, ended_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
        );
        stmts.push(
            insert
                .bind(&[
                    JsValue::from_str(&id),
                    task_id
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    title
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    note.as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    quantity_total
                        .map(|q| JsValue::from_f64(f64::from(q)))
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_f64(f64::from(Quantity::default())),
                    quantity_unit
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&now),
                    JsValue::from_str(&now),
                ])
                .map_err(d1_err)?,
        );

        if let Some(ref full) = task_id {
            let update = self.db.prepare(
                "UPDATE tasks SET status = 'in_progress', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
            );
            stmts.push(update.bind(&[JsValue::from_str(full)]).map_err(d1_err)?);
            if let Some(task) = linked_task.as_ref()
                && !task.fixed
            {
                let task_kind_prior = if task.sigma_minutes > 0 {
                    None
                } else {
                    load_task_kind_prior(&self.db, task).await?
                };
                let distribution = effective_distribution(
                    task.avg_minutes as f64,
                    task.sigma_minutes as f64,
                    task_kind_prior,
                );
                stmts.push(
                    self.db
                        .prepare(
                            "INSERT OR IGNORE INTO estimator_state (task_id, revision, mean_minutes, sigma_minutes, source) VALUES (?1, 0, ?2, ?3, ?4)",
                        )
                        .bind(&[
                            JsValue::from_str(full),
                            JsValue::from_f64(distribution.mu),
                            JsValue::from_f64(distribution.sigma),
                            JsValue::from_str(if task.sigma_minutes > 0 {
                                "task"
                            } else {
                                "fallback"
                            }),
                        ])
                        .map_err(d1_err)?,
                );
            }
        }

        if let Err(e) = self.db.batch(stmts).await {
            // A concurrent start for the same task can pass the SELECT check
            // above yet lose the race on the per-task unique index; surface
            // that as a BadRequest rather than an opaque Internal error (#1419).
            if format!("{e:?}").contains("UNIQUE constraint failed") {
                return Err(StorageError::BadRequest(
                    "task already has an open work session".into(),
                ));
            }
            return Err(d1_err(e));
        }

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(&id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        if let Some(op_id) = operation_id {
            record_progress_operation(&self.db, op_id, &request_hash, &session).await?;
        }
        Ok(session)
    }

    async fn pause_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        let payload = serde_json::json!({"op": "pause_work_session", "id": id}).to_string();
        let request_hash = progress_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) =
                check_progress_idempotency::<WorkSessionRow>(&self.db, op_id, &request_hash).await?
        {
            return Ok(stored);
        }

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        let now = takusu_types::now_rfc3339();
        let was_open = session.ended_at.is_none();
        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        let update = self
            .db
            .prepare("UPDATE work_sessions SET ended_at = COALESCE(ended_at, ?1) WHERE id = ?2");
        stmts.push(
            update
                .bind(&[JsValue::from_str(&now), JsValue::from_str(id)])
                .map_err(d1_err)?,
        );

        if let Some(ref task_id) = session.task_id
            && was_open
        {
            let task = select_one_task(&self.db, task_id).await?;
            if task.status != TaskStatus::Completed && task.status != TaskStatus::Skipped {
                let task_update = self.db.prepare(
                    "UPDATE tasks SET status = 'scheduled', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
                );
                stmts.push(
                    task_update
                        .bind(&[JsValue::from_str(task_id)])
                        .map_err(d1_err)?,
                );
            }
        }

        self.db.batch(stmts).await.map_err(d1_err)?;

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        if let Some(op_id) = operation_id {
            record_progress_operation(&self.db, op_id, &request_hash, &session).await?;
        }
        Ok(session)
    }

    async fn complete_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        let payload = serde_json::json!({"op": "complete_work_session", "id": id}).to_string();
        let request_hash = progress_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) =
                check_progress_idempotency::<WorkSessionRow>(&self.db, op_id, &request_hash).await?
        {
            return Ok(stored);
        }

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        let now_rfc = takusu_types::now_rfc3339();
        let now_ts = takusu_types::Timestamp::now();

        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        let update = self
            .db
            .prepare("UPDATE work_sessions SET ended_at = COALESCE(ended_at, ?1) WHERE id = ?2");
        stmts.push(
            update
                .bind(&[JsValue::from_str(&now_rfc), JsValue::from_str(id)])
                .map_err(d1_err)?,
        );

        if let Some(task_id) = session.task_id.clone() {
            let task_before = select_one_task(&self.db, &task_id).await?;

            if task_before.status != TaskStatus::Completed
                && task_before.status != TaskStatus::Skipped
            {
                let session_stmt = self.db.prepare(format!(
                    "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE task_id = ?1 ORDER BY started_at ASC",
                ));
                let sessions: Vec<WorkSessionRow> = d1_all(
                    &session_stmt
                        .bind(&[JsValue::from_str(&task_id)])
                        .map_err(d1_err)?,
                )
                .await?;
                let current_minutes = if let Some(end) = session.ended_at {
                    takusu_types::minutes_between_ts(session.started_at, end)
                } else {
                    takusu_types::minutes_between_ts(session.started_at, now_ts)
                };
                let total_active_minutes: i64 = sessions
                    .iter()
                    .map(|s| {
                        if s.id == id {
                            current_minutes
                        } else {
                            session_minutes(s)
                        }
                    })
                    .sum();

                // #1419 (P2): genuine partial progress (0 < done < total) splits
                // the unfinished remainder into a new task instead of silently
                // inflating quantity_done to the total. Zero progress keeps the
                // previous "declared done" behaviour.
                let mut effective_total = task_before.quantity_total;
                if let Some(total) = task_before.quantity_total
                    && task_before.quantity_done > 0
                    && task_before.quantity_done < total
                {
                    let retained = task_before.quantity_done;
                    let remainder_quantity = Quantity::new(total.get() - retained.get())
                        .expect("partial completion guarantees quantity_done < quantity_total");
                    let original_quantity_total = task_before
                        .original_quantity_total
                        .filter(|t| *t != 0)
                        .unwrap_or(total);
                    let display_id = allocate_display_id(&self.db, None).await?;
                    let remainder_id = uuid::Uuid::now_v7().to_string();
                    let normalized_title = takusu_search::memory::normalize_text(
                        &task_before.title,
                        Some(takusu_search::memory::MAX_CONTENT_SCALARS),
                    )
                    .ok();
                    let depends_json =
                        takusu_types::DependencyList::new(Vec::new()).to_json_string();

                    let remainder_insert = self.db.prepare(
                        "INSERT INTO tasks (id, display_id, title, normalized_title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
                    );
                    stmts.push(
                        remainder_insert
                            .bind(&[
                                JsValue::from_str(&remainder_id),
                                JsValue::from_f64(display_id as f64),
                                JsValue::from_str(&task_before.title),
                                normalized_title
                                    .as_deref()
                                    .map(JsValue::from_str)
                                    .unwrap_or(JsValue::NULL),
                                task_before
                                    .description
                                    .as_deref()
                                    .map(JsValue::from_str)
                                    .unwrap_or(JsValue::NULL),
                                task_before
                                    .start_at
                                    .map(|t| JsValue::from_str(&t.to_string()))
                                    .unwrap_or(JsValue::NULL),
                                JsValue::from_str(&task_before.end_at.to_string()),
                                JsValue::from_f64(task_before.avg_minutes as f64),
                                JsValue::from_f64(task_before.sigma_minutes as f64),
                                JsValue::from_str(&depends_json),
                                JsValue::from_bool(task_before.parallelizable),
                                JsValue::from_bool(task_before.allows_parallel),
                                JsValue::from_f64(task_before.abandonability.into()),
                                JsValue::from_str("pending"),
                                JsValue::NULL,
                                JsValue::NULL,
                                JsValue::from_bool(task_before.fixed),
                                JsValue::NULL,
                                JsValue::from_f64(f64::from(remainder_quantity)),
                                JsValue::from_f64(0.0),
                                task_before
                                    .quantity_unit
                                    .as_deref()
                                    .map(JsValue::from_str)
                                    .unwrap_or(JsValue::NULL),
                                JsValue::NULL,
                                JsValue::from_str(&task_id),
                                JsValue::from_f64(f64::from(original_quantity_total)),
                                JsValue::from_str(&now_rfc),
                                JsValue::from_str(&now_rfc),
                            ])
                            .map_err(d1_err)?,
                    );

                    let original_update = self.db.prepare(
                        "UPDATE tasks SET quantity_total = ?1, original_quantity_total = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?3",
                    );
                    stmts.push(
                        original_update
                            .bind(&[
                                JsValue::from_f64(f64::from(retained)),
                                JsValue::from_f64(f64::from(original_quantity_total)),
                                JsValue::from_str(&task_id),
                            ])
                            .map_err(d1_err)?,
                    );

                    effective_total = Some(retained);
                }

                let quantity_done = effective_total.unwrap_or(task_before.quantity_done);
                let delta_quantity = quantity_done.get() - task_before.quantity_done.get();

                let (new_avg, new_sigma) = if total_active_minutes > 0 && !task_before.fixed {
                    let quantity_fraction = effective_total
                        .map(|total| quantity_done.get() as f64 / total.get() as f64)
                        .unwrap_or(1.0)
                        .clamp(f64::EPSILON, 1.0);
                    let mutation = estimator_observation(
                        &self.db,
                        &task_before,
                        total_active_minutes,
                        quantity_fraction,
                        now_ts,
                        "completion",
                    )
                    .await?;
                    stmts.extend(mutation.statements);
                    (mutation.avg_minutes, mutation.sigma_minutes)
                } else {
                    (task_before.avg_minutes, task_before.sigma_minutes)
                };

                let status = TaskStatus::Completed;
                let completed_at = task_before.completed_at.or(Some(now_ts));

                let task_update = self.db.prepare(
                    "UPDATE tasks SET status = ?1, completed_at = ?2, quantity_done = ?3, avg_minutes = ?4, sigma_minutes = ?5, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?6",
                );
                stmts.push(
                    task_update
                        .bind(&[
                            JsValue::from_str(&status.to_string()),
                            completed_at
                                .map(|t| JsValue::from_str(&t.to_string()))
                                .unwrap_or(JsValue::NULL),
                            JsValue::from_f64(f64::from(quantity_done)),
                            JsValue::from_f64(new_avg as f64),
                            JsValue::from_f64(new_sigma as f64),
                            JsValue::from_str(&task_id),
                        ])
                        .map_err(d1_err)?,
                );

                if total_active_minutes > 0 {
                    let event_id = uuid::Uuid::now_v7().to_string();
                    let insert = self.db.prepare(
                        "INSERT INTO progress_events (id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    );
                    stmts.push(
                        insert
                            .bind(&[
                                JsValue::from_str(&event_id),
                                JsValue::from_str(id),
                                JsValue::from_str(&task_id),
                                JsValue::from_str(&now_rfc),
                                JsValue::from_f64(f64::from(quantity_done)),
                                JsValue::from_f64(delta_quantity as f64),
                                JsValue::from_f64(total_active_minutes as f64),
                                JsValue::from_str("completed"),
                            ])
                            .map_err(d1_err)?,
                    );
                }
            }
        }

        self.db.batch(stmts).await.map_err(d1_err)?;

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        if let Some(op_id) = operation_id {
            record_progress_operation(&self.db, op_id, &request_hash, &session).await?;
        }
        Ok(session)
    }

    async fn record_work_session_progress(
        &self,
        id: &str,
        body: &RecordWorkSessionProgress,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionProgressResult> {
        let payload =
            serde_json::json!({"op": "record_work_session_progress", "id": id, "body": body})
                .to_string();
        let request_hash = progress_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) = check_progress_idempotency::<WorkSessionProgressResult>(
                &self.db,
                op_id,
                &request_hash,
            )
            .await?
        {
            return Ok(stored);
        }

        let mut session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        // Update the session's total when the caller supplies a new one.
        // A value of 0 is treated as "clear the total", matching create/update.
        if let Some(raw_total) = body.quantity_total {
            let desired = (raw_total != 0).then_some(raw_total);
            if desired != session.quantity_total {
                self.db
                    .prepare("UPDATE work_sessions SET quantity_total = ?1 WHERE id = ?2")
                    .bind(&[
                        desired
                            .map(|q| JsValue::from_f64(f64::from(q)))
                            .unwrap_or(JsValue::NULL),
                        JsValue::from_str(id),
                    ])
                    .map_err(d1_err)?
                    .run()
                    .await
                    .map_err(d1_err)?;
                session = self
                    .db
                    .prepare(format!(
                        "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
                    ))
                    .bind(&[JsValue::from_str(id)])
                    .map_err(d1_err)?
                    .first_t()
                    .await?
                    .ok_or_else(|| {
                        StorageError::NotFound(format!("work session {id} not found"))
                    })?;
            }
        }

        if let Some(total) = session.quantity_total
            && body.quantity_done > total
        {
            return Err(StorageError::BadRequest(format!(
                "quantity_done cannot exceed quantity_total ({} > {})",
                body.quantity_done, total
            )));
        }

        let mut linked_task: Option<TaskRow> = None;
        if let Some(ref task_id) = session.task_id {
            let task = select_one_task(&self.db, task_id).await?;
            if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
                return Err(StorageError::BadRequest(format!(
                    "cannot record progress on a {} task",
                    task.status
                )));
            }

            // Keep the linked task's total in sync with the session total.
            if let Some(raw_total) = body.quantity_total {
                let desired = (raw_total != 0).then_some(raw_total);
                if desired != task.quantity_total {
                    self.db
                        .prepare("UPDATE tasks SET quantity_total = ?1 WHERE id = ?2")
                        .bind(&[
                            desired
                                .map(|q| JsValue::from_f64(f64::from(q)))
                                .unwrap_or(JsValue::NULL),
                            JsValue::from_str(task_id),
                        ])
                        .map_err(d1_err)?
                        .run()
                        .await
                        .map_err(d1_err)?;
                }
            }

            // Re-fetch after any total update so validation uses the new total.
            let task = select_one_task(&self.db, task_id).await?;
            linked_task = Some(task);
        }

        let delta = body.quantity_done.get() - session.quantity_done.get();

        if delta == 0 {
            let suggests_completion = match &linked_task {
                Some(task) => task
                    .quantity_total
                    .map(|total| task.quantity_done >= total)
                    .unwrap_or(false),
                None => session
                    .quantity_total
                    .map(|total| body.quantity_done >= total)
                    .unwrap_or(false),
            };
            let result = WorkSessionProgressResult {
                work_session: session,
                task: linked_task,
                event: None,
                suggests_completion,
                estimator: None,
            };
            if let Some(op_id) = operation_id {
                record_progress_operation(&self.db, op_id, &request_hash, &result).await?;
            }
            return Ok(result);
        }

        if session.ended_at.is_some() && body.quantity_done > session.quantity_done {
            return Err(StorageError::BadRequest(
                "cannot record progress on a closed work session".into(),
            ));
        }

        let now_rfc = takusu_types::now_rfc3339();
        let active_minutes = if session.ended_at.is_none() && delta > 0 {
            let last_event: Option<ProgressEventRow> = self
                .db
                .prepare(format!(
                    "SELECT {PROGRESS_EVENT_COLS} FROM progress_events WHERE work_session_id = ?1 ORDER BY at DESC, id DESC LIMIT 1",
                ))
                .bind(&[JsValue::from_str(id)])
                .map_err(d1_err)?
                .first_t()
                .await?;
            let base = if let Some(ref ev) = last_event {
                if ev.at >= session.started_at {
                    ev.at
                } else {
                    session.started_at
                }
            } else {
                session.started_at
            };
            takusu_types::minutes_between_ts(base, takusu_types::Timestamp::now())
        } else {
            0
        };

        let event_id = uuid::Uuid::now_v7().to_string();
        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        let insert = self.db.prepare(
            "INSERT INTO progress_events (id, work_session_id, task_id, at, quantity_done, delta_quantity, active_minutes, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        );
        stmts.push(
            insert
                .bind(&[
                    JsValue::from_str(&event_id),
                    JsValue::from_str(id),
                    session
                        .task_id
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&now_rfc),
                    JsValue::from_f64(f64::from(body.quantity_done)),
                    JsValue::from_f64(delta as f64),
                    JsValue::from_f64(active_minutes as f64),
                    body.note
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                ])
                .map_err(d1_err)?,
        );

        let ws_update = self
            .db
            .prepare("UPDATE work_sessions SET quantity_done = ?1 WHERE id = ?2");
        stmts.push(
            ws_update
                .bind(&[
                    JsValue::from_f64(f64::from(body.quantity_done)),
                    JsValue::from_str(id),
                ])
                .map_err(d1_err)?,
        );

        let mut result_task: Option<TaskRow> = None;
        let mut suggests_completion = false;
        let mut estimator_result = None;

        if let Some(ref task) = linked_task {
            let task_id = task.id.clone();
            let task_delta = body.quantity_done.get() - session.quantity_done.get();
            let new_done_i64 = task.quantity_done.get() + task_delta;
            let new_done = Quantity::new(new_done_i64)
                .map_err(|e| StorageError::BadRequest(format!("invalid task quantity: {e}")))?;

            if let Some(total) = task.quantity_total {
                if new_done > total {
                    return Err(StorageError::BadRequest(format!(
                        "quantity_done would exceed quantity_total ({} > {})",
                        new_done, total
                    )));
                }
                suggests_completion = new_done >= total;
            }

            let mut new_avg = task.avg_minutes;
            let mut new_sigma = task.sigma_minutes;
            if task_delta > 0
                && active_minutes > 0
                && let Some(total) = task.quantity_total
                && !task.fixed
            {
                let session_stmt = self.db.prepare(format!(
                    "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE task_id = ?1 ORDER BY started_at ASC",
                ));
                let sessions: Vec<WorkSessionRow> = d1_all(
                    &session_stmt
                        .bind(&[JsValue::from_str(&task_id)])
                        .map_err(d1_err)?,
                )
                .await?;
                let total_active_minutes = sessions.iter().map(session_minutes).sum::<i64>();
                if total_active_minutes > 0 {
                    let mutation = estimator_observation(
                        &self.db,
                        task,
                        total_active_minutes,
                        (new_done.get() as f64 / total.get() as f64).min(1.0),
                        takusu_types::Timestamp::now(),
                        "progress",
                    )
                    .await?;
                    estimator_result = Some(mutation.result);
                    new_avg = mutation.avg_minutes;
                    new_sigma = mutation.sigma_minutes;
                    stmts.extend(mutation.statements);
                }
            } else if task_delta < 0
                && !task.fixed
                && let Some(mutation) =
                    compensate_last_estimator_observation(&self.db, task).await?
            {
                estimator_result = Some(mutation.result);
                new_avg = mutation.avg_minutes;
                new_sigma = mutation.sigma_minutes;
                stmts.extend(mutation.statements);
            }

            let new_status = if task.status == TaskStatus::Completed {
                TaskStatus::Completed
            } else if task_delta < 0 {
                task.status
            } else {
                TaskStatus::InProgress
            };

            let task_update = self.db.prepare(
                "UPDATE tasks SET quantity_done = ?1, avg_minutes = ?2, sigma_minutes = ?3, status = ?4, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?5",
            );
            stmts.push(
                task_update
                    .bind(&[
                        JsValue::from_f64(f64::from(new_done)),
                        JsValue::from_f64(new_avg as f64),
                        JsValue::from_f64(new_sigma as f64),
                        JsValue::from_str(&new_status.to_string()),
                        JsValue::from_str(&task_id),
                    ])
                    .map_err(d1_err)?,
            );
        } else if let Some(total) = session.quantity_total {
            suggests_completion = body.quantity_done >= total;
        }

        self.db.batch(stmts).await.map_err(d1_err)?;

        if let Some(ref task) = linked_task {
            result_task = Some(select_one_task(&self.db, &task.id).await?);
        }

        let event: ProgressEventRow = self
            .db
            .prepare(format!(
                "SELECT {PROGRESS_EVENT_COLS} FROM progress_events WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(&event_id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::Internal("inserted progress event not found".into()))?;

        let work_session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        let result = WorkSessionProgressResult {
            work_session,
            task: result_task,
            event: Some(event),
            suggests_completion,
            estimator: estimator_result,
        };
        if let Some(op_id) = operation_id {
            record_progress_operation(&self.db, op_id, &request_hash, &result).await?;
        }
        Ok(result)
    }

    async fn get_work_session(&self, id: &str) -> StorageResult<WorkSessionRow> {
        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;
        Ok(session)
    }

    async fn list_work_sessions(
        &self,
        task_id: Option<&str>,
    ) -> StorageResult<Vec<WorkSessionRow>> {
        if let Some(id) = task_id {
            let full = resolve_task_id(&self.db, id).await?;
            let sessions: Vec<WorkSessionRow> = d1_all(
                &self
                    .db
                    .prepare(format!(
                        "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE task_id = ?1 ORDER BY started_at ASC",
                    ))
                    .bind(&[JsValue::from_str(&full)])
                    .map_err(d1_err)?,
            )
            .await?;
            Ok(sessions)
        } else {
            let sessions: Vec<WorkSessionRow> = d1_all(&self.db.prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions ORDER BY started_at DESC",
            )))
            .await?;
            Ok(sessions)
        }
    }

    async fn attach_work_session(
        &self,
        id: &str,
        body: &AttachWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        let payload =
            serde_json::json!({"op": "attach_work_session", "id": id, "body": body}).to_string();
        let request_hash = progress_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) =
                check_progress_idempotency::<WorkSessionRow>(&self.db, op_id, &request_hash).await?
        {
            return Ok(stored);
        }

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        let full = resolve_task_id(&self.db, &body.task_id).await?;
        let task = select_one_task(&self.db, &full).await?;
        if task.status == TaskStatus::Completed || task.status == TaskStatus::Skipped {
            return Err(StorageError::BadRequest(format!(
                "cannot attach a work session to a {} task",
                task.status
            )));
        }

        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        let update = self
            .db
            .prepare("UPDATE work_sessions SET task_id = ?1 WHERE id = ?2");
        stmts.push(
            update
                .bind(&[JsValue::from_str(&full), JsValue::from_str(id)])
                .map_err(d1_err)?,
        );

        let pe_update = self
            .db
            .prepare("UPDATE progress_events SET task_id = ?1 WHERE work_session_id = ?2");
        stmts.push(
            pe_update
                .bind(&[JsValue::from_str(&full), JsValue::from_str(id)])
                .map_err(d1_err)?,
        );

        if session.ended_at.is_none() {
            let task_update = self.db.prepare(
                "UPDATE tasks SET status = 'in_progress', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
            );
            stmts.push(
                task_update
                    .bind(&[JsValue::from_str(&full)])
                    .map_err(d1_err)?,
            );
        }

        self.db.batch(stmts).await.map_err(d1_err)?;

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        if let Some(op_id) = operation_id {
            record_progress_operation(&self.db, op_id, &request_hash, &session).await?;
        }
        Ok(session)
    }

    async fn convert_work_session(
        &self,
        id: &str,
        body: &ConvertWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<TaskRow> {
        let payload =
            serde_json::json!({"op": "convert_work_session", "id": id, "body": body}).to_string();
        let request_hash = progress_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) =
                check_progress_idempotency::<TaskRow>(&self.db, op_id, &request_hash).await?
        {
            return Ok(stored);
        }

        let now_rfc = takusu_types::now_rfc3339();
        let now_ts = takusu_types::Timestamp::now();

        let update = self
            .db
            .prepare("UPDATE work_sessions SET ended_at = COALESCE(ended_at, ?1) WHERE id = ?2");
        update
            .bind(&[JsValue::from_str(&now_rfc), JsValue::from_str(id)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;

        let session: WorkSessionRow = self
            .db
            .prepare(format!(
                "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE id = ?1",
            ))
            .bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("work session {id} not found")))?;

        let active_minutes = session_minutes(&session);
        let end_at = session.ended_at.unwrap_or(now_ts);
        let status = body.status.unwrap_or(TaskStatus::Completed);

        let display_id = allocate_display_id(&self.db, None).await?;
        let task_id = uuid::Uuid::now_v7().to_string();
        let title = body
            .title
            .clone()
            .or(session.title.clone())
            .unwrap_or_else(|| "converted session".into());
        let normalized_title = takusu_search::memory::normalize_text(
            &title,
            Some(takusu_search::memory::MAX_CONTENT_SCALARS),
        )
        .ok();
        let description = session.note.clone();
        let quantity_total = session.quantity_total.filter(|q| *q != 0);
        let quantity_unit = session.quantity_unit.as_deref();
        let fixed = body.fixed.unwrap_or(true);
        let avg_minutes = active_minutes.max(1);
        let sigma_minutes = 0;
        let completed_at = if status == TaskStatus::Completed {
            Some(now_ts)
        } else {
            None
        };
        let depends = takusu_types::DependencyList::new(Vec::new());
        let depends_json = depends.to_json_string();

        let insert = self.db.prepare(
            "INSERT INTO tasks (id, display_id, title, normalized_title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        );
        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        stmts.push(
            insert
                .bind(&[
                    JsValue::from_str(&task_id),
                    JsValue::from_f64(display_id as f64),
                    JsValue::from_str(&title),
                    normalized_title
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    description
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&session.started_at.to_string()),
                    JsValue::from_str(&end_at.to_string()),
                    JsValue::from_f64(avg_minutes as f64),
                    JsValue::from_f64(sigma_minutes as f64),
                    JsValue::from_str(&depends_json),
                    JsValue::from_bool(false),
                    JsValue::from_bool(false),
                    JsValue::from_f64(takusu_types::Abandonability::default().into()),
                    JsValue::from_str(&status.to_string()),
                    JsValue::NULL,
                    JsValue::NULL,
                    JsValue::from_bool(fixed),
                    JsValue::NULL,
                    quantity_total
                        .map(|q| JsValue::from_f64(f64::from(q)))
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_f64(f64::from(session.quantity_done)),
                    quantity_unit
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    completed_at
                        .map(|t| JsValue::from_str(&t.to_string()))
                        .unwrap_or(JsValue::NULL),
                    JsValue::NULL,
                    JsValue::NULL,
                    JsValue::from_str(&now_rfc),
                    JsValue::from_str(&now_rfc),
                ])
                .map_err(d1_err)?,
        );

        let ws_update = self
            .db
            .prepare("UPDATE work_sessions SET task_id = ?1 WHERE id = ?2");
        stmts.push(
            ws_update
                .bind(&[JsValue::from_str(&task_id), JsValue::from_str(id)])
                .map_err(d1_err)?,
        );

        let pe_update = self
            .db
            .prepare("UPDATE progress_events SET task_id = ?1 WHERE work_session_id = ?2");
        stmts.push(
            pe_update
                .bind(&[JsValue::from_str(&task_id), JsValue::from_str(id)])
                .map_err(d1_err)?,
        );

        self.db.batch(stmts).await.map_err(d1_err)?;

        let mut task = select_one_task(&self.db, &task_id).await?;
        if !task.fixed && active_minutes > 0 {
            let mutation =
                estimator_observation(&self.db, &task, active_minutes, 1.0, now_ts, "completion")
                    .await?;
            self.db.batch(mutation.statements).await.map_err(d1_err)?;
            task = select_one_task(&self.db, &task_id).await?;
        }

        if let Some(op_id) = operation_id {
            record_progress_operation(&self.db, op_id, &request_hash, &task).await?;
        }
        Ok(task)
    }

    async fn get_task_progress(&self, id: &str) -> StorageResult<TaskProgress> {
        let full = resolve_task_id(&self.db, id).await?;
        let task = select_one_task(&self.db, &full).await?;

        let session_stmt = self.db.prepare(format!(
            "SELECT {WORK_SESSION_COLS} FROM work_sessions WHERE task_id = ?1 ORDER BY started_at ASC",
        ));
        let sessions: Vec<WorkSessionRow> = d1_all(
            &session_stmt
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
        )
        .await?;

        let event_stmt = self.db.prepare(format!(
            "SELECT {PROGRESS_EVENT_COLS} FROM progress_events WHERE task_id = ?1 ORDER BY id ASC",
        ));
        let events: Vec<ProgressEventRow> = d1_all(
            &event_stmt
                .bind(&[JsValue::from_str(&full)])
                .map_err(d1_err)?,
        )
        .await?;

        let open_session = sessions.iter().find(|s| s.ended_at.is_none()).cloned();
        let total_active_minutes = sessions.iter().map(session_minutes).sum();
        let estimator = if task.fixed {
            None
        } else {
            Some(estimator_state(&self.db, &task).await?)
        };

        Ok(TaskProgress {
            task,
            open_session,
            sessions,
            events,
            total_active_minutes,
            estimator,
        })
    }

    async fn get_estimator_state(&self, id: &str) -> StorageResult<Option<EstimatorStateRow>> {
        let task_id = resolve_task_id(&self.db, id).await?;
        let task = select_one_task(&self.db, &task_id).await?;
        if task.fixed {
            return Ok(None);
        }
        Ok(Some(estimator_state(&self.db, &task).await?))
    }

    async fn get_evaluation_inputs(&self) -> StorageResult<EvaluationInputs> {
        let tasks = self.list_tasks(&TaskQuery::default()).await?;
        let schedule = self.get_schedule().await?;
        let ledger = self.list_event_ledger(None).await?;
        let schedule_revision = self.get_schedule_revision().await?;

        let progress = batch_evaluation_progress(&self.db, &tasks).await?;
        let coverage = self.get_coverage_evaluation().await?;

        Ok(EvaluationInputs {
            schedule_revision,
            tasks,
            schedule: schedule
                .map(|row| row.schedule.as_inner().clone())
                .unwrap_or_default(),
            progress,
            ledger,
            coverage,
        })
    }

    async fn get_coverage_evaluation(&self) -> StorageResult<CoverageEvaluation> {
        let stmt = self.db.prepare(
            "SELECT id, start_at, end_at, timezone, source, schedule_revision, calendar_health, created_at, settled_at, operation_id FROM coverage_confirmations ORDER BY created_at DESC",
        );
        let confirmations: Vec<CoverageConfirmationRow> = d1_all(&stmt).await?;

        let stmt = self.db.prepare(
            "SELECT id, start_at, end_at, classification, source, created_at, settled_at, operation_id FROM unsettled_intervals WHERE settled_at IS NULL ORDER BY start_at",
        );
        let unsettled: Vec<UnsettledIntervalRow> = d1_all(&stmt).await?;

        let schedule_revision_stmt = self.db.prepare(
            "SELECT COALESCE(MAX(schedule_revision), 0) AS value FROM coverage_confirmations",
        );
        let schedule_revision: i64 = d1_first::<serde_json::Value>(&schedule_revision_stmt)
            .await?
            .and_then(|v| v.get("value").and_then(|v| v.as_i64()))
            .unwrap_or(0);

        Ok(CoverageEvaluation {
            state: CoverageState::Bootstrap,
            confirmations,
            unsettled_intervals: unsettled,
            schedule_revision,
        })
    }

    async fn create_coverage_confirmation(
        &self,
        body: &CreateCoverageConfirmation,
    ) -> StorageResult<CoverageConfirmationRow> {
        let id = uuid::Uuid::now_v7().to_string();
        let stmt = self.db.prepare(
            "INSERT INTO coverage_confirmations (id, start_at, end_at, timezone, source, schedule_revision, calendar_health, created_at, operation_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?8)",
        );
        stmt.bind(&[
            JsValue::from_str(&id),
            JsValue::from_str(&body.start_at.to_string()),
            JsValue::from_str(&body.end_at.to_string()),
            JsValue::from_str(&body.timezone),
            JsValue::from_str(&body.source),
            JsValue::from_f64(body.schedule_revision as f64),
            JsValue::from_str(&body.calendar_health),
            body.operation_id
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;

        let stmt = self.db.prepare(
            "SELECT id, start_at, end_at, timezone, source, schedule_revision, calendar_health, created_at, settled_at, operation_id FROM coverage_confirmations WHERE id = ?1",
        );
        let stmt = stmt.bind(&[JsValue::from_str(&id)]).map_err(d1_err)?;
        d1_first::<CoverageConfirmationRow>(&stmt)
            .await?
            .ok_or_else(|| StorageError::NotFound("coverage confirmation not found".into()))
    }

    async fn create_unsettled_interval(
        &self,
        body: &CreateUnsettledInterval,
    ) -> StorageResult<UnsettledIntervalRow> {
        let id = uuid::Uuid::now_v7().to_string();
        let stmt = self.db.prepare(
            "INSERT INTO unsettled_intervals (id, start_at, end_at, classification, source, created_at, operation_id) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?6)",
        );
        stmt.bind(&[
            JsValue::from_str(&id),
            JsValue::from_str(&body.start_at.to_string()),
            JsValue::from_str(&body.end_at.to_string()),
            JsValue::from_str(&body.classification),
            JsValue::from_str(&body.source),
            body.operation_id
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;

        let stmt = self.db.prepare(
            "SELECT id, start_at, end_at, classification, source, created_at, settled_at, operation_id FROM unsettled_intervals WHERE id = ?1",
        );
        let stmt = stmt.bind(&[JsValue::from_str(&id)]).map_err(d1_err)?;
        d1_first::<UnsettledIntervalRow>(&stmt)
            .await?
            .ok_or_else(|| StorageError::NotFound("unsettled interval not found".into()))
    }

    async fn settle_unsettled_interval(
        &self,
        id: &str,
        operation_id: &str,
    ) -> StorageResult<UnsettledIntervalRow> {
        let stmt = self.db.prepare(
            "UPDATE unsettled_intervals SET settled_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), operation_id = ?1 WHERE id = ?2",
        );
        stmt.bind(&[JsValue::from_str(operation_id), JsValue::from_str(id)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;

        let stmt = self.db.prepare(
            "SELECT id, start_at, end_at, classification, source, created_at, settled_at, operation_id FROM unsettled_intervals WHERE id = ?1",
        );
        let stmt = stmt.bind(&[JsValue::from_str(id)]).map_err(d1_err)?;
        d1_first::<UnsettledIntervalRow>(&stmt)
            .await?
            .ok_or_else(|| StorageError::NotFound("unsettled interval not found".into()))
    }

    async fn get_schedule_revision(&self) -> StorageResult<i64> {
        #[derive(serde::Deserialize)]
        struct RevisionRow {
            revision: i64,
        }
        let stmt = self
            .db
            .prepare("SELECT revision FROM schedule_revisions WHERE id = 'active'");
        Ok(d1_first::<RevisionRow>(&stmt)
            .await?
            .map(|row| row.revision)
            .unwrap_or(0))
    }

    async fn list_event_ledger(
        &self,
        device_id: Option<&str>,
    ) -> StorageResult<Vec<EventLedgerRow>> {
        let sql = match device_id {
            Some(_) => {
                "SELECT id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at, delivered_at FROM event_ledger e WHERE NOT EXISTS (SELECT 1 FROM event_delivery_claims c WHERE c.event_id = e.id AND c.device_id = ?1 AND datetime(c.claimed_at) > datetime('now', '-10 minutes')) ORDER BY e.created_at, e.id"
            }
            None => {
                "SELECT id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at, delivered_at FROM event_ledger ORDER BY created_at, id"
            }
        };
        let stmt = self.db.prepare(sql);
        let stmt = match device_id {
            Some(device_id) => stmt.bind(&[JsValue::from_str(device_id)]).map_err(d1_err)?,
            None => stmt,
        };
        d1_all(&stmt).await
    }

    async fn insert_event_ledger(
        &self,
        event: &EventLedgerInsert,
    ) -> StorageResult<EventLedgerRow> {
        let insert = self
            .db
            .prepare("INSERT OR IGNORE INTO event_ledger (id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending_delivery', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))")
            .bind(&[
                JsValue::from_str(&event.id),
                JsValue::from_str(&event.kind),
                event
                    .task_id
                    .as_deref()
                    .map(JsValue::from_str)
                    .unwrap_or(JsValue::NULL),
                JsValue::from_str(&event.presentation),
                JsValue::from_str(&event.urgency),
                JsValue::from_f64(event.schedule_revision as f64),
                event
                    .distribution_revision
                    .map(|value| JsValue::from_f64(value as f64))
                    .unwrap_or(JsValue::NULL),
                JsValue::from_str(&event.observation_kind),
            ])
            .map_err(d1_err)?;
        insert.run().await.map_err(d1_err)?;
        self.db
            .prepare("SELECT id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at, delivered_at FROM event_ledger WHERE id = ?1")
            .bind(&[JsValue::from_str(&event.id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::Internal("event ledger row missing after insert".into()))
    }

    async fn commit_event_evaluation(
        &self,
        schedule_revision: i64,
        events: &[EventLedgerInsert],
    ) -> StorageResult<()> {
        let current = self.get_schedule_revision().await?;
        if current != schedule_revision {
            return Err(StorageError::Conflict("schedule revision changed".into()));
        }

        if events.is_empty() {
            return Ok(());
        }

        let sql = "INSERT OR IGNORE INTO event_ledger (id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending_delivery', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))";
        let mut stmts = Vec::with_capacity(events.len());
        for event in events {
            let stmt = self
                .db
                .prepare(sql)
                .bind(&[
                    JsValue::from_str(&event.id),
                    JsValue::from_str(&event.kind),
                    event
                        .task_id
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&event.presentation),
                    JsValue::from_str(&event.urgency),
                    JsValue::from_f64(event.schedule_revision as f64),
                    event
                        .distribution_revision
                        .map(|value| JsValue::from_f64(value as f64))
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&event.observation_kind),
                ])
                .map_err(d1_err)?;
            stmts.push(stmt);
        }

        self.db.batch(stmts).await.map_err(d1_err)?;
        Ok(())
    }

    async fn claim_event_delivery(&self, device_id: &str, event_id: &str) -> StorageResult<bool> {
        let existing: Option<IdRow> = d1_first(
            &self
                .db
                .prepare("SELECT id FROM event_ledger WHERE id = ?1")
                .bind(&[JsValue::from_str(event_id)])
                .map_err(d1_err)?,
        )
        .await?;
        if existing.is_none() {
            return Err(not_found(format!("event {event_id} not found")));
        }
        let result = self
            .db
            .prepare("INSERT INTO event_delivery_claims (event_id, device_id, claimed_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(event_id, device_id) DO UPDATE SET claimed_at = excluded.claimed_at WHERE datetime(event_delivery_claims.claimed_at) <= datetime('now', '-10 minutes')")
            .bind(&[JsValue::from_str(event_id), JsValue::from_str(device_id)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        Ok(result
            .meta()
            .map_err(d1_err)?
            .and_then(|meta| meta.rows_written)
            .unwrap_or(0)
            > 0)
    }

    async fn update_event_delivery_state(
        &self,
        event_id: &str,
        state: EventDeliveryState,
    ) -> StorageResult<EventLedgerRow> {
        let current: EventLedgerRow = self
            .db
            .prepare("SELECT id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at, delivered_at FROM event_ledger WHERE id = ?1")
            .bind(&[JsValue::from_str(event_id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| not_found(format!("event {event_id} not found")))?;
        if !valid_event_transition(current.delivery_state, state) {
            return Err(StorageError::Conflict(format!(
                "cannot transition event {event_id} from {} to {state}",
                current.delivery_state
            )));
        }
        let updated = self
            .db
            .prepare("UPDATE event_ledger SET delivery_state = ?1, delivered_at = CASE WHEN ?1 = 'delivered' THEN COALESCE(delivered_at, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ELSE delivered_at END WHERE id = ?2 AND delivery_state = ?3")
            .bind(&[
                JsValue::from_str(state.as_str()),
                JsValue::from_str(event_id),
                JsValue::from_str(current.delivery_state.as_str()),
            ])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        if updated
            .meta()
            .map_err(d1_err)?
            .and_then(|meta| meta.rows_written)
            .unwrap_or(0)
            == 0
        {
            return Err(StorageError::Conflict(format!(
                "event {event_id} changed during delivery update"
            )));
        }
        self.db
            .prepare("SELECT id, kind, task_id, presentation, urgency, schedule_revision, distribution_revision, observation_kind, delivery_state, created_at, delivered_at FROM event_ledger WHERE id = ?1")
            .bind(&[JsValue::from_str(event_id)])
            .map_err(d1_err)?
            .first_t()
            .await?
            .ok_or_else(|| StorageError::Internal("event ledger row missing after update".into()))
    }

    async fn split_task(
        &self,
        id: &str,
        body: &SplitTask,
        operation_id: Option<&str>,
    ) -> StorageResult<SplitResult> {
        if body.retained_quantity < 0 {
            return Err(StorageError::BadRequest(
                "retained_quantity cannot be negative".into(),
            ));
        }
        let payload = serde_json::json!({"op": "split", "id": id, "body": body}).to_string();
        let request_hash = progress_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) =
                check_progress_idempotency::<SplitResult>(&self.db, op_id, &request_hash).await?
        {
            return Ok(stored);
        }
        let full = resolve_task_id(&self.db, id).await?;
        let original = select_one_task(&self.db, &full).await?;
        if body.end_at.is_some() {
            validate_task_datetimes(None, body.end_at.as_ref(), original.start_at.as_ref(), None)?;
        }
        if original.status == TaskStatus::Completed || original.status == TaskStatus::Skipped {
            return Err(StorageError::BadRequest(format!(
                "cannot split a {} task",
                original.status
            )));
        }
        let total = original.quantity_total.ok_or_else(|| {
            StorageError::BadRequest("cannot split a task with no quantity_total".into())
        })?;
        if body.retained_quantity <= 0 {
            return Err(StorageError::BadRequest(
                "retained_quantity must be greater than 0".into(),
            ));
        }
        if body.retained_quantity > total {
            return Err(StorageError::BadRequest(
                "retained_quantity cannot exceed quantity_total".into(),
            ));
        }
        if body.retained_quantity == total {
            return Err(StorageError::BadRequest(
                "retained_quantity must be less than quantity_total".into(),
            ));
        }
        if body.retained_quantity < original.quantity_done {
            return Err(StorageError::BadRequest(
                "retained_quantity cannot be less than quantity_done".into(),
            ));
        }
        let remainder_quantity = Quantity::new(total.get() - body.retained_quantity.get())
            .expect("retained_quantity is less than total, so remainder is non-negative");
        let original_quantity_total = original
            .original_quantity_total
            .filter(|t| *t != 0)
            .unwrap_or(total);
        let remainder_id = uuid::Uuid::now_v7().to_string();
        let display_id = allocate_display_id(&self.db, None).await?;
        let depends = if body.set_dependency.unwrap_or(false) {
            vec![full.clone()]
        } else {
            Vec::new()
        };
        let depends_json = DependencyList::new(depends).to_json_string();
        let remainder_title = body.title.as_ref().unwrap_or(&original.title);
        let normalized_title = takusu_search::memory::normalize_text(
            remainder_title,
            Some(takusu_search::memory::MAX_CONTENT_SCALARS),
        )
        .ok();
        let insert = self.db.prepare(
            "INSERT INTO tasks (id, display_id, title, normalized_title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'pending', ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        );
        let new_done = original.quantity_done.min(body.retained_quantity);
        let mut stmts: Vec<worker::D1PreparedStatement> = Vec::new();
        stmts.push(
            insert
                .bind(&[
                    JsValue::from_str(&remainder_id),
                    JsValue::from_f64(display_id as f64),
                    JsValue::from_str(remainder_title.as_str()),
                    normalized_title
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    body.description
                        .as_ref()
                        .or(original.description.as_ref())
                        .map(|s| JsValue::from_str(s.as_str()))
                        .unwrap_or(JsValue::NULL),
                    original
                        .start_at
                        .as_ref()
                        .map(|s| JsValue::from_str(&s.to_string()))
                        .unwrap_or(JsValue::NULL),
                    JsValue::from_str(&body.end_at.unwrap_or(original.end_at).to_string()),
                    JsValue::from_f64(original.avg_minutes as f64),
                    JsValue::from_f64(original.sigma_minutes as f64),
                    JsValue::from_str(&depends_json),
                    JsValue::from_bool(original.parallelizable),
                    JsValue::from_bool(original.allows_parallel),
                    JsValue::from_f64(original.abandonability.into()),
                    JsValue::NULL,
                    JsValue::NULL,
                    JsValue::from_bool(original.fixed),
                    JsValue::NULL,
                    JsValue::from_f64(f64::from(remainder_quantity)),
                    JsValue::from_f64(0.0),
                    original
                        .quantity_unit
                        .as_deref()
                        .map(JsValue::from_str)
                        .unwrap_or(JsValue::NULL),
                    JsValue::NULL,
                    JsValue::from_str(&full),
                    JsValue::from_f64(f64::from(original_quantity_total)),
                ])
                .map_err(d1_err)?,
        );
        let update = self.db.prepare(
            "UPDATE tasks SET quantity_total = ?1, quantity_done = ?2, original_quantity_total = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?4",
        );
        stmts.push(
            update
                .bind(&[
                    JsValue::from_f64(f64::from(body.retained_quantity)),
                    JsValue::from_f64(f64::from(new_done)),
                    JsValue::from_f64(f64::from(original_quantity_total)),
                    JsValue::from_str(&full),
                ])
                .map_err(d1_err)?,
        );
        self.db.batch(stmts).await.map_err(d1_err)?;
        let original = select_one_task(&self.db, &full).await?;
        let remainder = select_one_task(&self.db, &remainder_id).await?;
        let result = SplitResult {
            original,
            remainder,
        };
        if let Some(op_id) = operation_id {
            record_progress_operation(&self.db, op_id, &request_hash, &result).await?;
        }
        Ok(result)
    }

    // ── Google Calendar sync ────────────────────────────────────────────

    async fn get_gcal_settings(&self) -> StorageResult<GoogleCalSettingsRow> {
        let stmt = self.db.prepare("SELECT id, enabled, calendar_id, client_id, client_secret, refresh_token, reminder_minutes, color_id, visibility, transparency, created_at, updated_at FROM google_cal_settings WHERE id = 'active'");
        let rows: Vec<GoogleCalSettingsRow> = d1_all(&stmt).await?;
        Ok(rows
            .into_iter()
            .next()
            .unwrap_or_else(|| GoogleCalSettingsRow {
                id: "active".to_string(),
                enabled: false,
                calendar_id: "primary".to_string(),
                client_id: String::new(),
                client_secret: String::new(),
                refresh_token: None,
                reminder_minutes: None,
                color_id: None,
                visibility: None,
                transparency: None,
                created_at: takusu_types::Timestamp::default(),
                updated_at: takusu_types::Timestamp::default(),
            }))
    }

    async fn update_gcal_settings(
        &self,
        body: &UpdateGoogleCalSettings,
    ) -> StorageResult<GoogleCalSettingsRow> {
        let existing = self.get_gcal_settings().await?;
        let enabled = body.enabled.unwrap_or(existing.enabled);
        let calendar_id = body
            .calendar_id
            .clone()
            .unwrap_or_else(|| existing.calendar_id.clone());
        let client_id = body
            .client_id
            .clone()
            .unwrap_or_else(|| existing.client_id.clone());
        let client_secret = body
            .client_secret
            .clone()
            .unwrap_or_else(|| existing.client_secret.clone());
        let refresh_token = body
            .refresh_token
            .clone()
            .or_else(|| existing.refresh_token.clone());
        let reminder_minutes = body
            .reminder_minutes
            .as_ref()
            .map_or(existing.reminder_minutes, |x| *x);
        let color_id = body.color_id.as_ref().map_or(existing.color_id, |x| *x);
        let visibility = body
            .visibility
            .as_ref()
            .map_or(existing.visibility.clone(), |x| x.clone());
        let transparency = body
            .transparency
            .as_ref()
            .map_or(existing.transparency.clone(), |x| x.clone());
        let stmt = self.db.prepare(
            "INSERT INTO google_cal_settings (id, enabled, calendar_id, client_id, client_secret, refresh_token, reminder_minutes, color_id, visibility, transparency, created_at, updated_at) VALUES ('active', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled, calendar_id=excluded.calendar_id, client_id=excluded.client_id, client_secret=excluded.client_secret, refresh_token=excluded.refresh_token, reminder_minutes=excluded.reminder_minutes, color_id=excluded.color_id, visibility=excluded.visibility, transparency=excluded.transparency, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
        );
        stmt.bind(&[
            JsValue::from_bool(enabled),
            JsValue::from_str(&calendar_id),
            JsValue::from_str(&client_id),
            JsValue::from_str(&client_secret),
            refresh_token
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            reminder_minutes
                .map(|n| JsValue::from_f64(n as f64))
                .unwrap_or(JsValue::NULL),
            color_id
                .map(|n| JsValue::from_f64(n as f64))
                .unwrap_or(JsValue::NULL),
            visibility
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
            transparency
                .as_deref()
                .map(JsValue::from_str)
                .unwrap_or(JsValue::NULL),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        self.get_gcal_settings().await
    }

    async fn list_gcal_mappings(&self) -> StorageResult<Vec<GoogleCalEventRow>> {
        let stmt = self
            .db
            .prepare("SELECT task_id, google_event_id, updated_at FROM google_cal_events");
        d1_all(&stmt).await
    }

    async fn upsert_gcal_mappings(&self, mappings: &[(String, String)]) -> StorageResult<()> {
        for (task_id, google_event_id) in mappings {
            let stmt = self.db.prepare(
                "INSERT INTO google_cal_events (task_id, google_event_id, updated_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(task_id) DO UPDATE SET google_event_id=excluded.google_event_id, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
            );
            stmt.bind(&[
                JsValue::from_str(task_id),
                JsValue::from_str(google_event_id),
            ])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        }
        Ok(())
    }

    async fn delete_gcal_mappings(&self, task_ids: &[String]) -> StorageResult<()> {
        for id in task_ids {
            let stmt = self
                .db
                .prepare("DELETE FROM google_cal_events WHERE task_id = ?1");
            stmt.bind(&[JsValue::from_str(id)])
                .map_err(d1_err)?
                .run()
                .await
                .map_err(d1_err)?;
        }
        Ok(())
    }

    async fn clear_gcal_mappings(&self) -> StorageResult<()> {
        let stmt = self.db.prepare("DELETE FROM google_cal_events");
        stmt.run().await.map_err(d1_err)?;
        Ok(())
    }

    async fn get_move_idempotency(
        &self,
        operation_id: &str,
        request_hash: &str,
    ) -> StorageResult<Option<MoveEntryResponse>> {
        check_progress_idempotency::<MoveEntryResponse>(&self.db, operation_id, request_hash).await
    }

    async fn record_move_idempotency(
        &self,
        operation_id: &str,
        request_hash: &str,
        response: &MoveEntryResponse,
    ) -> StorageResult<()> {
        record_progress_operation(&self.db, operation_id, request_hash, response).await
    }

    // ── Multi-device arbitration (WI-11) ─────────────────────────────────

    async fn register_device(&self, body: &CreateDevice) -> StorageResult<DeviceRow> {
        let priority = body.priority.unwrap_or(match body.platform {
            takusu_contracts::DevicePlatform::Desktop => 0,
            takusu_contracts::DevicePlatform::Android => 1,
        });
        let stmt = self.db.prepare(
            "INSERT INTO devices (id, name, platform, priority, evaluator_heartbeat_until, evaluator_lease_until, next_eval_at, audio_service_running, private_output_route, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, 0, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(id) DO UPDATE SET name=excluded.name, platform=excluded.platform, priority=excluded.priority, updated_at=excluded.updated_at",
        );
        stmt.bind(&[
            JsValue::from_str(&body.id),
            JsValue::from_str(&body.name),
            JsValue::from_str(&body.platform.to_string()),
            JsValue::from_f64(priority as f64),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        self.get_device(&body.id).await
    }

    async fn get_device(&self, id: &str) -> StorageResult<DeviceRow> {
        let stmt = self.db.prepare(
            "SELECT id, name, platform, priority, evaluator_heartbeat_until, evaluator_lease_until, next_eval_at, audio_service_running, private_output_route, created_at, updated_at FROM devices WHERE id = ?1",
        );
        let stmt = stmt.bind(&[JsValue::from_str(id)]).map_err(d1_err)?;
        let rows: Vec<DeviceRow> = d1_all(&stmt).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| not_found(format!("device {id} not found")))
    }

    async fn list_devices(&self) -> StorageResult<Vec<DeviceRow>> {
        let stmt = self.db.prepare(
            "SELECT id, name, platform, priority, evaluator_heartbeat_until, evaluator_lease_until, next_eval_at, audio_service_running, private_output_route, created_at, updated_at FROM devices ORDER BY priority, created_at",
        );
        d1_all(&stmt).await
    }

    async fn update_device(&self, id: &str, body: &UpdateDevice) -> StorageResult<DeviceRow> {
        let existing = self.get_device(id).await?;
        let name = body.name.clone().unwrap_or(existing.name);
        let priority = body.priority.unwrap_or(existing.priority);
        let audio_service_running = body
            .audio_service_running
            .unwrap_or(existing.audio_service_running);
        let private_output_route = body
            .private_output_route
            .unwrap_or(existing.private_output_route);
        let stmt = self.db.prepare(
            "UPDATE devices SET name = ?1, priority = ?2, audio_service_running = ?3, private_output_route = ?4, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?5",
        );
        stmt.bind(&[
            JsValue::from_str(&name),
            JsValue::from_f64(priority as f64),
            JsValue::from_bool(audio_service_running),
            JsValue::from_bool(private_output_route),
            JsValue::from_str(id),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        self.get_device(id).await
    }

    async fn delete_device(&self, id: &str) -> StorageResult<()> {
        // Verify the device exists before deleting; D1's run() does not
        // reliably report affected rows in all driver versions.
        let _ = self.get_device(id).await?;
        let stmt = self.db.prepare("DELETE FROM devices WHERE id = ?1");
        stmt.bind(&[JsValue::from_str(id)])
            .map_err(d1_err)?
            .run()
            .await
            .map_err(d1_err)?;
        Ok(())
    }

    async fn refresh_evaluator_heartbeat(
        &self,
        device_id: &str,
        until: Timestamp,
    ) -> StorageResult<DeviceRow> {
        let stmt = self.db.prepare(
            "UPDATE devices SET evaluator_heartbeat_until = ?1, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?2",
        );
        stmt.bind(&[
            JsValue::from_str(&until.to_string()),
            JsValue::from_str(device_id),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        self.get_device(device_id).await
    }

    async fn refresh_evaluator_lease(
        &self,
        device_id: &str,
        lease_until: Timestamp,
        next_eval_at: Option<Timestamp>,
    ) -> StorageResult<DeviceRow> {
        let stmt = self.db.prepare(
            "UPDATE devices SET evaluator_lease_until = ?1, next_eval_at = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?3",
        );
        stmt.bind(&[
            JsValue::from_str(&lease_until.to_string()),
            next_eval_at
                .map(|t| JsValue::from_str(&t.to_string()))
                .unwrap_or(JsValue::NULL),
            JsValue::from_str(device_id),
        ])
        .map_err(d1_err)?
        .run()
        .await
        .map_err(d1_err)?;
        self.get_device(device_id).await
    }

    async fn resolve_resident_authority(
        &self,
        candidate_id: &str,
    ) -> StorageResult<ResidentAuthority> {
        let devices = self.list_devices().await?;
        if devices.is_empty() {
            return Ok(ResidentAuthority {
                device_id: None,
                is_resident: false,
                next_eval_at: None,
            });
        }
        let settings = self.get_settings().await?;
        let priority_list: Vec<String> = settings.device_priority.as_inner().clone();
        let now = Timestamp::now();
        Ok(takusu_contracts::resolve_resident_authority_from_rows(
            &devices,
            &priority_list,
            candidate_id,
            now,
        ))
    }

    // ── Health ──────────────────────────────────────────────────────────

    async fn health_check(&self) -> StorageResult<String> {
        let stmt = self.db.prepare("SELECT COUNT(*) AS c FROM tasks");
        let row: Option<CountRow> = stmt.first_t().await?;
        let count = row.map(|r| r.c).unwrap_or(0);
        Ok(format!("d1 ok ({count} tasks)"))
    }
}
