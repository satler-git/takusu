//! Contact policy and speech policy for the resident agent (WI-17).
//!
//! The policy is split into two layers:
//!
//! 1. **Resident authority filtering** (`ContactPolicy`): decides which planner
//!    events are committed to the shared ledger. It enforces the per-day cap,
//!    unresponsive-frequency decay, and timed suppression.
//! 2. **Device delivery mode** (`SpeechPolicy`): each surface calls
//!    [`delivery_mode_for`] when claiming/replaying a ledger event to decide
//!    whether to speak, notify, suppress, or defer based on the device's own
//!    speech capability, private channel state, and quiet hours.
//!
//! Both layers are pure functions so they can be unit-tested and so the same
//! logic runs on the resident authority and on every surface.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use takusu_types::Timestamp;

use crate::events::{PlannerEvent, PlannerEventKind, Urgency};

/// Maximum number of proactive unknown-activity check-ins per day.
///
/// Start-time reminders, deadline notifications, routine cues, and current-task
/// cards are not counted against this cap.
pub const DAILY_CHECK_IN_CAP: usize = 5;

/// Time window during which a "ほっといて" suppression applies.
pub const SUPPRESSION_MINUTES: i64 = 60;

/// Decay threshold: after this many ignored proactive check-ins within a day,
/// further non-urgent proactive check-ins are suppressed for the rest of the
/// day.
pub const IGNORED_DECAY_THRESHOLD: usize = 2;

/// Types of planner events that ask about unknown activity and count against
/// the daily check-in cap.
pub fn is_proactive_check_in(kind: PlannerEventKind) -> bool {
    matches!(
        kind,
        PlannerEventKind::TaskNonStartContinued
            | PlannerEventKind::UnclassifiedGapContinued
            | PlannerEventKind::DistributionOverrun
            | PlannerEventKind::CarriedOverIncomplete
    )
}

/// Types of events that are start-time or deadline notifications and are
/// excluded from the check-in cap.
pub fn is_scheduled_notification(kind: PlannerEventKind) -> bool {
    matches!(
        kind,
        PlannerEventKind::TaskStartTimeReached | PlannerEventKind::DeadlineViolation
    )
}

/// Mutable contact-policy state shared across a day.
///
/// The resident authority keeps one `ContactPolicyState` in sync with the
/// committed event ledger. It is intentionally minimal and reconstructible from
/// recent ledger rows so a device re-registration or server restart does not
/// lose suppression intent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactPolicyState {
    /// Count of proactive check-ins already committed today that ask about
    /// unknown activity.
    pub proactive_check_in_count_today: usize,
    /// Count of ignored (not answered and not acted on) proactive check-ins
    /// today, used for frequency decay.
    pub ignored_check_in_count_today: usize,
    /// Wall-clock timestamp until which proactive contacts are suppressed.
    pub suppress_until: Option<Timestamp>,
}

impl ContactPolicyState {
    /// Build contact state from a recent ledger view.
    ///
    /// `today_start` and `today_end` are the day boundaries in the user's
    /// timezone. Each summary records whether the event was delivered but
    /// ignored by the user.
    pub fn from_ledger(
        committed_today: &[CommittedContactSummary],
        now: Timestamp,
        today_start: Timestamp,
        today_end: Timestamp,
        suppress_until: Option<Timestamp>,
    ) -> Self {
        let mut proactive = 0;
        let mut ignored = 0;

        for row in committed_today {
            if row.created_at < today_start || row.created_at > today_end {
                continue;
            }
            if row.kind.is_some_and(is_proactive_check_in) {
                proactive += 1;
                if row.ignored {
                    ignored += 1;
                }
            }
        }

        // If suppression is in the past, drop it.
        let suppress_until = suppress_until.filter(|t| *t > now);

        Self {
            proactive_check_in_count_today: proactive,
            ignored_check_in_count_today: ignored,
            suppress_until,
        }
    }

    /// Whether an active suppression window covers `now`.
    pub fn is_suppressed(&self, now: Timestamp) -> bool {
        self.suppress_until.is_some_and(|t| t > now)
    }

    /// Record that a proactive check-in was just delivered and was ignored.
    pub fn mark_ignored(&mut self, _now: Timestamp) {
        self.ignored_check_in_count_today += 1;
    }

    /// Start a timed suppression from `now`.
    pub fn suppress_for(&mut self, now: Timestamp, minutes: i64) {
        let target = add_minutes(now, minutes);
        self.suppress_until = Some(
            self.suppress_until
                .map_or(target, |existing| existing.max(target)),
        );
    }
}

/// A lightweight summary of an event already committed to the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedContactSummary {
    pub event_id: String,
    pub kind: Option<PlannerEventKind>,
    pub created_at: Timestamp,
    /// Whether the event was delivered but ignored by the user.
    pub ignored: bool,
}

/// A device-local speech policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechPolicy {
    /// Whether the device is allowed to speak proactively.
    pub can_speak_proactively: bool,
    /// Whether the user is currently in quiet hours (sleep, DND, etc.).
    pub quiet_hours: bool,
    /// True if the device is the resident evaluator and the audio path is
    /// private (Android: earphones / Bluetooth headset; desktop: always true).
    pub private_output: bool,
    /// True if there is an ongoing explicit voice session on this device.
    pub ongoing_voice_conversation: bool,
}

/// How a planner event should be delivered to a specific device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    /// Speak the presentation via TTS on this device.
    ///
    /// Only selected when proactive speech is allowed and the channel is
    /// private. Reactive speech is always allowed.
    Speak,
    /// Show a notification (or visual surface update) on this device.
    Notify,
    /// Drop the contact without re-asking.
    Suppress,
    /// Hold delivery until quiet hours end.
    DeferQuietHours,
}

/// Result of filtering a batch of events through the resident contact policy.
#[derive(Debug, Clone)]
pub struct ContactFilterResult {
    /// Events that should be committed to the ledger.
    pub committed: Vec<PlannerEvent>,
    /// Event IDs that were suppressed by policy.
    pub suppressed_ids: Vec<String>,
    /// Reason for each suppression, keyed by event ID.
    pub suppression_reasons: HashMap<String, SuppressionReason>,
}

/// Reason a contact was suppressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionReason {
    /// Explicit "ほっといて" suppression is active.
    Suppressed,
    /// Daily proactive check-in cap reached.
    DailyCap,
    /// Frequency decay after repeated ignored check-ins.
    FrequencyDecay,
}

/// Filter planner events through the resident contact policy.
///
/// This is the resident-authority layer: it removes events that should not be
/// committed to the shared ledger because of a global suppression, cap, or
/// decay rule. It does **not** decide the per-device delivery mode; that is
/// [`delivery_mode_for`].
pub fn filter_events(
    events: Vec<PlannerEvent>,
    state: &mut ContactPolicyState,
    now: Timestamp,
) -> ContactFilterResult {
    let mut committed = Vec::with_capacity(events.len());
    let mut suppressed_ids = Vec::new();
    let mut suppression_reasons = HashMap::new();

    let suppressed = state.is_suppressed(now);
    let decayed = state.ignored_check_in_count_today >= IGNORED_DECAY_THRESHOLD;

    for event in events {
        let event_id = event.id.clone();
        let kind = event.kind;
        let is_check_in = is_proactive_check_in(kind);
        let is_scheduled = is_scheduled_notification(kind);

        // Scheduled notifications and emergency/high-urgency events bypass the
        // daily cap and frequency decay. They still respect explicit user
        // suppression from the device state.
        let urgent = matches!(event.urgency, Urgency::Emergency | Urgency::High);

        if suppressed && !urgent && !is_scheduled {
            suppressed_ids.push(event_id.clone());
            suppression_reasons.insert(event_id, SuppressionReason::Suppressed);
            continue;
        }

        if is_check_in && !urgent {
            if state.proactive_check_in_count_today >= DAILY_CHECK_IN_CAP {
                suppressed_ids.push(event_id.clone());
                suppression_reasons.insert(event_id, SuppressionReason::DailyCap);
                continue;
            }
            if decayed {
                suppressed_ids.push(event_id.clone());
                suppression_reasons.insert(event_id, SuppressionReason::FrequencyDecay);
                continue;
            }
            state.proactive_check_in_count_today += 1;
        }

        committed.push(event);
    }

    ContactFilterResult {
        committed,
        suppressed_ids,
        suppression_reasons,
    }
}

/// Decide how a single ledger event should be delivered to a device.
///
/// This is the per-device layer. It must be called for **every** replay/claim,
/// including on non-resident devices, because one device may speak while
/// another only shows a notification.
pub fn delivery_mode_for(
    event: &PlannerEvent,
    policy: &SpeechPolicy,
    state: &ContactPolicyState,
    now: Timestamp,
) -> DeliveryMode {
    // Quiet hours always defer, even for high-urgency events.
    if policy.quiet_hours {
        return DeliveryMode::DeferQuietHours;
    }

    // Explicit timed suppression drops proactive check-ins.
    if state.is_suppressed(now) && event.urgency != Urgency::Emergency {
        return DeliveryMode::Suppress;
    }

    // Scheduled notifications are always delivered as a notification. High-
    // priority alerts are also delivered as a notification unless the device
    // has both proactive speech permission and a private output channel.
    let is_scheduled = is_scheduled_notification(event.kind);
    let can_speak = policy.can_speak_proactively
        && (policy.private_output || policy.ongoing_voice_conversation);
    let can_speak_private = policy.can_speak_proactively && policy.private_output;

    if is_scheduled {
        return DeliveryMode::Notify;
    }

    if event.urgency == Urgency::High {
        if can_speak_private {
            return DeliveryMode::Speak;
        }
        return DeliveryMode::Notify;
    }

    if can_speak {
        DeliveryMode::Speak
    } else {
        DeliveryMode::Notify
    }
}

/// Whether a `delay`/`snooze` should be followed by the one-time postpone
/// reason hook.
///
/// Short snoozes of tens of minutes never ask a reason. A move that crosses a
/// time band boundary or exceeds the short-snooze threshold asks exactly once.
pub fn should_ask_postpone_reason(snooze_minutes: i64) -> bool {
    snooze_minutes > 30
}

/// Template for the one-time postpone reason check-in.
pub fn postpone_reason_check_in(task_title: &str) -> String {
    format!(
        "{}をずらしました。なにか詰まってる? それとも時間だけ?",
        task_title
    )
}

fn add_minutes(timestamp: Timestamp, minutes: i64) -> Timestamp {
    Timestamp::from_second(
        timestamp
            .as_second()
            .saturating_add(minutes.saturating_mul(60)),
    )
    .unwrap_or(timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{PlannerEvent, PlannerEventKind, Urgency};
    use crate::presentation::{Action, ActionGroup, ActionKind, CheckInCard, Presentation};

    fn ts(sec: i64) -> Timestamp {
        Timestamp::from_second(sec).unwrap()
    }

    fn check_in_card(question: &str) -> CheckInCard {
        let act = ActionGroup::new(
            "行動",
            vec![Action {
                id: "act".into(),
                label: "行動する".into(),
                kind: ActionKind::Immediate,
                capability: None,
            }],
        )
        .unwrap();
        let shift = ActionGroup::new(
            "ズラす",
            vec![Action {
                id: "shift".into(),
                label: "ずらす".into(),
                kind: ActionKind::Panel,
                capability: None,
            }],
        )
        .unwrap();
        CheckInCard::new(question, act, shift).unwrap()
    }

    fn event(kind: PlannerEventKind, urgency: Urgency, id: &str) -> PlannerEvent {
        PlannerEvent {
            id: id.into(),
            kind,
            task_ref: None,
            band: None,
            presentation: Presentation::CheckIn(check_in_card("テスト")),
            urgency,
            schedule_revision: 1,
            distribution_revision: None,
            due_at: ts(0),
        }
    }

    fn speech_policy_all_true() -> SpeechPolicy {
        SpeechPolicy {
            can_speak_proactively: true,
            quiet_hours: false,
            private_output: true,
            ongoing_voice_conversation: false,
        }
    }

    #[test]
    fn scheduled_notifications_are_excluded_from_cap() {
        let mut state = ContactPolicyState {
            proactive_check_in_count_today: DAILY_CHECK_IN_CAP,
            ..Default::default()
        };
        let events = vec![event(
            PlannerEventKind::TaskStartTimeReached,
            Urgency::Normal,
            "start-1",
        )];
        let result = filter_events(events, &mut state, ts(0));
        assert_eq!(result.committed.len(), 1);
        assert!(result.suppressed_ids.is_empty());
    }

    #[test]
    fn proactive_check_in_cap_suppresses_additional_events() {
        let mut state = ContactPolicyState {
            proactive_check_in_count_today: DAILY_CHECK_IN_CAP,
            ..Default::default()
        };
        let events = vec![event(
            PlannerEventKind::UnclassifiedGapContinued,
            Urgency::Normal,
            "gap-1",
        )];
        let result = filter_events(events, &mut state, ts(0));
        assert!(result.committed.is_empty());
        assert_eq!(result.suppressed_ids, vec!["gap-1".to_string()]);
        assert_eq!(
            result.suppression_reasons["gap-1"],
            SuppressionReason::DailyCap
        );
    }

    #[test]
    fn ignored_decay_suppresses_non_urgent_check_ins() {
        let mut state = ContactPolicyState {
            ignored_check_in_count_today: IGNORED_DECAY_THRESHOLD,
            ..Default::default()
        };
        let events = vec![
            event(
                PlannerEventKind::DistributionOverrun,
                Urgency::Normal,
                "overrun-1",
            ),
            event(
                PlannerEventKind::DistributionOverrun,
                Urgency::High,
                "overrun-2",
            ),
        ];
        let result = filter_events(events, &mut state, ts(0));
        assert_eq!(result.committed.len(), 1);
        assert_eq!(result.committed[0].id, "overrun-2");
        assert_eq!(result.suppressed_ids, vec!["overrun-1".to_string()]);
    }

    #[test]
    fn suppression_window_drops_events() {
        let mut state = ContactPolicyState::default();
        state.suppress_for(ts(0), 60);
        let events = vec![event(
            PlannerEventKind::TaskNonStartContinued,
            Urgency::Normal,
            "non-start-1",
        )];
        let result = filter_events(events, &mut state, ts(30 * 60));
        assert!(result.committed.is_empty());
    }

    #[test]
    fn emergency_events_bypass_suppression_and_cap() {
        let mut state = ContactPolicyState {
            proactive_check_in_count_today: DAILY_CHECK_IN_CAP,
            ignored_check_in_count_today: IGNORED_DECAY_THRESHOLD,
            ..Default::default()
        };
        state.suppress_for(ts(0), 60);
        let events = vec![event(
            PlannerEventKind::UnclassifiedGapContinued,
            Urgency::Emergency,
            "emergency-1",
        )];
        let result = filter_events(events, &mut state, ts(30 * 60));
        assert_eq!(result.committed.len(), 1);
    }

    #[test]
    fn desktop_speaks_by_default() {
        let event = event(PlannerEventKind::DistributionOverrun, Urgency::Normal, "e1");
        let policy = SpeechPolicy {
            can_speak_proactively: true,
            quiet_hours: false,
            private_output: true,
            ongoing_voice_conversation: false,
        };
        let state = ContactPolicyState::default();
        assert_eq!(
            delivery_mode_for(&event, &policy, &state, ts(0)),
            DeliveryMode::Speak
        );
    }

    #[test]
    fn android_requires_private_output_to_speak() {
        let event = event(PlannerEventKind::DistributionOverrun, Urgency::Normal, "e1");
        let mut policy = speech_policy_all_true();
        policy.private_output = false;
        policy.ongoing_voice_conversation = false;
        let state = ContactPolicyState::default();
        assert_eq!(
            delivery_mode_for(&event, &policy, &state, ts(0)),
            DeliveryMode::Notify
        );

        policy.ongoing_voice_conversation = true;
        assert_eq!(
            delivery_mode_for(&event, &policy, &state, ts(0)),
            DeliveryMode::Speak
        );
    }

    #[test]
    fn quiet_hours_defer_everything() {
        let event = event(PlannerEventKind::DistributionOverrun, Urgency::High, "e1");
        let policy = SpeechPolicy {
            can_speak_proactively: true,
            quiet_hours: true,
            private_output: true,
            ongoing_voice_conversation: false,
        };
        let state = ContactPolicyState::default();
        assert_eq!(
            delivery_mode_for(&event, &policy, &state, ts(0)),
            DeliveryMode::DeferQuietHours
        );
    }

    #[test]
    fn scheduled_start_time_is_not_spoken_proactively_without_private_channel() {
        let event = event(
            PlannerEventKind::TaskStartTimeReached,
            Urgency::Normal,
            "start-1",
        );
        let mut policy = speech_policy_all_true();
        policy.private_output = false;
        let state = ContactPolicyState::default();
        assert_eq!(
            delivery_mode_for(&event, &policy, &state, ts(0)),
            DeliveryMode::Notify
        );
    }

    #[test]
    fn short_snooze_skips_postpone_reason() {
        assert!(!should_ask_postpone_reason(10));
        assert!(should_ask_postpone_reason(60));
    }

    #[test]
    fn postpone_reason_template_includes_title() {
        let text = postpone_reason_check_in("レポート");
        assert!(text.contains("レポート"));
        assert!(text.contains("なにか詰まってる"));
    }
}
