//! Versioned HTTP transport for an in-process Agent.
//!
//! The Android host mounts this router next to the planner API. Keeping the
//! transport in the Agent crate lets the CLI, Android, and a future server
//! adapter share the exact same session and approval contract.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::llm::{LlmClient, build_llm_client};
use crate::{
    AgentConfig, AgentError, AgentSession, ApprovalRequest, ApprovalResult, InvalidArgsError,
    ToolError, TurnEvent, TurnResult, UserInputAnswer, UserInputProvider, UserInputQuestion,
};

pub const API_VERSION: u8 = 1;

const MAX_SESSIONS: usize = 64;
const MAX_TURN_RESULTS: usize = 256;
const MAX_APPROVAL_RESULTS: usize = 256;

/// A simple FIFO-bounded `HashMap` to prevent unbounded growth of in-memory
/// caches.
struct BoundedMap<K, V> {
    capacity: usize,
    map: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Clone + Eq + std::hash::Hash, V> BoundedMap<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        self.map.get(key)
    }

    fn insert(&mut self, key: K, value: V) -> Option<V> {
        let new_key = !self.map.contains_key(&key);
        if new_key
            && self.capacity > 0
            && self.map.len() >= self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.map.remove(&oldest);
        }
        match self.map.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => Some(entry.insert(value)),
            std::collections::hash_map::Entry::Vacant(entry) => {
                self.order.push_back(entry.key().clone());
                entry.insert(value);
                None
            }
        }
    }

    fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        let value = self.map.remove(key)?;
        if let Some(pos) = self.order.iter().position(|k| key == (*k).borrow()) {
            self.order.remove(pos);
        }
        Some(value)
    }

    fn values(&self) -> impl Iterator<Item = &V> {
        self.map.values()
    }
}

/// Creates an AgentSession for an authenticated local user.
///
/// The host owns configuration and secrets. They never arrive in a session
/// creation request from JavaScript. The factory receives the current
/// `AgentConfig` so sessions can reflect runtime setting changes.
pub trait SessionFactory: Send + Sync {
    fn create(
        &self,
        config: &AgentConfig,
        token: Arc<RwLock<Arc<str>>>,
    ) -> Result<AgentSession, AgentError>;
}

impl<F> SessionFactory for F
where
    F: Fn(&AgentConfig, Arc<RwLock<Arc<str>>>) -> Result<AgentSession, AgentError> + Send + Sync,
{
    fn create(
        &self,
        config: &AgentConfig,
        token: Arc<RwLock<Arc<str>>>,
    ) -> Result<AgentSession, AgentError> {
        self(config, token)
    }
}

pub struct AgentApiState {
    pub token: Arc<RwLock<Arc<str>>>,
    pub factory: Arc<dyn SessionFactory>,
    user_input_provider: Arc<dyn UserInputProvider>,
    config: RwLock<AgentConfig>,
    sessions: Mutex<BoundedMap<String, Arc<AgentSession>>>,
    turn_results: Mutex<BoundedMap<(String, String, &'static str), TurnResultDto>>,
    approval_results: Mutex<BoundedMap<(String, String), ApprovalResultDto>>,
}

impl AgentApiState {
    pub fn new(
        token: impl AsRef<str>,
        factory: Arc<dyn SessionFactory>,
        user_input_provider: Arc<dyn UserInputProvider>,
        config: AgentConfig,
    ) -> Self {
        Self::new_with_token(
            Arc::new(RwLock::new(Arc::from(token.as_ref()))),
            factory,
            user_input_provider,
            config,
        )
    }

    pub fn new_with_token(
        token: Arc<RwLock<Arc<str>>>,
        factory: Arc<dyn SessionFactory>,
        user_input_provider: Arc<dyn UserInputProvider>,
        config: AgentConfig,
    ) -> Self {
        Self {
            token,
            factory,
            user_input_provider,
            config: RwLock::new(config),
            sessions: Mutex::new(BoundedMap::new(MAX_SESSIONS)),
            turn_results: Mutex::new(BoundedMap::new(MAX_TURN_RESULTS)),
            approval_results: Mutex::new(BoundedMap::new(MAX_APPROVAL_RESULTS)),
        }
    }

    fn session(&self, id: &str) -> Option<Arc<AgentSession>> {
        self.sessions.lock().ok()?.get(id).cloned()
    }
}

/// Provider that bridges a blocking `correct_asr` tool call to the mobile client
/// through the agent HTTP API.
#[derive(Clone, Debug)]
pub struct ApiUserInputProvider {
    resolvers: Arc<Mutex<HashMap<String, oneshot::Sender<Vec<UserInputAnswer>>>>>,
    timeout: std::time::Duration,
}

const USER_INPUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

impl ApiUserInputProvider {
    pub fn new() -> Self {
        Self::with_timeout(USER_INPUT_TIMEOUT)
    }

    pub fn with_timeout(timeout: std::time::Duration) -> Self {
        Self {
            resolvers: Arc::new(Mutex::new(HashMap::new())),
            timeout,
        }
    }
}

impl Default for ApiUserInputProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl UserInputProvider for ApiUserInputProvider {
    async fn request(
        &self,
        call_id: &str,
        _questions: Vec<UserInputQuestion>,
    ) -> Result<Vec<UserInputAnswer>, ToolError> {
        let (tx, rx) = oneshot::channel();
        // Best-effort insert: a poisoned guard still holds the resolver map,
        // so recovering via `into_inner()` and inserting is safe and avoids
        // crashing the request on a prior panic.
        self.resolvers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(call_id.to_string(), tx);
        // Wait for the client to call the resolve endpoint, with a timeout to
        // avoid keeping stale resolvers in memory indefinitely.
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(answers)) => Ok(answers),
            Ok(Err(_)) | Err(_) => {
                self.resolvers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(call_id);
                Err(ToolError::Cancelled)
            }
        }
    }

    async fn resolve(&self, call_id: &str, answers: Vec<UserInputAnswer>) -> Result<(), ToolError> {
        let tx = self
            .resolvers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(call_id)
            .ok_or_else(|| {
                ToolError::InvalidArgs(InvalidArgsError::new(
                    "call_id",
                    format!("no pending user input request for {call_id}"),
                ))
            })?;
        tx.send(answers).map_err(|_| ToolError::Cancelled)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub version: u8,
    #[serde(flatten)]
    pub value: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub permissions: Option<crate::Permissions>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateSessionSettings {
    #[serde(default)]
    pub permissions: Option<crate::Permissions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRequest {
    pub text: String,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditTurnRequest {
    pub text: String,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevertRequest {
    pub after_user: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateAgentLlmSettings {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub provider: Option<crate::llm::LlmProviderKind>,
    pub permissions: Option<crate::Permissions>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateAgentTtsSettings {
    pub backend: Option<takusu_audio::TtsBackend>,
    pub api_key: Option<String>,
    pub voice_id: Option<String>,
    pub language: Option<String>,
    pub sample_rate: Option<u32>,
    pub speed: Option<f32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateAgentAudioSettings {
    pub tts: Option<UpdateAgentTtsSettings>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateAgentSettings {
    pub llm: Option<UpdateAgentLlmSettings>,
    pub audio: Option<UpdateAgentAudioSettings>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TurnResultDto {
    pub text: String,
    pub changes: Vec<crate::ChangeReceipt>,
    pub schedule_dirty: bool,
    pub approval_request: Option<ApprovalRequest>,
}

impl From<TurnResult> for TurnResultDto {
    fn from(result: TurnResult) -> Self {
        Self {
            text: result.text,
            changes: result.changes,
            schedule_dirty: result.schedule_dirty,
            approval_request: result.approval_request,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionRequest {
    pub approve: bool,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputResolutionRequest {
    pub answers: Vec<UserInputAnswer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResultDto {
    pub id: String,
    pub approved: bool,
    pub changes: Vec<crate::ChangeReceipt>,
    pub schedule_dirty: bool,
}

impl From<ApprovalResult> for ApprovalResultDto {
    fn from(result: ApprovalResult) -> Self {
        Self {
            id: result.id,
            approved: result.approved,
            changes: result.changes,
            schedule_dirty: result.schedule_dirty,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesResponse {
    pub audio_input: bool,
    pub tts: bool,
    pub approvals: bool,
    pub user_input: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
}

pub fn router(state: Arc<AgentApiState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/capabilities", get(capabilities))
        .route("/settings", put(update_settings))
        .route("/sessions", post(create_session))
        .route("/sessions/{id}/turns", post(run_turn))
        .route("/sessions/{id}/turns/stream", post(run_turn_stream))
        .route(
            "/sessions/{id}/turns/{turn_index}/edit/stream",
            post(edit_turn_stream),
        )
        .route(
            "/sessions/{id}/turns/{turn_index}/revert",
            post(revert_turn),
        )
        .route("/sessions/{id}/settings", put(update_session_settings))
        .route("/sessions/{id}/approval", get(get_approval))
        .route(
            "/sessions/{id}/approvals/{approval_id}",
            post(resolve_approval),
        )
        .route(
            "/sessions/{id}/tool-calls/{call_id}/user-input",
            post(resolve_user_input),
        )
        .route("/sessions/{id}", delete(delete_session))
        .with_state(state)
}

async fn health(State(state): State<Arc<AgentApiState>>, headers: HeaderMap) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    Json(Versioned {
        version: API_VERSION,
        value: HealthResponse { ok: true },
    })
    .into_response()
}

async fn capabilities(State(state): State<Arc<AgentApiState>>, headers: HeaderMap) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    Json(Versioned {
        version: API_VERSION,
        value: CapabilitiesResponse {
            audio_input: true,
            tts: true,
            approvals: true,
            user_input: true,
        },
    })
    .into_response()
}

async fn update_settings(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Json(body): Json<Versioned<UpdateAgentSettings>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let config = state.config.read().await;
    let mut new_config = config.clone();
    drop(config);

    if let Some(llm) = body.value.llm {
        if let Some(base_url) = llm.base_url {
            new_config.llm.base_url = base_url;
        }
        if let Some(model) = llm.model {
            new_config.llm.model = model;
        }
        if let Some(api_key) = llm.api_key {
            new_config.llm.api_key = api_key;
        }
        if let Some(provider) = llm.provider {
            new_config.llm.provider = provider;
        }
        if let Some(permissions) = llm.permissions {
            new_config.llm.permissions = permissions;
        }
    }
    if let Some(audio) = body.value.audio
        && let Some(tts) = audio.tts
    {
        if let Some(backend) = tts.backend {
            new_config.audio.tts.backend = backend;
        }
        if let Some(api_key) = tts.api_key {
            new_config.audio.tts.api_key = api_key;
        }
        if let Some(voice_id) = tts.voice_id {
            new_config.audio.tts.voice_id = voice_id;
        }
        if let Some(language) = tts.language {
            new_config.audio.tts.language = language;
        }
        if let Some(sample_rate) = tts.sample_rate {
            new_config.audio.tts.sample_rate = sample_rate;
        }
        if let Some(speed) = tts.speed {
            new_config.audio.tts.speed = Some(speed);
        }
    }

    let new_llm: Arc<dyn LlmClient + Send + Sync> = match build_llm_client(&new_config.llm) {
        Ok(client) => client,
        Err(e) => return agent_error(AgentError::Llm(e)),
    };

    *state.config.write().await = new_config.clone();

    let sessions: Vec<_> = state
        .sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    for session in sessions {
        if let Err(e) = session.apply_config(&new_config, new_llm.clone()).await {
            tracing::warn!(session_id = %session.session_id(), error = %e, "apply_config failed for session");
        }
    }

    tracing::info!(model = %new_config.llm.model, backend = %new_config.audio.tts.backend, "agent settings updated");
    Json(Versioned {
        version: API_VERSION,
        value: serde_json::json!({ "ok": true }),
    })
    .into_response()
}

async fn create_session(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    body: Option<Json<Versioned<CreateSessionRequest>>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if let Some(Json(ref body)) = body
        && body.version != API_VERSION
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let config = state.config.read().await;
    let session = match state.factory.create(&config, state.token.clone()) {
        Ok(session) => session,
        Err(error) => return agent_error(error),
    };
    if let Some(Json(body)) = body
        && let Some(permissions) = body.value.permissions
        && let Err(e) = session.set_session_permissions(permissions)
    {
        return agent_error(e);
    }
    let id = session.session_id().to_string();
    let session = Arc::new(session);
    let mut sessions = match state.sessions.lock() {
        Ok(guard) => guard,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    sessions.insert(id.clone(), session);
    tracing::info!(session_id = %id, "agent session created via API");
    Json(Versioned {
        version: API_VERSION,
        value: CreateSessionResponse { session_id: id },
    })
    .into_response()
}

async fn run_turn(
    State(state): State<Arc<AgentApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Versioned<TurnRequest>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION || body.value.text.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let session = match state.session(&id) {
        Some(session) => session,
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    let key = body.value.idempotency_key.filter(|key| !key.is_empty());
    if let Some(key) = &key
        && let Ok(results) = state.turn_results.lock()
        && let Some(result) = results.get(&(id.clone(), key.clone(), "turn"))
    {
        return Json(Versioned {
            version: API_VERSION,
            value: result.clone(),
        })
        .into_response();
    }
    let result = match session.run_turn(&body.value.text).await {
        Ok(result) => {
            tracing::info!(session_id = %id, text_len = result.text.len(), changes = result.changes.len(), schedule_dirty = result.schedule_dirty, "agent turn completed");
            TurnResultDto::from(result)
        }
        Err(error) => {
            tracing::error!(session_id = %id, error = %error, "agent turn failed");
            return agent_error(error);
        }
    };
    if let Some(key) = key
        && let Ok(mut results) = state.turn_results.lock()
    {
        results.insert((id, key, "turn"), result.clone());
    }
    Json(Versioned {
        version: API_VERSION,
        value: result,
    })
    .into_response()
}

/// A server-sent event exchanged with streaming clients.
///
/// Turn events are serialized in their original shape so that existing clients
/// continue to parse them directly. TTS blocks are emitted as a separate
/// `{ "type": "TtsBlock", "data": "..." }` payload that is distinguished by
/// its top-level `type` field.
#[derive(Debug, Clone)]
enum StreamEvent {
    Turn(TurnEvent),
    TtsBlock(String),
}

impl StreamEvent {
    fn to_json(&self) -> String {
        match self {
            StreamEvent::Turn(event) => {
                serde_json::to_string(event).expect("TurnEvent should always serialize to JSON")
            }
            StreamEvent::TtsBlock(text) => {
                serde_json::json!({ "type": "TtsBlock", "data": text }).to_string()
            }
        }
    }
}

async fn run_turn_stream(
    State(state): State<Arc<AgentApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Versioned<TurnRequest>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION || body.value.text.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let session = match state.session(&id) {
        Some(session) => session,
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    let key = body.value.idempotency_key.filter(|key| !key.is_empty());
    if let Some(key) = &key
        && let Ok(results) = state.turn_results.lock()
        && let Some(result) = results.get(&(id.clone(), key.clone(), "turn"))
    {
        let cached = TurnResult {
            text: result.text.clone(),
            changes: result.changes.clone(),
            schedule_dirty: result.schedule_dirty,
            approval_request: result.approval_request.clone(),
        };
        let event = StreamEvent::Turn(TurnEvent::Done(cached));
        let json = event.to_json();
        let stream = futures_util::stream::once(std::future::ready(Ok::<_, Infallible>(
            Event::default().data(json),
        )));
        return Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response();
    }
    let text = body.value.text.clone();
    let (tx, rx) = unbounded_channel::<StreamEvent>();
    let tx_closed = tx.clone();
    let session2 = session.clone();
    let id2 = id.clone();
    let key2 = key.clone();
    let state2 = Arc::clone(&state);
    tokio::spawn(async move {
        tokio::select! {
            _ = tx_closed.closed() => {}
            result = session2.run_turn_stream(&text, |event| {
                let _ = tx.send(StreamEvent::Turn(event));
            }, |block| {
                let _ = tx.send(StreamEvent::TtsBlock(block));
            }) => {
                match result {
                    Ok(result) => {
                        tracing::info!(session_id = %id2, text_len = result.text.len(), changes = result.changes.len(), schedule_dirty = result.schedule_dirty, "agent turn stream completed");
                        let _ = tx.send(StreamEvent::Turn(TurnEvent::Done(result.clone())));
                        if let Some(key) = key2
                            && let Ok(mut results) = state2.turn_results.lock()
                        {
                            results.insert((id2, key, "turn"), TurnResultDto::from(result));
                        }
                    }
                    Err(error) => {
                        tracing::error!(session_id = %id2, error = %error, "agent turn stream failed");
                        let _ = tx.send(StreamEvent::Turn(TurnEvent::Error(error.to_string())));
                    }
                }
            }
        }
    });
    let stream = UnboundedReceiverStream::new(rx).map(|event| {
        let json = event.to_json();
        Ok::<_, Infallible>(Event::default().data(json))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn edit_turn_stream(
    State(state): State<Arc<AgentApiState>>,
    Path((id, turn_index)): Path<(String, usize)>,
    headers: HeaderMap,
    Json(body): Json<Versioned<EditTurnRequest>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION || body.value.text.trim().is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let session = match state.session(&id) {
        Some(session) => session,
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    let key = body.value.idempotency_key.filter(|key| !key.is_empty());
    if let Some(key) = &key
        && let Ok(results) = state.turn_results.lock()
        && let Some(result) = results.get(&(id.clone(), key.clone(), "edit"))
    {
        let cached = TurnResult {
            text: result.text.clone(),
            changes: result.changes.clone(),
            schedule_dirty: result.schedule_dirty,
            approval_request: result.approval_request.clone(),
        };
        let event = StreamEvent::Turn(TurnEvent::Done(cached));
        let json = event.to_json();
        let stream = futures_util::stream::once(std::future::ready(Ok::<_, Infallible>(
            Event::default().data(json),
        )));
        return Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response();
    }
    let text = body.value.text.clone();
    let (tx, rx) = unbounded_channel::<StreamEvent>();
    let tx_closed = tx.clone();
    let session2 = session.clone();
    let id2 = id.clone();
    let key2 = key.clone();
    let state2 = Arc::clone(&state);
    tokio::spawn(async move {
        tokio::select! {
            _ = tx_closed.closed() => {}
            result = session2.edit_turn_stream(turn_index, &text, |event| {
                let _ = tx.send(StreamEvent::Turn(event));
            }, |block| {
                let _ = tx.send(StreamEvent::TtsBlock(block));
            }) => {
                match result {
                    Ok(result) => {
                        tracing::info!(session_id = %id2, turn_index, text_len = result.text.len(), changes = result.changes.len(), schedule_dirty = result.schedule_dirty, "agent edit turn stream completed");
                        let _ = tx.send(StreamEvent::Turn(TurnEvent::Done(result.clone())));
                        if let Some(key) = key2
                            && let Ok(mut results) = state2.turn_results.lock()
                        {
                            results.insert((id2, key, "edit"), TurnResultDto::from(result));
                        }
                    }
                    Err(error) => {
                        tracing::error!(session_id = %id2, turn_index, error = %error, "agent edit turn stream failed");
                        let _ = tx.send(StreamEvent::Turn(TurnEvent::Error(error.to_string())));
                    }
                }
            }
        }
    });
    let stream = UnboundedReceiverStream::new(rx).map(|event| {
        let json = event.to_json();
        Ok::<_, Infallible>(Event::default().data(json))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn revert_turn(
    State(state): State<Arc<AgentApiState>>,
    Path((id, turn_index)): Path<(String, usize)>,
    headers: HeaderMap,
    Json(body): Json<Versioned<RevertRequest>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let session = match state.session(&id) {
        Some(session) => session,
        None => return StatusCode::NOT_FOUND.into_response(),
    };
    match session
        .truncate_history(turn_index, body.value.after_user)
        .await
    {
        Ok(()) => {
            tracing::info!(session_id = %id, turn_index, after_user = body.value.after_user, "agent history reverted");
            Json(Versioned {
                version: API_VERSION,
                value: serde_json::json!({ "ok": true }),
            })
            .into_response()
        }
        Err(error) => {
            tracing::error!(session_id = %id, turn_index, error = %error, "agent history revert failed");
            agent_error(error)
        }
    }
}

async fn update_session_settings(
    State(state): State<Arc<AgentApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Versioned<UpdateSessionSettings>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(session) = state.session(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(permissions) = body.value.permissions
        && let Err(e) = session.set_session_permissions(permissions)
    {
        return agent_error(e);
    }
    tracing::info!(session_id = %id, "agent session settings updated");
    Json(Versioned {
        version: API_VERSION,
        value: serde_json::json!({ "ok": true }),
    })
    .into_response()
}

async fn get_approval(
    State(state): State<Arc<AgentApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    let Some(session) = state.session(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match session.pending_approval() {
        Some(approval) => Json(Versioned {
            version: API_VERSION,
            value: Some(approval),
        })
        .into_response(),
        None => Json(Versioned {
            version: API_VERSION,
            value: Option::<ApprovalRequest>::None,
        })
        .into_response(),
    }
}

async fn resolve_approval(
    State(state): State<Arc<AgentApiState>>,
    Path((id, approval_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Versioned<ApprovalDecisionRequest>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let key = body
        .value
        .idempotency_key
        .clone()
        .unwrap_or_else(|| approval_id.clone());
    if let Ok(results) = state.approval_results.lock()
        && let Some(result) = results.get(&(id.clone(), key.clone()))
    {
        return Json(Versioned {
            version: API_VERSION,
            value: result.clone(),
        })
        .into_response();
    }
    let Some(session) = state.session(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let result = match session
        .resolve_approval(&approval_id, body.value.approve)
        .await
    {
        Ok(result) => {
            tracing::info!(session_id = %id, approval_id = %approval_id, approved = result.approved, changes = result.changes.len(), "approval resolved");
            ApprovalResultDto::from(result)
        }
        Err(error) => {
            tracing::error!(session_id = %id, approval_id = %approval_id, error = %error, "approval resolution failed");
            return agent_error(error);
        }
    };
    if let Ok(mut results) = state.approval_results.lock() {
        results.insert((id, key), result.clone());
    }
    Json(Versioned {
        version: API_VERSION,
        value: result,
    })
    .into_response()
}

async fn resolve_user_input(
    State(state): State<Arc<AgentApiState>>,
    Path((id, call_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Versioned<UserInputResolutionRequest>>,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    if body.version != API_VERSION {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if state.session(&id).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Tool-call ids for user input are `{session_id}-{uuid}`. Reject any
    // call_id that does not begin with the requested session id followed by
    // the separator, which prevents resolving user input from another session.
    if !call_id.starts_with(&format!("{id}-")) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Err(error) = state
        .user_input_provider
        .resolve(&call_id, body.value.answers)
        .await
    {
        tracing::error!(session_id = %id, call_id = %call_id, error = %error, "user input resolution failed");
        return agent_error(AgentError::Tool(error));
    }
    tracing::info!(session_id = %id, call_id = %call_id, "user input resolved");
    Json(Versioned {
        version: API_VERSION,
        value: serde_json::json!({ "ok": true }),
    })
    .into_response()
}

async fn delete_session(
    State(state): State<Arc<AgentApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(status) = auth_token(&state, &headers).await {
        return status.into_response();
    }
    match state.sessions.lock() {
        Ok(mut sessions) => {
            if sessions.remove(&id).is_some() {
                tracing::info!(session_id = %id, "agent session deleted");
                StatusCode::NO_CONTENT.into_response()
            } else {
                StatusCode::NOT_FOUND.into_response()
            }
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {token}"))
}

async fn auth_token(state: &AgentApiState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let token = state.token.read().await;
    if !authorized(headers, &token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

fn agent_error(error: AgentError) -> Response {
    let status = match &error {
        AgentError::Tool(crate::ToolError::InvalidArgs(_))
        | AgentError::Tool(crate::ToolError::NotFound(_)) => StatusCode::BAD_REQUEST,
        AgentError::Tool(crate::ToolError::Conflict(_)) => StatusCode::CONFLICT,
        AgentError::Tool(crate::ToolError::Cancelled) => StatusCode::GONE,
        AgentError::TooManyToolCalls => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::BAD_GATEWAY,
    };
    (
        status,
        Json(serde_json::json!({
            "version": API_VERSION,
            "error": error.to_string(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::Json;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
    use tokio::sync::RwLock;

    use super::*;
    use crate::ToolRegistry;

    struct StubFactory;

    impl SessionFactory for StubFactory {
        fn create(
            &self,
            _config: &AgentConfig,
            _token: Arc<RwLock<Arc<str>>>,
        ) -> Result<AgentSession, AgentError> {
            Err(AgentError::Tool(ToolError::InvalidArgs(
                InvalidArgsError::no_field("stub"),
            )))
        }
    }

    fn auth_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        headers
    }

    fn make_state(token: &str) -> Arc<AgentApiState> {
        Arc::new(AgentApiState::new(
            token,
            Arc::new(StubFactory),
            Arc::new(ApiUserInputProvider::new()),
            AgentConfig::default(),
        ))
    }

    #[tokio::test]
    async fn api_user_input_resolve_returns_answers() {
        let provider = ApiUserInputProvider::with_timeout(Duration::from_secs(60));
        let questions = vec![UserInputQuestion {
            text: "kore".into(),
            purpose: "test".into(),
        }];
        let request = provider.request("call-1", questions.clone());
        let resolve = async {
            provider
                .resolve(
                    "call-1",
                    vec![UserInputAnswer {
                        text: "これ".into(),
                    }],
                )
                .await
                .unwrap();
        };
        let (answers, ()) = tokio::join!(request, resolve);
        assert_eq!(answers.unwrap()[0].text, "これ");
    }

    #[tokio::test]
    async fn api_user_input_resolve_unknown_request_fails() {
        let provider = ApiUserInputProvider::new();
        let result = provider
            .resolve(
                "unknown",
                vec![UserInputAnswer {
                    text: "ignored".into(),
                }],
            )
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArgs(_))));
    }

    #[tokio::test]
    async fn api_user_input_request_times_out_and_cleans_map() {
        let provider = ApiUserInputProvider::with_timeout(Duration::from_millis(10));
        let result = provider.request("timeout-call", vec![]).await;
        assert!(matches!(result, Err(ToolError::Cancelled)));
    }

    #[tokio::test]
    async fn api_user_input_resolve_to_dropped_receiver_is_cancelled() {
        let provider = ApiUserInputProvider::new();
        let (tx, rx) = oneshot::channel();
        provider
            .resolvers
            .lock()
            .unwrap()
            .insert("dropped".to_string(), tx);
        // Drop the receiver before resolve is called; this simulates a session
        // ending or a turn being cancelled while the user-input request is pending.
        drop(rx);
        let result = provider
            .resolve(
                "dropped",
                vec![UserInputAnswer {
                    text: "ignored".into(),
                }],
            )
            .await;
        assert!(matches!(result, Err(ToolError::Cancelled)));
    }

    struct NullLlm;

    #[async_trait::async_trait]
    impl crate::llm::LlmClient for NullLlm {
        async fn chat(
            &self,
            _messages: &[crate::llm::Message],
            _tools: &[crate::tool::OpenAITool],
        ) -> Result<crate::llm::LlmResponse, crate::llm::LlmError> {
            Ok(crate::llm::LlmResponse {
                content: crate::llm::LlmResponseContent::Text("ok".into()),
                prompt_tokens: None,
                finish_reason: Some(crate::llm::FinishReason::Stop),
            })
        }
    }

    fn make_session_state(token: &str) -> (Arc<AgentApiState>, Arc<ApiUserInputProvider>, String) {
        let session = AgentSession::new(AgentConfig::default(), ToolRegistry::new(), NullLlm);
        let id = session.session_id().to_string();
        let provider = Arc::new(ApiUserInputProvider::new());
        let state_provider: Arc<dyn UserInputProvider> =
            Arc::<ApiUserInputProvider>::clone(&provider);
        let state = Arc::new(AgentApiState::new(
            token,
            Arc::new(StubFactory),
            state_provider,
            AgentConfig::default(),
        ));
        state
            .sessions
            .lock()
            .unwrap()
            .insert(id.clone(), Arc::new(session));
        (state, provider, id)
    }

    #[tokio::test]
    async fn resolve_user_input_accepts_same_session_call_id() {
        let (state, provider, id) = make_session_state("test-token");
        let call_id = format!("{}-{}", id, uuid::Uuid::now_v7());
        let (tx, rx) = oneshot::channel();
        provider
            .resolvers
            .lock()
            .unwrap()
            .insert(call_id.clone(), tx);

        let body = Versioned {
            version: API_VERSION,
            value: UserInputResolutionRequest {
                answers: vec![UserInputAnswer {
                    text: "これ".into(),
                }],
            },
        };
        let res = resolve_user_input(
            State(state),
            Path((id, call_id)),
            auth_headers("test-token"),
            Json(body),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);

        let answers = rx.await.expect("resolver received answers");
        assert_eq!(answers[0].text, "これ");
    }

    #[tokio::test]
    async fn resolve_user_input_rejects_other_session_call_id() {
        let (state, provider, id) = make_session_state("test-token");
        let other_call_id = "some-other-session-id-tool-call";
        let (tx, rx) = oneshot::channel();
        provider
            .resolvers
            .lock()
            .unwrap()
            .insert(other_call_id.to_string(), tx);
        drop(rx);

        let body = Versioned {
            version: API_VERSION,
            value: UserInputResolutionRequest {
                answers: vec![UserInputAnswer {
                    text: "これ".into(),
                }],
            },
        };
        let res = resolve_user_input(
            State(state),
            Path((id, other_call_id.to_string())),
            auth_headers("test-token"),
            Json(body),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resolve_user_input_rejects_call_id_equal_to_session_id() {
        let (state, _provider, id) = make_session_state("test-token");

        let body = Versioned {
            version: API_VERSION,
            value: UserInputResolutionRequest {
                answers: vec![UserInputAnswer {
                    text: "これ".into(),
                }],
            },
        };
        let res = resolve_user_input(
            State(state),
            Path((id.clone(), id)),
            auth_headers("test-token"),
            Json(body),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resolve_user_input_rejects_call_id_without_separator() {
        let (state, _provider, id) = make_session_state("test-token");

        let body = Versioned {
            version: API_VERSION,
            value: UserInputResolutionRequest {
                answers: vec![UserInputAnswer {
                    text: "これ".into(),
                }],
            },
        };
        let res = resolve_user_input(
            State(state),
            Path((id.clone(), format!("{id}suffix"))),
            auth_headers("test-token"),
            Json(body),
        )
        .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_settings_preserves_api_key_when_omitted() {
        let state = make_state("test-token");
        {
            let mut config = state.config.write().await;
            config.llm.base_url = "http://old".into();
            config.llm.model = "old-model".into();
            config.llm.api_key = "secret".into();
            config.audio.tts.speed = Some(1.5);
        }

        let body = Versioned {
            version: API_VERSION,
            value: UpdateAgentSettings {
                llm: Some(UpdateAgentLlmSettings {
                    base_url: Some("http://new".into()),
                    model: Some("new-model".into()),
                    ..Default::default()
                }),
                audio: Some(UpdateAgentAudioSettings {
                    tts: Some(UpdateAgentTtsSettings {
                        sample_rate: Some(48000),
                        ..Default::default()
                    }),
                }),
            },
        };
        let res =
            update_settings(State(state.clone()), auth_headers("test-token"), Json(body)).await;
        assert_eq!(res.status(), StatusCode::OK);

        let config = state.config.read().await;
        assert_eq!(config.llm.base_url, "http://new");
        assert_eq!(config.llm.model, "new-model");
        assert_eq!(config.llm.api_key, "secret");
        assert_eq!(config.audio.tts.sample_rate, 48000);
        assert_eq!(config.audio.tts.speed, Some(1.5));
    }

    #[tokio::test]
    async fn update_settings_overwrites_api_key_when_provided() {
        let state = make_state("test-token");
        {
            let mut config = state.config.write().await;
            config.llm.api_key = "old".into();
        }

        let body = Versioned {
            version: API_VERSION,
            value: UpdateAgentSettings {
                llm: Some(UpdateAgentLlmSettings {
                    api_key: Some("new".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        };
        let res =
            update_settings(State(state.clone()), auth_headers("test-token"), Json(body)).await;
        assert_eq!(res.status(), StatusCode::OK);

        let config = state.config.read().await;
        assert_eq!(config.llm.api_key, "new");
    }

    #[tokio::test]
    async fn update_settings_propagates_to_existing_session() {
        let (state, _provider, id) = make_session_state("test-token");
        {
            let mut config = state.config.write().await;
            config.llm.model = "old-model".into();
        }

        let body = Versioned {
            version: API_VERSION,
            value: UpdateAgentSettings {
                llm: Some(UpdateAgentLlmSettings {
                    model: Some("new-model".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        };
        let res =
            update_settings(State(state.clone()), auth_headers("test-token"), Json(body)).await;
        assert_eq!(res.status(), StatusCode::OK);

        let config = state.config.read().await;
        assert_eq!(config.llm.model, "new-model");

        let session = state
            .sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .expect("session should exist");
        assert_eq!(session.config.read().unwrap().llm.model, "new-model");
    }

    #[tokio::test]
    async fn update_settings_rejects_wrong_token() {
        let state = make_state("test-token");
        let body = Versioned {
            version: API_VERSION,
            value: UpdateAgentSettings::default(),
        };
        let res = update_settings(State(state), auth_headers("wrong"), Json(body)).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
