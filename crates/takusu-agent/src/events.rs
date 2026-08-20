//! Pure planner-event evaluation for the resident agent (WI-9).
//!
//! Evaluation deliberately has no storage or transport dependencies. The local
//! application host builds an [`EvaluationSnapshot`] from one consistent read,
//! evaluates it, and commits the returned events to the ledger in a separate
//! revision-checked transaction. Capabilities are minted when an event becomes
//! eligible for delivery, not while this pure policy function runs.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use takusu_client::{CoverageEvaluation, CoverageState};
use takusu_types::TaskStatus;
use takusu_types::Timestamp;
use takusu_types::estimator::{
    DurationDistribution, InterventionBand, next_crossing_active_minutes,
};

use crate::presentation::{
    Action, ActionGroup, ActionKind, CheckInCard, NonEmptyVec, Presentation, SettlementPrompt,
    TaskCard, WorkState,
};
use crate::coverage::task_authority;

/// Grace period before a missed start becomes a sync check-in.
pub const NON_START_GRACE_MINUTES: i64 = 15;
/// Minimum duration of an unclassified gap before a capture check-in.
pub const UNCLASSIFIED_GAP_THRESHOLD_MINUTES: i64 = 30;

/// A task projection needed by the evaluator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationTask {
    pub id: String,
    pub display_id: i64,
    pub title: String,
    pub scheduled_at: Option<Timestamp>,
    pub deadline_at: Timestamp,
    pub status: TaskStatus,
    pub fixed: bool,
    pub quantity_total: Option<i64>,
    pub quantity_done: i64,
}

/// A schedule entry used for canonical gap detection and next-task rendering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationScheduleEntry {
    pub task_id: String,
    pub start_at: Timestamp,
    pub end_at: Timestamp,
}

/// Active work-session information. `active_minutes` comes from work-session
/// storage and therefore excludes pauses and wall-clock idle time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationWork {
    pub task_id: String,
    pub active_minutes: f64,
    pub distribution: DurationDistribution,
    pub distribution_revision: i64,
    /// Set for a progress-based observation. A censored observation leaves this
    /// unset and is judged from the survival probability instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_band: Option<InterventionBand>,
}

/// The five gap categories in the event contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    FreeTime,
    Buffer,
    Routine,
    Unclassified,
    GenerationFailure,
}

/// A gap classification produced by the application layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationGap {
    pub start_at: Timestamp,
    pub end_at: Timestamp,
    pub kind: GapKind,
    /// Stable identity for a gap. If omitted, the canonical interval is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

/// View of committed event IDs. A committed event is never emitted again by
/// evaluation, including after delivery was deferred or a device reconnects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerView {
    pub committed_event_ids: HashSet<String>,
}

/// User-visible sleep impact already determined by the schedule application
/// layer. The pure evaluator only decides whether to emit its stable event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepImpact {
    pub detected: bool,
}

/// One consistent read of all inputs used by the evaluator.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvaluationSnapshot {
    pub schedule_revision: i64,
    pub now: Timestamp,
    pub tasks: Vec<EvaluationTask>,
    pub schedule: Vec<EvaluationScheduleEntry>,
    pub work: Vec<EvaluationWork>,
    pub gaps: Vec<EvaluationGap>,
    /// Tasks that the planner could not place during generation.
    #[serde(default)]
    pub unplaced_task_ids: Vec<String>,
    pub coverage: CoverageEvaluation,
    pub ledger: LedgerView,
    #[serde(default)]
    pub sleep_impact: Option<SleepImpact>,
}

/// Planner event categories. These names are stable wire identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlannerEventKind {
    TaskStartTimeReached,
    TaskNonStartContinued,
    UnclassifiedGapContinued,
    DistributionOverrun,
    DeadlineViolation,
    CarriedOverIncomplete,
    ScheduleGenerationFailure,
    SleepImpact,
    /// The current task card with coverage authority and optional settlement prompt.
    CurrentTask,
}

impl PlannerEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskStartTimeReached => "task_start_time_reached",
            Self::TaskNonStartContinued => "task_non_start_continued",
            Self::UnclassifiedGapContinued => "unclassified_gap_continued",
            Self::DistributionOverrun => "distribution_overrun",
            Self::DeadlineViolation => "deadline_violation",
            Self::CarriedOverIncomplete => "carried_over_incomplete",
            Self::ScheduleGenerationFailure => "schedule_generation_failure",
            Self::SleepImpact => "sleep_impact",
            Self::CurrentTask => "current_task",
        }
    }
}

/// Delivery urgency, kept separate from delivery modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Normal,
    High,
    Emergency,
}

/// A task identity included in event payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskRef {
    pub task_id: String,
    pub display_id: i64,
}

/// An event returned by the pure evaluator.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PlannerEvent {
    pub id: String,
    pub kind: PlannerEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_ref: Option<TaskRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band: Option<InterventionBand>,
    pub presentation: Presentation,
    pub urgency: Urgency,
    pub schedule_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distribution_revision: Option<i64>,
    pub due_at: Timestamp,
}

/// Result of one evaluator invocation.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvaluationResult {
    pub due_events: Vec<PlannerEvent>,
    /// `None` means no known boundary remains; a state-change signal should
    /// wake the resident evaluator when new planner state is committed.
    pub next_eval_at: Option<Timestamp>,
}

/// Evaluate planner events from a consistent snapshot.
pub fn evaluate_events(snapshot: &EvaluationSnapshot) -> EvaluationResult {
    let mut events = Vec::new();
    let mut next_eval_at = None;
    let tasks: HashMap<&str, &EvaluationTask> = snapshot
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect();
    let active_task_ids: HashSet<&str> = snapshot
        .work
        .iter()
        .map(|work| work.task_id.as_str())
        .collect();

    for task in &snapshot.tasks {
        if is_terminal(task.status) {
            continue;
        }
        let Some(start_at) = task.scheduled_at else {
            continue;
        };

        if start_at > snapshot.now {
            next_eval_at = earlier(next_eval_at, start_at);
            continue;
        }

        if task.status != TaskStatus::InProgress && !active_task_ids.contains(task.id.as_str()) {
            let grace_at = add_minutes(start_at, NON_START_GRACE_MINUTES);
            if snapshot.now < grace_at {
                push_if_new(
                    &mut events,
                    &snapshot.ledger,
                    task_start_event(snapshot, task, start_at),
                );
                next_eval_at = earlier(next_eval_at, grace_at);
            } else if !task.fixed {
                push_if_new(
                    &mut events,
                    &snapshot.ledger,
                    non_start_event(snapshot, task, grace_at),
                );
            }
        }
    }

    for work in &snapshot.work {
        let Some(task) = tasks.get(work.task_id.as_str()).copied() else {
            continue;
        };
        if task.fixed || is_terminal(task.status) {
            continue;
        }

        let band = work
            .progress_band
            .unwrap_or_else(|| work.distribution.band_at(work.active_minutes));
        if band.is_intervention() {
            push_if_new(
                &mut events,
                &snapshot.ledger,
                distribution_event(snapshot, task, work, band),
            );
        }

        if let Some(crossing) = next_crossing_active_minutes(work.distribution, work.active_minutes)
        {
            let delta = (crossing - work.active_minutes).max(0.0);
            if delta > 0.0 {
                next_eval_at = earlier(next_eval_at, add_seconds(snapshot.now, delta * 60.0));
            }
        }

        if let Some(predicted_end) =
            predicted_end(snapshot.now, work.active_minutes, work.distribution)
            && predicted_end > task.deadline_at
        {
            push_if_new(
                &mut events,
                &snapshot.ledger,
                deadline_event(snapshot, task, predicted_end),
            );
        }
    }

    for gap in &snapshot.gaps {
        if gap.kind != GapKind::Unclassified
            || gap.start_at > snapshot.now
            || gap.end_at <= snapshot.now
        {
            continue;
        }
        let threshold_at = add_minutes(gap.start_at, UNCLASSIFIED_GAP_THRESHOLD_MINUTES);
        if threshold_at > snapshot.now {
            next_eval_at = earlier(next_eval_at, threshold_at);
        } else {
            push_if_new(
                &mut events,
                &snapshot.ledger,
                gap_event(snapshot, gap, threshold_at),
            );
        }
    }

    if !snapshot.unplaced_task_ids.is_empty() {
        let boundary = Timestamp::from_second(0).expect("epoch timestamp is valid");
        push_if_new(
            &mut events,
            &snapshot.ledger,
            simple_event(
                snapshot,
                PlannerEventKind::ScheduleGenerationFailure,
                None,
                None,
                event_id(
                    PlannerEventKind::ScheduleGenerationFailure,
                    None,
                    snapshot.schedule_revision,
                    None,
                    boundary,
                    "unplaced",
                ),
                Presentation::ScheduleAlert(crate::presentation::ScheduleAlert {
                    kind: crate::presentation::ScheduleAlertKind::GenerationFailure,
                    message: "スケジュールに配置できないタスクがあります。組み直しますか".into(),
                }),
                Urgency::High,
                snapshot.now,
            ),
        );
    }

    for task in &snapshot.tasks {
        if is_terminal(task.status)
            || task.scheduled_at.is_none()
            || task.scheduled_at >= Some(snapshot.now)
            || snapshot
                .now
                .as_second()
                .saturating_sub(task.scheduled_at.unwrap().as_second())
                < 24 * 60 * 60
        {
            continue;
        }
        push_if_new(
            &mut events,
            &snapshot.ledger,
            simple_event(
                snapshot,
                PlannerEventKind::CarriedOverIncomplete,
                Some(task_ref(task)),
                None,
                event_id(
                    PlannerEventKind::CarriedOverIncomplete,
                    Some(&task.id),
                    snapshot.schedule_revision,
                    None,
                    task.scheduled_at.unwrap_or(snapshot.now),
                    "carry-over",
                ),
                Presentation::CheckIn(check_in(
                    format!("「{}」が未完了のまま残っています", task.title),
                    "精算して組み直す",
                    "そのまま今日にずらす",
                )),
                Urgency::High,
                snapshot.now,
            ),
        );
    }

    if snapshot.sleep_impact.is_some_and(|impact| impact.detected) {
        let boundary = Timestamp::from_second(0).expect("epoch timestamp is valid");
        push_if_new(
            &mut events,
            &snapshot.ledger,
            simple_event(
                snapshot,
                PlannerEventKind::SleepImpact,
                None,
                None,
                event_id(
                    PlannerEventKind::SleepImpact,
                    None,
                    snapshot.schedule_revision,
                    None,
                    boundary,
                    "sleep",
                ),
                Presentation::ScheduleAlert(crate::presentation::ScheduleAlert {
                    kind: crate::presentation::ScheduleAlertKind::Overdue,
                    message: "睡眠時間の影響でスケジュールの調整が必要です".into(),
                }),
                Urgency::High,
                snapshot.now,
            ),
        );
    }

    events.sort_by(|left, right| left.id.cmp(&right.id));

    if let Some(event) = current_task_event(snapshot) {
        push_if_new(&mut events, &snapshot.ledger, event);
    }

    EvaluationResult {
        due_events: events,
        next_eval_at,
    }
}

/// Build the canonical event key. All semantic inputs are length-delimited so
/// task IDs and timestamps cannot make two keys ambiguous.
pub fn event_id(
    kind: PlannerEventKind,
    task_id: Option<&str>,
    schedule_revision: i64,
    distribution_revision: Option<i64>,
    boundary: Timestamp,
    observation_kind: &str,
) -> String {
    fn part(value: &str) -> String {
        format!("{}:{value}", value.len())
    }
    format!(
        "planner:v1:{}:{}:s{}:d{}:b{}:o{}",
        kind.as_str(),
        part(task_id.unwrap_or("")),
        schedule_revision,
        distribution_revision.map_or_else(|| "-".into(), |revision| revision.to_string()),
        part(&boundary.to_string()),
        part(observation_kind),
    )
}

fn push_if_new(events: &mut Vec<PlannerEvent>, ledger: &LedgerView, event: PlannerEvent) {
    if !ledger.committed_event_ids.contains(&event.id) {
        events.push(event);
    }
}

fn is_terminal(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Completed | TaskStatus::Skipped)
}

fn task_ref(task: &EvaluationTask) -> TaskRef {
    TaskRef {
        task_id: task.id.clone(),
        display_id: task.display_id,
    }
}

fn task_start_event(
    snapshot: &EvaluationSnapshot,
    task: &EvaluationTask,
    boundary: Timestamp,
) -> PlannerEvent {
    let id = event_id(
        PlannerEventKind::TaskStartTimeReached,
        Some(&task.id),
        snapshot.schedule_revision,
        None,
        boundary,
        "start-time",
    );
    simple_event(
        snapshot,
        PlannerEventKind::TaskStartTimeReached,
        Some(task_ref(task)),
        None,
        id,
        Presentation::CheckIn(check_in(
            format!("「{}」の開始時刻です", task.title),
            "着手",
            "10分後にずらす",
        )),
        Urgency::Normal,
        boundary,
    )
}

fn non_start_event(
    snapshot: &EvaluationSnapshot,
    task: &EvaluationTask,
    boundary: Timestamp,
) -> PlannerEvent {
    simple_event(
        snapshot,
        PlannerEventKind::TaskNonStartContinued,
        Some(task_ref(task)),
        None,
        event_id(
            PlannerEventKind::TaskNonStartContinued,
            Some(&task.id),
            snapshot.schedule_revision,
            None,
            boundary,
            "non-start",
        ),
        Presentation::CheckIn(check_in(
            format!("「{}」は開始時刻を過ぎています", task.title),
            "今から着手",
            "10分後にずらす",
        )),
        Urgency::Normal,
        boundary,
    )
}

fn distribution_event(
    snapshot: &EvaluationSnapshot,
    task: &EvaluationTask,
    work: &EvaluationWork,
    band: InterventionBand,
) -> PlannerEvent {
    let canonical_boundary = task.scheduled_at.unwrap_or(snapshot.now);
    let question = match band {
        InterventionBand::Attention => format!("「{}」は予定より時間がかかっています", task.title),
        InterventionBand::Replan => format!("「{}」のペースだと後ろが崩れそうです", task.title),
        InterventionBand::Usual => unreachable!("usual band is not an intervention"),
    };
    let observation_kind = match (work.progress_band.is_some(), band) {
        (true, InterventionBand::Attention) => "progress:attention",
        (true, InterventionBand::Replan) => "progress:replan",
        (false, InterventionBand::Attention) => "censored:attention",
        (false, InterventionBand::Replan) => "censored:replan",
        (_, InterventionBand::Usual) => unreachable!("usual band is not an intervention"),
    };
    let mut event = simple_event(
        snapshot,
        PlannerEventKind::DistributionOverrun,
        Some(task_ref(task)),
        Some(band),
        event_id(
            PlannerEventKind::DistributionOverrun,
            Some(&task.id),
            snapshot.schedule_revision,
            Some(work.distribution_revision),
            canonical_boundary,
            observation_kind,
        ),
        Presentation::CheckIn(check_in(question, "進捗を記録", "組み直す")),
        if band == InterventionBand::Replan {
            Urgency::High
        } else {
            Urgency::Normal
        },
        snapshot.now,
    );
    event.distribution_revision = Some(work.distribution_revision);
    event
}

fn gap_event(
    snapshot: &EvaluationSnapshot,
    gap: &EvaluationGap,
    boundary: Timestamp,
) -> PlannerEvent {
    let identity = gap.identity.as_deref().unwrap_or("");
    simple_event(
        snapshot,
        PlannerEventKind::UnclassifiedGapContinued,
        None,
        None,
        event_id(
            PlannerEventKind::UnclassifiedGapContinued,
            Some(identity),
            snapshot.schedule_revision,
            None,
            gap.start_at,
            "gap",
        ),
        Presentation::CheckIn(check_in(
            "予定に空白が続いています。今なにしてますか".into(),
            "今回の活動を記録",
            "自由時間として残す",
        )),
        Urgency::Normal,
        boundary,
    )
}

fn deadline_event(
    snapshot: &EvaluationSnapshot,
    task: &EvaluationTask,
    _predicted_end: Timestamp,
) -> PlannerEvent {
    simple_event(
        snapshot,
        PlannerEventKind::DeadlineViolation,
        Some(task_ref(task)),
        Some(InterventionBand::Replan),
        event_id(
            PlannerEventKind::DeadlineViolation,
            Some(&task.id),
            snapshot.schedule_revision,
            None,
            task.deadline_at,
            "deadline",
        ),
        Presentation::CheckIn(check_in(
            format!("「{}」は期限に間に合わない見込みです", task.title),
            "このまま続ける",
            "後ろの予定をずらす",
        )),
        Urgency::High,
        snapshot.now,
    )
}

#[allow(clippy::too_many_arguments)]
fn simple_event(
    snapshot: &EvaluationSnapshot,
    kind: PlannerEventKind,
    task_ref: Option<TaskRef>,
    band: Option<InterventionBand>,
    id: String,
    presentation: Presentation,
    urgency: Urgency,
    due_at: Timestamp,
) -> PlannerEvent {
    PlannerEvent {
        id,
        kind,
        task_ref,
        band,
        presentation,
        urgency,
        schedule_revision: snapshot.schedule_revision,
        distribution_revision: None,
        due_at,
    }
}

fn check_in(question: String, act_label: &str, shift_label: &str) -> CheckInCard {
    let act = ActionGroup {
        title: "行動".into(),
        actions: NonEmptyVec::new(vec![Action {
            id: "act".into(),
            label: act_label.into(),
            kind: ActionKind::Immediate,
            capability: None,
        }])
        .expect("one action is non-empty"),
    };
    let shift = ActionGroup {
        title: "ズラす".into(),
        actions: NonEmptyVec::new(vec![Action {
            id: "shift".into(),
            label: shift_label.into(),
            kind: ActionKind::Panel,
            capability: None,
        }])
        .expect("one action is non-empty"),
    };
    CheckInCard::new(question, act, shift).expect("both action groups are non-empty")
}

fn predicted_end(
    now: Timestamp,
    active_minutes: f64,
    distribution: DurationDistribution,
) -> Option<Timestamp> {
    let remaining = (distribution.mean_minutes() - active_minutes).max(0.0);
    Some(add_seconds(now, remaining * 60.0))
}

fn earlier(current: Option<Timestamp>, candidate: Timestamp) -> Option<Timestamp> {
    Some(current.map_or(candidate, |current| current.min(candidate)))
}

fn add_minutes(timestamp: Timestamp, minutes: i64) -> Timestamp {
    add_seconds(timestamp, minutes as f64 * 60.0)
}

fn add_seconds(timestamp: Timestamp, seconds: f64) -> Timestamp {
    let seconds = seconds.round();
    let seconds = seconds.clamp(i64::MIN as f64, i64::MAX as f64) as i64;
    Timestamp::from_second(timestamp.as_second().saturating_add(seconds)).unwrap_or(timestamp)
}

fn current_task_event(snapshot: &EvaluationSnapshot) -> Option<PlannerEvent> {
    // Pick the current task: in-progress first, then the earliest scheduled
    // task whose interval contains `now`, then the earliest scheduled task.
    let candidate = snapshot
        .tasks
        .iter()
        .filter(|t| !is_terminal(t.status))
        .min_by_key(|t| {
            let priority = match t.status {
                TaskStatus::InProgress => 0,
                _ if t.scheduled_at.is_some_and(|s| s <= snapshot.now) => 1,
                _ => 2,
            };
            (priority, t.scheduled_at.unwrap_or(snapshot.now))
        })?;

    let authority = task_authority(snapshot.coverage.state);
    let settlement = build_settlement(snapshot);

    let work_state = if candidate.status == TaskStatus::InProgress {
        WorkState::InProgress
    } else if candidate.deadline_at < snapshot.now {
        WorkState::Overdue
    } else {
        WorkState::NotStarted
    };

    let start_at = candidate
        .scheduled_at
        .map(|t| t.to_string())
        .or_else(|| Some(snapshot.now.to_string()));
    let end_at = Some(candidate.deadline_at.to_string());

    let id = event_id(
        PlannerEventKind::CurrentTask,
        Some(&candidate.id),
        snapshot.schedule_revision,
        None,
        snapshot.now,
        "current",
    );

    let presentation = Presentation::CurrentTask(TaskCard {
        title: candidate.title.clone(),
        reference: format!("#{}", candidate.display_id),
        start_at,
        end_at,
        work_state,
        authority,
        next_task: None,
        settlement,
    });

    Some(PlannerEvent {
        id,
        kind: PlannerEventKind::CurrentTask,
        task_ref: Some(TaskRef {
            task_id: candidate.id.clone(),
            display_id: candidate.display_id,
        }),
        band: None,
        presentation,
        urgency: Urgency::Normal,
        schedule_revision: snapshot.schedule_revision,
        distribution_revision: None,
        due_at: snapshot.now,
    })
}

fn build_settlement(snapshot: &EvaluationSnapshot) -> Option<SettlementPrompt> {
    if snapshot.coverage.state != CoverageState::Stale {
        return None;
    }
    let interval = snapshot.coverage.unsettled_intervals.first()?;
    let start = interval.start_at.to_string();
    let end = interval.end_at.to_string();
    let question = format!("{start}〜{end} の未確定時間を整理してください");
    let act = ActionGroup {
        title: "行動".into(),
        actions: NonEmptyVec::new(vec![Action {
            id: "settle-start".into(),
            label: "この時間で作業".into(),
            kind: ActionKind::Immediate,
            capability: None,
        }])
        .expect("one action is non-empty"),
    };
    let shift = ActionGroup {
        title: "ズラす".into(),
        actions: NonEmptyVec::new(vec![Action {
            id: "settle-ignore".into(),
            label: "後で決める".into(),
            kind: ActionKind::Panel,
            capability: None,
        }])
        .expect("one action is non-empty"),
    };
    Some(SettlementPrompt {
        question,
        act,
        shift,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    fn task(id: &str, scheduled: i64, status: TaskStatus) -> EvaluationTask {
        EvaluationTask {
            id: id.into(),
            display_id: 1,
            title: id.into(),
            scheduled_at: Some(timestamp(scheduled)),
            deadline_at: timestamp(scheduled + 3_600),
            status,
            fixed: false,
            quantity_total: None,
            quantity_done: 0,
        }
    }

    fn snapshot() -> EvaluationSnapshot {
        EvaluationSnapshot {
            schedule_revision: 3,
            now: timestamp(10_000),
            ..Default::default()
        }
    }

    #[test]
    fn start_event_is_deterministic_and_has_typed_presentation() {
        let mut snapshot = snapshot();
        snapshot.now = timestamp(9_200);
        snapshot
            .tasks
            .push(task("task-1", 9_000, TaskStatus::Scheduled));
        let result = evaluate_events(&snapshot);
        assert_eq!(result.due_events.len(), 2);
        assert!(result.due_events.iter().any(|event| {
            event.kind == PlannerEventKind::TaskStartTimeReached
                && matches!(event.presentation, Presentation::CheckIn(_))
        }));
        assert!(result.due_events.iter().any(|event| {
            event.kind == PlannerEventKind::CurrentTask
                && matches!(event.presentation, Presentation::CurrentTask(_))
        }));
        let repeated = evaluate_events(&snapshot);
        assert_eq!(
            result
                .due_events
                .iter()
                .map(|event| &event.id)
                .collect::<Vec<_>>(),
            repeated
                .due_events
                .iter()
                .map(|event| &event.id)
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn non_start_waits_for_grace_and_fixed_tasks_are_excluded() {
        let mut snapshot = snapshot();
        snapshot.now = timestamp(9_899);
        snapshot
            .tasks
            .push(task("task-1", 9_000, TaskStatus::Scheduled));
        assert!(
            evaluate_events(&snapshot)
                .due_events
                .iter()
                .all(|event| event.kind != PlannerEventKind::TaskNonStartContinued)
        );

        snapshot.now = timestamp(10_000);
        let mut fixed = task("fixed", 9_000, TaskStatus::Scheduled);
        fixed.fixed = true;
        snapshot.tasks.push(fixed);
        assert_eq!(
            evaluate_events(&snapshot)
                .due_events
                .iter()
                .filter(|event| event.kind == PlannerEventKind::TaskNonStartContinued)
                .count(),
            1
        );
    }

    #[test]
    fn committed_event_does_not_fire_again() {
        let mut snapshot = snapshot();
        snapshot
            .tasks
            .push(task("task-1", 9_000, TaskStatus::Scheduled));
        let first = evaluate_events(&snapshot);
        snapshot
            .ledger
            .committed_event_ids
            .extend(first.due_events.iter().map(|event| event.id.clone()));
        assert!(evaluate_events(&snapshot).due_events.is_empty());
    }

    #[test]
    fn distribution_band_and_crossing_are_evaluated_from_active_time() {
        let mut snapshot = snapshot();
        let mut task = task("task-1", 9_000, TaskStatus::InProgress);
        task.deadline_at = timestamp(20_000);
        snapshot.tasks.push(task);
        snapshot.work.push(EvaluationWork {
            task_id: "task-1".into(),
            active_minutes: 80.0,
            distribution: DurationDistribution::new(60.0, 10.0),
            distribution_revision: 4,
            progress_band: None,
        });
        let result = evaluate_events(&snapshot);
        assert!(
            result
                .due_events
                .iter()
                .any(|event| event.kind == PlannerEventKind::DistributionOverrun)
        );
        assert!(result.next_eval_at.is_none() || result.next_eval_at.unwrap() > snapshot.now);
    }

    #[test]
    fn only_unclassified_gaps_fire() {
        let mut snapshot = snapshot();
        snapshot.now = timestamp(12_000);
        snapshot.gaps = vec![
            EvaluationGap {
                start_at: timestamp(9_000),
                end_at: timestamp(20_000),
                kind: GapKind::FreeTime,
                identity: None,
            },
            EvaluationGap {
                start_at: timestamp(9_000),
                end_at: timestamp(20_000),
                kind: GapKind::Unclassified,
                identity: Some("gap-1".into()),
            },
        ];
        let result = evaluate_events(&snapshot);
        assert_eq!(
            result
                .due_events
                .iter()
                .filter(|event| event.kind == PlannerEventKind::UnclassifiedGapContinued)
                .count(),
            1
        );
    }

    #[test]
    fn revision_and_observation_are_part_of_event_id() {
        let a = event_id(
            PlannerEventKind::DistributionOverrun,
            Some("task"),
            1,
            Some(2),
            timestamp(3),
            "censored",
        );
        let b = event_id(
            PlannerEventKind::DistributionOverrun,
            Some("task"),
            1,
            Some(3),
            timestamp(3),
            "censored",
        );
        assert_ne!(a, b);
    }
}
