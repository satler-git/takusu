use sha2::{Digest, Sha256};
use takusu_types::{TokenClaims, jwt};

use crate::token_cache::{TokenCache, TokenState};

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    jwt::hex(&result)
}

pub async fn verify_token_with_cache(
    token: &str,
    storage: &dyn takusu_contracts::Storage,
    token_cache: &TokenCache,
) -> Result<Option<TokenClaims>, takusu_contracts::StorageError> {
    match token_cache.get(token) {
        Some(TokenState::Valid(claims)) => return Ok(Some(claims)),
        Some(TokenState::Invalid) => return Ok(None),
        None => {}
    }

    let claims = storage.verify_token(token).await?;

    if let Some(ref claims) = claims {
        token_cache.put(token, TokenState::Valid(claims.clone()));
    } else {
        token_cache.put(token, TokenState::Invalid);
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use takusu_contracts::Storage;
    use takusu_contracts::{
        CreateHabit, CreateHabitScheduledSpan, CreateTask, GoogleCalEventRow, GoogleCalSettingsRow,
        HabitRow, HabitScheduledSpanRow, SaveScheduleRequest, ScheduleRow, SettingsRow, TaskQuery,
        TaskRow, TokenCreateResponse, TokenRow, UpdateGoogleCalSettings, UpdateHabit,
        UpdateSettings, UpdateTask,
    };
    use takusu_types::{DEFAULT_AUD, DEFAULT_ISS};

    // hash_token should be deterministic and 64 hex chars (SHA-256).
    #[test]
    fn hash_token_is_deterministic_64_hex() {
        let h1 = hash_token("tsk_abc");
        let h2 = hash_token("tsk_abc");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_token_different_inputs_differ() {
        assert_ne!(hash_token("a"), hash_token("b"));
        // Empty string still hashes (no panic).
        assert_eq!(hash_token("").len(), 64);
    }

    // Minimal mock storage that only implements verify_token; every other
    // method returns Internal. Counts verify_token calls.
    struct MockStorage {
        valid_tokens: std::collections::HashMap<String, TokenClaims>,
        call_count: Arc<AtomicUsize>,
    }

    impl MockStorage {
        fn new(valid: &[(String, TokenClaims)]) -> (Self, Arc<AtomicUsize>) {
            let count = Arc::new(AtomicUsize::new(0));
            let s = Self {
                valid_tokens: valid.iter().cloned().collect(),
                call_count: count.clone(),
            };
            (s, count)
        }
    }

    fn unimplemented<T>() -> Result<T, takusu_contracts::StorageError> {
        Err(takusu_contracts::StorageError::Internal(
            "mock: not implemented".into(),
        ))
    }

    fn dummy_claims(jti: &str) -> TokenClaims {
        TokenClaims {
            sub: jti.into(),
            jti: jti.into(),
            scope: "read-write".into(),
            label: None,
            aud: DEFAULT_AUD.into(),
            iss: DEFAULT_ISS.into(),
            iat: 0,
            exp: None,
        }
    }

    #[async_trait::async_trait]
    impl Storage for MockStorage {
        async fn verify_token(
            &self,
            token: &str,
        ) -> Result<Option<TokenClaims>, takusu_contracts::StorageError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.valid_tokens.get(token).cloned())
        }
        async fn list_tasks(
            &self,
            _: &TaskQuery,
        ) -> Result<Vec<TaskRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn task_exists_by_ical_uid(
            &self,
            _: &str,
        ) -> Result<bool, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_task(&self, _: &str) -> Result<TaskRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn create_task(
            &self,
            _: &CreateTask,
        ) -> Result<TaskRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn update_task(
            &self,
            _: &str,
            _: &UpdateTask,
        ) -> Result<TaskRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn replace_task(
            &self,
            _: &str,
            _: &CreateTask,
        ) -> Result<TaskRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn delete_task(&self, _: &str) -> Result<(), takusu_contracts::StorageError> {
            unimplemented!()
        }
        async fn list_comments(
            &self,
            _: &str,
        ) -> Result<Vec<takusu_contracts::CommentRow>, takusu_contracts::StorageError> {
            unimplemented!()
        }
        async fn create_comment(
            &self,
            _: &str,
            _: takusu_types::CommentAuthor,
            _: &str,
            _: Option<&str>,
        ) -> Result<takusu_contracts::CommentRow, takusu_contracts::StorageError> {
            unimplemented!()
        }
        async fn delete_comment(&self, _: &str) -> Result<(), takusu_contracts::StorageError> {
            unimplemented!()
        }
        async fn list_habits(&self) -> Result<Vec<HabitRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_habit(&self, _: &str) -> Result<HabitRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn create_habit(
            &self,
            _: &CreateHabit,
        ) -> Result<HabitRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn update_habit(
            &self,
            _: &str,
            _: &UpdateHabit,
        ) -> Result<HabitRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn replace_habit(
            &self,
            _: &str,
            _: &CreateHabit,
        ) -> Result<HabitRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn delete_habit(&self, _: &str) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_habit_scheduled_spans(
            &self,
            _: &str,
        ) -> Result<Vec<HabitScheduledSpanRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_all_habit_scheduled_spans(
            &self,
        ) -> Result<Vec<HabitScheduledSpanRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn create_habit_scheduled_span(
            &self,
            _: &str,
            _: &CreateHabitScheduledSpan,
        ) -> Result<HabitScheduledSpanRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn delete_habit_scheduled_span(
            &self,
            _: &str,
            _: &str,
        ) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_habit_steps(
            &self,
            _: &str,
        ) -> Result<Vec<takusu_contracts::HabitStepRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_all_habit_steps(
            &self,
        ) -> Result<Vec<takusu_contracts::HabitStepRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn replace_habit_steps(
            &self,
            _: &str,
            _: &[takusu_contracts::HabitStepInput],
        ) -> Result<Vec<takusu_contracts::HabitStepRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_schedule(
            &self,
        ) -> Result<Option<ScheduleRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn save_schedule(
            &self,
            _: &SaveScheduleRequest,
        ) -> Result<ScheduleRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn clear_schedule(&self) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn create_token(
            &self,
            _: Option<&str>,
        ) -> Result<TokenCreateResponse, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_tokens(&self) -> Result<Vec<TokenRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn revoke_token(&self, _: i64) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_settings(&self) -> Result<SettingsRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn update_settings(
            &self,
            _: &UpdateSettings,
        ) -> Result<SettingsRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_gcal_settings(
            &self,
        ) -> Result<GoogleCalSettingsRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn update_gcal_settings(
            &self,
            _: &UpdateGoogleCalSettings,
        ) -> Result<GoogleCalSettingsRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_gcal_mappings(
            &self,
        ) -> Result<Vec<GoogleCalEventRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn upsert_gcal_mappings(
            &self,
            _: &[(String, String)],
        ) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn delete_gcal_mappings(
            &self,
            _: &[String],
        ) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn clear_gcal_mappings(&self) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn health_check(&self) -> Result<String, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_skills(
            &self,
        ) -> Result<Vec<takusu_contracts::SkillRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_skill(
            &self,
            _: &str,
        ) -> Result<takusu_contracts::SkillRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn create_skill(
            &self,
            _: &takusu_contracts::CreateSkill,
        ) -> Result<takusu_contracts::SkillRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn update_skill(
            &self,
            _: &str,
            _: &takusu_contracts::UpdateSkill,
        ) -> Result<takusu_contracts::SkillRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn delete_skill(&self, _: &str) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_memory(
            &self,
            _: &str,
        ) -> Result<takusu_contracts::MemoryRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn create_memory(
            &self,
            _: &takusu_contracts::CreateMemory,
            _: Option<&str>,
        ) -> Result<takusu_contracts::MemoryRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn update_memory(
            &self,
            _: &str,
            _: &takusu_contracts::UpdateMemory,
            _: Option<&str>,
        ) -> Result<takusu_contracts::MemoryRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn delete_memory(
            &self,
            _: &str,
            _: i64,
            _: Option<&str>,
        ) -> Result<(), takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn search_memories(
            &self,
            _: &takusu_contracts::MemoryQuery,
        ) -> Result<Vec<takusu_contracts::MemoryRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn injectable_memories(
            &self,
            _: &takusu_contracts::MemoryInjectionQuery,
        ) -> Result<takusu_contracts::MemoryInjectionResult, takusu_contracts::StorageError>
        {
            unimplemented()
        }
        async fn find_similar_tasks(
            &self,
            _: &takusu_contracts::SimilarTaskQuery,
        ) -> Result<Vec<takusu_contracts::SimilarTaskRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn start_work_session(
            &self,
            _: &takusu_contracts::StartWorkSession,
            _: Option<&str>,
        ) -> Result<takusu_contracts::WorkSessionRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn pause_work_session(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> Result<takusu_contracts::WorkSessionRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn complete_work_session(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> Result<takusu_contracts::WorkSessionRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn record_work_session_progress(
            &self,
            _: &str,
            _: &takusu_contracts::RecordWorkSessionProgress,
            _: Option<&str>,
        ) -> Result<takusu_contracts::WorkSessionProgressResult, takusu_contracts::StorageError>
        {
            unimplemented()
        }
        async fn get_work_session(
            &self,
            _: &str,
        ) -> Result<takusu_contracts::WorkSessionRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn list_work_sessions(
            &self,
            _: Option<&str>,
        ) -> Result<Vec<takusu_contracts::WorkSessionRow>, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn attach_work_session(
            &self,
            _: &str,
            _: &takusu_contracts::AttachWorkSession,
            _: Option<&str>,
        ) -> Result<takusu_contracts::WorkSessionRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn convert_work_session(
            &self,
            _: &str,
            _: &takusu_contracts::ConvertWorkSession,
            _: Option<&str>,
        ) -> Result<takusu_contracts::TaskRow, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn get_task_progress(
            &self,
            _: &str,
        ) -> Result<takusu_contracts::TaskProgress, takusu_contracts::StorageError> {
            unimplemented()
        }
        async fn split_task(
            &self,
            _: &str,
            _: &takusu_contracts::SplitTask,
            _: Option<&str>,
        ) -> Result<takusu_contracts::SplitResult, takusu_contracts::StorageError> {
            unimplemented()
        }
    }

    #[tokio::test]
    async fn verify_token_with_cache_caches_valid_answer() {
        let token = "real-token".to_string();
        let (storage, calls) = MockStorage::new(&[(token.clone(), dummy_claims("real-token"))]);
        let cache = TokenCache::new(std::time::Duration::from_secs(60));
        let ok1 = verify_token_with_cache(&token, &storage, &cache)
            .await
            .unwrap();
        assert!(ok1.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Second call should hit the cache, not storage.
        let ok2 = verify_token_with_cache(&token, &storage, &cache)
            .await
            .unwrap();
        assert!(ok2.is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn verify_token_with_cache_caches_invalid_answer() {
        let (storage, calls) = MockStorage::new(&[]);
        let cache = TokenCache::new(std::time::Duration::from_secs(60));
        let ok1 = verify_token_with_cache("bogus", &storage, &cache)
            .await
            .unwrap();
        assert!(ok1.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Cached Invalid → no second storage hit.
        let ok2 = verify_token_with_cache("bogus", &storage, &cache)
            .await
            .unwrap();
        assert!(ok2.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
