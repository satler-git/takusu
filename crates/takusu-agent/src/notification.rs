//! Start-time notification evaluation for the resident planner (WI-4).
//!
//! This is the minimal pre-event-engine path: it looks at the current active
//! schedule, finds upcoming task start times, and builds a `CheckInCard` for
//! each one with server-issued one-shot capabilities for the notification
//! actions.

use jiff::Timestamp as JiffTimestamp;
use takusu_client::{Client, ScheduleEntry, TaskQuery, TaskRow};
use takusu_types::{TaskStatus, TaskStatusFilter};

use crate::capability::{ActionCapability, CapabilityRequest, InputPath, mint_capability};
use crate::presentation::{Action, ActionGroup, ActionKind, CheckInCard, Presentation};
use crate::tool::{InvalidArgsError, ToolError};

/// A local notification to post at a task's start time.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct StartTimeNotification {
    pub task_id: String,
    /// Wall-clock title for the notification.
    pub title: String,
    /// Wall-clock body for the notification.
    pub body: String,
    /// When the notification should be delivered.
    #[schemars(with = "String")]
    pub scheduled_at: JiffTimestamp,
    /// The `CheckInCard` presentation rendered for this task.
    pub check_in: Presentation,
}

/// Response body for the start-time notification endpoint.
///
/// `Versioned<Vec<_>>` cannot be flattened by serde, so the list is wrapped in
/// a struct.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct StartTimeNotificationList {
    pub notifications: Vec<StartTimeNotification>,
}

/// Request body for the start-time notification endpoint.
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct StartTimeNotificationRequest {
    /// Maximum number of upcoming start-time notifications to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Device identifier bound into the issued capabilities.
    #[serde(default = "default_device_id")]
    pub device_id: String,
    /// IANA or fixed-offset time zone used to format wall-clock times in
    /// notification bodies. Defaults to UTC.
    #[serde(default)]
    pub tz: Option<String>,
}

fn default_device_id() -> String {
    "mobile".to_string()
}

fn default_limit() -> usize {
    10
}

/// Evaluate the next start-time notifications for the current device.
///
/// Returns one notification per scheduled task whose `start_at` is in the
/// future, up to `limit`. Each notification carries a `CheckInCard` with
/// 「行動」 (着手) and 「ズラす」 (10分後, 組み直す) action groups. The
/// immediate actions carry server-issued one-shot capabilities.
pub async fn evaluate_start_time_notifications(
    client: &Client,
    request: &StartTimeNotificationRequest,
) -> Result<StartTimeNotificationList, crate::AgentError> {
    let tz = request
        .tz
        .as_deref()
        .and_then(|t| takusu_types::parse_timezone(t).ok())
        .unwrap_or(jiff::tz::TimeZone::UTC);
    let now = JiffTimestamp::now();
    let schedule = client.get_schedule().await?;
    let tasks = client
        .list_tasks(&TaskQuery {
            status: Some(TaskStatusFilter::Scheduled),
            ..Default::default()
        })
        .await?;
    let task_map: std::collections::HashMap<String, TaskRow> =
        tasks.into_iter().map(|t| (t.id.clone(), t)).collect();

    let today = now.to_zoned(tz.clone()).date();
    let tomorrow = today
        .tomorrow()
        .map_err(|e| crate::AgentError::Tool(ToolError::Other(e.into())))?;

    let mut upcoming: Vec<(&ScheduleEntry, &TaskRow)> = schedule
        .schedule
        .as_inner()
        .iter()
        .filter(|e| e.start_at.0 > now)
        .filter(|e| {
            let start_date = e.start_at.0.to_zoned(tz.clone()).date();
            start_date == today || start_date == tomorrow
        })
        .filter_map(|e| task_map.get(&e.task_id).map(|t| (e, t)))
        .filter(|(_, t)| t.status == TaskStatus::Scheduled)
        .collect();

    // Sort by start time and take the next ones.
    upcoming.sort_by_key(|a| a.0.start_at.0);
    upcoming.truncate(request.limit);

    let mut notifications = Vec::with_capacity(upcoming.len());
    for (entry, task) in upcoming {
        let scheduled_at = entry.start_at.0;
        let start_label = format_start_time(scheduled_at, &tz);
        let question = format!("「{}」の開始時刻です", task.title);

        let start_cap = mint_start_capability(&request.device_id, &task.id, entry.start_at)?;
        let snooze_cap = mint_snooze_capability(&request.device_id, &task.id, entry.start_at)?;

        let check_in = build_check_in(&question, &start_cap, &snooze_cap, &task.id, &task.title)?;

        notifications.push(StartTimeNotification {
            task_id: task.id.clone(),
            title: question.clone(),
            body: format!("{} ({start_label})", task.title),
            scheduled_at,
            check_in: Presentation::CheckIn(check_in),
        });
    }

    Ok(StartTimeNotificationList { notifications })
}

fn mint_start_capability(
    device_id: &str,
    task_id: &str,
    scheduled_at: takusu_types::Timestamp,
) -> Result<ActionCapability, crate::AgentError> {
    let request = CapabilityRequest {
        task_id: task_id.to_string(),
        action: "start".to_string(),
        device_id: device_id.to_string(),
        scheduled_at: Some(scheduled_at),
        ..Default::default()
    };
    Ok(mint_capability(request, InputPath::NotificationCapability))
}

fn mint_snooze_capability(
    device_id: &str,
    task_id: &str,
    scheduled_at: takusu_types::Timestamp,
) -> Result<ActionCapability, crate::AgentError> {
    let request = CapabilityRequest {
        task_id: task_id.to_string(),
        action: "delay".to_string(),
        device_id: device_id.to_string(),
        snooze_minutes: Some(10),
        scheduled_at: Some(scheduled_at),
        ..Default::default()
    };
    Ok(mint_capability(request, InputPath::NotificationCapability))
}

fn build_check_in(
    question: &str,
    start_cap: &ActionCapability,
    snooze_cap: &ActionCapability,
    task_id: &str,
    title: &str,
) -> Result<CheckInCard, crate::AgentError> {
    let act = ActionGroup::new(
        "行動",
        vec![Action {
            id: start_cap.id.clone(),
            label: "着手".to_string(),
            kind: ActionKind::Immediate,
            capability: Some(start_cap.clone()),
        }],
    )
    .map_err(|e| crate::AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::no_field(e))))?;

    let shift = ActionGroup::new(
        "ズラす",
        vec![
            Action {
                id: snooze_cap.id.clone(),
                label: "10分後".to_string(),
                kind: ActionKind::Immediate,
                capability: Some(snooze_cap.clone()),
            },
            Action {
                id: task_id.to_string(),
                label: "組み直す".to_string(),
                kind: ActionKind::Panel,
                capability: None,
            },
        ],
    )
    .map_err(|e| crate::AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::no_field(e))))?;

    CheckInCard::new(format!("{question} ({title})"), act, shift)
        .map_err(|e| crate::AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::no_field(e))))
}

fn format_start_time(ts: JiffTimestamp, tz: &jiff::tz::TimeZone) -> String {
    let zoned = ts.to_zoned(tz.clone());
    format!("{:02}:{:02}", zoned.hour(), zoned.minute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_check_in_has_both_groups() {
        let start = mint_capability(
            CapabilityRequest {
                task_id: "task-1".into(),
                action: "start".into(),
                device_id: "mobile".into(),
                ..Default::default()
            },
            InputPath::NotificationCapability,
        );
        let snooze = mint_capability(
            CapabilityRequest {
                task_id: "task-1".into(),
                action: "delay".into(),
                device_id: "mobile".into(),
                snooze_minutes: Some(10),
                ..Default::default()
            },
            InputPath::NotificationCapability,
        );
        let card = build_check_in(
            "「レポート」の開始時刻です",
            &start,
            &snooze,
            "task-1",
            "レポート",
        )
        .unwrap();
        assert_eq!(card.act.actions.as_slice()[0].label, "着手");
        assert_eq!(card.shift.actions.as_slice()[0].label, "10分後");
        assert_eq!(card.shift.actions.as_slice()[1].label, "組み直す");
        assert!(card.act.actions.as_slice()[0].capability.is_some());
        assert!(card.shift.actions.as_slice()[1].capability.is_none());
    }

    #[test]
    fn versioned_start_time_notification_list_serializes() {
        let ts = JiffTimestamp::from_second(1_000_000_000).unwrap();
        let list = StartTimeNotificationList {
            notifications: vec![StartTimeNotification {
                task_id: "task-1".into(),
                title: "title".into(),
                body: "body".into(),
                scheduled_at: ts,
                check_in: Presentation::Text {
                    text: "ok".to_string(),
                },
            }],
        };
        let versioned = crate::transport::Versioned {
            version: 1,
            value: list,
        };
        let json = serde_json::to_string(&versioned).expect("must serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must parse");
        assert_eq!(parsed["version"], 1);
        assert!(parsed["notifications"].is_array());
    }

    #[test]
    fn format_start_time_uses_timezone() {
        let ts = jiff::civil::date(2026, 8, 15)
            .at(10, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp();
        let jst = jiff::tz::TimeZone::get("Asia/Tokyo").unwrap();
        assert_eq!(format_start_time(ts, &jst), "19:00");
        assert_eq!(format_start_time(ts, &jiff::tz::TimeZone::UTC), "10:00");
    }

    #[test]
    fn start_capability_expires_after_scheduled_time() {
        let scheduled = JiffTimestamp::from_second(1_000_000_000).unwrap();
        let cap = mint_capability(
            CapabilityRequest {
                task_id: "task-1".into(),
                action: "start".into(),
                device_id: "mobile".into(),
                scheduled_at: Some(takusu_types::Timestamp(scheduled)),
                ..Default::default()
            },
            InputPath::NotificationCapability,
        );
        assert!(cap.expires_at > scheduled);
        assert_eq!(cap.input_path, InputPath::NotificationCapability);
    }
}
