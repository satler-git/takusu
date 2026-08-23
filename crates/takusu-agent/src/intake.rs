//! Resumable intake interview state (WI-16).
//!
//! The agent uses this to remember where the intake interview stopped so a
//! later session can continue from the same point. It is part of the session
//! snapshot and is restored on resume.

use serde::{Deserialize, Serialize};

/// Where the user is in the fixed-order intake interview.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum IntakeStage {
    /// Interview has not started.
    #[default]
    NotStarted,
    /// Collecting deadlines and imminent tasks.
    Deadlines,
    /// Collecting recurring commitments.
    Recurring,
    /// Confirming calendar import.
    CalendarImport,
    /// The user paused or finished the interview.
    Complete,
}

/// Saved state for an interruptible intake interview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct IntakeState {
    pub stage: IntakeStage,
    /// `proposal_id` used to batch the current set of intake proposals so they
    /// can be approved together.
    pub proposal_id: Option<String>,
    /// Whether a coverage confirmation is waiting for the current batch to be
    /// committed before it can be recorded.
    pub coverage_pending: bool,
    /// Display ids of tasks/habits created so far in this intake. Used to
    /// resume the summary and avoid re-asking about items already accepted.
    pub collected_ids: Vec<String>,
}

impl IntakeState {
    /// Move to the next stage in the fixed order.
    pub fn advance(&mut self) {
        self.stage = match self.stage {
            IntakeStage::NotStarted => IntakeStage::Deadlines,
            IntakeStage::Deadlines => IntakeStage::Recurring,
            IntakeStage::Recurring => IntakeStage::CalendarImport,
            IntakeStage::CalendarImport | IntakeStage::Complete => IntakeStage::Complete,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_intake_state_is_not_started() {
        let state = IntakeState::default();
        assert_eq!(state.stage, IntakeStage::NotStarted);
        assert!(state.proposal_id.is_none());
        assert!(!state.coverage_pending);
        assert!(state.collected_ids.is_empty());
    }

    #[test]
    fn advance_moves_through_stages() {
        let mut state = IntakeState::default();
        state.advance();
        assert_eq!(state.stage, IntakeStage::Deadlines);
        state.advance();
        assert_eq!(state.stage, IntakeStage::Recurring);
        state.advance();
        assert_eq!(state.stage, IntakeStage::CalendarImport);
        state.advance();
        assert_eq!(state.stage, IntakeStage::Complete);
        state.advance();
        assert_eq!(state.stage, IntakeStage::Complete);
    }

    #[test]
    fn serde_uses_snake_case_for_stages() {
        let json = serde_json::to_string(&IntakeStage::CalendarImport).unwrap();
        assert_eq!(json, "\"calendar_import\"");
        let stage: IntakeStage = serde_json::from_str("\"not_started\"").unwrap();
        assert_eq!(stage, IntakeStage::NotStarted);
    }

    #[test]
    fn intake_state_deserializes_with_missing_optional_fields() {
        let json = r#"{"stage":"deadlines"}"#;
        let state: IntakeState = serde_json::from_str(json).unwrap();
        assert_eq!(state.stage, IntakeStage::Deadlines);
        assert!(state.proposal_id.is_none());
        assert!(!state.coverage_pending);
        assert!(state.collected_ids.is_empty());
    }
}
