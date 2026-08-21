//! Coverage trust-state computation for the resident agent (WI-10).
//!
//! Coverage answers the question "do we trust that the plan matches reality
//! right now?". It is derived from structured coverage confirmations,
//! unresolved gap intervals, calendar sync health, and the active schedule
//! revision. The result drives whether the current task is presented as a
//! candidate or authoritative "今やること", and whether a settlement prompt
//! should take precedence over the current task.

use takusu_client::{CoverageConfirmationRow, CoverageEvaluation, CoverageState};
use takusu_types::Timestamp;

use crate::presentation::TaskAuthority;

/// Hours after which a coverage confirmation is considered stale.
pub const COVERAGE_CONFIRMATION_TTL_HOURS: i64 = 18;

/// Health reported for the external calendar the confirmation was drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalendarHealth {
    Ok,
    Stale,
    Error,
    Other,
}

impl From<&str> for CalendarHealth {
    fn from(s: &str) -> Self {
        match s {
            "ok" => Self::Ok,
            "stale" => Self::Stale,
            "error" => Self::Error,
            _ => Self::Other,
        }
    }
}

/// Source of a coverage confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverageSource {
    TargetPeriod,
    IntakeComplete,
    System,
    Other,
}

impl From<&str> for CoverageSource {
    fn from(s: &str) -> Self {
        match s {
            "target_period" => Self::TargetPeriod,
            "intake_complete" => Self::IntakeComplete,
            "system" => Self::System,
            _ => Self::Other,
        }
    }
}

/// Compute the effective coverage state for a planner evaluation.
///
/// Precedence (weakest to strongest): `bootstrap`, `stale`, `today-covered`,
/// `trusted`. A stale signal (unresolved interval, expired confirmation, or
/// stale calendar sync) overrides bootstrap and today-covered. Trusted is
/// reached when a confirmation covers the target period and no stale signal is
/// present.
///
/// `now` is the evaluation time. `target_start` and `target_end` are the local
/// boundaries of the period the user is trying to cover (usually today's
/// start..end). They are supplied by the caller because the evaluator does not
/// know the user's time zone.
pub fn compute_coverage(
    evaluation: &CoverageEvaluation,
    now: Timestamp,
    target_start: Timestamp,
    target_end: Timestamp,
) -> CoverageState {
    // Unresolved intervals that overlap the target period and have already
    // started (whether ended or still in progress) make the coverage stale.
    // Future unsettled intervals do not affect the current coverage decision.
    let unresolved = evaluation
        .unsettled_intervals
        .iter()
        .filter(|i| i.settled_at.is_none())
        .any(|i| i.start_at <= now && i.start_at < target_end && i.end_at > target_start);
    if unresolved {
        return CoverageState::Stale;
    }

    // No confirmation at all -> bootstrap.
    let confirmation = match evaluation.confirmations.iter().max_by_key(|c| c.created_at) {
        Some(c) => c,
        None => return CoverageState::Bootstrap,
    };

    // Stale calendar sync overrides everything.
    if CalendarHealth::from(confirmation.calendar_health.as_str()) == CalendarHealth::Stale
        || CalendarHealth::from(confirmation.calendar_health.as_str()) == CalendarHealth::Error
    {
        return CoverageState::Stale;
    }

    // Confirmation is too old for the target period.
    if is_expired_confirmation(confirmation, now) {
        return CoverageState::Stale;
    }

    // A future confirmation does not yet cover the current target period.
    if confirmation.start_at > target_end {
        return CoverageState::Bootstrap;
    }

    // Does the confirmation cover the target period?
    if confirmation.start_at <= target_start && confirmation.end_at >= target_end {
        // A trusted source reaches the trusted state.
        if CoverageSource::from(confirmation.source.as_str()) == CoverageSource::TargetPeriod
            || CoverageSource::from(confirmation.source.as_str()) == CoverageSource::System
        {
            return CoverageState::Trusted;
        }
        return CoverageState::TodayCovered;
    }

    // A newer confirmation that overlaps the target period but does not fully
    // cover it is still better than no confirmation, but not fully trusted.
    if confirmation.end_at > target_start && confirmation.start_at < target_end {
        return CoverageState::TodayCovered;
    }

    // Confirmation predates the target period entirely.
    CoverageState::Stale
}

/// Map a coverage state to the authority a current-task card should carry.
pub fn task_authority(state: CoverageState) -> TaskAuthority {
    match state {
        CoverageState::TodayCovered | CoverageState::Trusted => TaskAuthority::TodayCovered,
        CoverageState::Bootstrap | CoverageState::Stale => TaskAuthority::Candidate,
    }
}

fn is_expired_confirmation(confirmation: &CoverageConfirmationRow, now: Timestamp) -> bool {
    let ttl_seconds = COVERAGE_CONFIRMATION_TTL_HOURS * 3600;
    let elapsed = now
        .as_second()
        .saturating_sub(confirmation.created_at.as_second());
    elapsed > ttl_seconds
}

/// Build a coverage evaluation that explicitly records a bootstrap state.
pub fn bootstrap_evaluation(schedule_revision: i64) -> CoverageEvaluation {
    CoverageEvaluation {
        state: CoverageState::Bootstrap,
        confirmations: Vec::new(),
        unsettled_intervals: Vec::new(),
        schedule_revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use takusu_client::UnsettledIntervalRow;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn confirmation(
        start: &str,
        end: &str,
        source: &str,
        calendar_health: &str,
        created: &str,
    ) -> CoverageConfirmationRow {
        CoverageConfirmationRow {
            id: "c1".into(),
            start_at: ts(start),
            end_at: ts(end),
            timezone: "Asia/Tokyo".into(),
            source: source.into(),
            schedule_revision: 1,
            calendar_health: calendar_health.into(),
            created_at: ts(created),
            settled_at: None,
            operation_id: None,
        }
    }

    #[test]
    fn no_confirmation_is_bootstrap() {
        let eval = CoverageEvaluation::default();
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::Bootstrap);
        assert_eq!(task_authority(state), TaskAuthority::Candidate);
    }

    #[test]
    fn target_period_confirmation_is_trusted() {
        let eval = CoverageEvaluation {
            confirmations: vec![confirmation(
                "2025-01-02T00:00:00Z",
                "2025-01-02T23:59:59Z",
                "target_period",
                "ok",
                "2025-01-02T08:00:00Z",
            )],
            ..CoverageEvaluation::default()
        };
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::Trusted);
        assert_eq!(task_authority(state), TaskAuthority::TodayCovered);
    }

    #[test]
    fn today_covered_without_target_source() {
        let eval = CoverageEvaluation {
            confirmations: vec![confirmation(
                "2025-01-02T00:00:00Z",
                "2025-01-02T23:59:59Z",
                "manual",
                "ok",
                "2025-01-02T08:00:00Z",
            )],
            ..CoverageEvaluation::default()
        };
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::TodayCovered);
    }

    #[test]
    fn unresolved_interval_makes_stale() {
        let eval = CoverageEvaluation {
            confirmations: vec![confirmation(
                "2025-01-02T00:00:00Z",
                "2025-01-02T23:59:59Z",
                "target_period",
                "ok",
                "2025-01-02T08:00:00Z",
            )],
            unsettled_intervals: vec![UnsettledIntervalRow {
                id: "u1".into(),
                start_at: ts("2025-01-02T09:00:00Z"),
                end_at: ts("2025-01-02T09:30:00Z"),
                classification: "unclassified".into(),
                source: "capture".into(),
                created_at: ts("2025-01-02T09:35:00Z"),
                settled_at: None,
                operation_id: None,
            }],
            ..CoverageEvaluation::default()
        };
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::Stale);
        assert_eq!(task_authority(state), TaskAuthority::Candidate);
    }

    #[test]
    fn in_progress_unresolved_interval_makes_stale() {
        let eval = CoverageEvaluation {
            confirmations: vec![confirmation(
                "2025-01-02T00:00:00Z",
                "2025-01-02T23:59:59Z",
                "target_period",
                "ok",
                "2025-01-02T08:00:00Z",
            )],
            unsettled_intervals: vec![UnsettledIntervalRow {
                id: "u1".into(),
                start_at: ts("2025-01-02T09:00:00Z"),
                end_at: ts("2025-01-02T11:00:00Z"),
                classification: "unclassified".into(),
                source: "capture".into(),
                created_at: ts("2025-01-02T09:05:00Z"),
                settled_at: None,
                operation_id: None,
            }],
            ..CoverageEvaluation::default()
        };
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::Stale);
    }

    #[test]
    fn future_unresolved_interval_does_not_make_stale() {
        let eval = CoverageEvaluation {
            confirmations: vec![confirmation(
                "2025-01-02T00:00:00Z",
                "2025-01-02T23:59:59Z",
                "target_period",
                "ok",
                "2025-01-02T08:00:00Z",
            )],
            unsettled_intervals: vec![UnsettledIntervalRow {
                id: "u1".into(),
                start_at: ts("2025-01-02T14:00:00Z"),
                end_at: ts("2025-01-02T15:00:00Z"),
                classification: "unclassified".into(),
                source: "capture".into(),
                created_at: ts("2025-01-02T09:05:00Z"),
                settled_at: None,
                operation_id: None,
            }],
            ..CoverageEvaluation::default()
        };
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::Trusted);
    }

    #[test]
    fn stale_calendar_sync_makes_stale() {
        let eval = CoverageEvaluation {
            confirmations: vec![confirmation(
                "2025-01-02T00:00:00Z",
                "2025-01-02T23:59:59Z",
                "target_period",
                "stale",
                "2025-01-02T08:00:00Z",
            )],
            ..CoverageEvaluation::default()
        };
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::Stale);
    }

    #[test]
    fn expired_confirmation_makes_stale() {
        let eval = CoverageEvaluation {
            confirmations: vec![confirmation(
                "2025-01-01T00:00:00Z",
                "2025-01-01T23:59:59Z",
                "target_period",
                "ok",
                "2025-01-01T08:00:00Z",
            )],
            ..CoverageEvaluation::default()
        };
        let state = compute_coverage(
            &eval,
            ts("2025-01-02T10:00:00Z"),
            ts("2025-01-02T00:00:00Z"),
            ts("2025-01-02T23:59:59Z"),
        );
        assert_eq!(state, CoverageState::Stale);
    }
}
