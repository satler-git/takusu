//! Shared device surface state for resident-agent controls.
//!
//! Turn events describe the agent's work while audio callbacks describe the
//! platform-owned recording and playback lifecycle. Both inputs update one
//! operation-scoped, revisioned snapshot and broadcast state changes so every
//! surface on the device can render the same state without owning a transition.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::TurnEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateScope {
    User,
    Session,
    Device,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceState {
    Idle,
    Listening,
    Transcribing,
    Thinking,
    WaitingForUser,
    WaitingForApproval,
    Speaking,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SurfaceSnapshot {
    pub scope: StateScope,
    pub state: SurfaceState,
    pub revision: u64,
    /// Identifies the current device-local turn or audio operation. A late
    /// callback for an older operation is ignored by the state machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Default for SurfaceSnapshot {
    fn default() -> Self {
        Self {
            scope: StateScope::Device,
            state: SurfaceState::Idle,
            revision: 0,
            operation_id: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioCallback {
    Listening,
    Transcribing,
    Speaking,
    PlaybackFinished,
}

/// Backwards-compatible name for callers that model callbacks as events.
pub type AudioEvent = AudioCallback;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceCommand {
    ConfirmRecording,
    OpenPanel,
    StopTts,
    OpenApproval,
    ShowRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SurfaceCommandResponse {
    pub command: SurfaceCommand,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub snapshot: SurfaceSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceEvent {
    Snapshot(SurfaceSnapshot),
    StateChanged(SurfaceSnapshot),
}

#[derive(Debug, Clone)]
struct SurfaceStateData {
    snapshot: SurfaceSnapshot,
    owner: Option<String>,
    active: bool,
    turn_done: bool,
    next_operation_id: u64,
}

#[derive(Debug)]
struct SurfaceStateInner {
    state: Mutex<SurfaceStateData>,
    events: broadcast::Sender<SurfaceEvent>,
}

#[derive(Debug, Clone)]
pub struct SurfaceStateMachine {
    inner: Arc<SurfaceStateInner>,
}

impl Default for SurfaceStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl SurfaceStateMachine {
    const EVENT_CAPACITY: usize = 32;

    pub fn new() -> Self {
        let (events, _) = broadcast::channel(Self::EVENT_CAPACITY);
        Self {
            inner: Arc::new(SurfaceStateInner {
                state: Mutex::new(SurfaceStateData {
                    snapshot: SurfaceSnapshot::default(),
                    owner: None,
                    active: false,
                    turn_done: false,
                    next_operation_id: 0,
                }),
                events,
            }),
        }
    }

    pub fn snapshot(&self) -> SurfaceSnapshot {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot
            .clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SurfaceEvent> {
        self.inner.events.subscribe()
    }

    /// Start a new device-local operation and return its opaque operation id.
    /// Starting a newer operation makes callbacks for the previous one stale.
    pub fn begin_operation(&self, owner: Option<String>) -> u64 {
        let mut data = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        data.next_operation_id = data.next_operation_id.saturating_add(1);
        let operation_id = data.next_operation_id;
        let before = data.snapshot.clone();
        data.owner = owner;
        data.active = true;
        data.turn_done = false;
        data.snapshot.operation_id = Some(operation_id);
        data.snapshot.state = SurfaceState::Idle;
        data.snapshot.error = None;
        self.commit_locked(&mut data, before);
        operation_id
    }

    /// Finish an operation only when it is still the active operation.
    pub fn finish_operation(&self, operation_id: u64) -> SurfaceSnapshot {
        self.update_for_operation(operation_id, |data| {
            data.snapshot.state = SurfaceState::Idle;
            data.snapshot.error = None;
            data.active = false;
            data.turn_done = false;
            data.owner = None;
        })
    }

    /// Finish the operation owned by a session, if that session still owns it.
    pub fn finish_if_owner(&self, owner: &str) -> SurfaceSnapshot {
        self.finish_owned_operation(owner, false)
    }

    /// Finish an approval only when the session still owns the approval state.
    pub fn finish_approval_if_owner(&self, owner: &str) -> SurfaceSnapshot {
        self.finish_owned_operation(owner, true)
    }

    fn finish_owned_operation(&self, owner: &str, approval_only: bool) -> SurfaceSnapshot {
        let operation_id = {
            let data = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (data.active
                && data.owner.as_deref() == Some(owner)
                && (!approval_only || data.snapshot.state == SurfaceState::WaitingForApproval))
                .then_some(data.snapshot.operation_id)
                .flatten()
        };
        match operation_id {
            Some(operation_id) => self.finish_operation(operation_id),
            None => self.snapshot(),
        }
    }

    pub fn reset(&self) -> SurfaceSnapshot {
        let mut data = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let before = data.snapshot.clone();
        data.snapshot.state = SurfaceState::Idle;
        data.snapshot.operation_id = None;
        data.snapshot.error = None;
        data.active = false;
        data.turn_done = false;
        data.owner = None;
        self.commit_locked(&mut data, before)
    }

    pub fn set_waiting_for_approval(&self, operation_id: u64) -> SurfaceSnapshot {
        self.update_for_operation(operation_id, |data| {
            data.snapshot.state = SurfaceState::WaitingForApproval;
            data.snapshot.error = None;
        })
    }

    pub fn apply_turn_event(&self, event: &TurnEvent) -> SurfaceSnapshot {
        let operation_id = self.ensure_operation();
        self.apply_turn_event_for(operation_id, event)
    }

    pub fn apply_turn_event_for(&self, operation_id: u64, event: &TurnEvent) -> SurfaceSnapshot {
        match event {
            TurnEvent::Thinking(_) | TurnEvent::Text(_) => {
                self.update_for_operation(operation_id, |data| {
                    if data.snapshot.state != SurfaceState::Speaking {
                        data.snapshot.state = SurfaceState::Thinking;
                        data.snapshot.error = None;
                    }
                })
            }
            TurnEvent::ToolCall { name, .. } if name == "correct_asr" => {
                self.update_for_operation(operation_id, |data| {
                    data.snapshot.state = SurfaceState::WaitingForUser;
                    data.snapshot.error = None;
                })
            }
            TurnEvent::ToolCall { .. } | TurnEvent::ToolResult { .. } => {
                self.update_for_operation(operation_id, |data| {
                    if data.snapshot.state != SurfaceState::Speaking {
                        data.snapshot.state = SurfaceState::Thinking;
                        data.snapshot.error = None;
                    }
                })
            }
            TurnEvent::Error(error) => self.update_for_operation(operation_id, |data| {
                data.snapshot.state = SurfaceState::Error;
                data.snapshot.error = Some(error.clone());
            }),
            TurnEvent::Done(result) => self.update_for_operation(operation_id, |data| {
                data.turn_done = true;
                if result.approval_request.is_some() {
                    data.snapshot.state = SurfaceState::WaitingForApproval;
                    data.snapshot.error = None;
                } else if data.snapshot.state != SurfaceState::Speaking {
                    data.snapshot.state = SurfaceState::Idle;
                    data.snapshot.error = None;
                    data.active = false;
                    data.owner = None;
                }
            }),
        }
    }

    pub fn begin_audio_operation(&self) -> u64 {
        let mut data = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        data.next_operation_id = data.next_operation_id.saturating_add(1);
        let operation_id = data.next_operation_id;
        let before = data.snapshot.clone();
        data.owner = None;
        data.active = true;
        data.turn_done = false;
        data.snapshot.operation_id = Some(operation_id);
        data.snapshot.state = SurfaceState::Listening;
        data.snapshot.error = None;
        self.commit_locked(&mut data, before);
        operation_id
    }

    pub fn apply_audio_callback(&self, callback: AudioCallback) -> SurfaceSnapshot {
        let operation_id = if callback == AudioCallback::Listening {
            self.begin_audio_operation()
        } else {
            self.ensure_operation()
        };
        self.apply_audio_callback_for(operation_id, callback)
    }

    pub fn apply_audio_callback_for(
        &self,
        operation_id: u64,
        callback: AudioCallback,
    ) -> SurfaceSnapshot {
        match callback {
            AudioCallback::Listening => self.update_for_operation(operation_id, |data| {
                data.snapshot.state = SurfaceState::Listening;
                data.snapshot.error = None;
            }),
            AudioCallback::Transcribing => self.update_for_operation(operation_id, |data| {
                data.snapshot.state = SurfaceState::Transcribing;
                data.snapshot.error = None;
            }),
            AudioCallback::Speaking => self.update_for_operation(operation_id, |data| {
                data.snapshot.state = SurfaceState::Speaking;
                data.snapshot.error = None;
            }),
            AudioCallback::PlaybackFinished => self.update_for_operation(operation_id, |data| {
                if data.snapshot.state == SurfaceState::Speaking {
                    data.snapshot.state = SurfaceState::Idle;
                    data.snapshot.error = None;
                    if data.turn_done {
                        data.active = false;
                        data.owner = None;
                    }
                }
            }),
        }
    }

    pub fn command(&self, command: SurfaceCommand) -> SurfaceCommandResponse {
        self.command_for(None, command)
    }

    pub fn command_for(
        &self,
        operation_id: Option<u64>,
        command: SurfaceCommand,
    ) -> SurfaceCommandResponse {
        let expected = match command {
            SurfaceCommand::ConfirmRecording => SurfaceState::Listening,
            SurfaceCommand::OpenPanel => SurfaceState::Thinking,
            SurfaceCommand::StopTts => SurfaceState::Speaking,
            SurfaceCommand::OpenApproval => SurfaceState::WaitingForApproval,
            SurfaceCommand::ShowRecovery => SurfaceState::Error,
        };
        let mut data = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !data.active
            || operation_id
                .is_some_and(|operation_id| data.snapshot.operation_id != Some(operation_id))
            || data.snapshot.state != expected
        {
            return SurfaceCommandResponse {
                command,
                accepted: false,
                reason: Some(format!("command requires {expected:?} state")),
                snapshot: data.snapshot.clone(),
            };
        }

        let before = data.snapshot.clone();
        match command {
            SurfaceCommand::ConfirmRecording => {
                data.snapshot.state = SurfaceState::Transcribing;
                data.snapshot.error = None;
            }
            SurfaceCommand::StopTts => {
                data.snapshot.state = SurfaceState::Idle;
                data.snapshot.error = None;
                if data.turn_done {
                    data.active = false;
                    data.owner = None;
                }
            }
            SurfaceCommand::OpenPanel
            | SurfaceCommand::OpenApproval
            | SurfaceCommand::ShowRecovery => {}
        }
        let snapshot = self.commit_locked(&mut data, before);
        SurfaceCommandResponse {
            command,
            accepted: true,
            reason: None,
            snapshot,
        }
    }

    fn ensure_operation(&self) -> u64 {
        let mut data = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if data.active
            && let Some(operation_id) = data.snapshot.operation_id
        {
            return operation_id;
        }
        data.next_operation_id = data.next_operation_id.saturating_add(1);
        let operation_id = data.next_operation_id;
        data.active = true;
        data.turn_done = false;
        data.owner = None;
        data.snapshot.operation_id = Some(operation_id);
        operation_id
    }

    fn update_for_operation(
        &self,
        operation_id: u64,
        update: impl FnOnce(&mut SurfaceStateData),
    ) -> SurfaceSnapshot {
        let mut data = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !data.active || data.snapshot.operation_id != Some(operation_id) {
            return data.snapshot.clone();
        }
        let before = data.snapshot.clone();
        update(&mut data);
        self.commit_locked(&mut data, before)
    }

    fn commit_locked(
        &self,
        data: &mut SurfaceStateData,
        before: SurfaceSnapshot,
    ) -> SurfaceSnapshot {
        if data.snapshot != before {
            data.snapshot.revision = before.revision.saturating_add(1);
            let snapshot = data.snapshot.clone();
            // Send while holding the state lock so subscribers observe revisions
            // in the same order in which state transitions commit.
            let _ = self
                .inner
                .events
                .send(SurfaceEvent::StateChanged(snapshot.clone()));
            snapshot
        } else {
            data.snapshot.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{ApprovalRequest, TurnResult};

    fn approval_result() -> TurnResult {
        TurnResult {
            text: String::new(),
            changes: Vec::new(),
            schedule_dirty: false,
            approval_request: Some(ApprovalRequest {
                id: "approval-1".into(),
                why: "test".into(),
                changes: Vec::new(),
                inferred_fields: Vec::new(),
                warnings: Vec::new(),
                expires_at: jiff::Timestamp::now(),
            }),
            presentation: None,
        }
    }

    fn done_result() -> TurnResult {
        TurnResult {
            text: "done".into(),
            changes: Vec::new(),
            schedule_dirty: false,
            approval_request: None,
            presentation: None,
        }
    }

    fn machine_in(state: SurfaceState) -> SurfaceStateMachine {
        let machine = SurfaceStateMachine::new();
        match state {
            SurfaceState::Idle => {
                machine.begin_operation(None);
            }
            SurfaceState::Listening => {
                machine.apply_audio_callback(AudioCallback::Listening);
            }
            SurfaceState::Transcribing => {
                machine.apply_audio_callback(AudioCallback::Transcribing);
            }
            SurfaceState::Thinking => {
                machine.apply_turn_event(&TurnEvent::Thinking("thinking".into()));
            }
            SurfaceState::WaitingForUser => {
                machine.apply_turn_event(&TurnEvent::ToolCall {
                    name: "correct_asr".into(),
                    call_id: "call-1".into(),
                    arguments: serde_json::json!({}),
                });
            }
            SurfaceState::WaitingForApproval => {
                machine.apply_turn_event(&TurnEvent::Done(Box::new(approval_result())));
            }
            SurfaceState::Speaking => {
                machine.apply_audio_callback(AudioCallback::Speaking);
            }
            SurfaceState::Error => {
                machine.apply_turn_event(&TurnEvent::Error("failed".into()));
            }
        }
        assert_eq!(machine.snapshot().state, state);
        machine
    }

    #[test]
    fn initial_snapshot_is_device_scoped_and_idle() {
        assert_eq!(
            SurfaceStateMachine::new().snapshot(),
            SurfaceSnapshot::default()
        );
    }

    #[test]
    fn turn_and_audio_events_follow_the_scripted_state_sequence() {
        let machine = SurfaceStateMachine::new();
        let mut events = machine.subscribe();

        machine.apply_turn_event(&TurnEvent::Thinking("working".into()));
        machine.apply_audio_callback(AudioCallback::Speaking);
        machine.apply_audio_callback(AudioCallback::PlaybackFinished);

        let states: Vec<_> = (0..3)
            .map(|_| match events.try_recv().unwrap() {
                SurfaceEvent::StateChanged(snapshot) => snapshot.state,
                SurfaceEvent::Snapshot(_) => panic!("subscription does not emit snapshots"),
            })
            .collect();
        assert_eq!(
            states,
            vec![
                SurfaceState::Thinking,
                SurfaceState::Speaking,
                SurfaceState::Idle
            ]
        );
    }

    #[test]
    fn turn_events_enter_user_and_approval_waiting_states() {
        let machine = SurfaceStateMachine::new();
        let operation_id = machine.begin_operation(Some("session-1".into()));
        machine.apply_turn_event_for(
            operation_id,
            &TurnEvent::ToolCall {
                name: "correct_asr".into(),
                call_id: "call-1".into(),
                arguments: serde_json::json!({}),
            },
        );
        assert_eq!(machine.snapshot().state, SurfaceState::WaitingForUser);

        machine.apply_turn_event_for(
            operation_id,
            &TurnEvent::ToolResult {
                name: "correct_asr".into(),
                call_id: "call-1".into(),
                content: "[]".into(),
                is_error: false,
            },
        );
        assert_eq!(machine.snapshot().state, SurfaceState::Thinking);

        machine.apply_turn_event_for(operation_id, &TurnEvent::Done(Box::new(approval_result())));
        assert_eq!(machine.snapshot().state, SurfaceState::WaitingForApproval);
    }

    #[test]
    fn each_surface_command_is_only_accepted_in_its_contract_state() {
        let states = [
            SurfaceState::Idle,
            SurfaceState::Listening,
            SurfaceState::Transcribing,
            SurfaceState::Thinking,
            SurfaceState::WaitingForUser,
            SurfaceState::WaitingForApproval,
            SurfaceState::Speaking,
            SurfaceState::Error,
        ];
        let commands = [
            (SurfaceCommand::ConfirmRecording, SurfaceState::Listening),
            (SurfaceCommand::OpenPanel, SurfaceState::Thinking),
            (SurfaceCommand::StopTts, SurfaceState::Speaking),
            (
                SurfaceCommand::OpenApproval,
                SurfaceState::WaitingForApproval,
            ),
            (SurfaceCommand::ShowRecovery, SurfaceState::Error),
        ];

        for state in states {
            for (command, expected_state) in commands {
                let machine = machine_in(state);
                let operation_id = machine.snapshot().operation_id;
                let response = machine.command_for(operation_id, command);
                assert_eq!(response.accepted, state == expected_state);
                assert_eq!(response.command, command);
                assert_eq!(
                    response.snapshot.state,
                    if state == expected_state {
                        match command {
                            SurfaceCommand::ConfirmRecording => SurfaceState::Transcribing,
                            SurfaceCommand::StopTts => SurfaceState::Idle,
                            _ => state,
                        }
                    } else {
                        state
                    }
                );
            }
        }
    }

    #[test]
    fn stale_operation_events_and_commands_are_ignored() {
        let machine = SurfaceStateMachine::new();
        let first = machine.begin_operation(Some("first".into()));
        machine.apply_turn_event_for(first, &TurnEvent::Thinking("first".into()));
        let second = machine.begin_operation(Some("second".into()));
        machine.apply_turn_event_for(second, &TurnEvent::Thinking("second".into()));

        machine.apply_turn_event_for(first, &TurnEvent::Error("stale".into()));
        let command = machine.command_for(Some(first), SurfaceCommand::OpenPanel);
        assert!(!command.accepted);
        assert_eq!(machine.snapshot().operation_id, Some(second));
        assert_eq!(machine.snapshot().state, SurfaceState::Thinking);
    }

    #[test]
    fn playback_finish_does_not_clear_a_pending_approval() {
        let machine = machine_in(SurfaceState::WaitingForApproval);
        let operation_id = machine.snapshot().operation_id.unwrap();
        machine.apply_audio_callback_for(operation_id, AudioCallback::PlaybackFinished);
        assert_eq!(machine.snapshot().state, SurfaceState::WaitingForApproval);
    }

    #[test]
    fn done_and_playback_finish_converge_in_either_order() {
        let first = SurfaceStateMachine::new();
        let first_operation = first.begin_operation(Some("first".into()));
        first.apply_audio_callback_for(first_operation, AudioCallback::Speaking);
        first.apply_audio_callback_for(first_operation, AudioCallback::PlaybackFinished);
        first.apply_turn_event_for(first_operation, &TurnEvent::Done(Box::new(done_result())));
        assert_eq!(first.snapshot().state, SurfaceState::Idle);

        let second = SurfaceStateMachine::new();
        let second_operation = second.begin_operation(Some("second".into()));
        second.apply_audio_callback_for(second_operation, AudioCallback::Speaking);
        second.apply_turn_event_for(second_operation, &TurnEvent::Done(Box::new(done_result())));
        assert_eq!(second.snapshot().state, SurfaceState::Speaking);
        second.apply_audio_callback_for(second_operation, AudioCallback::PlaybackFinished);
        assert_eq!(second.snapshot().state, SurfaceState::Idle);
    }

    #[test]
    fn stop_tts_keeps_a_turn_operation_alive_until_done() {
        let machine = SurfaceStateMachine::new();
        let operation_id = machine.begin_operation(Some("turn".into()));
        machine.apply_audio_callback_for(operation_id, AudioCallback::Speaking);
        let response = machine.command_for(Some(operation_id), SurfaceCommand::StopTts);
        assert!(response.accepted);
        machine.apply_turn_event_for(operation_id, &TurnEvent::Done(Box::new(done_result())));
        assert_eq!(machine.snapshot().state, SurfaceState::Idle);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_callbacks_keep_broadcast_revisions_monotonic() {
        let machine = SurfaceStateMachine::new();
        let mut events = machine.subscribe();
        let first = machine.clone();
        let second = machine.clone();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_barrier = barrier.clone();
        let first_task = tokio::spawn(async move {
            first_barrier.wait().await;
            first.apply_audio_callback(AudioCallback::Listening);
        });
        let second_barrier = barrier.clone();
        let second_task = tokio::spawn(async move {
            second_barrier.wait().await;
            second.apply_audio_callback(AudioCallback::Transcribing);
        });
        barrier.wait().await;
        let _ = tokio::try_join!(first_task, second_task);

        let mut revisions = Vec::new();
        while revisions.len() < 2 {
            match tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for broadcast event"))
            {
                Ok(SurfaceEvent::StateChanged(snapshot)) => revisions.push(snapshot.revision),
                Ok(SurfaceEvent::Snapshot(_)) => panic!("subscription does not emit snapshots"),
                Err(e) => panic!("expected two broadcast events, got {e:?}"),
            }
        }
        assert_eq!(revisions, vec![1, 2]);
    }
}
