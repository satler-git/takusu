//! Habit step parsing, validation, and two-phase save.
//!
//! The agent accepts habit steps with 1-indexed display positions and
//! `depends_on` references that may point to steps being created in the same
//! request. Because the server assigns real ids only on save, a two-phase
//! protocol is used: phase 1 saves with `depends_on` entries that reference
//! already-existing steps; phase 2 re-saves once the server has returned ids
//! for the newly created steps so cross-references can be resolved.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;
use takusu_client::HabitStepInput;
use takusu_types::{Abandonability, TimeOfDay};

use crate::{AgentError, AgentSession, InvalidArgsError, ToolError};

/// Intermediate representation of a habit step submitted by the agent.
/// Display positions are 1-indexed; storage uses 0-indexed positions.
#[derive(Debug, Clone)]
pub(crate) struct PendingHabitStep {
    pub position: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_time: TimeOfDay,
    pub end_time: TimeOfDay,
    pub avg_minutes: i64,
    pub sigma_minutes: Option<i64>,
    pub parallelizable: Option<bool>,
    pub allows_parallel: Option<bool>,
    pub abandonability: Option<Abandonability>,
    pub fixed: Option<bool>,
    pub depends_on_positions: Vec<i64>,
}

/// Typed deserialization target for a single habit step argument.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HabitStepInputArgs {
    position: i64,
    title: String,
    #[serde(default)]
    description: Option<String>,
    start_time: TimeOfDay,
    end_time: TimeOfDay,
    avg_minutes: i64,
    #[serde(default)]
    sigma_minutes: Option<i64>,
    #[serde(default)]
    parallelizable: Option<bool>,
    #[serde(default)]
    allows_parallel: Option<bool>,
    #[serde(default)]
    abandonability: Option<Abandonability>,
    #[serde(default)]
    fixed: Option<bool>,
    #[serde(default)]
    depends_on: Vec<i64>,
}

pub(crate) fn parse_habit_step(value: &Value) -> Result<PendingHabitStep, AgentError> {
    let args: HabitStepInputArgs = serde_json::from_value(value.clone()).map_err(|e| {
        AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::new(
            "steps",
            e.to_string(),
        )))
    })?;
    if args.position < 1 {
        return Err(AgentError::Tool(ToolError::InvalidArgs(
            InvalidArgsError::new("steps", "position must be >= 1"),
        )));
    }
    Ok(PendingHabitStep {
        position: args.position,
        title: args.title,
        description: args.description,
        start_time: args.start_time,
        end_time: args.end_time,
        avg_minutes: args.avg_minutes,
        sigma_minutes: args.sigma_minutes,
        parallelizable: args.parallelizable,
        allows_parallel: args.allows_parallel,
        abandonability: args.abandonability,
        fixed: args.fixed,
        depends_on_positions: args.depends_on,
    })
}

pub(crate) fn build_habit_step_inputs(
    pending: &[PendingHabitStep],
    position_to_id: &HashMap<i64, String>,
) -> Vec<HabitStepInput> {
    pending
        .iter()
        .map(|s| HabitStepInput {
            id: position_to_id.get(&s.position).cloned(),
            position: s.position - 1,
            title: s.title.clone(),
            description: s.description.clone(),
            start_time: s.start_time,
            end_time: s.end_time,
            avg_minutes: s.avg_minutes,
            sigma_minutes: s.sigma_minutes,
            parallelizable: s.parallelizable,
            allows_parallel: s.allows_parallel,
            abandonability: s.abandonability,
            fixed: s.fixed,
            depends_on: s
                .depends_on_positions
                .iter()
                .filter_map(|pos| position_to_id.get(pos).cloned())
                .collect(),
        })
        .collect()
}

/// Detect cycles in step dependencies. Display positions are 1-indexed.
pub(crate) fn detect_step_dependency_cycle(
    positions: &[i64],
    edges: &[(i64, i64)],
) -> Option<Vec<i64>> {
    let mut adj: HashMap<i64, Vec<i64>> = HashMap::new();
    for (from, to) in edges {
        adj.entry(*from).or_default().push(*to);
    }
    let mut visited = HashSet::new();
    let mut stack = HashSet::new();
    let mut path = Vec::new();

    fn dfs(
        node: i64,
        adj: &HashMap<i64, Vec<i64>>,
        visited: &mut HashSet<i64>,
        stack: &mut HashSet<i64>,
        path: &mut Vec<i64>,
    ) -> Option<Vec<i64>> {
        if !visited.insert(node) {
            return if stack.contains(&node) {
                let start = path.iter().position(|&p| p == node).unwrap_or(path.len());
                Some(path[start..].to_vec())
            } else {
                None
            };
        }
        stack.insert(node);
        path.push(node);
        let neighbors = adj.get(&node).map(|v| v.as_slice()).unwrap_or(&[]);
        for next in neighbors {
            if let Some(cycle) = dfs(*next, adj, visited, stack, path) {
                return Some(cycle);
            }
        }
        path.pop();
        stack.remove(&node);
        None
    }

    for &pos in positions {
        if !visited.contains(&pos)
            && let Some(cycle) = dfs(pos, &adj, &mut visited, &mut stack, &mut path)
        {
            return Some(cycle);
        }
    }
    None
}

/// Build phase 2 inputs from the server response to a phase 1 save.
/// Returns `None` when no phase 2 is needed, otherwise `Some(Ok(...))` or
/// an error if the response does not contain every submitted position.
/// The server may return rows in any order, so mapping uses `row.position`.
pub(crate) fn phase2_inputs_for_response(
    pending: &[PendingHabitStep],
    phase1_position_to_id: &HashMap<i64, String>,
    response_rows: &[takusu_client::HabitStepRow],
) -> Option<Result<Vec<HabitStepInput>, AgentError>> {
    let needs_phase2 = pending.iter().any(|s| {
        s.depends_on_positions
            .iter()
            .any(|pos| !phase1_position_to_id.contains_key(pos))
    });
    if !needs_phase2 {
        return None;
    }

    let mut phase2_position_to_id: HashMap<i64, String> = HashMap::new();
    for row in response_rows {
        phase2_position_to_id.insert(row.position + 1, row.id.clone());
    }

    for s in pending {
        if !phase2_position_to_id.contains_key(&s.position) {
            return Some(Err(AgentError::Tool(ToolError::Other(
                "server did not return all submitted steps".into(),
            ))));
        }
    }

    Some(Ok(build_habit_step_inputs(
        pending,
        &phase2_position_to_id,
    )))
}

impl AgentSession {
    /// Bulk-replace habit steps from agent-facing input. Positions are
    /// 1-indexed display numbers; `depends_on` is resolved across the submitted
    /// step list. Existing steps are matched by display position. A two-phase
    /// save is used when a step depends on a new step so real ids can be
    /// assigned.
    pub(crate) async fn replace_habit_steps_from_input(
        &self,
        habit_id: &str,
        steps_value: Value,
        existing_steps: &[takusu_client::HabitStepRow],
    ) -> Result<(), AgentError> {
        let steps_array = steps_value.as_array().ok_or_else(|| {
            AgentError::Tool(ToolError::InvalidArgs(InvalidArgsError::new(
                "steps",
                "must be an array",
            )))
        })?;
        let pending: Vec<PendingHabitStep> = steps_array
            .iter()
            .map(parse_habit_step)
            .collect::<Result<Vec<_>, _>>()?;

        // Check positions are unique within the submitted list.
        let mut seen_positions = HashSet::new();
        for s in &pending {
            if !seen_positions.insert(s.position) {
                return Err(AgentError::Tool(ToolError::InvalidArgs(
                    InvalidArgsError::new("steps", format!("duplicate position {}", s.position)),
                )));
            }
        }

        // Match existing steps by display position so generated task links are
        // preserved. New positions create new steps.
        let existing_by_position: HashMap<i64, String> = existing_steps
            .iter()
            .map(|s| (s.position + 1, s.id.clone()))
            .collect();

        // Validate depends_on positions refer to steps in the submitted list.
        let submitted_positions: HashSet<i64> = pending.iter().map(|s| s.position).collect();
        let mut edges = Vec::new();
        for s in &pending {
            for dep_pos in &s.depends_on_positions {
                if !submitted_positions.contains(dep_pos) {
                    return Err(AgentError::Tool(ToolError::InvalidArgs(
                        InvalidArgsError::new(
                            "steps",
                            format!("depends_on position {dep_pos} not found in submitted steps"),
                        ),
                    )));
                }
                edges.push((s.position, *dep_pos));
            }
        }

        // Pre-validate dependency graph to avoid persisting an intermediate
        // invalid state. Cycles make phase2 impossible to resolve.
        if let Some(cycle) = detect_step_dependency_cycle(
            &pending.iter().map(|s| s.position).collect::<Vec<_>>(),
            &edges,
        ) {
            return Err(AgentError::Tool(ToolError::InvalidArgs(
                InvalidArgsError::new(
                    "steps",
                    format!("dependency cycle detected among positions {cycle:?}"),
                ),
            )));
        }

        // Phase 1: only existing step ids are known, so the position->id map
        // contains existing steps only. `depends_on` entries that point to new
        // steps are omitted because their ids are not yet assigned.
        let phase1_position_to_id = existing_by_position.clone();
        let phase1 = build_habit_step_inputs(&pending, &phase1_position_to_id);

        let result = self
            .client()
            .replace_habit_steps(habit_id, &phase1)
            .await
            .map_err(|e| AgentError::Tool(ToolError::Other(Box::new(e))))?;

        if let Some(phase2) =
            phase2_inputs_for_response(&pending, &phase1_position_to_id, &result)
        {
            let phase2 = phase2?;
            self.client()
                .replace_habit_steps(habit_id, &phase2)
                .await
                .map_err(|e| AgentError::Tool(ToolError::Other(Box::new(e))))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_habit_step_rejects_wrong_types_for_optional_fields() {
        let value = json!({
            "position": 1,
            "title": "warmup",
            "start_time": "08:00",
            "end_time": "08:15",
            "avg_minutes": 15,
            "parallelizable": "yes",
        });
        let err = parse_habit_step(&value).unwrap_err();
        assert!(format!("{err}").contains("expected a boolean"));
    }

    #[test]
    fn parse_habit_step_accepts_null_optional_fields() {
        let value = json!({
            "position": 1,
            "title": "warmup",
            "start_time": "08:00",
            "end_time": "08:15",
            "avg_minutes": 15,
            "parallelizable": null,
            "abandonability": null,
        });
        let step = parse_habit_step(&value).unwrap();
        assert_eq!(step.parallelizable, None);
        assert_eq!(step.abandonability, None);
    }

    #[test]
    fn build_habit_step_inputs_resolves_ids_and_depends_on_from_position_map() {
        let steps = vec![
            PendingHabitStep {
                position: 2,
                title: "run".into(),
                description: None,
                start_time: "08:15".parse().unwrap(),
                end_time: "08:45".parse().unwrap(),
                avg_minutes: 30,
                sigma_minutes: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                fixed: None,
                depends_on_positions: vec![1],
            },
            PendingHabitStep {
                position: 1,
                title: "warmup".into(),
                description: None,
                start_time: "08:00".parse().unwrap(),
                end_time: "08:15".parse().unwrap(),
                avg_minutes: 15,
                sigma_minutes: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                fixed: None,
                depends_on_positions: vec![],
            },
        ];

        // Simulate phase 1: only existing step id for position 1 is known.
        let phase1_map: HashMap<i64, String> = [(1, "existing-1".into())].into();
        let phase1 = build_habit_step_inputs(&steps, &phase1_map);
        assert_eq!(phase1[0].id, None); // position 2 is new
        assert_eq!(phase1[0].depends_on, vec!["existing-1".to_string()]);
        assert_eq!(phase1[1].id, Some("existing-1".into()));

        // Simulate phase 2: real ids are known for both positions, returned in
        // an order that differs from input order (as the server may do).
        let phase2_map: HashMap<i64, String> = [(1, "real-1".into()), (2, "real-2".into())].into();
        let phase2 = build_habit_step_inputs(&steps, &phase2_map);
        assert_eq!(phase2[0].id, Some("real-2".into()));
        assert_eq!(phase2[0].depends_on, vec!["real-1".to_string()]);
        assert_eq!(phase2[1].id, Some("real-1".into()));
    }

    #[test]
    fn build_habit_step_inputs_omits_unknown_dependency_positions() {
        let steps = vec![PendingHabitStep {
            position: 1,
            title: "a".into(),
            description: None,
            start_time: "08:00".parse().unwrap(),
            end_time: "08:15".parse().unwrap(),
            avg_minutes: 15,
            sigma_minutes: None,
            parallelizable: None,
            allows_parallel: None,
            abandonability: None,
            fixed: None,
            depends_on_positions: vec![99],
        }];
        let map: HashMap<i64, String> = HashMap::new();
        let inputs = build_habit_step_inputs(&steps, &map);
        assert!(inputs[0].depends_on.is_empty());
    }

    #[test]
    fn detect_step_dependency_cycle_finds_simple_cycle() {
        let positions = vec![1, 2, 3];
        let edges = vec![(1, 2), (2, 3), (3, 1)];
        let cycle = detect_step_dependency_cycle(&positions, &edges);
        assert!(cycle.is_some());
        let cycle = cycle.unwrap();
        assert!(cycle.contains(&1));
        assert!(cycle.contains(&2));
        assert!(cycle.contains(&3));
    }

    #[test]
    fn detect_step_dependency_cycle_accepts_dag() {
        let positions = vec![1, 2, 3];
        let edges = vec![(1, 2), (1, 3)];
        assert!(detect_step_dependency_cycle(&positions, &edges).is_none());
    }

    #[test]
    fn parse_habit_step_rejects_non_array_depends_on() {
        let value = json!({
            "position": 1,
            "title": "warmup",
            "start_time": "08:00",
            "end_time": "08:15",
            "avg_minutes": 15,
            "depends_on": "1",
        });
        let err = parse_habit_step(&value).unwrap_err();
        assert!(format!("{err}").contains("expected a sequence"));
    }

    #[test]
    fn parse_habit_step_rejects_unknown_fields() {
        let value = json!({
            "position": 1,
            "title": "warmup",
            "start_time": "08:00",
            "end_time": "08:15",
            "avg_minutes": 15,
            "unknown_field": true,
        });
        let err = parse_habit_step(&value).unwrap_err();
        assert!(format!("{err}").contains("unknown field `unknown_field`"));
    }

    fn habit_step_row(id: &str, position: i64) -> takusu_client::HabitStepRow {
        takusu_client::HabitStepRow {
            id: id.into(),
            habit_id: "habit-1".into(),
            position,
            title: "step".into(),
            description: None,
            start_time: "08:00".parse().unwrap(),
            end_time: "08:15".parse().unwrap(),
            avg_minutes: 15,
            sigma_minutes: 3,
            parallelizable: false,
            allows_parallel: false,
            abandonability: 0.5.into(),
            fixed: false,
            depends_on: Vec::new().into(),
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn phase2_inputs_for_response_returns_none_when_all_deps_are_existing() {
        let steps = vec![PendingHabitStep {
            position: 1,
            title: "a".into(),
            description: None,
            start_time: "08:00".parse().unwrap(),
            end_time: "08:15".parse().unwrap(),
            avg_minutes: 15,
            sigma_minutes: None,
            parallelizable: None,
            allows_parallel: None,
            abandonability: None,
            fixed: None,
            depends_on_positions: vec![],
        }];
        let phase1_map: HashMap<i64, String> = [(1, "existing-1".into())].into();
        let response = vec![habit_step_row("real-1", 0)];
        assert!(phase2_inputs_for_response(&steps, &phase1_map, &response).is_none());
    }

    #[test]
    fn phase2_inputs_for_response_resolves_new_step_ids_and_deps_in_any_order() {
        let steps = vec![
            PendingHabitStep {
                position: 2,
                title: "b".into(),
                description: None,
                start_time: "08:15".parse().unwrap(),
                end_time: "08:30".parse().unwrap(),
                avg_minutes: 15,
                sigma_minutes: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                fixed: None,
                depends_on_positions: vec![1],
            },
            PendingHabitStep {
                position: 1,
                title: "a".into(),
                description: None,
                start_time: "08:00".parse().unwrap(),
                end_time: "08:15".parse().unwrap(),
                avg_minutes: 15,
                sigma_minutes: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                fixed: None,
                depends_on_positions: vec![],
            },
        ];
        let phase1_map: HashMap<i64, String> = HashMap::new();
        let response = vec![habit_step_row("real-1", 0), habit_step_row("real-2", 1)];
        let phase2 = phase2_inputs_for_response(&steps, &phase1_map, &response)
            .unwrap()
            .unwrap();
        assert_eq!(phase2[0].id, Some("real-2".into()));
        assert_eq!(phase2[0].depends_on, vec!["real-1".to_string()]);
        assert_eq!(phase2[1].id, Some("real-1".into()));
    }

    #[test]
    fn phase2_inputs_for_response_errors_when_a_position_is_missing() {
        let steps = vec![
            PendingHabitStep {
                position: 1,
                title: "a".into(),
                description: None,
                start_time: "08:00".parse().unwrap(),
                end_time: "08:15".parse().unwrap(),
                avg_minutes: 15,
                sigma_minutes: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                fixed: None,
                depends_on_positions: vec![2],
            },
            PendingHabitStep {
                position: 2,
                title: "b".into(),
                description: None,
                start_time: "08:15".parse().unwrap(),
                end_time: "08:30".parse().unwrap(),
                avg_minutes: 15,
                sigma_minutes: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                fixed: None,
                depends_on_positions: vec![],
            },
        ];
        let phase1_map: HashMap<i64, String> = HashMap::new();
        let response = vec![habit_step_row("real-1", 0)];
        let result = phase2_inputs_for_response(&steps, &phase1_map, &response);
        assert!(result.unwrap().is_err());
    }
}
