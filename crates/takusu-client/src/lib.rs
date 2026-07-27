//! # takusu-client — HTTP client for takusu REST API
//!
//! Provides types and a `Client` for interacting with the takusu REST API.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use takusu_util::url_encode;
use tokio::sync::RwLock;

#[derive(Debug)]
pub enum ClientError {
    Http(reqwest::Error),
    Api { status: u16, body: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Http(e) => write!(f, "HTTP error: {e}"),
            ClientError::Api { status, body } => write!(f, "API error {status}: {body}"),
        }
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> Self {
        ClientError::Http(e)
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: Arc<RwLock<Arc<str>>>,
}

/// Build a `reqwest::Client` that is safe to use on Android.
///
/// `reqwest` 0.13 defaults to `rustls-platform-verifier` for certificate
/// verification. On Android that verifier requires a JNI context that is not
/// available in the embedded UniFFI runtime, so any HTTPS request panics and
/// kills the server task, surfacing as "unexpected end of stream" to the
/// client. Use bundled webpki root certificates instead on Android.
pub fn default_http_client(
    timeout_seconds: Option<u64>,
) -> Result<reqwest::Client, reqwest::Error> {
    #[cfg(target_os = "android")]
    {
        let certs: Vec<reqwest::Certificate> = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter_map(|c| reqwest::Certificate::from_der(c.as_ref()).ok())
            .collect();
        assert!(
            !certs.is_empty(),
            "no bundled root certificates were loaded; HTTPS cannot be used"
        );
        let mut builder = reqwest::Client::builder()
            .use_rustls_tls()
            .tls_certs_only(certs);
        if let Some(secs) = timeout_seconds {
            builder = builder.timeout(Duration::from_secs(secs));
        }
        builder.build()
    }
    #[cfg(not(target_os = "android"))]
    {
        let mut builder = reqwest::Client::builder();
        if let Some(secs) = timeout_seconds {
            builder = builder.timeout(Duration::from_secs(secs));
        }
        builder.build()
    }
}

impl Client {
    pub fn new(base_url: &str, token: &str) -> Self {
        Self::new_with_token(base_url, Arc::new(RwLock::new(Arc::from(token))))
    }

    pub fn new_with_token(base_url: &str, token: Arc<RwLock<Arc<str>>>) -> Self {
        Self {
            http: default_http_client(None).expect("failed to build HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

    async fn token(&self) -> Arc<str> {
        self.token.read().await.clone()
    }

    async fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let token = self.token().await;
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&*token)
    }

    // ── Health ──

    pub async fn health(&self) -> Result<String, ClientError> {
        let resp = self
            .http
            .get(format!("{}/health", self.base_url))
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.text().await?)
    }

    // ── Task ──

    pub async fn list_tasks(&self, query: &TaskQuery) -> Result<Vec<TaskRow>, ClientError> {
        let url = format!("{}/api/tasks", self.base_url);
        let token = self.token().await;
        let mut req = self.http.get(&url).bearer_auth(&*token);
        let mut params: Vec<(&str, &str)> = Vec::new();
        let limit_string = query.limit.map(|n| n.to_string());
        if let Some(ref s) = query.status {
            params.push(("status", s));
        }
        if let Some(ref v) = query.from {
            params.push(("from", v));
        }
        if let Some(ref v) = query.until {
            params.push(("until", v));
        }
        if let Some(v) = query.no_overdue {
            params.push(("no_overdue", if v { "true" } else { "false" }));
        }
        if let Some(ref v) = query.habit_id {
            params.push(("habit_id", v));
        }
        if let Some(ref v) = query.ical_uid {
            params.push(("ical_uid", v));
        }
        if let Some(ref v) = query.q {
            params.push(("q", v));
        }
        if let Some(ref s) = limit_string {
            params.push(("limit", s));
        }
        if !params.is_empty() {
            req = req.query(&params);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn get_task(&self, id: &str) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(reqwest::Method::GET, &format!("/api/tasks/{encoded_id}"))
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn create_task(&self, body: &CreateTask) -> Result<TaskRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/tasks")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn update_task(&self, id: &str, body: &UpdateTask) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(reqwest::Method::PATCH, &format!("/api/tasks/{encoded_id}"))
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn replace_task(&self, id: &str, body: &CreateTask) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(reqwest::Method::PUT, &format!("/api/tasks/{encoded_id}"))
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn delete_task(&self, id: &str) -> Result<(), ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(reqwest::Method::DELETE, &format!("/api/tasks/{encoded_id}"))
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(())
    }

    pub async fn start_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/tasks/{encoded_id}/work/start"),
            )
            .await
            .json(&serde_json::json!({}));
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn pause_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/tasks/{encoded_id}/work/pause"),
            )
            .await
            .json(&serde_json::json!({}));
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn record_progress(
        &self,
        id: &str,
        body: &RecordProgress,
        operation_id: Option<&str>,
    ) -> Result<ProgressResult, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/tasks/{encoded_id}/progress"),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn complete_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/tasks/{encoded_id}/work/complete"),
            )
            .await
            .json(&serde_json::json!({}));
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn get_task_progress(&self, id: &str) -> Result<TaskProgress, ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/tasks/{encoded_id}/progress"),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn split_task(
        &self,
        id: &str,
        body: &SplitTask,
        operation_id: Option<&str>,
    ) -> Result<SplitResult, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/tasks/{encoded_id}/split"),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn analyze_task_dependencies(
        &self,
    ) -> Result<DependencyAnalysisResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/tasks/dependency-analysis")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    // ── Habit ──

    pub async fn list_habits(&self) -> Result<Vec<HabitRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/habits")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn get_habit(&self, id: &str) -> Result<HabitDetail, ClientError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/habits/{}", url_encode(id)),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn estimate_habit(
        &self,
        id: &str,
        body: &HabitEstimateRequest,
    ) -> Result<HabitEstimateResult, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, &format!("/api/habits/{id}/estimate"))
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn create_habit(&self, body: &CreateHabit) -> Result<HabitRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/habits")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn update_habit(
        &self,
        id: &str,
        body: &UpdateHabit,
    ) -> Result<HabitRow, ClientError> {
        let resp = self
            .request(
                reqwest::Method::PATCH,
                &format!("/api/habits/{}", url_encode(id)),
            )
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn replace_habit(
        &self,
        id: &str,
        body: &CreateHabit,
    ) -> Result<HabitRow, ClientError> {
        let resp = self
            .request(
                reqwest::Method::PUT,
                &format!("/api/habits/{}", url_encode(id)),
            )
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn delete_habit(&self, id: &str) -> Result<(), ClientError> {
        let resp = self
            .request(
                reqwest::Method::DELETE,
                &format!("/api/habits/{}", url_encode(id)),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(())
    }

    // ── Habit scheduled spans (#303 / #503) ──

    pub async fn list_habit_scheduled_spans(
        &self,
        id: &str,
    ) -> Result<Vec<HabitScheduledSpanRow>, ClientError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/habits/{}/scheduled-spans", url_encode(id)),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn list_all_habit_scheduled_spans(
        &self,
    ) -> Result<Vec<HabitScheduledSpanRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/habits/scheduled-spans")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn create_habit_scheduled_span(
        &self,
        id: &str,
        body: &CreateHabitScheduledSpan,
    ) -> Result<HabitScheduledSpanRow, ClientError> {
        let resp = self
            .request(
                reqwest::Method::POST,
                &format!("/api/habits/{}/scheduled-spans", url_encode(id)),
            )
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn delete_habit_scheduled_span(
        &self,
        id: &str,
        span_id: &str,
    ) -> Result<(), ClientError> {
        let resp = self
            .request(
                reqwest::Method::DELETE,
                &format!(
                    "/api/habits/{}/scheduled-spans/{}",
                    url_encode(id),
                    url_encode(span_id)
                ),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(())
    }

    // ── Habit steps (#95) ──

    pub async fn list_habit_steps(&self, id: &str) -> Result<Vec<HabitStepRow>, ClientError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/habits/{}/steps", url_encode(id)),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn list_all_habit_steps(&self) -> Result<Vec<HabitStepRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/habits/steps")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn replace_habit_steps(
        &self,
        id: &str,
        steps: &[HabitStepInput],
    ) -> Result<Vec<HabitStepRow>, ClientError> {
        let resp = self
            .request(
                reqwest::Method::PUT,
                &format!("/api/habits/{}/steps", url_encode(id)),
            )
            .await
            .json(steps)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn analyze_habit_step_dependencies(
        &self,
        habit_id: &str,
    ) -> Result<DependencyAnalysisResponse, ClientError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!(
                    "/api/habits/{}/steps/dependency-analysis",
                    url_encode(habit_id)
                ),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    // ── Schedule ──

    pub async fn preview_schedule(
        &self,
        body: &SchedulePreviewRequest,
    ) -> Result<serde_json::Value, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/schedule/preview")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn replace_schedule(
        &self,
        body: &SaveScheduleRequest,
    ) -> Result<ScheduleRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/schedule/replace")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn get_schedule(&self) -> Result<ScheduleRow, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/schedule")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn generate_schedule(
        &self,
        body: &GenerateSchedule,
    ) -> Result<ScheduleRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/schedule/generate")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn reschedule(&self, body: &Reschedule) -> Result<ScheduleRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/schedule/reschedule")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn move_entry(
        &self,
        task_id: &str,
        body: &MoveEntry,
    ) -> Result<MoveEntryResponse, ClientError> {
        let resp = self
            .request(
                reqwest::Method::PATCH,
                &format!("/api/schedule/entries/{}", url_encode(task_id)),
            )
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        resp.json().await.map_err(ClientError::Http)
    }

    pub async fn clear_schedule(&self) -> Result<(), ClientError> {
        let resp = self
            .request(reqwest::Method::DELETE, "/api/schedule")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(())
    }

    // ── Token ──

    pub async fn create_token(
        &self,
        label: Option<&str>,
    ) -> Result<TokenCreateResponse, ClientError> {
        let body = serde_json::json!({ "label": label });
        let resp = self
            .request(reqwest::Method::POST, "/api/tokens")
            .await
            .json(&body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/tokens")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn revoke_token(&self, id: i64) -> Result<(), ClientError> {
        let resp = self
            .request(
                reqwest::Method::DELETE,
                &format!("/api/tokens/{}", url_encode(&id.to_string())),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(())
    }

    // ── Sync (Google Calendar) ──

    pub async fn get_sync_settings(&self) -> Result<SyncSettingsResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/sync/settings")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn update_sync_settings(
        &self,
        body: &UpdateSyncSettings,
    ) -> Result<SyncSettingsResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::PUT, "/api/sync/settings")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn get_oauth_url(
        &self,
        redirect_uri: &str,
    ) -> Result<serde_json::Value, ClientError> {
        let body = serde_json::json!({ "redirect_uri": redirect_uri });
        let resp = self
            .request(reqwest::Method::POST, "/api/sync/oauth/url")
            .await
            .json(&body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn oauth_callback(
        &self,
        code: &str,
        redirect_uri: Option<&str>,
    ) -> Result<serde_json::Value, ClientError> {
        let body = if let Some(uri) = redirect_uri {
            serde_json::json!({ "code": code, "redirect_uri": uri })
        } else {
            serde_json::json!({ "code": code })
        };
        let resp = self
            .request(reqwest::Method::POST, "/api/sync/oauth/callback")
            .await
            .json(&body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn trigger_sync(&self) -> Result<serde_json::Value, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/sync/trigger")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn delete_all_gcal_events(&self) -> Result<DeleteAllGcalResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/sync/delete-all")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    // ── Settings ──

    pub async fn get_settings(&self) -> Result<SettingsResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/settings")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn update_settings(
        &self,
        body: &UpdateSettings,
    ) -> Result<SettingsResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::PUT, "/api/settings")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    // ── Skills (#WI-6) ──

    pub async fn list_skills(&self) -> Result<Vec<SkillRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/skills")
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn get_skill(&self, slug: &str) -> Result<SkillRow, ClientError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/skills/{}", url_encode(slug)),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn create_skill(&self, body: &CreateSkill) -> Result<SkillRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/skills")
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn update_skill(
        &self,
        slug: &str,
        body: &UpdateSkill,
    ) -> Result<SkillRow, ClientError> {
        let resp = self
            .request(
                reqwest::Method::PATCH,
                &format!("/api/skills/{}", url_encode(slug)),
            )
            .await
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn delete_skill(&self, slug: &str) -> Result<(), ClientError> {
        let resp = self
            .request(
                reqwest::Method::DELETE,
                &format!("/api/skills/{}", url_encode(slug)),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(())
    }

    // ── Memory (#WI-7) ──

    pub async fn create_memory(
        &self,
        body: &CreateMemory,
        operation_id: Option<&str>,
    ) -> Result<MemoryRow, ClientError> {
        let mut req = self
            .request(reqwest::Method::POST, "/api/memory")
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn get_memory(&self, id: &str) -> Result<MemoryRow, ClientError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/memory/{}", url_encode(id)),
            )
            .await
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn update_memory(
        &self,
        id: &str,
        body: &UpdateMemory,
        operation_id: Option<&str>,
    ) -> Result<MemoryRow, ClientError> {
        let mut req = self
            .request(
                reqwest::Method::PATCH,
                &format!("/api/memory/{}", url_encode(id)),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn delete_memory(
        &self,
        id: &str,
        observed_revision: i64,
        operation_id: Option<&str>,
    ) -> Result<(), ClientError> {
        let path = format!(
            "/api/memory/{}?observed_revision={}",
            url_encode(id),
            observed_revision
        );
        let mut req = self.request(reqwest::Method::DELETE, &path).await;
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(())
    }

    pub async fn search_memory(&self, query: &MemoryQuery) -> Result<Vec<MemoryRow>, ClientError> {
        let limit = query.limit.map(|l| l.to_string());
        let mut params: Vec<(&str, &str)> = Vec::new();
        params.push(("q", &query.q));
        if let Some(ref kind) = query.kind {
            params.push(("kind", kind));
        }
        if let Some(ref subject_type) = query.subject_type {
            params.push(("subject_type", subject_type));
        }
        if let Some(ref subject_id) = query.subject_id {
            params.push(("subject_id", subject_id));
        }
        if let Some(ref limit_string) = limit {
            params.push(("limit", limit_string));
        }
        let mut req = self
            .request(reqwest::Method::GET, "/api/memory/search")
            .await;
        if !params.is_empty() {
            req = req.query(&params);
        }
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }

    pub async fn find_similar_tasks(
        &self,
        query: &SimilarTaskQuery,
    ) -> Result<Vec<SimilarTaskRow>, ClientError> {
        let limit = query.limit.unwrap_or(10).to_string();
        let mut req = self
            .request(reqwest::Method::GET, "/api/tasks/similar")
            .await;
        req = req.query(&[("q", query.title.as_str()), ("limit", limit.as_str())]);
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp.json().await?)
    }
}

// ── Types (mirrors server model.rs) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: String,
    pub display_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_at: Option<String>,
    pub end_at: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub depends: String,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub parallelizable: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub allows_parallel: bool,
    pub abandonability: f64,
    pub status: String,
    pub habit_id: Option<String>,
    pub ical_uid: Option<String>,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub user_edited: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub fixed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub habit_step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_total: Option<i64>,
    #[serde(default)]
    pub quantity_done: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_from_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_quantity_total: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_minutes: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTask {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    pub end_at: String,
    pub avg_minutes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ical_uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_done: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub original_quantity_total: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateTask {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_edited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub habit_step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_done: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quantity_unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub original_quantity_total: Option<i64>,
}

#[derive(Debug, Default)]
pub struct TaskQuery {
    pub status: Option<String>,
    pub from: Option<String>,
    pub until: Option<String>,
    pub no_overdue: Option<bool>,
    pub habit_id: Option<String>,
    pub ical_uid: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitRow {
    pub id: String,
    #[serde(default)]
    pub display_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub recurrence: String,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub parallelizable: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub allows_parallel: bool,
    pub abandonability: f64,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub active: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub fixed: bool,
    #[serde(default)]
    pub window_mode: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateHabit {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub recurrence: String,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub window_mode: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateHabit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fixed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub window_mode: Option<String>,
}

/// A scheduled span for a habit (#303 / #503).
///
/// Effect depends on `habits.active`:
/// - active habit: span dates suppress task generation (a pause).
/// - disabled habit: span dates enable task generation (an activation window).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitScheduledSpanRow {
    pub id: String,
    pub habit_id: String,
    pub start_date: String,
    pub end_date: String,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct CreateHabitScheduledSpan {
    pub start_date: String,
    pub end_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A step of a multi-step habit (#95).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitStepRow {
    pub id: String,
    pub habit_id: String,
    pub position: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub parallelizable: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub allows_parallel: bool,
    pub abandonability: f64,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub fixed: bool,
    pub depends_on: String,
    pub created_at: String,
}

/// Input element for `PUT /api/habits/:id/steps` (bulk replace, #95).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitStepInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub position: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub start_time: String,
    pub end_time: String,
    pub avg_minutes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sigma_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallelizable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allows_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abandonability: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<bool>,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Habit detail response: the habit row plus its steps (#95).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitDetail {
    #[serde(flatten)]
    pub habit: HabitRow,
    pub steps: Vec<HabitStepRow>,
}

/// Request body for `POST /api/habits/{id}/estimate` (#919).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HabitEstimateRequest {
    #[serde(default)]
    pub detect_outliers: bool,
    #[serde(default)]
    pub apply: bool,
}

/// One completed task observation included in a habit estimate (#919).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitEstimateSample {
    pub task_id: String,
    pub title: String,
    pub actual_minutes: i64,
    pub excluded: bool,
}

/// Estimate result for a single habit step (#919).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitEstimateStep {
    pub step_id: String,
    pub title: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub sample_count: usize,
    pub excluded_count: usize,
    pub applied: bool,
}

/// Response from `POST /api/habits/{id}/estimate` (#919).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HabitEstimateResult {
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub sample_count: usize,
    pub excluded_count: usize,
    pub samples: Vec<HabitEstimateSample>,
    pub steps: Vec<HabitEstimateStep>,
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub habit: Option<HabitRow>,
}

/// A node on a dependency witness path (#355).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyNode {
    pub id: String,
    pub title: String,
}

/// A redundant (composite) dependency edge with a witness path (#355).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedundantDependency {
    pub from: String,
    pub from_title: String,
    pub to: String,
    pub to_title: String,
    pub via: Vec<DependencyNode>,
}

/// Response for `GET /api/tasks/dependency-analysis` and the habit step
/// variant (#355).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyAnalysisResponse {
    pub redundant: Vec<RedundantDependency>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SchedulePreviewRequest {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveScheduleRequest {
    pub entries: Vec<ScheduleEntry>,
    #[serde(default)]
    pub mark_scheduled_task_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRow {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub schedule: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub task_id: String,
    pub start_at: String,
    pub end_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GenerateSchedule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    #[serde(default = "default_sleep")]
    pub sleep: String,
}

#[allow(dead_code)]
fn default_sleep() -> String {
    "recommended".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Reschedule {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: String,
}

#[derive(Debug, Serialize)]
pub struct MoveEntry {
    pub start_at: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveEntryResponse {
    pub task_id: String,
    pub start_at: String,
    pub end_at: String,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRow {
    pub id: i64,
    pub jti: String,
    pub scope: String,
    pub label: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenCreateResponse {
    pub id: i64,
    pub token: String,
    pub scope: String,
    pub label: Option<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
}

// ── Sync types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSettingsResponse {
    #[serde(with = "takusu_util::bool_compat", default)]
    pub enabled: bool,
    pub calendar_id: String,
    pub client_id: String,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub has_client_secret: bool,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub has_refresh_token: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAllGcalResponse {
    pub deleted: usize,
    pub failed: Vec<DeleteAllGcalFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAllGcalFailure {
    pub task_id: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct UpdateSyncSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

// ── Skill types (#WI-6) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRow {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(with = "takusu_util::bool_compat", default)]
    pub built_in: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSkill {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub built_in: Option<bool>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateSkill {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

// ── Memory types (#WI-7) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: String,
    pub kind: String,
    pub key: String,
    pub content: String,
    #[serde(default)]
    pub subject_type: String,
    #[serde(default)]
    pub subject_id: String,
    pub source: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateMemory {
    pub kind: String,
    pub key: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(default)]
    pub upsert: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UpdateMemory {
    pub observed_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MemoryQuery {
    pub q: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarTaskRow {
    pub task_id: String,
    pub display_id: i64,
    pub title: String,
    pub avg_minutes: i64,
    pub sigma_minutes: i64,
    pub actual_minutes: Option<i64>,
    pub completed_at: Option<String>,
    pub similarity: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SimilarTaskQuery {
    #[serde(rename = "q")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

// ── Active-session progress management (#WI-9) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskWorkSessionRow {
    pub id: String,
    pub task_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEventRow {
    pub id: String,
    pub task_id: String,
    pub at: String,
    pub quantity_done: Option<i64>,
    pub delta_quantity: Option<i64>,
    pub active_minutes: i64,
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RecordProgress {
    pub quantity_done: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressResult {
    pub task: TaskRow,
    pub event: Option<ProgressEventRow>,
    #[serde(default)]
    pub suggests_completion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub task: TaskRow,
    pub open_session: Option<TaskWorkSessionRow>,
    pub sessions: Vec<TaskWorkSessionRow>,
    pub events: Vec<ProgressEventRow>,
    pub total_active_minutes: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SplitTask {
    pub retained_quantity: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_dependency: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitResult {
    pub original: TaskRow,
    pub remainder: TaskRow,
}

// ── Settings types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsResponse {
    pub tz: String,
    pub sleep_start: String,
    pub sleep_end: String,
    /// #459: 1 日の快適な作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    pub comfortable_minutes: Option<i64>,
    /// #459: 1 日の最大作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    pub maximum_minutes: Option<i64>,
    /// 使用する solver。`"sa"` / `"priority"` / `"auto"`。空または不明な場合は `auto`。
    #[serde(default)]
    pub solver: String,
    /// 求解時間の上限（ミリ秒）。`None` または `0` の場合は制限なし。
    #[serde(default)]
    pub time_budget_ms: Option<i64>,
    /// 乱数シード。`None` の場合は決定的なデフォルト。
    #[serde(default)]
    pub seed: Option<i64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start する。
    #[serde(with = "takusu_util::bool_compat", default)]
    pub warm_start: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct UpdateSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sleep_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sleep_end: Option<String>,
    /// #459: 1 日の快適な作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comfortable_minutes: Option<i64>,
    /// #459: 1 日の最大作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_minutes: Option<i64>,
    /// 使用する solver。`"sa"` / `"priority"` / `"auto"`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solver: Option<String>,
    /// 求解時間の上限（ミリ秒）。`None` または `0` で制限なし。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_budget_ms: Option<i64>,
    /// 乱数シード。`None` でデフォルト。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start する。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warm_start: Option<bool>,
}
