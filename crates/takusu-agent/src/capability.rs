//! Quick-action capability minting and authorization for the resident agent.
//!
//! Screen and notification actions receive a server-issued, one-shot
//! capability bound to a device, action, and expiry. The common authorization
//! endpoint consumes the capability and executes the action with a stable
//! operation ID derived from the capability ID, so retries replay the same
//! result instead of applying a second mutation.
//!
//! This is the Phase 1 immediate layer for `start`, `pause`, `progress`,
//! `complete`, and short `delay` (snooze) quick actions. It is intentionally
//! separate from the agent turn flow: it does not start an LLM session and it
//! never consults `Permissions` directly. The capability itself is the
//! authorization boundary.

use std::sync::Arc;

use indexmap::IndexMap;
use jiff::Timestamp as JiffTimestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use takusu_client::{Client, MoveEntry, RecordWorkSessionProgress, StartWorkSession, TaskRow};
use takusu_types::{Quantity, QuantityError, TaskStatus, Timestamp, minutes_between_ts};

use crate::presentation::{Presentation, WorkTransition, WorkTransitionKind};

/// Default lifetime of a quick-action capability.
pub const CAPABILITY_TTL_MINUTES: i64 = 5;

/// Maximum number of in-flight capabilities kept in memory.
pub const MAX_CAPABILITIES: usize = 256;

/// Largest quantity that can be represented exactly in both JSON numbers and
/// IEEE-754 doubles (i.e. `Number.MAX_SAFE_INTEGER` in JavaScript).
const MAX_SAFE_PROGRESS_QUANTITY: i64 = 9_007_199_254_740_991;

/// Trusted input path for an action. The server, not the client, decides the
/// path based on how the capability was issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputPath {
    ScreenCapability,
    NotificationCapability,
    ExplicitVoiceSession,
    AmbientWakeWord,
    PlainText,
}

/// A server-issued, one-shot action capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionCapability {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    pub device_id: String,
    pub action: String,
    pub input_path: InputPath,
    #[schemars(with = "String")]
    pub expires_at: JiffTimestamp,
    pub one_shot: bool,
}

/// Request to mint a quick-action capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityRequest {
    pub task_id: String,
    pub action: String,
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_done: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("capability expired")]
    Expired,
    #[error("capability already consumed")]
    Consumed,
    #[error("capability mismatch")]
    Mismatch,
    #[error("invalid action")]
    InvalidAction,
    #[error("task state does not allow this action")]
    InvalidTaskState,
    #[error("no open work session for this task")]
    NoOpenWorkSession,
    #[error("task is not scheduled")]
    NotScheduled,
    #[error("client error: {0}")]
    Client(#[from] takusu_client::ClientError),
    #[error("invalid quantity: {0}")]
    Quantity(#[from] takusu_types::QuantityError),
    #[error("quantity not provided")]
    MissingQuantity,
    #[error("snooze minutes not provided")]
    MissingSnooze,
    #[error("snooze minutes must be positive")]
    InvalidSnooze,
}

impl From<CapabilityError> for crate::AgentError {
    fn from(e: CapabilityError) -> Self {
        match e {
            CapabilityError::Client(err) => crate::AgentError::Client(err),
            CapabilityError::InvalidAction
            | CapabilityError::Quantity(_)
            | CapabilityError::MissingQuantity
            | CapabilityError::MissingSnooze
            | CapabilityError::InvalidSnooze => crate::AgentError::Tool(
                crate::ToolError::InvalidArgs(crate::InvalidArgsError::no_field(e.to_string())),
            ),
            _ => crate::AgentError::Tool(crate::ToolError::Conflict(e.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CapabilityRecord {
    pub request: CapabilityRequest,
    pub capability: ActionCapability,
    pub consumed: bool,
    pub result: Option<Presentation>,
    /// Original schedule start captured on the first delay attempt. Stored so
    /// retries move the entry to the same absolute target and the TTS detail
    /// can report the actual delay minutes.
    pub snooze_original_start: Option<Timestamp>,
    /// Pre-computed `original_start + snooze_minutes` target. Reusing this on
    /// retry makes `move_entry` idempotent without server-side idempotency keys.
    pub snooze_target: Option<Timestamp>,
}

/// In-memory capability store. Capabilities are short-lived and per-device; a
/// bounded in-memory map is sufficient for Phase 1.
#[derive(Debug, Default)]
pub struct CapabilityStore {
    records: tokio::sync::Mutex<IndexMap<String, Arc<tokio::sync::Mutex<CapabilityRecord>>>>,
}

impl CapabilityStore {
    pub fn new() -> Self {
        Self {
            records: tokio::sync::Mutex::new(IndexMap::new()),
        }
    }

    /// Store a freshly minted capability.
    ///
    /// When the store is full, evict the oldest entry that is expired or already
    /// consumed; if none exists, evict the oldest *unlocked* entry. Locked
    /// records (currently being authorized) are never evicted; if every record
    /// is locked the oldest record is removed as a last resort. This bounds
    /// memory while avoiding dropping in-flight capabilities in the common case.
    pub async fn insert(&self, request: CapabilityRequest, capability: ActionCapability) {
        let now = JiffTimestamp::now();
        let mut records = self.records.lock().await;
        while records.len() >= MAX_CAPABILITIES {
            let evict = records
                .iter()
                .find_map(|(k, v)| {
                    if let Ok(guard) = v.try_lock()
                        && (guard.consumed || guard.capability.expires_at <= now)
                    {
                        return Some(k.clone());
                    }
                    None
                })
                .or_else(|| {
                    records
                        .iter()
                        .find_map(|(k, v)| v.try_lock().ok().map(|_| k.clone()))
                })
                .or_else(|| {
                    tracing::warn!(
                        "capability store full and all {} records are locked; evicting oldest",
                        records.len()
                    );
                    records.keys().next().cloned()
                });
            if let Some(key) = evict {
                records.shift_remove(&key);
            } else {
                break;
            }
        }
        records.insert(
            capability.id.clone(),
            Arc::new(tokio::sync::Mutex::new(CapabilityRecord {
                request,
                capability,
                consumed: false,
                result: None,
                snooze_original_start: None,
                snooze_target: None,
            })),
        );
    }

    /// Look up a capability record by id.
    pub async fn get(&self, id: &str) -> Option<Arc<tokio::sync::Mutex<CapabilityRecord>>> {
        let records = self.records.lock().await;
        records.get(id).cloned()
    }

    /// Remove expired or consumed capabilities in the background. Callers are
    /// not required to invoke this; it is provided for cleanup in tests.
    pub async fn remove_expired(&self) {
        let now = JiffTimestamp::now();
        let mut records = self.records.lock().await;
        records.retain(|_, record| {
            // Keep entries that are still in use (strong_count > 1) or that
            // are unconsumed and not yet expired. We cannot await the lock
            // here, so we use try_lock as a best-effort check.
            Arc::strong_count(record) > 1
                || record
                    .try_lock()
                    .map_or(true, |r| !r.consumed && r.capability.expires_at > now)
        });
    }
}

/// Mint a new capability from a request.
pub fn mint_capability(request: CapabilityRequest, input_path: InputPath) -> ActionCapability {
    let id = format!("cap-{}", uuid::Uuid::now_v7());
    let expires_at = JiffTimestamp::now()
        .checked_add(jiff::Span::new().minutes(CAPABILITY_TTL_MINUTES))
        .expect("capability TTL overflowed: timestamp is out of range");
    ActionCapability {
        id,
        event_id: None,
        device_id: request.device_id.clone(),
        action: request.action.clone(),
        input_path,
        expires_at,
        one_shot: true,
    }
}

/// Consume a capability and execute its action. Parallel calls on the same
/// capability serialize on the per-capability mutex, so the first successful
/// result is cached and returned on retry.
pub async fn authorize_action(
    client: &Client,
    record: Arc<tokio::sync::Mutex<CapabilityRecord>>,
    capability: &ActionCapability,
) -> Result<Presentation, CapabilityError> {
    let mut guard = record.lock().await;

    if guard.consumed {
        if capability != &guard.capability {
            return Err(CapabilityError::Mismatch);
        }
        return guard.result.clone().ok_or(CapabilityError::Consumed);
    }

    if capability.expires_at < JiffTimestamp::now() {
        return Err(CapabilityError::Expired);
    }

    if capability != &guard.capability {
        return Err(CapabilityError::Mismatch);
    }

    let action = guard.request.action.clone();
    match action.as_str() {
        "start" | "pause" | "progress" | "complete" | "delay" => {}
        _ => return Err(CapabilityError::InvalidAction),
    }

    let task = client.get_task(&guard.request.task_id).await?;
    let operation_id = capability.id.as_str();
    let result = match action.as_str() {
        "start" => execute_start(client, &task, operation_id).await?,
        "pause" => execute_pause(client, &task, operation_id).await?,
        "progress" => execute_progress(client, &guard.request, &task, operation_id).await?,
        "complete" => execute_complete(client, &task, operation_id).await?,
        "delay" => execute_delay(client, &task, &mut guard).await?,
        _ => return Err(CapabilityError::InvalidAction),
    };

    guard.consumed = true;
    guard.result = Some(result.clone());
    Ok(result)
}

async fn execute_start(
    client: &Client,
    task: &TaskRow,
    operation_id: &str,
) -> Result<Presentation, CapabilityError> {
    if task.status != TaskStatus::Scheduled && task.status != TaskStatus::Pending {
        return Err(CapabilityError::InvalidTaskState);
    }
    let body = StartWorkSession {
        task_id: Some(task.id.clone()),
        ..Default::default()
    };
    client.start_work_session(&body, Some(operation_id)).await?;
    Ok(work_transition(WorkTransitionKind::Start, task))
}

async fn execute_pause(
    client: &Client,
    task: &TaskRow,
    operation_id: &str,
) -> Result<Presentation, CapabilityError> {
    if task.status != TaskStatus::InProgress {
        return Err(CapabilityError::InvalidTaskState);
    }
    let open = client
        .open_work_session_for_task(&task.id)
        .await?
        .ok_or(CapabilityError::NoOpenWorkSession)?;
    client
        .pause_work_session(&open.id, Some(operation_id))
        .await?;
    Ok(work_transition(WorkTransitionKind::Pause, task))
}

async fn execute_progress(
    client: &Client,
    request: &CapabilityRequest,
    task: &TaskRow,
    operation_id: &str,
) -> Result<Presentation, CapabilityError> {
    let quantity = request
        .quantity_done
        .ok_or(CapabilityError::MissingQuantity)?;
    if quantity > MAX_SAFE_PROGRESS_QUANTITY {
        return Err(CapabilityError::Quantity(QuantityError::TooLarge(quantity)));
    }
    let quantity = Quantity::new(quantity)?;
    if task.status != TaskStatus::InProgress {
        return Err(CapabilityError::InvalidTaskState);
    }
    let open = client
        .open_work_session_for_task(&task.id)
        .await?
        .ok_or(CapabilityError::NoOpenWorkSession)?;
    let body = RecordWorkSessionProgress {
        quantity_done: quantity,
        note: request.note.clone(),
        quantity_total: None,
    };
    client
        .record_work_session_progress(&open.id, &body, Some(operation_id))
        .await?;
    Ok(work_transition(WorkTransitionKind::Progress, task))
}

async fn execute_complete(
    client: &Client,
    task: &TaskRow,
    operation_id: &str,
) -> Result<Presentation, CapabilityError> {
    if task.status != TaskStatus::InProgress {
        return Err(CapabilityError::InvalidTaskState);
    }
    let open = client
        .open_work_session_for_task(&task.id)
        .await?
        .ok_or(CapabilityError::NoOpenWorkSession)?;
    client
        .complete_work_session(&open.id, Some(operation_id))
        .await?;
    Ok(work_transition(WorkTransitionKind::Complete, task))
}

async fn execute_delay(
    client: &Client,
    task: &TaskRow,
    guard: &mut CapabilityRecord,
) -> Result<Presentation, CapabilityError> {
    let minutes = guard
        .request
        .snooze_minutes
        .ok_or(CapabilityError::MissingSnooze)?;
    if minutes <= 0 {
        return Err(CapabilityError::InvalidSnooze);
    }

    // Capture the schedule entry and pre-compute the absolute target on the
    // first attempt. Reusing the same target on retry makes `move_entry`
    // idempotent without server-side idempotency support.
    let (original_start, target) = if let (Some(original), Some(target)) =
        (guard.snooze_original_start, guard.snooze_target)
    {
        (original, target)
    } else {
        let schedule = client.get_schedule().await?;
        let entry = schedule
            .schedule
            .as_inner()
            .iter()
            .find(|e| e.task_id == task.id)
            .ok_or(CapabilityError::NotScheduled)?;
        let original = entry.start_at;
        let target = Timestamp(
            original
                .0
                .checked_add(jiff::Span::new().minutes(minutes))
                .unwrap_or(original.0),
        );
        guard.snooze_original_start = Some(original);
        guard.snooze_target = Some(target);
        (original, target)
    };

    let body = MoveEntry {
        start_at: target,
        force: false,
    };
    let response = client.move_entry(&task.id, &body).await?;
    let actual_minutes = minutes_between_ts(original_start, response.start_at);
    Ok(Presentation::WorkTransition(WorkTransition {
        kind: WorkTransitionKind::Delay,
        reference: format!("#{}", task.display_id),
        title: task.title.clone(),
        detail: format!("{}分後", actual_minutes),
    }))
}

fn work_transition(kind: WorkTransitionKind, task: &TaskRow) -> Presentation {
    Presentation::WorkTransition(WorkTransition {
        kind,
        reference: format!("#{}", task.display_id),
        title: task.title.clone(),
        detail: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_capability_has_expected_fields() {
        let request = CapabilityRequest {
            task_id: "task-1".into(),
            action: "start".into(),
            device_id: "mobile".into(),
            ..Default::default()
        };
        let capability = mint_capability(request, InputPath::ScreenCapability);
        assert!(capability.id.starts_with("cap-"));
        assert_eq!(capability.action, "start");
        assert_eq!(capability.input_path, InputPath::ScreenCapability);
        assert!(capability.one_shot);
        assert!(capability.expires_at > JiffTimestamp::now());
    }

    #[tokio::test]
    async fn capability_store_retains_and_evicts() {
        let store = CapabilityStore::new();
        for i in 0..MAX_CAPABILITIES + 10 {
            let request = CapabilityRequest {
                task_id: format!("task-{i}"),
                action: "start".into(),
                device_id: "mobile".into(),
                ..Default::default()
            };
            let capability = mint_capability(request.clone(), InputPath::ScreenCapability);
            store.insert(request, capability).await;
        }
        let records = store.records.lock().await;
        assert!(records.len() <= MAX_CAPABILITIES);
    }

    #[tokio::test]
    async fn capability_store_evicts_consumed_or_expired_first() {
        let store = CapabilityStore::new();
        // Fill the store to one below the cap so the next insert is a "full" insert.
        for i in 0..MAX_CAPABILITIES - 1 {
            let request = CapabilityRequest {
                task_id: format!("task-{i}"),
                action: "start".into(),
                device_id: "mobile".into(),
                ..Default::default()
            };
            let capability = mint_capability(request.clone(), InputPath::ScreenCapability);
            store.insert(request, capability).await;
        }

        // Insert a consumed record and an expired record.
        let consumed_id = {
            let request = CapabilityRequest {
                task_id: "consumed".into(),
                action: "start".into(),
                device_id: "mobile".into(),
                ..Default::default()
            };
            let capability = mint_capability(request.clone(), InputPath::ScreenCapability);
            let id = capability.id.clone();
            store.insert(request, capability).await;
            id
        };
        {
            let record = store.get(&consumed_id).await.unwrap();
            let mut guard = record.lock().await;
            guard.consumed = true;
        }

        let _expired_id = {
            let request = CapabilityRequest {
                task_id: "expired".into(),
                action: "start".into(),
                device_id: "mobile".into(),
                ..Default::default()
            };
            let mut capability = mint_capability(request.clone(), InputPath::ScreenCapability);
            capability.expires_at = JiffTimestamp::from_second(0).unwrap();
            let id = capability.id.clone();
            store.insert(request, capability).await;
            id
        };

        // One more insert should remove one of the evictable records.
        let request = CapabilityRequest {
            task_id: "new".into(),
            action: "start".into(),
            device_id: "mobile".into(),
            ..Default::default()
        };
        let capability = mint_capability(request.clone(), InputPath::ScreenCapability);
        store.insert(request, capability).await;

        let records = store.records.lock().await;
        assert_eq!(records.len(), MAX_CAPABILITIES);
    }

    #[tokio::test]
    async fn authorize_action_rejects_expired() {
        let client = Client::new("http://127.0.0.1:1", "token");
        let request = CapabilityRequest {
            task_id: "task-1".into(),
            action: "start".into(),
            device_id: "mobile".into(),
            ..Default::default()
        };
        let mut capability = mint_capability(request.clone(), InputPath::ScreenCapability);
        capability.expires_at = JiffTimestamp::from_second(0).unwrap();
        let record = Arc::new(tokio::sync::Mutex::new(CapabilityRecord {
            request,
            capability: capability.clone(),
            consumed: false,
            result: None,
            snooze_original_start: None,
            snooze_target: None,
        }));
        let err = authorize_action(&client, record, &capability)
            .await
            .unwrap_err();
        assert!(matches!(err, CapabilityError::Expired));
    }

    #[tokio::test]
    async fn authorize_action_rejects_mismatch() {
        let client = Client::new("http://127.0.0.1:1", "token");
        let request = CapabilityRequest {
            task_id: "task-1".into(),
            action: "start".into(),
            device_id: "mobile".into(),
            ..Default::default()
        };
        let capability = mint_capability(request.clone(), InputPath::ScreenCapability);
        let mut tampered = capability.clone();
        tampered.action = "pause".into();
        let record = Arc::new(tokio::sync::Mutex::new(CapabilityRecord {
            request,
            capability,
            consumed: false,
            result: None,
            snooze_original_start: None,
            snooze_target: None,
        }));
        let err = authorize_action(&client, record, &tampered)
            .await
            .unwrap_err();
        assert!(matches!(err, CapabilityError::Mismatch));
    }

    #[tokio::test]
    async fn authorize_action_replays_consumed_result() {
        let client = Client::new("http://127.0.0.1:1", "token");
        let request = CapabilityRequest {
            task_id: "task-1".into(),
            action: "start".into(),
            device_id: "mobile".into(),
            ..Default::default()
        };
        let capability = mint_capability(request.clone(), InputPath::ScreenCapability);
        let result = Presentation::Text {
            text: "ok".to_string(),
        };
        let record = Arc::new(tokio::sync::Mutex::new(CapabilityRecord {
            request,
            capability: capability.clone(),
            consumed: true,
            result: Some(result.clone()),
            snooze_original_start: None,
            snooze_target: None,
        }));
        let got = authorize_action(&client, record, &capability)
            .await
            .unwrap();
        assert!(matches!(got, Presentation::Text { text } if text == "ok"));
    }

    #[tokio::test]
    async fn authorize_action_rejects_invalid_action() {
        let client = Client::new("http://127.0.0.1:1", "token");
        let request = CapabilityRequest {
            task_id: "task-1".into(),
            action: "unknown".into(),
            device_id: "mobile".into(),
            ..Default::default()
        };
        let capability = mint_capability(request.clone(), InputPath::ScreenCapability);
        let record = Arc::new(tokio::sync::Mutex::new(CapabilityRecord {
            request: request.clone(),
            capability: capability.clone(),
            consumed: false,
            result: None,
            snooze_original_start: None,
            snooze_target: None,
        }));
        let err = authorize_action(&client, record, &capability)
            .await
            .unwrap_err();
        assert!(matches!(err, CapabilityError::InvalidAction));
    }
}
