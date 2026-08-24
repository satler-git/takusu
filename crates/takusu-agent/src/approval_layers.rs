//! Approval layer classification for proposed changes.
//!
//! Every operation is classified by the damage of misattribution (the wrong
//! utterance or tap being treated as the user's intent) and the trusted input
//! path. Classification runs **before** any `Permissions` default grant is
//! consulted, so a default-granted operation arriving over an unverified path
//! cannot bypass its required layer.
//!
//! The four layers are:
//!
//! - `Immediate`: a valid screen/notification capability, or an explicit
//!   continuous session whose identity was established at start. Only
//!   `start`, `pause`, and a short bounded `snooze` are this cheap.
//! - `AmbientImmediate`: a wake-word `start`/`pause`. Speaker verification is
//!   required; on failure the operation degrades to a screen fallback.
//! - `VoiceConfirmed`: a readback of the change followed by a closed-vocabulary
//!   affirmative and passing speaker verification. Covers `progress`,
//!   `complete`, and single task creates/edits with readable knock-on effects.
//! - `ScreenRequired`: everything else, including `delete`, whole-schedule
//!   replacement, and unreadable impact. Falls back here on ambiguity,
//!   silence, timeout, or verification failure.

use serde::{Deserialize, Serialize};

use crate::capability::InputPath;
use crate::tool::{ChangeOperation, ProposedChange, TargetKind};

/// Maximum number of minutes a `Snooze` can be and still be an immediate-layer
/// reversible operation. Cross-time-band moves use ordinary approval-gated
/// `Move` or schedule changes.
pub const MAX_SHORT_SNOOZE_MINUTES: i64 = 30;

/// The four approval layers, ordered from least to most restrictive.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalLayer {
    Immediate,
    AmbientImmediate,
    VoiceConfirmed,
    ScreenRequired,
}

impl ApprovalLayer {
    /// Whether this layer requires explicit voice confirmation (readback +
    /// affirmative) rather than a screen approval.
    #[allow(dead_code)]
    pub fn requires_voice_confirmation(self) -> bool {
        matches!(self, Self::VoiceConfirmed)
    }

    /// Whether this layer requires speaker verification before execution.
    #[allow(dead_code)]
    pub fn requires_speaker_verification(self) -> bool {
        matches!(self, Self::AmbientImmediate | Self::VoiceConfirmed)
    }

    /// Whether this layer can execute immediately after the input evidence is
    /// validated, with no further round trip.
    pub fn is_immediate(self) -> bool {
        matches!(self, Self::Immediate)
    }

    /// Whether this layer executes as an ambient-immediate action, i.e. the
    /// result is read back but no explicit confirmation is required.
    #[allow(dead_code)]
    pub fn is_ambient_immediate(self) -> bool {
        matches!(self, Self::AmbientImmediate)
    }
}

/// Classify a single proposed change using its trusted input path.
pub fn classify(change: &ProposedChange, input_path: InputPath) -> ApprovalLayer {
    match input_path {
        InputPath::PlainText => classify_plain_text(change),
        InputPath::ScreenCapability | InputPath::NotificationCapability => {
            classify_capability(change)
        }
        InputPath::ExplicitVoiceSession => classify_explicit_voice(change),
        InputPath::AmbientWakeWord => classify_ambient_wake_word(change),
    }
}

/// Classify a set of proposed changes using the trusted input path.
///
/// The set's layer is the most restrictive of the individual layers, with one
/// refinement: a compound proposal on tasks with at least one
/// voice-confirmable create/edit/progress/complete, and where the only
/// "heavier" operations are `Move`/`Snooze` with readable knock-on effects,
/// stays `VoiceConfirmed`. A set containing `Delete`, whole-schedule
/// `Generate`/`Reschedule`, or habit/skill/memory changes remains
/// `ScreenRequired`.
pub fn classify_set(changes: &[ProposedChange], input_path: InputPath) -> ApprovalLayer {
    if changes.is_empty() {
        return ApprovalLayer::Immediate;
    }

    let mut layer = ApprovalLayer::Immediate;
    let mut has_voice_confirmed_op = false;
    let mut has_long_move_or_snooze = false;
    let mut all_are_task_ops = true;

    for change in changes {
        let op = change.operation;
        let l = classify(change, input_path);
        if l > layer {
            layer = l;
        }
        if is_voice_confirmed_single_task_op(op, change.target.kind) {
            has_voice_confirmed_op = true;
        }
        if is_long_move_or_snooze(op, change.target.kind, change) {
            has_long_move_or_snooze = true;
        }

        // Track whether every change is a task-level operation. Delete,
        // schedule, habit, skill, or memory changes break this.
        if !is_voice_compound_task_op(op, change.target.kind) {
            all_are_task_ops = false;
        }
    }

    // A compound voice proposal with only task-level readable knock-on effects
    // stays voice-confirmed. This captures the "lunch-shift compound" scenario:
    // a task update plus a knock-on move/reschedule is readable, so it does not
    // get promoted to screen-required just because a `Move` is involved.
    if layer == ApprovalLayer::ScreenRequired
        && all_are_task_ops
        && has_voice_confirmed_op
        && has_long_move_or_snooze
    {
        layer = ApprovalLayer::VoiceConfirmed;
    }

    layer
}

fn classify_plain_text(_change: &ProposedChange) -> ApprovalLayer {
    // Plain text is not a trusted input path; every operation requires a screen.
    ApprovalLayer::ScreenRequired
}

fn classify_capability(change: &ProposedChange) -> ApprovalLayer {
    match change.operation {
        ChangeOperation::Start | ChangeOperation::Pause | ChangeOperation::Undo => {
            ApprovalLayer::Immediate
        }
        ChangeOperation::Move | ChangeOperation::Snooze if is_short_snooze(change) => {
            ApprovalLayer::Immediate
        }
        _ => ApprovalLayer::ScreenRequired,
    }
}

fn classify_explicit_voice(change: &ProposedChange) -> ApprovalLayer {
    match (change.operation, change.target.kind) {
        (ChangeOperation::Start | ChangeOperation::Pause | ChangeOperation::Undo, _) => {
            ApprovalLayer::Immediate
        }
        (ChangeOperation::Move | ChangeOperation::Snooze, _) if is_short_snooze(change) => {
            ApprovalLayer::Immediate
        }
        (ChangeOperation::Progress | ChangeOperation::Complete, TargetKind::Task) => {
            ApprovalLayer::VoiceConfirmed
        }
        (ChangeOperation::Create | ChangeOperation::Update, TargetKind::Task) => {
            // Single task create/update is voice-confirmable; schedule-wide or
            // habit/skill/memory changes are not.
            ApprovalLayer::VoiceConfirmed
        }
        (ChangeOperation::Move | ChangeOperation::Reschedule | ChangeOperation::Snooze, _) => {
            // Long snooze or cross-time-band move/reschedule requires screen.
            ApprovalLayer::ScreenRequired
        }
        _ => ApprovalLayer::ScreenRequired,
    }
}

fn classify_ambient_wake_word(change: &ProposedChange) -> ApprovalLayer {
    match change.operation {
        ChangeOperation::Start | ChangeOperation::Pause => ApprovalLayer::AmbientImmediate,
        _ => ApprovalLayer::ScreenRequired,
    }
}

fn is_short_snooze(change: &ProposedChange) -> bool {
    let Some(args) = change.arguments.as_ref() else {
        // Without duration info, treat as a normal (potentially long) move.
        return false;
    };

    // Look for snooze_minutes in the arguments. This is used by the quick
    // action and voice snooze proposals.
    if let Some(minutes) = args.get("snooze_minutes").and_then(|v| v.as_i64()) {
        return minutes > 0 && minutes <= MAX_SHORT_SNOOZE_MINUTES;
    }

    // Also accept a direct `start_at` that is within the short snooze window.
    if let Some(start_at) = args
        .get("start_at")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<jiff::Timestamp>().ok())
    {
        let now = jiff::Timestamp::now();
        if start_at > now
            && let Ok(minutes) = (start_at - now).total(jiff::Unit::Minute)
        {
            return minutes > 0.0 && minutes <= MAX_SHORT_SNOOZE_MINUTES as f64;
        }
    }

    false
}

fn is_voice_confirmed_single_task_op(op: ChangeOperation, target: TargetKind) -> bool {
    use ChangeOperation::*;

    if target != TargetKind::Task {
        return false;
    }

    matches!(op, Progress | Complete | Create | Update)
}

fn is_long_move_or_snooze(
    op: ChangeOperation,
    target: TargetKind,
    change: &ProposedChange,
) -> bool {
    if target != TargetKind::Task {
        return false;
    }

    matches!(op, ChangeOperation::Move | ChangeOperation::Snooze) && !is_short_snooze(change)
}

fn is_voice_compound_task_op(op: ChangeOperation, target: TargetKind) -> bool {
    use ChangeOperation::*;

    if target != TargetKind::Task {
        return false;
    }

    matches!(
        op,
        Start | Pause | Undo | Progress | Complete | Create | Update | Move | Snooze
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Target;

    fn change(op: ChangeOperation) -> ProposedChange {
        ProposedChange {
            operation: op,
            target: Target::new(TargetKind::Task, "1"),
            description: "test".into(),
            ..Default::default()
        }
    }

    fn snooze_change(minutes: i64) -> ProposedChange {
        ProposedChange {
            operation: ChangeOperation::Snooze,
            target: Target::new(TargetKind::Task, "1"),
            description: format!("{minutes}分後に再開"),
            arguments: Some(serde_json::json!({ "snooze_minutes": minutes })),
            ..Default::default()
        }
    }

    #[test]
    fn start_from_screen_capability_is_immediate() {
        assert_eq!(
            classify(&change(ChangeOperation::Start), InputPath::ScreenCapability),
            ApprovalLayer::Immediate
        );
    }

    #[test]
    fn start_from_plain_text_is_screen_required() {
        assert_eq!(
            classify(&change(ChangeOperation::Start), InputPath::PlainText),
            ApprovalLayer::ScreenRequired
        );
    }

    #[test]
    fn short_snooze_from_capability_is_immediate() {
        assert_eq!(
            classify(&snooze_change(10), InputPath::ScreenCapability),
            ApprovalLayer::Immediate
        );
    }

    #[test]
    fn long_snooze_from_capability_is_screen_required() {
        assert_eq!(
            classify(&snooze_change(120), InputPath::ScreenCapability),
            ApprovalLayer::ScreenRequired
        );
    }

    #[test]
    fn progress_from_explicit_voice_session_is_voice_confirmed() {
        assert_eq!(
            classify(
                &change(ChangeOperation::Progress),
                InputPath::ExplicitVoiceSession
            ),
            ApprovalLayer::VoiceConfirmed
        );
    }

    #[test]
    fn progress_from_screen_capability_is_screen_required() {
        assert_eq!(
            classify(
                &change(ChangeOperation::Progress),
                InputPath::ScreenCapability
            ),
            ApprovalLayer::ScreenRequired
        );
    }

    #[test]
    fn wake_word_start_is_ambient_immediate() {
        assert_eq!(
            classify(&change(ChangeOperation::Start), InputPath::AmbientWakeWord),
            ApprovalLayer::AmbientImmediate
        );
    }

    #[test]
    fn delete_is_always_screen_required() {
        for path in [
            InputPath::PlainText,
            InputPath::ScreenCapability,
            InputPath::ExplicitVoiceSession,
            InputPath::AmbientWakeWord,
        ] {
            assert_eq!(
                classify(&change(ChangeOperation::Delete), path),
                ApprovalLayer::ScreenRequired,
                "delete from {path:?} should be screen-required"
            );
        }
    }

    #[test]
    fn compound_task_set_stays_voice_confirmed() {
        let changes = vec![
            change(ChangeOperation::Move),
            change(ChangeOperation::Update),
        ];
        assert_eq!(
            classify_set(&changes, InputPath::ExplicitVoiceSession),
            ApprovalLayer::VoiceConfirmed
        );
    }

    #[test]
    fn undo_from_capability_is_immediate() {
        assert_eq!(
            classify(&change(ChangeOperation::Undo), InputPath::ScreenCapability),
            ApprovalLayer::Immediate
        );
    }

    #[test]
    fn undo_from_plain_text_is_screen_required() {
        assert_eq!(
            classify(&change(ChangeOperation::Undo), InputPath::PlainText),
            ApprovalLayer::ScreenRequired
        );
    }

    #[test]
    fn undo_from_explicit_voice_is_immediate() {
        assert_eq!(
            classify(
                &change(ChangeOperation::Undo),
                InputPath::ExplicitVoiceSession
            ),
            ApprovalLayer::Immediate
        );
    }

    #[test]
    fn compound_with_delete_promotes_to_screen_required() {
        let changes = vec![
            change(ChangeOperation::Update),
            change(ChangeOperation::Delete),
        ];
        assert_eq!(
            classify_set(&changes, InputPath::ExplicitVoiceSession),
            ApprovalLayer::ScreenRequired
        );
    }
}
