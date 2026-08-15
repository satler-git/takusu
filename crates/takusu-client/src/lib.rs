//! # takusu-client — HTTP client for takusu REST API
//!
//! Provides types and a `Client` for interacting with the takusu REST API.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use takusu_types::{TimeOfDay, url_encode};
use tokio::sync::RwLock;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error {status}: {body}")]
    Api { status: u16, body: String },
    #[error("multiple open work sessions for task {0}")]
    MultipleOpenWorkSessions(String),
}

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

    /// Convert an HTTP response into either a successful `Response` or an
    /// `ClientError::Api` when the status code indicates failure (>= 400).
    async fn handle_response(resp: reqwest::Response) -> Result<reqwest::Response, ClientError> {
        let status = resp.status().as_u16();
        if status >= 400 {
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::Api { status, body });
        }
        Ok(resp)
    }

    // ── Health ──

    pub async fn health(&self) -> Result<String, ClientError> {
        let resp = self
            .http
            .get(format!("{}/health", self.base_url))
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.text().await?)
    }

    // ── Task ──

    pub async fn list_tasks(&self, query: &TaskQuery) -> Result<Vec<TaskRow>, ClientError> {
        let url = format!("{}/api/tasks", self.base_url);
        let token = self.token().await;
        let mut req = self.http.get(&url).bearer_auth(&*token);
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(s) = query.status {
            params.push(("status", s.to_string()));
        }
        if let Some(v) = query.from {
            params.push(("from", v.to_string()));
        }
        if let Some(v) = query.until {
            params.push(("until", v.to_string()));
        }
        if let Some(v) = query.no_overdue {
            params.push(("no_overdue", v.to_string()));
        }
        if let Some(ref v) = query.habit_id {
            params.push(("habit_id", v.clone()));
        }
        if let Some(ref v) = query.ical_uid {
            params.push(("ical_uid", v.clone()));
        }
        if let Some(ref v) = query.q {
            params.push(("q", v.clone()));
        }
        if let Some(n) = query.limit {
            params.push(("limit", n.to_string()));
        }
        if !params.is_empty() {
            req = req.query(&params);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn get_task(&self, id: &str) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(reqwest::Method::GET, &format!("/api/tasks/{encoded_id}"))
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn create_task(&self, body: &CreateTask) -> Result<TaskRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/tasks")
            .await
            .json(body)
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn delete_task(&self, id: &str) -> Result<(), ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(reqwest::Method::DELETE, &format!("/api/tasks/{encoded_id}"))
            .await
            .send()
            .await?;
        Self::handle_response(resp).await?;
        Ok(())
    }

    pub async fn start_work_session(
        &self,
        body: &StartWorkSession,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, ClientError> {
        let mut req = self
            .request(reqwest::Method::POST, "/api/work-sessions")
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn list_work_sessions(
        &self,
        task_id: Option<&str>,
    ) -> Result<Vec<WorkSessionRow>, ClientError> {
        let path = if let Some(id) = task_id {
            format!("/api/work-sessions?task_id={}", url_encode(id))
        } else {
            "/api/work-sessions".into()
        };
        let resp = self
            .request(reqwest::Method::GET, &path)
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// Return the single open work session for a task, or `None` if there is
    /// none. Returns an error if more than one open session is found, which
    /// indicates a data-integrity violation.
    pub async fn open_work_session_for_task(
        &self,
        task_id: &str,
    ) -> Result<Option<WorkSessionRow>, ClientError> {
        let sessions = self.list_work_sessions(Some(task_id)).await?;
        let open: Vec<_> = sessions
            .into_iter()
            .filter(|s| s.ended_at.is_none())
            .collect();
        if open.len() > 1 {
            return Err(ClientError::MultipleOpenWorkSessions(task_id.into()));
        }
        Ok(open.into_iter().next())
    }

    pub async fn get_work_session(&self, id: &str) -> Result<WorkSessionRow, ClientError> {
        let encoded_id = url_encode(id);
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/work-sessions/{encoded_id}"),
            )
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn pause_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/work-sessions/{encoded_id}/pause"),
            )
            .await
            .json(&serde_json::json!({}));
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn complete_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/work-sessions/{encoded_id}/complete"),
            )
            .await
            .json(&serde_json::json!({}));
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn record_work_session_progress(
        &self,
        id: &str,
        body: &RecordWorkSessionProgress,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionProgressResult, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/work-sessions/{encoded_id}/progress"),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn attach_work_session(
        &self,
        id: &str,
        body: &AttachWorkSession,
        operation_id: Option<&str>,
    ) -> Result<WorkSessionRow, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/work-sessions/{encoded_id}/attach"),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn convert_work_session(
        &self,
        id: &str,
        body: &ConvertWorkSession,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, ClientError> {
        let encoded_id = url_encode(id);
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/work-sessions/{encoded_id}/convert"),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    // ── Habit ──

    pub async fn list_habits(&self) -> Result<Vec<HabitRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/habits")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn create_habit(&self, body: &CreateHabit) -> Result<HabitRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/habits")
            .await
            .json(body)
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn list_all_habit_steps(&self) -> Result<Vec<HabitStepRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/habits/steps")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    // ── Schedule ──

    pub async fn preview_schedule(
        &self,
        body: &SchedulePreviewRequest,
    ) -> Result<SchedulePreviewResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/schedule/preview")
            .await
            .json(body)
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn get_schedule(&self) -> Result<ScheduleRow, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/schedule")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn reschedule(&self, body: &Reschedule) -> Result<ScheduleRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/schedule/reschedule")
            .await
            .json(body)
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn move_entry(
        &self,
        task_id: &str,
        body: &MoveEntry,
        operation_id: Option<&str>,
    ) -> Result<MoveEntryResponse, ClientError> {
        let mut req = self
            .request(
                reqwest::Method::PATCH,
                &format!("/api/schedule/entries/{}", url_encode(task_id)),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn clear_schedule(&self) -> Result<(), ClientError> {
        let resp = self
            .request(reqwest::Method::DELETE, "/api/schedule")
            .await
            .send()
            .await?;
        Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/tokens")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        Self::handle_response(resp).await?;
        Ok(())
    }

    // ── Sync (Google Calendar) ──

    pub async fn get_sync_settings(&self) -> Result<SyncSettingsResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/sync/settings")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn get_oauth_url(&self, redirect_uri: &str) -> Result<OAuthUrlResponse, ClientError> {
        let body = serde_json::json!({ "redirect_uri": redirect_uri });
        let resp = self
            .request(reqwest::Method::POST, "/api/sync/oauth/url")
            .await
            .json(&body)
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn oauth_callback(
        &self,
        code: &str,
        redirect_uri: Option<&str>,
    ) -> Result<OAuthCallbackResponse, ClientError> {
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn trigger_sync(&self) -> Result<TriggerSyncResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/sync/trigger")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn delete_all_gcal_events(&self) -> Result<DeleteAllGcalResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/sync/delete-all")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    // ── Settings ──

    pub async fn get_settings(&self) -> Result<SettingsResponse, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/settings")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    // ── Skills (#WI-6) ──

    pub async fn list_skills(&self) -> Result<Vec<SkillRow>, ClientError> {
        let resp = self
            .request(reqwest::Method::GET, "/api/skills")
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn create_skill(&self, body: &CreateSkill) -> Result<SkillRow, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/skills")
            .await
            .json(body)
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
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
        Self::handle_response(resp).await?;
        Ok(())
    }

    pub async fn search_memory(&self, query: &MemoryQuery) -> Result<Vec<MemoryRow>, ClientError> {
        let limit = query.limit.map(|l| l.to_string());
        let kind = query.kind.map(|k| k.to_string());
        let subject_type = query.subject_type.map(|st| st.to_string());
        let mut params: Vec<(&str, &str)> = Vec::new();
        params.push(("q", &query.q));
        if let Some(ref kind) = kind {
            params.push(("kind", kind));
        }
        if let Some(ref subject_type) = subject_type {
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// Read auto-injection retrieval (WI-4): matches `proper_noun` / `fact`
    /// memories whose `normalized_key` occurs as a substring of the utterance,
    /// plus per-kind counts, all computed server-side.
    ///
    /// The utterance is sent in the JSON body (POST), not a URL query, so long
    /// utterances do not hit proxy URI-length limits and the text is not
    /// committed to URL access logs.
    pub async fn injectable_memories(
        &self,
        query: &MemoryInjectionQuery,
    ) -> Result<MemoryInjectionResult, ClientError> {
        let resp = self
            .request(reqwest::Method::POST, "/api/memory/inject")
            .await
            .json(query)
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
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
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    // ── Task comments (WI-1) ──

    pub async fn list_comments(&self, task_id: &str) -> Result<Vec<CommentRow>, ClientError> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/api/tasks/{}/comments", url_encode(task_id)),
            )
            .await
            .send()
            .await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// Create a comment attributed to the user (public endpoint).
    pub async fn create_comment(
        &self,
        task_id: &str,
        body: &CreateComment,
        operation_id: Option<&str>,
    ) -> Result<CommentRow, ClientError> {
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/tasks/{}/comments", url_encode(task_id)),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    /// Create a comment attributed to the agent (agent-only endpoint).
    pub async fn create_agent_comment(
        &self,
        task_id: &str,
        body: &CreateComment,
        operation_id: Option<&str>,
    ) -> Result<CommentRow, ClientError> {
        let mut req = self
            .request(
                reqwest::Method::POST,
                &format!("/api/tasks/{}/comments/agent", url_encode(task_id)),
            )
            .await
            .json(body);
        if let Some(op_id) = operation_id {
            req = req.header("Idempotency-Key", op_id);
        }
        let resp = req.send().await?;
        let resp = Self::handle_response(resp).await?;
        Ok(resp.json().await?)
    }

    pub async fn delete_comment(&self, id: &str) -> Result<(), ClientError> {
        let resp = self
            .request(
                reqwest::Method::DELETE,
                &format!("/api/comments/{}", url_encode(id)),
            )
            .await
            .send()
            .await?;
        Self::handle_response(resp).await?;
        Ok(())
    }
}

// ── Shared domain types (re-exported from takusu-contracts) ──
//
// `TaskRow` / `CreateTask` / `HabitRow` / `ScheduleRow` / `MemoryRow` and the
// rest of the request/response domain types live in `takusu-contracts::model` so
// the server, client, and worker share a single definition (#1294). The
// `sqlx::FromRow` derives there are gated behind the `sqlx` feature, which this
// crate does not enable, so the re-exported types are plain serde structs here.
pub use takusu_contracts::model::*;

// ── Client-only request/response types ──

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

// ── Sync types ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSettingsResponse {
    #[serde(default)]
    pub enabled: bool,
    pub calendar_id: String,
    pub client_id: String,
    #[serde(default)]
    pub has_client_secret: bool,
    #[serde(default)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthUrlResponse {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCallbackResponse {
    #[serde(default)]
    pub refresh_token_set: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerSyncResponse {
    pub status: String,
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

// ── Settings response (client-only; UpdateSettings is shared) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsResponse {
    pub tz: String,
    pub sleep_start: TimeOfDay,
    pub sleep_end: TimeOfDay,
    /// #459: 1 日の快適な作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    pub comfortable_minutes: Option<i64>,
    /// #459: 1 日の最大作業時間（分）。`None` または `0` の場合はデフォルトを使う。
    pub maximum_minutes: Option<i64>,
    /// 使用する solver。`"sa"` / `"priority"` / `"auto"`。未設定の場合は `sa`。未知値はエラー。
    #[serde(with = "takusu_types::enum_serde", default)]
    pub solver: takusu_types::Solver,
    /// 求解時間の上限（ミリ秒）。`None` または `0` の場合は制限なし。
    #[serde(default)]
    pub time_budget_ms: Option<i64>,
    /// 乱数シード。`None` の場合は決定的なデフォルト。
    #[serde(default)]
    pub seed: Option<i64>,
    /// 前回スケジュールから priority/ALNS の初期解を warm start する。
    #[serde(default)]
    pub warm_start: bool,
}
