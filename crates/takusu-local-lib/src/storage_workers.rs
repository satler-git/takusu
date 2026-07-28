use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use takusu_storage::{
    ApplyHabitEstimateRequest, CreateHabit, CreateHabitScheduledSpan, CreateMemory, CreateSkill,
    CreateTask, GoogleCalEventRow, GoogleCalSettingsRow, HabitRow, HabitScheduledSpanRow,
    HabitStepEstimateInput, HabitStepInput, HabitStepRow, MemoryQuery, MemoryRow, ProgressResult,
    RecordProgress, SaveScheduleRequest, ScheduleRow, SettingsRow, SimilarTaskQuery,
    SimilarTaskRow, SkillRow, SplitResult, SplitTask, Storage, StorageError, TaskProgress,
    TaskQuery, TaskRow, TokenCreateResponse, TokenRow, UpdateGoogleCalSettings, UpdateHabit,
    UpdateMemory, UpdateSettings, UpdateSkill, UpdateTask, storage::StorageResult,
};
use takusu_util::EnumLabel;
use takusu_util::{TokenClaims, url_encode};
use tokio::sync::RwLock;

const RETRY_STATUSES: &[u16] = &[429, 500, 502, 503, 504];
const RETRY_DELAYS_MS: &[u64] = &[100, 200, 400];

#[derive(Clone)]
struct Credentials {
    url: Arc<str>,
    token: Arc<str>,
}

/// Request body variant for [`WorkersStorage::send_request`].
#[derive(Clone)]
enum RequestBody {
    /// No request body (GET / DELETE without body).
    None,
    /// A pre-serialised JSON body string.
    Json(String),
}

impl RequestBody {
    /// Serialise `body` into a [`RequestBody::Json`].
    fn json<B: Serialize>(body: &B) -> StorageResult<Self> {
        serde_json::to_string(body)
            .map(RequestBody::Json)
            .map_err(|e| StorageError::Internal(format!("serialize body: {e}")))
    }
}

pub struct WorkersStorage {
    http: Client,
    credentials: RwLock<Credentials>,
}

impl WorkersStorage {
    pub fn new_with(base_url: String, token: String) -> Self {
        Self {
            http: Client::new(),
            credentials: RwLock::new(Credentials {
                url: Arc::from(base_url.trim_end_matches('/')),
                token: Arc::from(token.into_boxed_str()),
            }),
        }
    }

    /// Like [`new_with`](Self::new_with) but with a caller-supplied HTTP
    /// client.  On Android the default `Client::new()` pulls in
    /// `rustls-platform-verifier`, which panics unless initialised with a JNI
    /// context.  Callers that cannot provide that context should instead build
    /// a client with bundled root certificates (e.g. `webpki-root-certs`) and
    /// pass it here.
    pub fn new_with_client(client: Client, base_url: String, token: String) -> Self {
        Self {
            http: client,
            credentials: RwLock::new(Credentials {
                url: Arc::from(base_url.trim_end_matches('/')),
                token: Arc::from(token.into_boxed_str()),
            }),
        }
    }

    pub async fn update_credentials(&self, base_url: String, token: String) {
        *self.credentials.write().await = Credentials {
            url: Arc::from(base_url.trim_end_matches('/')),
            token: Arc::from(token.into_boxed_str()),
        };
    }

    async fn credentials(&self) -> Credentials {
        self.credentials.read().await.clone()
    }

    /// Unified HTTP request helper.  Builds the request with auth headers and
    /// optional JSON body / idempotency key, runs it through the retry loop,
    /// and returns the raw response.  Response decoding is left to the
    /// caller ([`send_json`](Self::send_json) / [`send_empty`](Self::send_empty)).
    async fn send_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: RequestBody,
        idempotency_key: Option<&str>,
    ) -> StorageResult<reqwest::Response> {
        self.send_with_retry(move || {
            let method = method.clone();
            let body = body.clone();
            async move {
                let creds = self.credentials().await;
                let url = format!("{}{}", creds.url.as_ref(), path);
                let mut req = self
                    .http
                    .request(method.clone(), &url)
                    .bearer_auth(creds.token.as_ref());
                if let RequestBody::Json(json) = &body {
                    req = req
                        .header("content-type", "application/json")
                        .body(json.clone());
                }
                if let Some(op_id) = idempotency_key {
                    req = req.header("Idempotency-Key", op_id);
                }
                req.build()
            }
        })
        .await
    }

    /// Convenience wrapper around [`send_request`](Self::send_request) for
    /// requests whose response body should be deserialised into `T`.
    async fn send_json<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: RequestBody,
        idempotency_key: Option<&str>,
    ) -> StorageResult<T> {
        let resp = self
            .send_request(method, path, body, idempotency_key)
            .await?;
        map_response(resp).await
    }

    /// Convenience wrapper around [`send_request`](Self::send_request) for
    /// requests whose response body should be discarded.
    async fn send_empty(
        &self,
        method: reqwest::Method,
        path: &str,
        body: RequestBody,
        idempotency_key: Option<&str>,
    ) -> StorageResult<()> {
        let resp = self
            .send_request(method, path, body, idempotency_key)
            .await?;
        map_empty(resp).await
    }

    async fn send_with_retry<F, Fut>(&self, build: F) -> StorageResult<reqwest::Response>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = reqwest::Result<reqwest::Request>> + Send,
    {
        let creds = self.credentials().await;
        if creds.url.is_empty() || creds.token.is_empty() {
            return Err(StorageError::Internal("worker not configured".into()));
        }
        let mut attempt = 0;
        loop {
            let req = build()
                .await
                .map_err(|e| StorageError::Internal(format!("build request: {e}")))?;
            let result = self.http.execute(req).await;
            match result {
                Ok(resp) if !RETRY_STATUSES.contains(&resp.status().as_u16()) => return Ok(resp),
                Ok(resp) if attempt < RETRY_DELAYS_MS.len() => {
                    let status = resp.status().as_u16();
                    let delay = RETRY_DELAYS_MS[attempt];
                    tracing::warn!(
                        "worker returned retryable status {status} (attempt {}), sleeping {delay}ms",
                        attempt + 1
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    attempt += 1;
                }
                Ok(resp) => return Ok(resp),
                Err(e) if attempt < RETRY_DELAYS_MS.len() => {
                    let delay = RETRY_DELAYS_MS[attempt];
                    tracing::warn!(
                        "worker request failed (attempt {}): {e}, sleeping {delay}ms",
                        attempt + 1
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    attempt += 1;
                }
                Err(e) => {
                    return Err(StorageError::Internal(format!("worker http: {e}")));
                }
            }
        }
    }
}

async fn map_response<T: DeserializeOwned>(resp: reqwest::Response) -> StorageResult<T> {
    let status = resp.status().as_u16();
    if status >= 400 {
        let body = resp.text().await.unwrap_or_default();
        return Err(map_status(status, body));
    }
    resp.json::<T>()
        .await
        .map_err(|e| StorageError::Internal(format!("decode: {e}")))
}

async fn map_empty(resp: reqwest::Response) -> StorageResult<()> {
    let status = resp.status().as_u16();
    if status >= 400 {
        let body = resp.text().await.unwrap_or_default();
        return Err(map_status(status, body));
    }
    Ok(())
}

fn map_status(status: u16, body: String) -> StorageError {
    match status {
        401 => StorageError::Unauthorized,
        404 => StorageError::NotFound(body),
        400 => StorageError::BadRequest(body),
        409 => StorageError::Conflict(body),
        _ => StorageError::Internal(format!("status {status}: {body}")),
    }
}

#[async_trait]
impl Storage for WorkersStorage {
    async fn verify_token(&self, token: &str) -> StorageResult<Option<TokenClaims>> {
        let creds = self.credentials().await;
        if creds.url.is_empty() || creds.token.is_empty() {
            return Ok(None);
        }
        let resp = self
            .send_with_retry(move || async move {
                let creds = self.credentials().await;
                let url = format!("{}/api/auth/verify", creds.url.as_ref());
                self.http.get(&url).bearer_auth(token).build()
            })
            .await?;
        match resp.status().as_u16() {
            200 => resp
                .json::<TokenClaims>()
                .await
                .map(Some)
                .map_err(|e| StorageError::Internal(format!("invalid verify response: {e}"))),
            401 => Ok(None),
            other => {
                let body = resp.text().await.unwrap_or_default();
                Err(StorageError::Internal(format!(
                    "verify status {other}: {body}"
                )))
            }
        }
    }

    async fn list_tasks(&self, _query: &TaskQuery) -> StorageResult<Vec<TaskRow>> {
        let mut path = String::from("/api/tasks");
        let q = _query;
        let mut parts: Vec<String> = Vec::new();
        if let Some(s) = q.status {
            parts.push(format!("status={}", url_encode(s.as_str())));
        }
        if let Some(f) = q.from {
            parts.push(format!("from={}", url_encode(&f.to_string())));
        }
        if let Some(u) = q.until {
            parts.push(format!("until={}", url_encode(&u.to_string())));
        }
        if q.no_overdue == Some(true) {
            parts.push("no_overdue=true".into());
        }
        if let Some(h) = &q.habit_id {
            parts.push(format!("habit_id={}", url_encode(h)));
        }
        if let Some(u) = &q.ical_uid {
            parts.push(format!("ical_uid={}", url_encode(u)));
        }
        if let Some(query_str) = &q.q {
            parts.push(format!("q={}", url_encode(query_str)));
        }
        if let Some(limit) = q.limit {
            parts.push(format!("limit={limit}"));
        }
        if !parts.is_empty() {
            path.push('?');
            path.push_str(&parts.join("&"));
        }
        self.send_json(reqwest::Method::GET, &path, RequestBody::None, None)
            .await
    }

    async fn task_exists_by_ical_uid(&self, uid: &str) -> StorageResult<bool> {
        let tasks = self
            .list_tasks(&TaskQuery {
                ical_uid: Some(uid.to_string()),
                ..Default::default()
            })
            .await?;
        Ok(!tasks.is_empty())
    }

    async fn get_task(&self, id: &str) -> StorageResult<TaskRow> {
        let full = self.resolve_task_id(id).await?;
        self.send_json(
            reqwest::Method::GET,
            &format!("/api/tasks/{}", url_encode(&full)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn create_task(&self, body: &CreateTask) -> StorageResult<TaskRow> {
        self.send_json(
            reqwest::Method::POST,
            "/api/tasks",
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn update_task(&self, id: &str, body: &UpdateTask) -> StorageResult<TaskRow> {
        let full = self.resolve_task_id(id).await?;
        self.send_json(
            reqwest::Method::PATCH,
            &format!("/api/tasks/{}", url_encode(&full)),
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn replace_task(&self, id: &str, body: &CreateTask) -> StorageResult<TaskRow> {
        let full = self.resolve_task_id(id).await?;
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/tasks/{}", url_encode(&full)),
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn delete_task(&self, id: &str) -> StorageResult<()> {
        let full = self.resolve_task_id(id).await?;
        self.send_empty(
            reqwest::Method::DELETE,
            &format!("/api/tasks/{}", url_encode(&full)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn list_habits(&self) -> StorageResult<Vec<HabitRow>> {
        self.send_json(reqwest::Method::GET, "/api/habits", RequestBody::None, None)
            .await
    }

    async fn get_habit(&self, id: &str) -> StorageResult<HabitRow> {
        self.send_json(
            reqwest::Method::GET,
            &format!("/api/habits/{}", url_encode(id)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn create_habit(&self, body: &CreateHabit) -> StorageResult<HabitRow> {
        self.send_json(
            reqwest::Method::POST,
            "/api/habits",
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn update_habit(&self, id: &str, body: &UpdateHabit) -> StorageResult<HabitRow> {
        self.send_json(
            reqwest::Method::PATCH,
            &format!("/api/habits/{}", url_encode(id)),
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn replace_habit(&self, id: &str, body: &CreateHabit) -> StorageResult<HabitRow> {
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/habits/{}", url_encode(id)),
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn delete_habit(&self, id: &str) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            &format!("/api/habits/{}", url_encode(id)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn list_habit_scheduled_spans(
        &self,
        habit_id: &str,
    ) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        self.send_json(
            reqwest::Method::GET,
            &format!("/api/habits/{}/scheduled-spans", url_encode(habit_id)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn list_all_habit_scheduled_spans(&self) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        self.send_json(
            reqwest::Method::GET,
            "/api/habits/scheduled-spans",
            RequestBody::None,
            None,
        )
        .await
    }

    async fn create_habit_scheduled_span(
        &self,
        habit_id: &str,
        body: &CreateHabitScheduledSpan,
    ) -> StorageResult<HabitScheduledSpanRow> {
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/habits/{}/scheduled-spans", url_encode(habit_id)),
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn delete_habit_scheduled_span(
        &self,
        habit_id: &str,
        span_id: &str,
    ) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            &format!(
                "/api/habits/{}/scheduled-spans/{}",
                url_encode(habit_id),
                url_encode(span_id)
            ),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn list_habit_steps(&self, habit_id: &str) -> StorageResult<Vec<HabitStepRow>> {
        self.send_json(
            reqwest::Method::GET,
            &format!("/api/habits/{}/steps", url_encode(habit_id)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn list_all_habit_steps(&self) -> StorageResult<Vec<HabitStepRow>> {
        self.send_json(
            reqwest::Method::GET,
            "/api/habits/steps",
            RequestBody::None,
            None,
        )
        .await
    }

    async fn replace_habit_steps(
        &self,
        habit_id: &str,
        steps: &[HabitStepInput],
    ) -> StorageResult<Vec<HabitStepRow>> {
        self.send_json(
            reqwest::Method::PUT,
            &format!("/api/habits/{}/steps", url_encode(habit_id)),
            RequestBody::json(&steps)?,
            None,
        )
        .await
    }

    async fn apply_habit_estimate(
        &self,
        habit_id: &str,
        avg_minutes: i64,
        sigma_minutes: i64,
        step_estimates: &[HabitStepEstimateInput],
    ) -> StorageResult<()> {
        let body = ApplyHabitEstimateRequest {
            avg_minutes,
            sigma_minutes,
            steps: step_estimates.to_vec(),
        };
        self.send_empty(
            reqwest::Method::POST,
            &format!("/api/habits/{}/estimate", url_encode(habit_id)),
            RequestBody::json(&body)?,
            None,
        )
        .await?;
        Ok(())
    }

    async fn get_schedule(&self) -> StorageResult<Option<ScheduleRow>> {
        let resp = self
            .send_with_retry(move || async move {
                let creds = self.credentials().await;
                let url = format!("{}/api/schedule", creds.url.as_ref());
                self.http
                    .get(&url)
                    .bearer_auth(creds.token.as_ref())
                    .build()
            })
            .await?;
        match resp.status().as_u16() {
            200 => {
                let row: ScheduleRow = resp
                    .json()
                    .await
                    .map_err(|e| StorageError::Internal(format!("decode: {e}")))?;
                Ok(Some(row))
            }
            404 => Ok(None),
            other => {
                let body = resp.text().await.unwrap_or_default();
                Err(StorageError::Internal(format!(
                    "schedule status {other}: {body}"
                )))
            }
        }
    }

    async fn save_schedule(&self, req: &SaveScheduleRequest) -> StorageResult<ScheduleRow> {
        self.send_json(
            reqwest::Method::POST,
            "/api/schedule/save",
            RequestBody::json(req)?,
            None,
        )
        .await
    }

    async fn clear_schedule(&self) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            "/api/schedule",
            RequestBody::None,
            None,
        )
        .await
    }

    async fn create_token(&self, label: Option<&str>) -> StorageResult<TokenCreateResponse> {
        self.send_json(
            reqwest::Method::POST,
            "/api/tokens",
            RequestBody::json(&json!({ "label": label }))?,
            None,
        )
        .await
    }

    async fn list_tokens(&self) -> StorageResult<Vec<TokenRow>> {
        self.send_json(reqwest::Method::GET, "/api/tokens", RequestBody::None, None)
            .await
    }

    async fn revoke_token(&self, id: i64) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            &format!("/api/tokens/{id}"),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn get_settings(&self) -> StorageResult<SettingsRow> {
        self.send_json(
            reqwest::Method::GET,
            "/api/settings",
            RequestBody::None,
            None,
        )
        .await
    }

    async fn update_settings(&self, body: &UpdateSettings) -> StorageResult<SettingsRow> {
        self.send_json(
            reqwest::Method::PUT,
            "/api/settings",
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn get_gcal_settings(&self) -> StorageResult<GoogleCalSettingsRow> {
        self.send_json(
            reqwest::Method::GET,
            "/api/sync/settings",
            RequestBody::None,
            None,
        )
        .await
    }

    async fn update_gcal_settings(
        &self,
        body: &UpdateGoogleCalSettings,
    ) -> StorageResult<GoogleCalSettingsRow> {
        self.send_json(
            reqwest::Method::PUT,
            "/api/sync/settings",
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn list_gcal_mappings(&self) -> StorageResult<Vec<GoogleCalEventRow>> {
        self.send_json(
            reqwest::Method::GET,
            "/api/sync/mappings",
            RequestBody::None,
            None,
        )
        .await
    }

    async fn upsert_gcal_mappings(&self, mappings: &[(String, String)]) -> StorageResult<()> {
        let body = json!({
            "mappings": mappings.iter().map(|(t, e)| json!({
                "task_id": t,
                "google_event_id": e
            })).collect::<Vec<_>>()
        });
        self.send_empty(
            reqwest::Method::POST,
            "/api/sync/mappings",
            RequestBody::json(&body)?,
            None,
        )
        .await
    }

    async fn delete_gcal_mappings(&self, task_ids: &[String]) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            "/api/sync/mappings",
            RequestBody::json(&json!({ "task_ids": task_ids }))?,
            None,
        )
        .await
    }

    async fn clear_gcal_mappings(&self) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            "/api/sync/mappings?all=1",
            RequestBody::None,
            None,
        )
        .await
    }

    async fn list_skills(&self) -> StorageResult<Vec<SkillRow>> {
        self.send_json(reqwest::Method::GET, "/api/skills", RequestBody::None, None)
            .await
    }

    async fn get_skill(&self, slug: &str) -> StorageResult<SkillRow> {
        self.send_json(
            reqwest::Method::GET,
            &format!("/api/skills/{}", url_encode(slug)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn create_skill(&self, body: &CreateSkill) -> StorageResult<SkillRow> {
        self.send_json(
            reqwest::Method::POST,
            "/api/skills",
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn update_skill(&self, slug: &str, body: &UpdateSkill) -> StorageResult<SkillRow> {
        self.send_json(
            reqwest::Method::PATCH,
            &format!("/api/skills/{}", url_encode(slug)),
            RequestBody::json(body)?,
            None,
        )
        .await
    }

    async fn delete_skill(&self, slug: &str) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            &format!("/api/skills/{}", url_encode(slug)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn get_memory(&self, id: &str) -> StorageResult<MemoryRow> {
        self.send_json(
            reqwest::Method::GET,
            &format!("/api/memory/{}", url_encode(id)),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn create_memory(
        &self,
        body: &CreateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow> {
        self.send_json(
            reqwest::Method::POST,
            "/api/memory",
            RequestBody::json(body)?,
            operation_id,
        )
        .await
    }

    async fn update_memory(
        &self,
        id: &str,
        body: &UpdateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow> {
        self.send_json(
            reqwest::Method::PATCH,
            &format!("/api/memory/{}", url_encode(id)),
            RequestBody::json(body)?,
            operation_id,
        )
        .await
    }

    async fn delete_memory(
        &self,
        id: &str,
        observed_revision: i64,
        operation_id: Option<&str>,
    ) -> StorageResult<()> {
        self.send_empty(
            reqwest::Method::DELETE,
            &format!(
                "/api/memory/{}?observed_revision={observed_revision}",
                url_encode(id)
            ),
            RequestBody::None,
            operation_id,
        )
        .await
    }

    async fn search_memories(&self, query: &MemoryQuery) -> StorageResult<Vec<MemoryRow>> {
        let mut path = String::from("/api/memory/search");
        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("q={}", url_encode(&query.q)));
        if let Some(ref kind) = query.kind {
            parts.push(format!("kind={}", url_encode(kind.as_str())));
        }
        if let Some(ref subject_type) = query.subject_type {
            parts.push(format!(
                "subject_type={}",
                url_encode(subject_type.as_str())
            ));
        }
        if let Some(ref subject_id) = query.subject_id {
            parts.push(format!("subject_id={}", url_encode(subject_id)));
        }
        if let Some(limit) = query.limit {
            parts.push(format!("limit={limit}"));
        }
        path.push('?');
        path.push_str(&parts.join("&"));
        self.send_json(reqwest::Method::GET, &path, RequestBody::None, None)
            .await
    }

    async fn find_similar_tasks(
        &self,
        query: &SimilarTaskQuery,
    ) -> StorageResult<Vec<SimilarTaskRow>> {
        let mut path = "/api/tasks/similar?".to_string();
        path.push_str(&format!("q={}", url_encode(&query.title)));
        if let Some(limit) = query.limit {
            path.push_str(&format!("&limit={limit}"));
        }
        self.send_json(reqwest::Method::GET, &path, RequestBody::None, None)
            .await
    }

    async fn start_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<TaskRow> {
        let full = self.resolve_task_id(id).await?;
        let body = json!({});
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/tasks/{full}/work/start"),
            RequestBody::json(&body)?,
            operation_id,
        )
        .await
    }

    async fn pause_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<TaskRow> {
        let full = self.resolve_task_id(id).await?;
        let body = json!({});
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/tasks/{full}/work/pause"),
            RequestBody::json(&body)?,
            operation_id,
        )
        .await
    }

    async fn record_progress(
        &self,
        id: &str,
        body: &RecordProgress,
        operation_id: Option<&str>,
    ) -> StorageResult<ProgressResult> {
        let full = self.resolve_task_id(id).await?;
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/tasks/{full}/progress"),
            RequestBody::json(body)?,
            operation_id,
        )
        .await
    }

    async fn complete_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<TaskRow> {
        let full = self.resolve_task_id(id).await?;
        let body = json!({});
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/tasks/{full}/work/complete"),
            RequestBody::json(&body)?,
            operation_id,
        )
        .await
    }

    async fn get_task_progress(&self, id: &str) -> StorageResult<TaskProgress> {
        let full = self.resolve_task_id(id).await?;
        self.send_json(
            reqwest::Method::GET,
            &format!("/api/tasks/{full}/progress"),
            RequestBody::None,
            None,
        )
        .await
    }

    async fn split_task(
        &self,
        id: &str,
        body: &SplitTask,
        operation_id: Option<&str>,
    ) -> StorageResult<SplitResult> {
        let full = self.resolve_task_id(id).await?;
        self.send_json(
            reqwest::Method::POST,
            &format!("/api/tasks/{full}/split"),
            RequestBody::json(body)?,
            operation_id,
        )
        .await
    }

    async fn health_check(&self) -> StorageResult<String> {
        let creds = self.credentials().await;
        if creds.url.is_empty() || creds.token.is_empty() {
            return Ok("worker not configured".into());
        }
        let url = format!("{}/health", creds.url.as_ref());
        // Per-request timeout so an unreachable worker fails fast instead of
        // hanging indefinitely (the shared client has no default timeout).
        let resp = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| StorageError::Internal(format!("worker health check failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(StorageError::Internal(format!(
                "worker health check returned {status}"
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| StorageError::Internal(format!("worker health check body read: {e}")))?;
        Ok(format!("worker ok: {}", body.trim()))
    }

    async fn update_workers_credentials(&self, url: &str, token: &str) -> StorageResult<()> {
        self.update_credentials(url.to_string(), token.to_string())
            .await;
        Ok(())
    }
}

impl WorkersStorage {
    async fn resolve_task_id(&self, id: &str) -> StorageResult<String> {
        // Allow display ids with a leading `#` (e.g. `#42`) written by the LLM.
        let id = id.strip_prefix('#').unwrap_or(id);

        // `h{habit_display_id}#{task_display_id}` → habit task lookup (#380).
        if let Some(rest) = id.strip_prefix(['h', 'H'])
            && let Some((hdisp, tdisp)) = rest.split_once('#')
            && let (Ok(hnum), Ok(tnum)) = (hdisp.parse::<i64>(), tdisp.parse::<i64>())
        {
            let tasks: Vec<TaskRow> = self
                .send_json::<Vec<TaskRow>>(
                    reqwest::Method::GET,
                    "/api/tasks",
                    RequestBody::None,
                    None,
                )
                .await?;
            let habits: Vec<HabitRow> = self
                .send_json::<Vec<HabitRow>>(
                    reqwest::Method::GET,
                    "/api/habits",
                    RequestBody::None,
                    None,
                )
                .await?;
            let habit_id = habits
                .iter()
                .find(|h| h.display_id == hnum)
                .map(|h| h.id.as_str());
            if let Some(hid) = habit_id
                && let Some(t) = tasks
                    .iter()
                    .find(|t| t.habit_id.as_deref() == Some(hid) && t.display_id == tnum)
            {
                return Ok(t.id.clone());
            }
            return Err(StorageError::NotFound(format!("task {id} not found")));
        }
        // Numeric input → resolve via display_id for non-habit tasks only (#380).
        if let Ok(num) = id.parse::<i64>() {
            let tasks: Vec<TaskRow> = self
                .send_json::<Vec<TaskRow>>(
                    reqwest::Method::GET,
                    "/api/tasks",
                    RequestBody::None,
                    None,
                )
                .await?;
            if let Some(t) = tasks
                .iter()
                .find(|t| t.display_id == num && t.habit_id.is_none())
            {
                return Ok(t.id.clone());
            }
            return Err(StorageError::NotFound(format!("task {id} not found")));
        }
        if id.contains('-') {
            return Ok(id.to_string());
        }
        // UUID prefix — fetch all tasks and filter client-side (matches
        // SqliteStorage's `LIKE prefix%` behaviour).
        let tasks: Vec<TaskRow> = self
            .send_json::<Vec<TaskRow>>(reqwest::Method::GET, "/api/tasks", RequestBody::None, None)
            .await?;
        let mut matches: Vec<String> = tasks
            .iter()
            .filter(|t| t.id.starts_with(id))
            .map(|t| t.id.clone())
            .collect();
        match matches.len() {
            0 => Err(StorageError::NotFound(format!("task {id} not found"))),
            1 => Ok(matches.remove(0)),
            _ => Err(StorageError::BadRequest(format!(
                "ambiguous task id prefix: {id}"
            ))),
        }
    }
}
