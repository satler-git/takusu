//! Phase 4 hardening tests:
//! - Token verify caching: verify_token is called once, then cached.
//! - Cache invalidation on token create/revoke.
//! - Retry/backoff: transient 503 is retried.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::Method;
use axum::http::Request;
use axum::http::StatusCode;
use axum::routing::get;
use http_body_util::BodyExt;
use serde_json::json;
use takusu_contracts::{
    CommentRow, CreateHabit, CreateHabitScheduledSpan, CreateTask, GoogleCalEventRow,
    GoogleCalSettingsRow, HabitRow, HabitScheduledSpanRow, HabitStepInput, HabitStepRow,
    SaveScheduleRequest, ScheduleRow, SettingsRow, Storage, StorageError, TaskQuery, TaskRow,
    TokenCreateResponse, TokenRow, UpdateGoogleCalSettings, UpdateHabit, UpdateSettings,
    UpdateTask, storage::StorageResult,
};
use takusu_local::router::router;
use takusu_local::state::{AppState, build_agent_state};
use takusu_local_lib::TokenClaims;
use takusu_local_lib::app::TakusuApp;
use takusu_local_lib::generate_root_jwt;
use takusu_local_lib::storage_workers::WorkersStorage;
use takusu_local_lib::token_cache::TokenCache;
use takusu_types::CommentAuthor;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tower::ServiceExt;

const JWT_SECRET: &str = "test-secret-do-not-use-in-production";
static ROOT_TOKEN: LazyLock<String> = LazyLock::new(|| {
    generate_root_jwt(JWT_SECRET, None).expect("root token generation should not fail")
});

#[derive(Default)]
struct Counters {
    verify: AtomicUsize,
    issue: AtomicUsize,
    revoke: AtomicUsize,
}

impl Counters {
    fn verify_get(&self) -> usize {
        self.verify.load(Ordering::SeqCst)
    }
}

fn make_state(storage: Arc<dyn Storage>) -> AppState {
    let token_cache = Arc::new(TokenCache::with_default_ttl());
    let app = Arc::new(TakusuApp::new(storage, token_cache));
    let agent = build_agent_state(ROOT_TOKEN.as_str(), "");
    AppState::new(app, Arc::new(RwLock::new(Arc::from(ROOT_TOKEN.as_str()))), agent)
}

fn counting_storage(counters: Arc<Counters>) -> Arc<dyn Storage> {
    Arc::new(CountingStorage {
        counters,
        jwt_secret: JWT_SECRET.into(),
        verify_error: false,
    })
}

fn counting_storage_with_verify_error(counters: Arc<Counters>) -> Arc<dyn Storage> {
    Arc::new(CountingStorage {
        counters,
        jwt_secret: JWT_SECRET.into(),
        verify_error: true,
    })
}

fn req(method: Method, uri: &str, token: Option<&str>) -> Request<axum::body::Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(axum::body::Body::empty()).unwrap()
}

async fn body_str(body: axum::body::Body) -> String {
    let bytes = body.collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

struct CountingStorage {
    counters: Arc<Counters>,
    jwt_secret: String,
    verify_error: bool,
}

#[async_trait]
impl Storage for CountingStorage {
    async fn verify_token(&self, t: &str) -> StorageResult<Option<TokenClaims>> {
        if self.verify_error {
            self.counters.verify.fetch_add(1, Ordering::SeqCst);
            return Err(StorageError::Internal("mock worker unreachable".into()));
        }
        match takusu_local_lib::jwt::verify(&self.jwt_secret, t, takusu_local_lib::DEFAULT_AUD) {
            Ok(claims) if claims.is_root() => Ok(Some(claims)),
            Ok(claims) => {
                self.counters.verify.fetch_add(1, Ordering::SeqCst);
                Ok(Some(claims))
            }
            Err(_) => {
                self.counters.verify.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
        }
    }
    async fn list_tasks(&self, _q: &TaskQuery) -> StorageResult<Vec<TaskRow>> {
        Ok(vec![])
    }
    async fn task_exists_by_ical_uid(&self, _uid: &str) -> StorageResult<bool> {
        Ok(false)
    }
    async fn get_task(&self, _id: &str) -> StorageResult<TaskRow> {
        Err(StorageError::NotFound("n/a".into()))
    }
    async fn create_task(&self, _b: &CreateTask) -> StorageResult<TaskRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn update_task(&self, _id: &str, _b: &UpdateTask) -> StorageResult<TaskRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn replace_task(&self, _id: &str, _b: &CreateTask) -> StorageResult<TaskRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn delete_task(&self, _id: &str) -> StorageResult<()> {
        Ok(())
    }
    async fn list_comments(&self, _task_id: &str) -> StorageResult<Vec<CommentRow>> {
        Ok(vec![])
    }
    async fn create_comment(
        &self,
        _task_id: &str,
        _author: CommentAuthor,
        _content: &str,
        _operation_id: Option<&str>,
    ) -> StorageResult<CommentRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn delete_comment(&self, _id: &str) -> StorageResult<()> {
        Ok(())
    }
    async fn list_habits(&self) -> StorageResult<Vec<HabitRow>> {
        Ok(vec![])
    }
    async fn get_habit(&self, _id: &str) -> StorageResult<HabitRow> {
        Err(StorageError::NotFound("n/a".into()))
    }
    async fn create_habit(&self, _b: &CreateHabit) -> StorageResult<HabitRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn update_habit(&self, _id: &str, _b: &UpdateHabit) -> StorageResult<HabitRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn replace_habit(&self, _id: &str, _b: &CreateHabit) -> StorageResult<HabitRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn delete_habit(&self, _id: &str) -> StorageResult<()> {
        Ok(())
    }
    async fn list_habit_scheduled_spans(
        &self,
        _id: &str,
    ) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        Ok(vec![])
    }
    async fn list_all_habit_scheduled_spans(&self) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        Ok(vec![])
    }
    async fn create_habit_scheduled_span(
        &self,
        _id: &str,
        _b: &CreateHabitScheduledSpan,
    ) -> StorageResult<HabitScheduledSpanRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn delete_habit_scheduled_span(&self, _id: &str, _span_id: &str) -> StorageResult<()> {
        Ok(())
    }
    async fn list_habit_steps(&self, _id: &str) -> StorageResult<Vec<HabitStepRow>> {
        Ok(vec![])
    }
    async fn list_all_habit_steps(&self) -> StorageResult<Vec<HabitStepRow>> {
        Ok(vec![])
    }
    async fn replace_habit_steps(
        &self,
        _id: &str,
        _steps: &[HabitStepInput],
    ) -> StorageResult<Vec<HabitStepRow>> {
        Ok(vec![])
    }
    async fn get_schedule(&self) -> StorageResult<Option<ScheduleRow>> {
        Ok(None)
    }
    async fn save_schedule(&self, _r: &SaveScheduleRequest) -> StorageResult<ScheduleRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn clear_schedule(&self) -> StorageResult<()> {
        Ok(())
    }
    async fn create_token(&self, _l: Option<&str>) -> StorageResult<TokenCreateResponse> {
        self.counters.issue.fetch_add(1, Ordering::SeqCst);
        Ok(TokenCreateResponse {
            id: 1,
            token: "tsk_new".into(),
            scope: takusu_types::TokenScope::ReadWrite,
            label: None,
            created_at: "2026-06-22T00:00:00Z".parse().unwrap(),
            expires_at: None,
        })
    }
    async fn list_tokens(&self) -> StorageResult<Vec<TokenRow>> {
        Ok(vec![])
    }
    async fn revoke_token(&self, _id: i64) -> StorageResult<()> {
        self.counters.revoke.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn get_settings(&self) -> StorageResult<SettingsRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn update_settings(&self, _b: &UpdateSettings) -> StorageResult<SettingsRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn get_gcal_settings(&self) -> StorageResult<GoogleCalSettingsRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn update_gcal_settings(
        &self,
        _b: &UpdateGoogleCalSettings,
    ) -> StorageResult<GoogleCalSettingsRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn list_gcal_mappings(&self) -> StorageResult<Vec<GoogleCalEventRow>> {
        Ok(vec![])
    }
    async fn upsert_gcal_mappings(&self, _m: &[(String, String)]) -> StorageResult<()> {
        Ok(())
    }
    async fn delete_gcal_mappings(&self, _t: &[String]) -> StorageResult<()> {
        Ok(())
    }
    async fn clear_gcal_mappings(&self) -> StorageResult<()> {
        Ok(())
    }
    async fn health_check(&self) -> StorageResult<String> {
        Ok("mock ok".into())
    }
    async fn list_skills(&self) -> StorageResult<Vec<takusu_contracts::SkillRow>> {
        Ok(vec![])
    }
    async fn get_skill(&self, _id: &str) -> StorageResult<takusu_contracts::SkillRow> {
        Err(StorageError::NotFound("n/a".into()))
    }
    async fn create_skill(
        &self,
        _b: &takusu_contracts::CreateSkill,
    ) -> StorageResult<takusu_contracts::SkillRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn update_skill(
        &self,
        _id: &str,
        _b: &takusu_contracts::UpdateSkill,
    ) -> StorageResult<takusu_contracts::SkillRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn delete_skill(&self, _id: &str) -> StorageResult<()> {
        Ok(())
    }
    async fn get_memory(&self, _id: &str) -> StorageResult<takusu_contracts::MemoryRow> {
        Err(StorageError::NotFound("n/a".into()))
    }
    async fn create_memory(
        &self,
        _b: &takusu_contracts::CreateMemory,
        _op: Option<&str>,
    ) -> StorageResult<takusu_contracts::MemoryRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn update_memory(
        &self,
        _id: &str,
        _b: &takusu_contracts::UpdateMemory,
        _op: Option<&str>,
    ) -> StorageResult<takusu_contracts::MemoryRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn delete_memory(&self, _id: &str, _rev: i64, _op: Option<&str>) -> StorageResult<()> {
        Ok(())
    }
    async fn search_memories(
        &self,
        _q: &takusu_contracts::MemoryQuery,
    ) -> StorageResult<Vec<takusu_contracts::MemoryRow>> {
        Ok(vec![])
    }
    async fn injectable_memories(
        &self,
        _q: &takusu_contracts::MemoryInjectionQuery,
    ) -> StorageResult<takusu_contracts::MemoryInjectionResult> {
        Ok(takusu_contracts::MemoryInjectionResult::default())
    }
    async fn find_similar_tasks(
        &self,
        _q: &takusu_contracts::SimilarTaskQuery,
    ) -> StorageResult<Vec<takusu_contracts::SimilarTaskRow>> {
        Ok(vec![])
    }
    async fn start_work_session(
        &self,
        _body: &takusu_contracts::StartWorkSession,
        _operation_id: Option<&str>,
    ) -> StorageResult<takusu_contracts::WorkSessionRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn pause_work_session(
        &self,
        _id: &str,
        _operation_id: Option<&str>,
    ) -> StorageResult<takusu_contracts::WorkSessionRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn complete_work_session(
        &self,
        _id: &str,
        _operation_id: Option<&str>,
    ) -> StorageResult<takusu_contracts::WorkSessionRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn record_work_session_progress(
        &self,
        _id: &str,
        _body: &takusu_contracts::RecordWorkSessionProgress,
        _operation_id: Option<&str>,
    ) -> StorageResult<takusu_contracts::WorkSessionProgressResult> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn get_work_session(&self, _id: &str) -> StorageResult<takusu_contracts::WorkSessionRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn list_work_sessions(
        &self,
        _task_id: Option<&str>,
    ) -> StorageResult<Vec<takusu_contracts::WorkSessionRow>> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn attach_work_session(
        &self,
        _id: &str,
        _body: &takusu_contracts::AttachWorkSession,
        _operation_id: Option<&str>,
    ) -> StorageResult<takusu_contracts::WorkSessionRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn convert_work_session(
        &self,
        _id: &str,
        _body: &takusu_contracts::ConvertWorkSession,
        _operation_id: Option<&str>,
    ) -> StorageResult<TaskRow> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn get_task_progress(&self, _id: &str) -> StorageResult<takusu_contracts::TaskProgress> {
        Err(StorageError::Internal("n/a".into()))
    }
    async fn split_task(
        &self,
        _id: &str,
        _body: &takusu_contracts::SplitTask,
        _operation_id: Option<&str>,
    ) -> StorageResult<takusu_contracts::SplitResult> {
        Err(StorageError::Internal("n/a".into()))
    }
}

#[tokio::test]
async fn token_cache_hits_on_repeated_requests() {
    let counters = Arc::new(Counters::default());
    let storage: Arc<dyn Storage> = counting_storage(counters.clone());
    let state = make_state(storage);
    let app = router(state);
    let token = "tsk_user_xyz";

    let r1 = app
        .clone()
        .oneshot(req(Method::GET, "/api/tasks", Some(token)))
        .await
        .unwrap();
    let _ = body_str(r1.into_body()).await;
    assert_eq!(counters.verify_get(), 1, "first call should hit storage");

    for _ in 0..5 {
        let r = app
            .clone()
            .oneshot(req(Method::GET, "/api/tasks", Some(token)))
            .await
            .unwrap();
        let _ = body_str(r.into_body()).await;
    }
    assert_eq!(
        counters.verify_get(),
        1,
        "subsequent calls should be cached"
    );
}

#[tokio::test]
async fn token_cache_caches_invalid_responses() {
    let counters = Arc::new(Counters::default());
    let storage: Arc<dyn Storage> = counting_storage(counters.clone());
    let state = make_state(storage);
    let app = router(state);

    let r1 = app
        .clone()
        .oneshot(req(Method::GET, "/api/tasks", Some("tsk_garbage")))
        .await
        .unwrap();
    let _ = body_str(r1.into_body()).await;
    assert_eq!(counters.verify_get(), 1);

    for _ in 0..3 {
        let r = app
            .clone()
            .oneshot(req(Method::GET, "/api/tasks", Some("tsk_garbage")))
            .await
            .unwrap();
        let _ = body_str(r.into_body()).await;
    }
    assert_eq!(
        counters.verify_get(),
        1,
        "invalid token should also be cached"
    );
}

#[tokio::test]
async fn root_token_bypasses_storage_when_unreachable() {
    let counters = Arc::new(Counters::default());
    let storage: Arc<dyn Storage> = counting_storage_with_verify_error(counters.clone());
    let state = make_state(storage);
    let app = router(state.clone());

    let update_req = Request::builder()
        .method(Method::PUT)
        .uri("/api/workers/config")
        .header("authorization", format!("Bearer {}", ROOT_TOKEN.as_str()))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({ "url": "http://new-worker.example", "token": "new-token" }).to_string(),
        ))
        .unwrap();
    let r = app.oneshot(update_req).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        counters.verify_get(),
        0,
        "root token should bypass storage verify"
    );

    let root = state.root_token.read().await;
    assert_eq!(
        &**root, "new-token",
        "root_token should be updated to the new worker token"
    );
}

#[tokio::test]
async fn token_create_invalidates_cache() {
    let counters = Arc::new(Counters::default());
    let storage: Arc<dyn Storage> = counting_storage(counters.clone());
    let state = make_state(storage);
    let app = router(state);
    let token = "tsk_user_a";

    let r1 = app
        .clone()
        .oneshot(req(Method::GET, "/api/tasks", Some(token)))
        .await
        .unwrap();
    let _ = body_str(r1.into_body()).await;
    assert_eq!(counters.verify_get(), 1);

    for _ in 0..3 {
        let r = app
            .clone()
            .oneshot(req(Method::GET, "/api/tasks", Some(token)))
            .await
            .unwrap();
        let _ = body_str(r.into_body()).await;
    }
    assert_eq!(counters.verify_get(), 1, "cached before create");

    let create_req = Request::builder()
        .method(Method::POST)
        .uri("/api/tokens")
        .header("authorization", format!("Bearer {}", ROOT_TOKEN.as_str()))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({ "label": "test" }).to_string(),
        ))
        .unwrap();
    let cr = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(cr.status(), StatusCode::OK);

    let r2 = app
        .clone()
        .oneshot(req(Method::GET, "/api/tasks", Some(token)))
        .await
        .unwrap();
    let _ = body_str(r2.into_body()).await;
    assert_eq!(
        counters.verify_get(),
        2,
        "cache should be invalidated after token create"
    );
}

#[tokio::test]
async fn token_revoke_invalidates_cache() {
    let counters = Arc::new(Counters::default());
    let storage: Arc<dyn Storage> = counting_storage(counters.clone());
    let state = make_state(storage);
    let app = router(state);
    let token = "tsk_user_b";

    let r1 = app
        .clone()
        .oneshot(req(Method::GET, "/api/tasks", Some(token)))
        .await
        .unwrap();
    let _ = body_str(r1.into_body()).await;
    assert_eq!(counters.verify_get(), 1);

    let revoke_req = Request::builder()
        .method(Method::DELETE)
        .uri("/api/tokens/1")
        .header("authorization", format!("Bearer {}", ROOT_TOKEN.as_str()))
        .body(axum::body::Body::empty())
        .unwrap();
    let rr = app.clone().oneshot(revoke_req).await.unwrap();
    assert_eq!(rr.status(), StatusCode::NO_CONTENT);

    let r2 = app
        .clone()
        .oneshot(req(Method::GET, "/api/tasks", Some(token)))
        .await
        .unwrap();
    let _ = body_str(r2.into_body()).await;
    assert_eq!(
        counters.verify_get(),
        2,
        "cache should be invalidated after token revoke"
    );
}

#[derive(Clone)]
struct RetryMockState {
    call_count: Arc<AtomicUsize>,
    fail_first_n: usize,
}

async fn spawn_retry_mock(state: RetryMockState) -> String {
    async fn verify(State(s): State<RetryMockState>) -> Result<Json<TokenClaims>, StatusCode> {
        let n = s.call_count.fetch_add(1, Ordering::SeqCst);
        if n < s.fail_first_n {
            Err(StatusCode::SERVICE_UNAVAILABLE)
        } else {
            Ok(Json(TokenClaims {
                sub: "sub".into(),
                jti: "jti".into(),
                scope: takusu_types::TokenScope::ReadWrite.to_string(),
                label: None,
                aud: takusu_local_lib::DEFAULT_AUD.into(),
                iss: takusu_local_lib::DEFAULT_ISS.into(),
                iat: 0,
                exp: None,
            }))
        }
    }

    let app = Router::new()
        .route("/api/auth/verify", get(verify))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn workers_storage_retries_503() {
    let state = RetryMockState {
        call_count: Arc::new(AtomicUsize::new(0)),
        fail_first_n: 2,
    };
    let url = spawn_retry_mock(state.clone()).await;
    let storage = WorkersStorage::new_with(url, ROOT_TOKEN.to_string());

    let valid = storage.verify_token("tsk_anything").await.unwrap();
    assert!(valid.is_some());
    assert_eq!(
        state.call_count.load(Ordering::SeqCst),
        3,
        "should be called 1 initial + 2 retries"
    );
}

#[tokio::test]
async fn workers_storage_no_retry_on_404() {
    async fn verify() -> StatusCode {
        StatusCode::NOT_FOUND
    }

    let app = Router::new().route("/api/auth/verify", get(verify));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    let url = format!("http://{addr}");

    let storage = WorkersStorage::new_with(url, "tsk_anything".into());
    let result = storage
        .get_task("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(matches!(result, Err(StorageError::NotFound(_))));
}
