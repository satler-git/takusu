//! Task CRUD, batch creation, work tracking, and iCal import (#11).
//!
//! Extracted from the `app.rs` god module. Holds all task-lifecycle methods
//! (create / update / replace / delete / batch), work-tracking wrappers
//! (start / pause / progress / complete), task splitting, completion query,
//! and iCal import.

use std::collections::HashMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use takusu_search::search::{Completion, complete};
use takusu_storage::{
    CreateTask, CreateTaskBatch, CreateTaskBatchItem, CreateTaskBatchResult, ProgressResult,
    RecordProgress, SplitResult, SplitTask, TaskProgress, TaskQuery, TaskRow, UpdateTask,
};

use super::dependency;
use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind};
use crate::validate::{
    Validate, parse_settings_timezone, validate_minutes, validate_task_datetimes,
};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct IcalImportResult {
    pub imported: usize,
    pub task_ids: Vec<String>,
}

impl super::TakusuApp {
    pub async fn create_task(&self, body: &CreateTask) -> Result<TaskRow, AppError> {
        body.validate()?;
        let mut body = body.clone();
        // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
        if let Some(ref dep_ids) = body.depends
            && !dep_ids.is_empty()
        {
            let tasks = self
                .storage
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(storage_to_app)?;
            let (_adj, id_to_idx) = dependency::build_dep_graph(&tasks)?;
            // Resolve display_id numbers / full UUIDs before
            // validating against the dep graph (which is keyed by UUID).
            let mut resolved = Vec::with_capacity(dep_ids.len());
            for did in dep_ids {
                let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                if !id_to_idx.contains_key(&full) {
                    return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                        "depends on unknown task: {did}"
                    ))));
                }
                resolved.push(full);
            }
            body.depends = Some(resolved);
        }
        self.storage
            .create_task(&body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn create_task_batch(
        &self,
        body: &CreateTaskBatch,
    ) -> Result<Vec<CreateTaskBatchResult>, AppError> {
        if body.tasks.is_empty() {
            return Ok(Vec::new());
        }

        // Validate/normalize each item and build a map from client_id to local index.
        let mut items: Vec<CreateTaskBatchItem> = Vec::with_capacity(body.tasks.len());
        let mut client_to_local: HashMap<String, usize> = HashMap::new();
        for (i, item) in body.tasks.iter().enumerate() {
            item.task.validate()?;
            if let Some(ref cid) = item.client_id {
                if cid.is_empty() {
                    return Err(AppError::BadRequest(BadRequestKind::Other(
                        "client_id must not be empty".into(),
                    )));
                }
                if client_to_local.insert(cid.clone(), i).is_some() {
                    return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                        "duplicate client_id: {cid}"
                    ))));
                }
            }
            let task = item.task.clone();
            // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
            items.push(CreateTaskBatchItem {
                task,
                client_id: item.client_id.clone(),
            });
        }

        // Load existing tasks to detect cycles across old and new dependencies.
        let existing_tasks = self
            .storage
            .list_tasks(&TaskQuery::default())
            .await
            .map_err(storage_to_app)?;
        let (mut adj, existing_id_to_idx) = dependency::build_dep_graph(&existing_tasks)?;
        let existing_offset = existing_tasks.len();
        adj.resize(existing_offset + items.len(), Vec::new());

        // Resolve each item's depends to graph indices.
        let mut deps_by_local: Vec<Vec<usize>> = Vec::with_capacity(items.len());
        for (local_idx, item) in items.iter().enumerate() {
            let mut dep_indices = Vec::new();
            if let Some(ref dep_ids) = item.task.depends {
                for did in dep_ids {
                    if let Some(&dep_local) = client_to_local.get(did) {
                        dep_indices.push(existing_offset + dep_local);
                    } else {
                        let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                        let &idx = existing_id_to_idx.get(&full).ok_or_else(|| {
                            AppError::BadRequest(BadRequestKind::Other(format!(
                                "depends on unknown task: {did}"
                            )))
                        })?;
                        dep_indices.push(idx);
                    }
                }
            }
            let global_idx = existing_offset + local_idx;
            adj[global_idx].extend(dep_indices.iter().copied());
            deps_by_local.push(dep_indices);
        }

        crate::graph::detect_cycle(&adj)
            .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;

        // Topologically sort and then reverse so dependencies are created
        // before dependents (the graph edges point from dependent to dependency).
        let mut order = crate::graph::topo_sort(&adj)
            .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;
        order.reverse();
        let mut created_ids: Vec<Option<String>> = vec![None; items.len()];
        let mut results: Vec<Option<CreateTaskBatchResult>> = vec![None; items.len()];
        for global_idx in order {
            if global_idx < existing_offset {
                continue;
            }
            let local_idx = global_idx - existing_offset;
            let item = &items[local_idx];
            let mut resolved_dep_ids = Vec::with_capacity(deps_by_local[local_idx].len());
            for &dep_global in &deps_by_local[local_idx] {
                if dep_global < existing_offset {
                    resolved_dep_ids.push(existing_tasks[dep_global].id.clone());
                } else {
                    let dep_local = dep_global - existing_offset;
                    let actual = created_ids[dep_local].as_ref().ok_or_else(|| {
                        AppError::Internal("dependency not created before dependent".into())
                    })?;
                    resolved_dep_ids.push(actual.clone());
                }
            }
            let mut task = item.task.clone();
            task.depends = Some(resolved_dep_ids).filter(|v| !v.is_empty());
            let row = self
                .storage
                .create_task(&task)
                .await
                .map_err(storage_to_app)?;
            created_ids[local_idx] = Some(row.id.clone());
            results[local_idx] = Some(CreateTaskBatchResult {
                client_id: item.client_id.clone(),
                task: row,
            });
        }

        results
            .into_iter()
            .map(|opt| opt.ok_or_else(|| AppError::Internal("missing batch task result".into())))
            .collect()
    }

    pub async fn list_tasks(&self, query: &TaskQuery) -> Result<Vec<TaskRow>, AppError> {
        self.storage.list_tasks(query).await.map_err(storage_to_app)
    }

    pub async fn complete_task_query(
        &self,
        input: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Completion>, AppError> {
        const DEFAULT_COMPLETION_LIMIT: usize = 30;
        const MAX_COMPLETION_LIMIT: usize = 100;

        let limit = match limit {
            None => DEFAULT_COMPLETION_LIMIT,
            Some(0) => {
                return Err(AppError::BadRequest(BadRequestKind::Other(
                    "completion limit must be at least 1".to_string(),
                )));
            }
            Some(n) if n > MAX_COMPLETION_LIMIT => {
                return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                    "completion limit must be at most {MAX_COMPLETION_LIMIT}"
                ))));
            }
            Some(n) => n,
        };

        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let today = Timestamp::now().to_zoned(tz).date();

        let tasks = self
            .storage
            .list_tasks(&TaskQuery {
                limit: Some(200),
                ..TaskQuery::default()
            })
            .await
            .map_err(storage_to_app)?;
        let habits = self.storage.list_habits().await.map_err(storage_to_app)?;

        Ok(complete(input, today, &tasks, &habits, Some(limit)))
    }

    pub async fn get_task(&self, id: &str) -> Result<TaskRow, AppError> {
        self.storage.get_task(id).await.map_err(storage_to_app)
    }

    pub async fn update_task(&self, id: &str, body: &UpdateTask) -> Result<TaskRow, AppError> {
        body.validate()?;
        let mut body = body.clone();

        // Fetch the existing task once if any downstream logic needs it.
        let needs_existing = body.start_at.is_some()
            || body.end_at.is_some()
            || body.depends.is_some()
            || body.user_edited.is_none();
        let existing = if needs_existing {
            Some(self.storage.get_task(id).await.map_err(storage_to_app)?)
        } else {
            None
        };

        // Validate datetime fields and their logical ordering (#934).
        // Context-dependent: needs the existing row, so not part of Validate.
        if body.start_at.is_some() || body.end_at.is_some() {
            let existing = existing.as_ref().unwrap();
            validate_task_datetimes(
                body.start_at.as_ref().map(|o| o.as_ref()),
                body.end_at.as_ref(),
                existing.start_at.as_ref(),
                Some(&existing.end_at),
            )?;
        }

        // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
        // start_at/end_at use Option<Option<Timestamp>>:
        //   None = no change, Some(None) = clear to NULL, Some(Some(ts)) = set.

        if let Some(dep_ids) = &body.depends {
            let tasks = self
                .storage
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(storage_to_app)?;
            let (mut adj, id_to_idx) = dependency::build_dep_graph(&tasks)?;
            let full_id = existing.as_ref().unwrap().id.clone();
            let target_idx = id_to_idx
                .get(&full_id)
                .ok_or_else(|| AppError::NotFound(format!("task {id} not found")))?;
            // Resolve display_id numbers / full UUIDs before
            // validating against the dep graph (which is keyed by UUID).
            let mut resolved = Vec::with_capacity(dep_ids.len());
            for did in dep_ids {
                let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                if !id_to_idx.contains_key(&full) {
                    return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                        "depends on unknown task: {did}"
                    ))));
                }
                resolved.push(full);
            }
            adj[*target_idx] = resolved
                .iter()
                .filter_map(|did| id_to_idx.get(did).copied())
                .collect();
            crate::graph::detect_cycle(&adj)
                .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;
            body.depends = Some(resolved);
        }

        // User-edited flag: for habit-derived tasks, mark as user-edited when
        // habit-managed fields are touched by an HTTP request, unless the
        // caller explicitly set user_edited (e.g. "revert to habit" sets false).
        if body.user_edited.is_none() {
            let existing = existing.as_ref().unwrap();
            if existing.habit_id.is_some() {
                let touched = body.title.is_some()
                    || body.description.is_some()
                    || body.start_at.is_some()
                    || body.end_at.is_some()
                    || body.avg_minutes.is_some()
                    || body.sigma_minutes.is_some()
                    || body.parallelizable.is_some()
                    || body.allows_parallel.is_some()
                    || body.abandonability.is_some()
                    || body.fixed.is_some();
                if touched {
                    body.user_edited = Some(true);
                }
            }
        }

        self.storage
            .update_task(id, &body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn replace_task(&self, id: &str, body: &CreateTask) -> Result<TaskRow, AppError> {
        body.validate()?;
        let mut body = body.clone();
        // Timestamps are already normalized (RFC 3339 UTC) by deserialization.
        if let Some(ref dep_ids) = body.depends
            && !dep_ids.is_empty()
        {
            let tasks = self
                .storage
                .list_tasks(&TaskQuery::default())
                .await
                .map_err(storage_to_app)?;
            let (mut adj, id_to_idx) = dependency::build_dep_graph(&tasks)?;
            let full_id = self.storage.get_task(id).await.map_err(storage_to_app)?.id;
            let target_idx = id_to_idx
                .get(&full_id)
                .ok_or_else(|| AppError::NotFound(format!("task {id} not found")))?;
            // Resolve display_id numbers / full UUIDs before
            // validating against the dep graph (which is keyed by UUID).
            let mut resolved = Vec::with_capacity(dep_ids.len());
            for did in dep_ids {
                let full = self.storage.get_task(did).await.map_err(storage_to_app)?.id;
                if !id_to_idx.contains_key(&full) {
                    return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                        "depends on unknown task: {did}"
                    ))));
                }
                resolved.push(full);
            }
            adj[*target_idx] = resolved
                .iter()
                .filter_map(|did| id_to_idx.get(did).copied())
                .collect();
            crate::graph::detect_cycle(&adj)
                .map_err(|_| AppError::BadRequest(BadRequestKind::CycleDetected))?;
            body.depends = Some(resolved);
            return self
                .storage
                .replace_task(id, &body)
                .await
                .map_err(storage_to_app);
        }
        self.storage
            .replace_task(id, &body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_task(&self, id: &str) -> Result<(), AppError> {
        self.storage.delete_task(id).await.map_err(storage_to_app)
    }

    pub async fn start_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, AppError> {
        self.storage
            .start_task_work(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn pause_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, AppError> {
        self.storage
            .pause_task_work(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn record_progress(
        &self,
        id: &str,
        body: &RecordProgress,
        operation_id: Option<&str>,
    ) -> Result<ProgressResult, AppError> {
        self.storage
            .record_progress(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn complete_task_work(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> Result<TaskRow, AppError> {
        self.storage
            .complete_task_work(id, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_task_progress(&self, id: &str) -> Result<TaskProgress, AppError> {
        self.storage
            .get_task_progress(id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn split_task(
        &self,
        id: &str,
        body: &SplitTask,
        operation_id: Option<&str>,
    ) -> Result<SplitResult, AppError> {
        if body.end_at.is_some() {
            let original = self.storage.get_task(id).await.map_err(storage_to_app)?;
            validate_task_datetimes(None, body.end_at.as_ref(), original.start_at.as_ref(), None)?;
        }
        self.storage
            .split_task(id, body, operation_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn import_ical(&self, ical_body: &str) -> Result<IcalImportResult, AppError> {
        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let events = takusu_ical::parse_ical(ical_body, &tz)
            .map_err(|e| AppError::BadRequest(BadRequestKind::Other(e.to_string())))?;
        let mut imported = 0usize;
        let mut task_ids = Vec::new();
        for event in &events {
            if let Some(ref uid) = event.uid
                && self.task_exists_by_ical_uid(uid).await?
            {
                continue;
            }
            let start_at: takusu_types::Timestamp = event.start_at.into();
            let end_at: takusu_types::Timestamp = event.end_at.into();
            let avg_minutes = takusu_types::minutes_between_ts(start_at, end_at);
            validate_minutes(avg_minutes, Some(0))?;
            let task = self
                .storage
                .create_task(&CreateTask {
                    title: event.title.clone(),
                    description: event.description.clone(),
                    start_at: Some(start_at),
                    end_at,
                    avg_minutes,
                    sigma_minutes: Some(0),
                    depends: Some(vec![]),
                    parallelizable: Some(false),
                    allows_parallel: Some(false),
                    abandonability: Some(0.5.into()),
                    ical_uid: event.uid.clone(),
                    habit_id: None,
                    fixed: Some(true),
                    habit_step_id: None,
                    quantity_total: None,
                    quantity_done: None,
                    quantity_unit: None,
                    original_quantity_total: None,
                })
                .await
                .map_err(storage_to_app)?;
            imported += 1;
            task_ids.push(task.id);
        }
        Ok(IcalImportResult { imported, task_ids })
    }

    async fn task_exists_by_ical_uid(&self, uid: &str) -> Result<bool, AppError> {
        self.storage
            .task_exists_by_ical_uid(uid)
            .await
            .map_err(storage_to_app)
    }
}
