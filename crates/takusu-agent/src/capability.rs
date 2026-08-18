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
use takusu_types::{Quantity, QuantityError, TaskStatus, Timestamp};

use crate::presentation::{Presentation, WorkTransition, WorkTransitionKind};

/// Default lifetime of a quick-action capability.
pub const CAPABILITY_TTL_MINUTES: i64 = 5;

/// Grace period after a scheduled notification's delivery time during which
/// the action capability remains valid. Used for start-time notifications where
/// the user may not tap immediately (WI-4).
pub const NOTIFICATION_GRACE_MINUTES: i64 = 30;

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

/// A quick action that a one-shot capability can authorize.
///
/// Kept as a string on the wire for forward compatibility, but parsed into this
/// enum as soon as it reaches the server (WI-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAction {
    Start,
    Pause,
    Progress,
    Complete,
    Delay,
}

impl std::fmt::Display for QuickAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Start => "start",
            Self::Pause => "pause",
            Self::Progress => "progress",
            Self::Complete => "complete",
            Self::Delay => "delay",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for QuickAction {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "start" => Ok(Self::Start),
            "pause" => Ok(Self::Pause),
            "progress" => Ok(Self::Progress),
            "complete" => Ok(Self::Complete),
            "delay" => Ok(Self::Delay),
            _ => Err("unknown quick action"),
        }
    }
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
    /// The task this capability is authorized to act on.
    pub task_id: String,
    /// Snooze duration in minutes, present for `delay` capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_minutes: Option<i64>,
    /// Target `start_at` for `delay` capabilities, computed client-side or on first tap (WI-4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub snooze_target: Option<Timestamp>,
    /// Quantity completed, present for `progress` capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_done: Option<i64>,
    /// Note to attach with progress, present for `progress` capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The scheduled delivery time for notification capabilities (WI-4).
    ///
    /// When present, the server derives a longer expiry that covers the
    /// scheduled time plus a short grace period, so the action remains usable
    /// when the notification fires while the app is not in the foreground.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub scheduled_at: Option<Timestamp>,
    /// The original request the capability was minted from.
    ///
    /// Included so a client can return the capability unchanged across server
    /// restarts, when the in-memory `CapabilityStore` is empty. The server
    /// ignores this field during authorization; all authoritative parameters
    /// live as top-level fields on the capability itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<CapabilityRequest>,
}

/// Request to mint a quick-action capability.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityRequest {
    pub task_id: String,
    pub action: String,
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_minutes: Option<i64>,
    /// Target `start_at` for `delay` capabilities (WI-4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub snooze_target: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_done: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The scheduled delivery time for notification capabilities (WI-4).
    ///
    /// When present, the server derives a longer expiry that covers the
    /// scheduled time plus a short grace period, so the action remains usable
    /// when the notification fires while the app is not in the foreground.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub scheduled_at: Option<Timestamp>,
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
    /// Parsed `QuickAction` stored on first successful authorization so callers
    /// do not have to re-parse the action string for state-change hints.
    pub action: Option<QuickAction>,
    /// Target `start_at` captured on the first delay attempt. Reusing this on
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

    /// Evict entries until the store has room for one more record.
    ///
    /// When the store is full, evict the oldest entry that is expired or already
    /// consumed; if none exists, evict the oldest *unlocked* entry. Locked
    /// records (currently being authorized) are never evicted; if every record
    /// is locked the oldest record is removed as a last resort. This bounds
    /// memory while avoiding dropping in-flight capabilities in the common case.
    fn evict_if_needed(records: &mut IndexMap<String, Arc<tokio::sync::Mutex<CapabilityRecord>>>) {
        let now = JiffTimestamp::now();
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
    }

    /// Store a freshly minted capability.
    pub async fn insert(&self, request: CapabilityRequest, capability: ActionCapability) {
        let mut records = self.records.lock().await;
        Self::evict_if_needed(&mut records);
        records.insert(
            capability.id.clone(),
            Arc::new(tokio::sync::Mutex::new(CapabilityRecord {
                request: request.clone(),
                capability: capability.clone(),
                consumed: false,
                result: None,
                action: None,
                snooze_target: None,
            })),
        );
    }

    /// Look up a capability record by id.
    pub async fn get(&self, id: &str) -> Option<Arc<tokio::sync::Mutex<CapabilityRecord>>> {
        let records = self.records.lock().await;
        records.get(id).cloned()
    }

    /// Look up a capability record by id, inserting a fresh record if absent.
    ///
    /// This guarantees `authorize_action` always has a per-capability mutex, so
    /// concurrent or retried calls (e.g. notification actions after a server
    /// restart) serialize on the same record. The record is built from the
    /// capability's own authoritative fields, not from the client-provided
    /// `request` copy.
    pub async fn get_or_insert(
        &self,
        capability: &ActionCapability,
    ) -> Option<Arc<tokio::sync::Mutex<CapabilityRecord>>> {
        let mut records = self.records.lock().await;
        if let Some(record) = records.get(&capability.id) {
            return Some(record.clone());
        }
        Self::evict_if_needed(&mut records);
        let request = capability
            .request
            .clone()
            .unwrap_or_else(|| CapabilityRequest {
                task_id: capability.task_id.clone(),
                action: capability.action.clone(),
                device_id: capability.device_id.clone(),
                snooze_minutes: capability.snooze_minutes,
                snooze_target: capability.snooze_target,
                quantity_done: capability.quantity_done,
                note: capability.note.clone(),
                scheduled_at: capability.scheduled_at,
            });
        let record = Arc::new(tokio::sync::Mutex::new(CapabilityRecord {
            request,
            capability: capability.clone(),
            consumed: false,
            result: None,
            action: None,
            snooze_target: None,
        }));
        records.insert(capability.id.clone(), record.clone());
        Some(record)
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
    let now = JiffTimestamp::now();
    let expires_at = if let Some(scheduled) = request.scheduled_at {
        // For notification capabilities the user may not act immediately
        // when the notification fires, so keep the capability valid through
        // the scheduled time plus a short grace period.
        let target = scheduled
            .0
            .checked_add(jiff::Span::new().minutes(NOTIFICATION_GRACE_MINUTES));
        target.unwrap_or(now)
    } else {
        now.checked_add(jiff::Span::new().minutes(CAPABILITY_TTL_MINUTES))
            .expect("capability TTL overflowed: timestamp is out of range")
    };
    ActionCapability {
        id,
        event_id: None,
        device_id: request.device_id.clone(),
        action: request.action.clone(),
        input_path,
        expires_at,
        one_shot: true,
        task_id: request.task_id.clone(),
        snooze_minutes: request.snooze_minutes,
        snooze_target: request.snooze_target,
        quantity_done: request.quantity_done,
        note: request.note.clone(),
        scheduled_at: request.scheduled_at,
        request: Some(request),
    }
}

/// Consume a capability and execute its action. Parallel calls on the same
/// capability serialize on the per-capability mutex, so the first successful
/// result is cached and returned on retry.
/// Return a client capability that is safe to compare against the stored record.
/// If the client did not include a `snooze_target` but the record already has
/// one, inherit the record's value so a retry (or a server restart) is not
/// rejected as a mismatch.
fn normalized_capability(client: &ActionCapability, stored: &ActionCapability) -> ActionCapability {
    let mut c = client.clone();
    if c.snooze_target.is_none() {
        c.snooze_target = stored.snooze_target;
    }
    c
}

pub async fn authorize_action(
    client: &Client,
    record: Option<Arc<tokio::sync::Mutex<CapabilityRecord>>>,
    capability: &ActionCapability,
) -> Result<(Presentation, QuickAction), CapabilityError> {
    let mut guard = if let Some(arc) = record.as_ref() {
        Some(arc.lock().await)
    } else {
        None
    };

    let action = match guard.as_ref().and_then(|g| g.action) {
        Some(action) => action,
        None => capability
            .action
            .parse()
            .map_err(|_| CapabilityError::InvalidAction)?,
    };

    // Replay a previously consumed result. Allow the client to omit the
    // server-computed `snooze_target` on retry.
    if let Some(ref g) = guard
        && g.consumed
    {
        if normalized_capability(capability, &g.capability) != g.capability {
            return Err(CapabilityError::Mismatch);
        }
        return g
            .result
            .clone()
            .zip(g.action)
            .ok_or(CapabilityError::Consumed);
    }

    // Reject expired capabilities before mutating any in-memory record state.
    if capability.expires_at < JiffTimestamp::now() {
        return Err(CapabilityError::Expired);
    }

    // The client may add a `snooze_target` to a delay capability after minting
    // (it is computed from the local clock at tap time). Accept this addition
    // and keep the record's authoritative copy in sync.
    if let Some(ref mut g) = guard
        && matches!(action, QuickAction::Delay)
        && g.capability.snooze_target.is_none()
        && capability.snooze_target.is_some()
    {
        g.capability.snooze_target = capability.snooze_target;
        g.snooze_target = capability.snooze_target;
    }

    if let Some(ref g) = guard
        && normalized_capability(capability, &g.capability) != g.capability
    {
        return Err(CapabilityError::Mismatch);
    }

    // If the client included the original `request` copy, verify it matches
    // the authoritative top-level capability fields. The capability itself is
    // the trust boundary; a tampered embedded request must not be used.
    if let Some(ref request) = capability.request
        && (request.task_id != capability.task_id
            || request.action != capability.action
            || request.device_id != capability.device_id
            || request.snooze_minutes != capability.snooze_minutes
            || request.quantity_done != capability.quantity_done
            || request.note != capability.note
            || request.scheduled_at != capability.scheduled_at)
    {
        return Err(CapabilityError::Mismatch);
    }

    let task = client.get_task(&capability.task_id).await?;
    let operation_id = capability.id.as_str();

    // Build a mutable record reference. If the in-memory store has one, use it
    // so state (consumed, result, delay target) is cached. Otherwise use a
    // local record reconstructed from the capability itself; this keeps the
    // action usable after a server restart when the store is empty.
    let mut local_record = CapabilityRecord {
        request: capability
            .request
            .clone()
            .unwrap_or_else(|| CapabilityRequest {
                task_id: capability.task_id.clone(),
                action: capability.action.clone(),
                device_id: capability.device_id.clone(),
                snooze_minutes: capability.snooze_minutes,
                snooze_target: capability.snooze_target,
                quantity_done: capability.quantity_done,
                note: capability.note.clone(),
                scheduled_at: capability.scheduled_at,
            }),
        capability: capability.clone(),
        consumed: false,
        result: None,
        action: None,
        snooze_target: None,
    };
    let is_stored = guard.is_some();
    let record_ref = guard.as_deref_mut().unwrap_or(&mut local_record);

    let result = match action {
        QuickAction::Start => execute_start(client, &task, operation_id).await?,
        QuickAction::Pause => execute_pause(client, &task, operation_id).await?,
        QuickAction::Progress => {
            execute_progress(client, &record_ref.capability, &task, operation_id).await?
        }
        QuickAction::Complete => execute_complete(client, &task, operation_id).await?,
        QuickAction::Delay => execute_delay(client, &task, record_ref).await?,
    };

    if is_stored {
        record_ref.consumed = true;
        record_ref.result = Some(result.clone());
        record_ref.action = Some(action);
    }

    Ok((result, action))
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
    capability: &ActionCapability,
    task: &TaskRow,
    operation_id: &str,
) -> Result<Presentation, CapabilityError> {
    let quantity = capability
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
        note: capability.note.clone(),
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
        .capability
        .snooze_minutes
        .ok_or(CapabilityError::MissingSnooze)?;
    if minutes <= 0 {
        return Err(CapabilityError::InvalidSnooze);
    }

    // The target is computed from the time the user taps the action, not from
    // the original schedule start, so "10分後" consistently means "10 minutes
    // from now". The first computed target is cached in the record so retries
    // are idempotent within the same server process. If the client already
    // provided a target (e.g. across a server restart), use it as-is.
    let target = if let Some(target) = guard.capability.snooze_target {
        target
    } else if let Some(target) = guard.snooze_target {
        target
    } else {
        let now = JiffTimestamp::now();
        let target = Timestamp(
            now.checked_add(jiff::Span::new().minutes(minutes))
                .unwrap_or(now),
        );
        guard.snooze_target = Some(target);
        guard.capability.snooze_target = Some(target);
        target
    };

    let body = MoveEntry {
        start_at: target,
        force: false,
    };
    let operation_id = guard.capability.id.as_str();
    client
        .move_entry(&task.id, &body, Some(operation_id))
        .await?;
    Ok(Presentation::WorkTransition(WorkTransition {
        kind: WorkTransitionKind::Delay,
        reference: format!("#{}", task.display_id),
        title: task.title.clone(),
        detail: format!("{}分後", minutes),
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
            action: None,
            snooze_target: None,
        }));
        let err = authorize_action(&client, Some(record), &capability)
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
            action: None,
            snooze_target: None,
        }));
        let err = authorize_action(&client, Some(record), &tampered)
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
            action: Some(QuickAction::Start),
            snooze_target: None,
        }));
        let (got, _) = authorize_action(&client, Some(record), &capability)
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
            action: None,
            snooze_target: None,
        }));
        let err = authorize_action(&client, Some(record), &capability)
            .await
            .unwrap_err();
        assert!(matches!(err, CapabilityError::InvalidAction));
    }
}
