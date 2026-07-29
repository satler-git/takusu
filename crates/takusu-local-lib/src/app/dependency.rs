//! Task / habit-step dependency graph analysis (#11).
//!
//! Extracted from the `app.rs` god module. Holds the shared dependency-graph
//! builders (`topo_sort_steps`, `build_dep_graph`), the witness-path response
//! structs (`DependencyNode`, `RedundantDependency`), and the
//! `TakusuApp::analyze_*` methods that surface redundant (composite) edges.

use std::collections::HashMap;

use serde::Serialize;
use takusu_storage::{HabitStepRow, TaskQuery, TaskRow};
use takusu_types::TaskStatus;

use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind};

/// Topologically sort habit steps by their `depends_on` DAG (#95). Steps with
/// no dependencies come first. Returns indices into `steps`. Cycles are
/// rejected (defensive — validation already caught them at replace time).
pub(super) fn topo_sort_steps(steps: &[HabitStepRow]) -> Result<Vec<usize>, AppError> {
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, s) in steps.iter().enumerate() {
        id_to_idx.insert(s.id.clone(), i);
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for (i, s) in steps.iter().enumerate() {
        let deps: Vec<String> = s.depends_on.to_vec();
        for dep in &deps {
            if let Some(&dep_idx) = id_to_idx.get(dep) {
                // edge dep_idx → i (dep must come before i)
                adj[dep_idx].push(i);
            }
        }
    }
    crate::graph::topo_sort(&adj).map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))
}

#[allow(clippy::type_complexity)]
pub(super) fn build_dep_graph(
    tasks: &[TaskRow],
) -> Result<(Vec<Vec<usize>>, HashMap<String, usize>), AppError> {
    let mut id_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, t) in tasks.iter().enumerate() {
        id_to_idx.insert(t.id.clone(), i);
    }
    let mut adj = vec![Vec::new(); tasks.len()];
    for t in tasks {
        let idx = id_to_idx[&t.id];
        let deps: Vec<String> = t.depends.to_vec();
        for dep_id in &deps {
            if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                adj[idx].push(dep_idx);
            }
        }
    }
    Ok((adj, id_to_idx))
}

/// A node on a dependency witness path (task or habit step) (#355).
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DependencyNode {
    pub id: String,
    pub title: String,
}

/// A redundant (composite / transitively implied) dependency edge with a
/// witness path proving the direct edge is unnecessary (#355).
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RedundantDependency {
    pub from: String,
    pub from_title: String,
    pub to: String,
    pub to_title: String,
    /// Witness path `from → … → to` (endpoints included, length >= 3).
    pub via: Vec<DependencyNode>,
}

/// Response for `GET /api/tasks/dependency-analysis` and the habit step
/// variant (#355).
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DependencyAnalysisResponse {
    pub redundant: Vec<RedundantDependency>,
}

impl super::TakusuApp {
    pub async fn analyze_task_dependencies(&self) -> Result<Vec<RedundantDependency>, AppError> {
        let tasks = self
            .storage
            .list_tasks(&TaskQuery::default())
            .await
            .map_err(storage_to_app)?;
        let active: Vec<&TaskRow> = tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Completed && t.status != TaskStatus::Skipped)
            .collect();
        let mut id_to_idx: HashMap<String, usize> = HashMap::new();
        for (i, t) in active.iter().enumerate() {
            id_to_idx.insert(t.id.clone(), i);
        }
        let mut adj = vec![Vec::new(); active.len()];
        for (i, t) in active.iter().enumerate() {
            let deps: Vec<String> = t.depends.to_vec();
            for dep_id in &deps {
                if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                    adj[i].push(dep_idx);
                }
            }
        }
        let redundant = crate::graph::find_redundant_edges(&adj)
            .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;
        let node = |idx: usize| DependencyNode {
            id: active[idx].id.clone(),
            title: active[idx].title.clone(),
        };
        Ok(redundant
            .into_iter()
            .map(|e| RedundantDependency {
                from: active[e.from].id.clone(),
                from_title: active[e.from].title.clone(),
                to: active[e.to].id.clone(),
                to_title: active[e.to].title.clone(),
                via: e.via.iter().map(|&i| node(i)).collect(),
            })
            .collect())
    }

    /// Detect redundant (composite) edges in a habit's step dependency DAG.
    pub async fn analyze_habit_step_dependencies(
        &self,
        habit_id: &str,
    ) -> Result<Vec<RedundantDependency>, AppError> {
        let steps = self
            .storage
            .list_habit_steps(habit_id)
            .await
            .map_err(storage_to_app)?;
        let mut id_to_idx: HashMap<String, usize> = HashMap::new();
        for (i, s) in steps.iter().enumerate() {
            id_to_idx.insert(s.id.clone(), i);
        }
        let mut adj = vec![Vec::new(); steps.len()];
        for (i, s) in steps.iter().enumerate() {
            let deps: Vec<String> = s.depends_on.to_vec();
            for dep_id in &deps {
                if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                    adj[i].push(dep_idx);
                }
            }
        }
        let redundant = crate::graph::find_redundant_edges(&adj)
            .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;
        let node = |idx: usize| DependencyNode {
            id: steps[idx].id.clone(),
            title: steps[idx].title.clone(),
        };
        Ok(redundant
            .into_iter()
            .map(|e| RedundantDependency {
                from: steps[e.from].id.clone(),
                from_title: steps[e.from].title.clone(),
                to: steps[e.to].id.clone(),
                to_title: steps[e.to].title.clone(),
                via: e.via.iter().map(|&i| node(i)).collect(),
            })
            .collect())
    }
}
