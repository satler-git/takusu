use std::sync::Arc;

use takusu_contracts::{
    CommentRow, CreateComment, CreateMemory, CreateSkill, MemoryInjectionQuery,
    MemoryInjectionResult, MemoryQuery, MemoryRow, SettingsRow, SimilarTaskQuery, SimilarTaskRow,
    SkillRow, Storage, TokenCreateResponse, TokenRow, UpdateMemory, UpdateSettings, UpdateSkill,
};
use takusu_types::CommentAuthor;

use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind, ConflictKind, SkillOp};
use crate::token_cache::TokenCache;
use crate::validate::{Validate, parse_settings_timezone};

mod dependency;
mod devices;
mod events;
mod gcal;
mod habit;
mod habit_sync;
mod schedule;
mod task;
mod work_session;

pub use dependency::{DependencyAnalysisResponse, DependencyNode, RedundantDependency};
pub use gcal::{
    DeleteAllGcalFailure, DeleteAllGcalResult, GoogleCalSettingsOutput, OAuthCallbackResponse,
    OAuthUrlResponse, SyncTriggerResponse,
};
pub use task::IcalImportResult;

fn default_settings_row() -> SettingsRow {
    SettingsRow {
        id: "active".to_string(),
        tz: "UTC".to_string(),
        sleep_start: takusu_types::TimeOfDay::new(22, 0).unwrap(),
        sleep_end: takusu_types::TimeOfDay::new(6, 0).unwrap(),
        comfortable_minutes: None,
        maximum_minutes: None,
        solver: takusu_types::Solver::Sa,
        time_budget_ms: None,
        seed: None,
        warm_start: false,
        plan_length_days: 14,
        device_priority: takusu_types::JsonString::new(vec![
            "desktop".to_string(),
            "android".to_string(),
        ]),
        created_at: takusu_types::Timestamp::default(),
        updated_at: takusu_types::Timestamp::default(),
    }
}

pub struct TakusuApp {
    pub storage: Arc<dyn Storage>,
    pub token_cache: Arc<TokenCache>,
    timezone_cache: tokio::sync::Mutex<Option<jiff::tz::TimeZone>>,
}

impl TakusuApp {
    pub fn new(storage: Arc<dyn Storage>, token_cache: Arc<TokenCache>) -> Self {
        Self {
            storage,
            token_cache,
            timezone_cache: tokio::sync::Mutex::new(None),
        }
    }

    pub async fn update_workers_credentials(&self, url: &str, token: &str) -> Result<(), AppError> {
        self.storage
            .update_workers_credentials(url, token)
            .await
            .map_err(storage_to_app)
    }

    // ── Settings ──────────────────────────────────────────

    pub(super) async fn get_settings_or_default(&self) -> Result<SettingsRow, AppError> {
        self.storage
            .get_settings()
            .await
            .map_err(storage_to_app)
            .or_else(|e| {
                if matches!(e, AppError::NotFound(_)) {
                    Ok(default_settings_row())
                } else {
                    Err(e)
                }
            })
    }

    /// Return the server's configured timezone, falling back to UTC when
    /// settings have not been created yet. The result is cached for the
    /// lifetime of the `TakusuApp` and invalidated by `update_settings`.
    pub async fn server_timezone(&self) -> Result<jiff::tz::TimeZone, AppError> {
        let mut cache = self.timezone_cache.lock().await;
        if let Some(ref tz) = *cache {
            return Ok(tz.clone());
        }
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        *cache = Some(tz.clone());
        Ok(tz)
    }

    pub async fn get_settings(&self) -> Result<SettingsRow, AppError> {
        self.storage.get_settings().await.map_err(storage_to_app)
    }

    pub async fn update_settings(&self, body: &UpdateSettings) -> Result<SettingsRow, AppError> {
        body.validate()?;
        let row = self
            .storage
            .update_settings(body)
            .await
            .map_err(storage_to_app)?;
        // Invalidate the cached timezone since settings may have changed.
        if body.tz.is_some() {
            *self.timezone_cache.lock().await = None;
        }
        Ok(row)
    }

    // ── Skills ────────────────────────────────────────────

    pub async fn create_skill(&self, body: &CreateSkill) -> Result<SkillRow, AppError> {
        body.validate()?;
        if let Ok(existing) = self.storage.get_skill(&body.slug).await {
            if existing.built_in {
                return Err(AppError::Conflict(ConflictKind::BuiltInSkill {
                    slug: body.slug.clone(),
                    op: SkillOp::Overwrite,
                }));
            }
            return Err(AppError::Conflict(ConflictKind::AlreadyExists(
                body.slug.clone(),
            )));
        }
        self.storage
            .create_skill(body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn list_skills(&self) -> Result<Vec<SkillRow>, AppError> {
        self.storage.list_skills().await.map_err(storage_to_app)
    }

    pub async fn get_skill(&self, slug: &str) -> Result<SkillRow, AppError> {
        self.storage.get_skill(slug).await.map_err(storage_to_app)
    }

    pub async fn update_skill(&self, slug: &str, body: &UpdateSkill) -> Result<SkillRow, AppError> {
        let existing = self.storage.get_skill(slug).await.map_err(storage_to_app)?;
        if existing.built_in {
            return Err(AppError::Conflict(ConflictKind::BuiltInSkill {
                slug: slug.to_string(),
                op: SkillOp::Edit,
            }));
        }
        if body
            .name
            .as_ref()
            .is_some_and(|n| n.is_empty() || n.len() > 100)
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "name must be 1..100 characters".into(),
            )));
        }
        if body.description.as_ref().is_some_and(|d| d.len() > 500) {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "description must be at most 500 characters".into(),
            )));
        }
        if body
            .body
            .as_ref()
            .is_some_and(|b| b.is_empty() || b.len() > 64 * 1024)
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "body length is invalid".into(),
            )));
        }
        self.storage
            .update_skill(slug, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_skill(&self, slug: &str) -> Result<(), AppError> {
        let existing = self.storage.get_skill(slug).await.map_err(storage_to_app)?;
        if existing.built_in {
            return Err(AppError::Conflict(ConflictKind::BuiltInSkill {
                slug: slug.to_string(),
                op: SkillOp::Delete,
            }));
        }
        self.storage
            .delete_skill(slug)
            .await
            .map_err(storage_to_app)
    }

    // ── Memory (#WI-7) ────────────────────────────────────

    pub async fn create_memory(
        &self,
        body: &CreateMemory,
        operation_id: Option<&str>,
    ) -> Result<MemoryRow, AppError> {
        body.validate()?;
        self.storage
            .create_memory(body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_memory(&self, id: &str) -> Result<MemoryRow, AppError> {
        self.storage.get_memory(id).await.map_err(storage_to_app)
    }

    pub async fn update_memory(
        &self,
        id: &str,
        body: &UpdateMemory,
        operation_id: Option<&str>,
    ) -> Result<MemoryRow, AppError> {
        if body.content.as_ref().is_none_or(|c| c.is_empty()) {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "content is required".into(),
            )));
        }
        if takusu_search::memory::normalize_content(body.content.as_deref().unwrap_or("")).is_err()
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "invalid content".into(),
            )));
        }
        self.storage
            .update_memory(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_memory(
        &self,
        id: &str,
        observed_revision: i64,
        operation_id: Option<&str>,
    ) -> Result<(), AppError> {
        self.storage
            .delete_memory(id, observed_revision, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn search_memories(&self, query: &MemoryQuery) -> Result<Vec<MemoryRow>, AppError> {
        if takusu_search::memory::normalize_query(&query.q).is_err() {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "invalid query".into(),
            )));
        }
        self.storage
            .search_memories(query)
            .await
            .map_err(storage_to_app)
    }

    pub async fn injectable_memories(
        &self,
        query: &MemoryInjectionQuery,
    ) -> Result<MemoryInjectionResult, AppError> {
        if takusu_search::memory::normalize_text(
            &query.text,
            Some(takusu_search::memory::MAX_INJECTION_UTTERANCE_SCALARS),
        )
        .is_err()
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "invalid text".into(),
            )));
        }
        self.storage
            .injectable_memories(query)
            .await
            .map_err(storage_to_app)
    }

    pub async fn find_similar_tasks(
        &self,
        query: &SimilarTaskQuery,
    ) -> Result<Vec<SimilarTaskRow>, AppError> {
        if takusu_search::memory::normalize_text(
            &query.title,
            Some(takusu_search::memory::MAX_QUERY_SCALARS),
        )
        .is_err()
        {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "invalid title".into(),
            )));
        }
        self.storage
            .find_similar_tasks(query)
            .await
            .map_err(storage_to_app)
    }

    // ── Task comments (WI-1) ──────────────────────────────

    pub async fn list_comments(&self, task_id: &str) -> Result<Vec<CommentRow>, AppError> {
        self.storage
            .list_comments(task_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn create_comment(
        &self,
        task_id: &str,
        author: CommentAuthor,
        body: &CreateComment,
        operation_id: Option<&str>,
    ) -> Result<CommentRow, AppError> {
        body.validate()?;
        self.storage
            .create_comment(task_id, author, &body.content, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_comment(&self, id: &str) -> Result<(), AppError> {
        self.storage
            .delete_comment(id)
            .await
            .map_err(storage_to_app)
    }

    // ── Tokens ────────────────────────────────────────────

    pub async fn create_token(&self, label: Option<&str>) -> Result<TokenCreateResponse, AppError> {
        let resp = self
            .storage
            .create_token(label)
            .await
            .map_err(storage_to_app)?;
        self.token_cache.invalidate();
        Ok(resp)
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenRow>, AppError> {
        self.storage.list_tokens().await.map_err(storage_to_app)
    }

    pub async fn revoke_token(&self, id: i64) -> Result<(), AppError> {
        self.storage
            .revoke_token(id)
            .await
            .map_err(storage_to_app)?;
        self.token_cache.invalidate();
        Ok(())
    }

    // ── Health ────────────────────────────────────────────

    /// Backend health check. Returns a short status string from the storage
    /// backend (e.g. "worker ok" or "sqlite ok (v3.x)").
    pub async fn health_check(&self) -> Result<String, AppError> {
        self.storage.health_check().await.map_err(storage_to_app)
    }
}

#[cfg(test)]
mod tests {
    use crate::validate::{validate_minutes, validate_task_datetimes};
    use takusu_contracts::SleepInput;
    use takusu_types::parse_timezone;

    // ── parse_timezone accepts IANA and fixed-offset timezones (#607) ────

    #[test]
    fn parse_timezone_accepts_iana_and_fixed_offset() {
        assert!(parse_timezone("Asia/Tokyo").is_ok());
        assert!(parse_timezone("UTC").is_ok());
        assert!(parse_timezone("+09:00").is_ok());
        assert!(parse_timezone("-05:30").is_ok());
        assert!(parse_timezone("+0900").is_ok());
        assert!(parse_timezone("+09").is_ok());
        assert!(parse_timezone("not/a/tz").is_err());
    }

    #[test]
    fn parse_timezone_rejects_excessive_offset() {
        // UTC±14 is the widest real-world offset.
        assert!(parse_timezone("+14:00:00").is_ok());
        assert!(parse_timezone("-14:00:00").is_ok());
        assert!(parse_timezone("+14:00:01").is_err());
        assert!(parse_timezone("+24:00:00").is_err());
        assert!(parse_timezone("+25:59:59").is_err());
        assert!(parse_timezone("+26:00:00").is_err());
    }

    // ── validate_minutes bounds (#604) ────────────────────────────────

    #[test]
    fn minutes_reject_negative_avg() {
        assert!(validate_minutes(-1, None).is_err());
        assert!(validate_minutes(0, None).is_ok());
    }

    #[test]
    fn minutes_reject_negative_sigma() {
        assert!(validate_minutes(10, Some(-1)).is_err());
        assert!(validate_minutes(10, Some(0)).is_ok());
    }

    #[test]
    fn minutes_reject_excessive_avg() {
        let max_minutes = 60 * 24 * 365;
        assert!(validate_minutes(max_minutes, None).is_ok());
        assert!(validate_minutes(max_minutes + 1, None).is_err());
    }

    #[test]
    fn minutes_reject_excessive_sigma() {
        let max_minutes = 60 * 24 * 365;
        assert!(validate_minutes(10, Some(max_minutes)).is_ok());
        assert!(validate_minutes(10, Some(max_minutes + 1)).is_err());
    }

    // Regression (#780): SleepInput parsing must reject invalid HH:MM strings.
    // TimeOfDay validates ranges, so custom sleep strings like "22:70-06:00"
    // are rejected at the boundary rather than silently accepted.
    #[test]
    fn regression_sleep_input_rejects_invalid_hhmm() {
        // Minutes out of range and hours out of range should both error.
        assert!(
            "22:70-06:00".parse::<SleepInput>().is_err(),
            "custom sleep with invalid minutes should be rejected"
        );
        assert!(
            "22:00-25:00".parse::<SleepInput>().is_err(),
            "custom sleep with invalid hours should be rejected"
        );
        assert!(
            "22:00-06:00".parse::<SleepInput>().is_ok(),
            "valid custom sleep should still be accepted"
        );
    }

    // ── validate_task_datetimes (#934) ─────────────────────────────────

    #[test]
    fn validate_task_datetimes_accepts_valid_range() {
        let s: takusu_types::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        let e: takusu_types::Timestamp = "2026-07-22T12:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(Some(Some(&s)), Some(&e), None, None).is_ok());
    }

    #[test]
    fn validate_task_datetimes_rejects_reversed() {
        let s: takusu_types::Timestamp = "2026-07-22T12:00:00Z".parse().unwrap();
        let e: takusu_types::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(Some(Some(&s)), Some(&e), None, None).is_err());
    }

    #[test]
    fn validate_task_datetimes_fills_existing_for_partial_update() {
        let e: takusu_types::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        let existing: takusu_types::Timestamp = "2026-07-22T08:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e), Some(&existing), None).is_ok());
        let e2: takusu_types::Timestamp = "2026-07-22T07:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e2), Some(&existing), None).is_err());
    }

    #[test]
    fn validate_task_datetimes_rejects_invalid_existing() {
        // With typed Timestamp, invalid strings cannot be constructed.
        // This test now verifies that a None existing value with a Some end
        // still works (no existing to compare against).
        let e: takusu_types::Timestamp = "2026-07-22T10:00:00Z".parse().unwrap();
        assert!(validate_task_datetimes(None, Some(&e), None, None).is_ok());
    }
}
