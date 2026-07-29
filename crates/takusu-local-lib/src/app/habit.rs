//! Habit CRUD, preview, estimate, scheduled spans, and steps (#11).
//!
//! Extracted from the `app.rs` god module. Holds all habit-lifecycle methods
//! and the habit preview / estimate logic that builds on the
//! [`super::habit_sync`] helpers.

use std::collections::HashSet;

use jiff::Timestamp;
use takusu_core::{Minutes, NormalDist, ParallelMode, Point};
use takusu_storage::{
    CreateHabit, CreateHabitBatch, CreateHabitBatchResult, CreateHabitScheduledSpan, HabitDetail,
    HabitEstimateRequest, HabitEstimateResult, HabitEstimateSample, HabitEstimateStep,
    HabitPreviewRequest, HabitPreviewTask, HabitRow, HabitScheduledSpanRow, HabitStepEstimateInput,
    HabitStepInput, HabitStepRow, TaskQuery, TaskRow, UpdateHabit,
};
use takusu_types::{TaskStatusFilter, WindowMode};

use super::dependency::topo_sort_steps;
use super::habit_sync::{
    build_habit_from_preview, core_task_to_preview, freq_fallback_slots, step_input_to_preview_row,
    step_to_core_task, step_to_core_task_period,
};
use super::schedule::{iso_to_point, point_to_local_date};
use crate::error::storage_to_app;
use crate::error::{AppError, BadRequestKind};
use crate::validate::{Validate, parse_recurrence, parse_settings_timezone};

impl super::TakusuApp {
    pub async fn create_habit(&self, body: &CreateHabit) -> Result<HabitRow, AppError> {
        body.validate()?;

        self.storage
            .create_habit(body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn create_habit_batch(
        &self,
        body: &CreateHabitBatch,
    ) -> Result<Vec<CreateHabitBatchResult>, AppError> {
        if body.habits.is_empty() {
            return Ok(Vec::new());
        }
        let mut seen_client_ids: HashSet<String> = HashSet::new();
        for item in &body.habits {
            if let Some(ref cid) = item.client_id {
                if cid.is_empty() {
                    return Err(AppError::BadRequest(BadRequestKind::Other(
                        "client_id must not be empty".into(),
                    )));
                }
                if !seen_client_ids.insert(cid.clone()) {
                    return Err(AppError::BadRequest(BadRequestKind::Other(format!(
                        "duplicate client_id: {cid}"
                    ))));
                }
            }
        }
        let mut results = Vec::with_capacity(body.habits.len());
        for item in &body.habits {
            let row = self.create_habit(&item.habit).await?;
            results.push(CreateHabitBatchResult {
                client_id: item.client_id.clone(),
                habit: row,
            });
        }
        Ok(results)
    }

    /// Preview the tasks that would be generated from a habit definition
    /// without persisting it. Uses `takusu_habit` directly so the preview
    /// matches the server's task-generation logic.
    pub async fn preview_habit(
        &self,
        request: &HabitPreviewRequest,
    ) -> Result<Vec<HabitPreviewTask>, AppError> {
        request.validate()?;

        let settings = self.get_settings_or_default().await?;
        let tz = parse_settings_timezone(&settings.tz)?;
        let now_ts = Timestamp::now();
        let start_of_today = now_ts
            .to_zoned(tz.clone())
            .start_of_day()
            .map_err(|e| AppError::Internal(format!("start_of_day: {e}")))?
            .timestamp();
        let from_default = Point::from_timestamp(start_of_today, 5);
        let from = request
            .from
            .as_ref()
            .map(|s| iso_to_point(s, &tz))
            .transpose()?
            .unwrap_or(from_default);
        let until = request
            .until
            .as_ref()
            .map(|s| iso_to_point(s, &tz))
            .transpose()?
            .unwrap_or(Point(from.0 + 30 * 288));
        let max_occurrences = request.max_occurrences.unwrap_or(20).max(1) as usize;

        let habit = build_habit_from_preview(request, &tz)?;
        let is_period = request.window_mode == Some(WindowMode::Period);

        let mut store = takusu_habit::HabitStore::new();
        store.add(habit);

        let mut occurrences: Vec<(Point, Point)> = Vec::new();
        if is_period {
            let rule = parse_recurrence(&request.recurrence)?;
            let until_lookahead = Point(until.0 + 365 * 288);
            let occs: Vec<(String, Point)> = store
                .generate(from, until_lookahead)
                .into_iter()
                .map(|gt| {
                    let sp = gt.task.start.unwrap_or(Point(0));
                    let date = point_to_local_date(sp.0, &tz)?;
                    Ok((date, sp))
                })
                .collect::<Result<Vec<_>, AppError>>()?;
            for (i, (_, occ_start)) in occs.iter().enumerate() {
                if occ_start.0 >= until.0 {
                    break;
                }
                let deadline = if let Some((_, next_start)) = occs.get(i + 1) {
                    *next_start
                } else {
                    Point(occ_start.0 + freq_fallback_slots(&rule))
                };
                occurrences.push((*occ_start, deadline));
                if occurrences.len() >= max_occurrences {
                    break;
                }
            }
        } else {
            for gt in store.generate(from, until) {
                let sp = gt.task.start.unwrap_or(Point(0));
                occurrences.push((sp, gt.task.end));
                if occurrences.len() >= max_occurrences {
                    break;
                }
            }
        }

        let step_rows: Vec<HabitStepRow> = request
            .steps
            .iter()
            .map(step_input_to_preview_row)
            .collect();

        let mut tasks: Vec<HabitPreviewTask> = Vec::new();
        for (occ_start, deadline) in occurrences {
            if !step_rows.is_empty() {
                let order = topo_sort_steps(&step_rows)?;
                for &idx in &order {
                    let step = &step_rows[idx];
                    let core = if is_period {
                        step_to_core_task_period(step, occ_start, deadline)
                    } else {
                        step_to_core_task(step, occ_start, &tz)?
                    };
                    tasks.push(core_task_to_preview(&core, &step.title));
                }
            } else {
                let sigma = request.sigma_minutes.unwrap_or(0);
                let cost = NormalDist::from_minutes(Minutes(request.avg_minutes), Minutes(sigma));
                let core = takusu_core::Task {
                    id: 0,
                    start: Some(occ_start),
                    end: deadline,
                    cost_estimate: cost,
                    depends: vec![],
                    parallel_mode: ParallelMode::from_bools(
                        request.parallelizable.unwrap_or(false),
                        request.allows_parallel.unwrap_or(false),
                    ),
                    abandonability: request.abandonability.unwrap_or(0.5.into()),
                    fixed: request.fixed.unwrap_or(false),
                    habit_group: None,
                };
                tasks.push(core_task_to_preview(&core, &request.title));
            }
        }

        Ok(tasks)
    }

    pub async fn list_habits(&self) -> Result<Vec<HabitRow>, AppError> {
        self.storage.list_habits().await.map_err(storage_to_app)
    }

    pub async fn get_habit(&self, id: &str) -> Result<HabitDetail, AppError> {
        let habit = self.storage.get_habit(id).await.map_err(storage_to_app)?;
        let steps = self
            .storage
            .list_habit_steps(id)
            .await
            .map_err(storage_to_app)?;
        Ok(HabitDetail { habit, steps })
    }

    /// Compute a habit's `avg_minutes` / `sigma_minutes` from the actual
    /// durations of completed, non-fixed tasks. Fixed habits and fixed tasks
    /// are ignored. Outliers are optionally detected and excluded using the
    /// median absolute deviation (MAD) when `request.detect_outliers` is true.
    ///
    /// For habits with steps, an estimate is computed per non-fixed step and
    /// persisted atomically via `Storage::apply_habit_estimate`. Fixed steps
    /// are left untouched and still included in the combined total. For habits
    /// without steps, the habit's own estimate is updated directly.
    pub async fn estimate_habit(
        &self,
        id: &str,
        request: &HabitEstimateRequest,
    ) -> Result<HabitEstimateResult, AppError> {
        let habit = self.storage.get_habit(id).await.map_err(storage_to_app)?;
        if habit.fixed {
            return Err(AppError::BadRequest(BadRequestKind::Other(
                "cannot estimate fixed habit from actuals".into(),
            )));
        }

        let completed = self
            .storage
            .list_tasks(&TaskQuery {
                status: Some(TaskStatusFilter::Completed),
                habit_id: Some(id.to_string()),
                ..TaskQuery::default()
            })
            .await
            .map_err(storage_to_app)?;

        // Group actual minutes by habit_step_id. None means the task was
        // generated for the habit itself rather than a specific step.
        let mut by_step: std::collections::HashMap<Option<String>, Vec<(TaskRow, i64)>> =
            std::collections::HashMap::new();
        for t in completed {
            if t.fixed {
                continue;
            }
            let actual = match t.actual_minutes {
                Some(a) if a > 0 => a,
                _ => continue,
            };
            by_step
                .entry(t.habit_step_id.clone())
                .or_default()
                .push((t, actual));
        }

        // `list_habit_steps` is already ordered by position ASC, created_at ASC,
        // so iterate it directly to keep the response deterministic.
        let step_rows = self
            .storage
            .list_habit_steps(id)
            .await
            .map_err(storage_to_app)?;

        let mut step_inputs: Vec<HabitStepEstimateInput> = Vec::new();
        let mut steps: Vec<HabitEstimateStep> = Vec::new();
        let mut has_step_samples = false;
        let mut combined_avg: i128 = 0;
        let mut combined_sigma_sq: f64 = 0.0;

        // Per-step estimates for habits with steps. Fixed steps are included
        // in the combined total and in the response, but not in the update
        // input, so they are never touched by `apply_habit_estimate`.
        for step in &step_rows {
            let (effective_avg, effective_sigma, sample_count, excluded_count) = if step.fixed {
                (step.avg_minutes, step.sigma_minutes, 0, 0)
            } else {
                let entries = by_step.remove(&Some(step.id.clone())).unwrap_or_default();
                let minutes: Vec<i64> = entries.iter().map(|(_, m)| *m).collect();
                let (avg, sigma, excluded) = takusu_types::estimate_from_samples_with_outliers(
                    &minutes,
                    request.detect_outliers,
                );

                // If a step has no samples, keep its current estimate so the
                // combined total and the persisted values remain meaningful.
                let effective_avg = if minutes.is_empty() {
                    step.avg_minutes
                } else {
                    avg
                };
                let effective_sigma = if minutes.is_empty() {
                    step.sigma_minutes
                } else {
                    sigma
                };

                if !minutes.is_empty() {
                    has_step_samples = true;
                }

                step_inputs.push(HabitStepEstimateInput {
                    step_id: step.id.clone(),
                    avg_minutes: effective_avg,
                    sigma_minutes: effective_sigma,
                });

                (
                    effective_avg,
                    effective_sigma,
                    entries.len(),
                    excluded.len(),
                )
            };

            combined_avg += effective_avg as i128;
            combined_sigma_sq += (effective_sigma as f64).powi(2);

            steps.push(HabitEstimateStep {
                step_id: step.id.clone(),
                title: step.title.clone(),
                avg_minutes: effective_avg,
                sigma_minutes: effective_sigma,
                sample_count,
                excluded_count,
                applied: request.apply && !step.fixed && sample_count > 0,
            });
        }

        let overall_entries = by_step.remove(&None).unwrap_or_default();
        let overall_minutes: Vec<i64> = overall_entries.iter().map(|(_, m)| *m).collect();
        let (overall_avg, overall_sigma, overall_excluded) =
            takusu_types::estimate_from_samples_with_outliers(
                &overall_minutes,
                request.detect_outliers,
            );

        let overall_excluded_set: std::collections::HashSet<usize> =
            overall_excluded.iter().copied().collect();
        let overall_samples: Vec<HabitEstimateSample> = overall_entries
            .into_iter()
            .enumerate()
            .map(|(i, (t, actual))| HabitEstimateSample {
                task_id: t.id,
                title: t.title,
                actual_minutes: actual,
                excluded: overall_excluded_set.contains(&i),
            })
            .collect();

        // Habits with steps use the combined step total. Habits without steps
        // use the overall task estimate.
        let (final_avg, final_sigma) = if step_rows.is_empty() {
            (overall_avg, overall_sigma)
        } else {
            let max = takusu_types::MAX_ESTIMATE_MINUTES as i128;
            let min = takusu_types::MIN_ESTIMATE_MINUTES as i128;
            (
                combined_avg.clamp(min, max) as i64,
                (combined_sigma_sq.sqrt().round() as i128).clamp(min, max) as i64,
            )
        };

        let has_samples = has_step_samples || !overall_minutes.is_empty();
        let applied = request.apply && has_samples;
        let habit_row = if applied {
            self.storage
                .apply_habit_estimate(id, final_avg, final_sigma, &step_inputs)
                .await
                .map_err(storage_to_app)?;
            Some(self.storage.get_habit(id).await.map_err(storage_to_app)?)
        } else {
            None
        };

        let total_sample_count =
            steps.iter().map(|s| s.sample_count).sum::<usize>() + overall_samples.len();
        let total_excluded_count =
            steps.iter().map(|s| s.excluded_count).sum::<usize>() + overall_excluded.len();

        Ok(HabitEstimateResult {
            avg_minutes: final_avg,
            sigma_minutes: final_sigma,
            sample_count: total_sample_count,
            excluded_count: total_excluded_count,
            samples: overall_samples,
            steps,
            applied,
            habit: habit_row,
        })
    }

    pub async fn update_habit(&self, id: &str, body: &UpdateHabit) -> Result<HabitRow, AppError> {
        body.validate()?;

        self.storage
            .update_habit(id, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn replace_habit(&self, id: &str, body: &CreateHabit) -> Result<HabitRow, AppError> {
        body.validate()?;

        self.storage
            .replace_habit(id, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_habit(&self, id: &str) -> Result<(), AppError> {
        self.storage.delete_habit(id).await.map_err(storage_to_app)
    }

    // ── Habit scheduled spans (#303 / #503) ──────────────

    pub async fn list_habit_scheduled_spans(
        &self,
        id: &str,
    ) -> Result<Vec<HabitScheduledSpanRow>, AppError> {
        self.storage
            .list_habit_scheduled_spans(id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn list_all_habit_scheduled_spans(
        &self,
    ) -> Result<Vec<HabitScheduledSpanRow>, AppError> {
        self.storage
            .list_all_habit_scheduled_spans()
            .await
            .map_err(storage_to_app)
    }

    pub async fn create_habit_scheduled_span(
        &self,
        id: &str,
        body: &CreateHabitScheduledSpan,
    ) -> Result<HabitScheduledSpanRow, AppError> {
        body.validate()?;
        self.storage
            .create_habit_scheduled_span(id, body)
            .await
            .map_err(storage_to_app)
    }

    pub async fn delete_habit_scheduled_span(
        &self,
        id: &str,
        span_id: &str,
    ) -> Result<(), AppError> {
        self.storage
            .delete_habit_scheduled_span(id, span_id)
            .await
            .map_err(storage_to_app)
    }

    // ── Habit steps (#95) ───────────────────────────────

    pub async fn list_habit_steps(&self, id: &str) -> Result<Vec<HabitStepRow>, AppError> {
        self.storage
            .list_habit_steps(id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn list_all_habit_steps(&self) -> Result<Vec<HabitStepRow>, AppError> {
        self.storage
            .list_all_habit_steps()
            .await
            .map_err(storage_to_app)
    }

    pub async fn replace_habit_steps(
        &self,
        id: &str,
        steps: &[HabitStepInput],
    ) -> Result<Vec<HabitStepRow>, AppError> {
        steps.validate()?;
        self.storage
            .replace_habit_steps(id, steps)
            .await
            .map_err(storage_to_app)
    }
}
