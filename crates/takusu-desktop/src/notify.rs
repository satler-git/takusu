//! Desktop notifications via `org.freedesktop.Notifications`.
//!
//! Shows check-ins and alerts with action buttons and routes the activation
//! through the server-issued `ActionCapability`.

#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use serde::Deserialize;
use takusu_agent::SurfaceCommand;
use takusu_agent::capability::ActionCapability;
use takusu_agent::presentation::ActionKind;
use zbus::Connection;

use crate::state::DesktopError;
use crate::transport::DesktopTransport;

/// Payload for a desktop notification.
#[derive(Debug, Clone, Default)]
pub struct DesktopNotification {
    pub id: u32,
    pub title: String,
    pub body: String,
    /// One logical action per capability. `None` means a non-capability label.
    pub actions: Vec<NotificationAction>,
}

/// A single action shown on a notification.
#[derive(Debug, Clone)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
    pub kind: ActionKind,
    pub capability: Option<ActionCapability>,
}

impl Default for NotificationAction {
    fn default() -> Self {
        Self {
            key: String::new(),
            label: String::new(),
            kind: ActionKind::Immediate,
            capability: None,
        }
    }
}

/// D-Bus proxy for the freedesktop notification server.
#[zbus::proxy(
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications",
    interface = "org.freedesktop.Notifications"
)]
pub trait Notifications {
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, zbus::zvariant::Value<'static>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

/// Persistent ID → action mapping so action invocations can be routed.
#[derive(Debug, Default)]
pub struct NotificationState {
    /// Maps notification id to the list of actions it offered.
    by_id: HashMap<u32, Vec<NotificationAction>>,
}

impl NotificationState {
    pub fn insert(&mut self, id: u32, actions: Vec<NotificationAction>) {
        self.by_id.insert(id, actions);
    }

    pub fn take(&mut self, id: u32) -> Vec<NotificationAction> {
        self.by_id.remove(&id).unwrap_or_default()
    }
}

/// Parsed action data serialized into the notification action key.
#[derive(Debug, Clone, Deserialize)]
struct EncodedAction {
    capability_id: String,
}

impl EncodedAction {
    fn encode(capability_id: &str, label: &str) -> String {
        format!("{}|{}", capability_id, label.replace('|', "\\|"))
    }

    fn decode(key: &str) -> Option<Self> {
        let mut parts = key.splitn(2, '|');
        let capability_id = parts.next()?.to_string();
        let _label = parts.next()?;
        Some(Self { capability_id })
    }
}

/// Show a desktop notification with action buttons.
pub async fn show(
    proxy: &NotificationsProxy<'static>,
    state: &std::sync::Mutex<NotificationState>,
    _transport: &dyn DesktopTransport,
    notification: &DesktopNotification,
) -> Result<u32, DesktopError> {
    let mut action_pairs: Vec<String> = Vec::new();

    for action in &notification.actions {
        let key = if let Some(cap) = &action.capability {
            EncodedAction::encode(&cap.id, &action.label)
        } else {
            action.key.clone()
        };

        action_pairs.push(key);
        action_pairs.push(action.label.clone());
    }

    // If no actions were provided, always offer a default "Open".
    if action_pairs.is_empty() {
        action_pairs.push("open".into());
        action_pairs.push("開く".into());
    }

    // Hints: urgency 1 (normal) and desktop-entry for icon association.
    let mut hints: HashMap<&str, zbus::zvariant::Value<'static>> = HashMap::new();
    hints.insert("urgency", zbus::zvariant::Value::U8(1));
    hints.insert("desktop-entry", zbus::zvariant::Value::Str("takusu".into()));

    let action_refs: Vec<&str> = action_pairs.iter().map(|s| s.as_str()).collect();

    let id = proxy
        .notify(
            "takusu",
            notification.id,
            "takusu",
            &notification.title,
            &notification.body,
            &action_refs,
            hints,
            -1,
        )
        .await
        .map_err(|e| DesktopError::Notification(e.to_string()))?;

    state
        .lock()
        .unwrap()
        .insert(id, notification.actions.clone());
    Ok(id)
}

/// Route a single notification action to the transport.
pub async fn route_notification_action(
    state: &std::sync::Mutex<NotificationState>,
    transport: &dyn DesktopTransport,
    notification_id: u32,
    action_key: &str,
) -> Result<(), DesktopError> {
    let actions = state.lock().unwrap().take(notification_id);

    if let Some(parsed) = EncodedAction::decode(action_key) {
        if let Some(action) = actions.iter().find(|a| {
            a.capability
                .as_ref()
                .is_some_and(|c| c.id == parsed.capability_id)
        }) {
            if let Some(cap) = &action.capability {
                let _presentation = transport.authorize_action(cap, None).await?;
                if let Some(event_id) = cap.event_id.as_deref() {
                    transport
                        .update_planner_event_state(
                            event_id,
                            takusu_contracts::EventDeliveryState::Resolved,
                        )
                        .await?;
                }
            }
        } else {
            tracing::warn!(notification_id, action_key, "unknown notification action");
        }
        return Ok(());
    }

    if action_key.starts_with("open") {
        tracing::info!("notification {} opened", notification_id);
        return Ok(());
    }

    if let Some(action) = actions.iter().find(|a| a.key == action_key) {
        match action.kind {
            ActionKind::Panel => {
                let response = transport.send_command(SurfaceCommand::OpenPanel).await?;
                if !response.accepted {
                    tracing::warn!(
                        command = ?SurfaceCommand::OpenPanel,
                        reason = ?response.reason,
                        "panel command not accepted"
                    );
                }
            }
            ActionKind::Approval => {
                let response = transport.send_command(SurfaceCommand::OpenApproval).await?;
                if !response.accepted {
                    tracing::warn!(
                        command = ?SurfaceCommand::OpenApproval,
                        reason = ?response.reason,
                        "approval command not accepted"
                    );
                }
            }
            ActionKind::Immediate => {
                tracing::warn!(
                    notification_id,
                    action_key,
                    "unroutable immediate notification action"
                );
            }
        }
    } else {
        tracing::warn!(notification_id, action_key, "unknown notification action");
    }

    Ok(())
}

/// Listen for `ActionInvoked` and route to the transport.
pub async fn run_action_listener(
    proxy: &NotificationsProxy<'static>,
    state: Arc<std::sync::Mutex<NotificationState>>,
    transport: Arc<dyn DesktopTransport + Send + Sync>,
) -> Result<(), DesktopError> {
    let mut action_stream = proxy
        .receive_action_invoked()
        .await
        .map_err(|e| DesktopError::Notification(e.to_string()))?;

    while let Some(signal) = action_stream.next().await {
        let args = signal
            .args()
            .map_err(|e| DesktopError::Notification(e.to_string()))?;
        if let Err(e) =
            route_notification_action(&state, transport.as_ref(), *args.id(), args.action_key())
                .await
        {
            tracing::warn!(error=%e, notification_id=args.id(), action_key=args.action_key(), "failed to route notification action");
        }
    }

    Ok(())
}

/// Connect to the session bus and return a notification proxy.
pub async fn connect() -> Result<(Connection, NotificationsProxy<'static>), DesktopError> {
    let connection = Connection::session()
        .await
        .map_err(|e| DesktopError::Notification(e.to_string()))?;
    let proxy = NotificationsProxy::new(&connection)
        .await
        .map_err(|e| DesktopError::Notification(e.to_string()))?;
    Ok((connection, proxy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;
    use takusu_agent::surface::SurfaceStateMachine;

    fn test_capability(id: &str) -> ActionCapability {
        ActionCapability {
            id: id.into(),
            event_id: None,
            device_id: "desktop".into(),
            action: "start".into(),
            input_path: takusu_agent::capability::InputPath::NotificationCapability,
            expires_at: jiff::Timestamp::now(),
            one_shot: true,
            task_id: "task-1".into(),
            snooze_minutes: None,
            snooze_target: None,
            quantity_done: None,
            quantity_total: None,
            note: None,
            scheduled_at: None,
            request: None,
        }
    }

    #[test]
    fn encoded_action_roundtrips() {
        let key = EncodedAction::encode("cap-abc", "今から始める");
        let parsed = EncodedAction::decode(&key).unwrap();
        assert_eq!(parsed.capability_id, "cap-abc");

        // Pipes in the label are escaped.
        let key = EncodedAction::encode("cap-abc", "a|b");
        assert!(EncodedAction::decode(&key).is_some());
    }

    #[tokio::test]
    async fn routes_notification_action_to_transport() {
        let cap = test_capability("cap-123");
        let state = std::sync::Mutex::new(NotificationState::default());
        state.lock().unwrap().insert(
            42,
            vec![NotificationAction {
                key: "act".into(),
                label: "今から始める".into(),
                kind: ActionKind::Immediate,
                capability: Some(cap.clone()),
            }],
        );

        let snapshot = SurfaceStateMachine::new().snapshot();
        let transport = MockTransport::new(snapshot);

        let key = EncodedAction::encode(&cap.id, "今から始める");
        route_notification_action(&state, &transport, 42, &key)
            .await
            .unwrap();

        assert_eq!(transport.authorized(), vec!["cap-123"]);
    }

    #[tokio::test]
    async fn ignores_unknown_or_open_action_keys() {
        let cap = test_capability("cap-123");
        let state = std::sync::Mutex::new(NotificationState::default());
        state.lock().unwrap().insert(
            42,
            vec![NotificationAction {
                key: "act".into(),
                label: "今から始める".into(),
                kind: ActionKind::Immediate,
                capability: Some(cap),
            }],
        );

        let snapshot = SurfaceStateMachine::new().snapshot();
        let transport = MockTransport::new(snapshot);

        // Unknown capability id.
        route_notification_action(&state, &transport, 42, "cap-missing|foo")
            .await
            .unwrap();
        // Open action.
        route_notification_action(&state, &transport, 42, "open")
            .await
            .unwrap();

        assert!(transport.authorized().is_empty());
    }
}
