//! Application-layer planner event evaluation and ledger delivery operations.

use std::collections::{HashMap, HashSet};

use takusu_agent::coverage::{bootstrap_evaluation, compute_coverage};
use takusu_agent::events::{
    EvaluationGap, EvaluationScheduleEntry, EvaluationSnapshot, EvaluationTask, EvaluationWork,
    GapKind, LedgerView, PlannerEvent, SleepImpact, evaluate_events,
};
use takusu_contracts::{
    EvaluationInputs, EventDeliveryState, EventLedgerInsert, EventLedgerRow, SettingsRow,
    UnsettledIntervalRow,
};
use takusu_types::TaskStatus;
use takusu_types::estimator::{DurationDistribution, InterventionBand, effective_distribution};

use super::TakusuApp;
use crate::error::{AppError, storage_to_app};

impl TakusuApp {
    pub async fn get_schedule_revision(&self) -> Result<i64, AppError> {
        self.storage
            .get_schedule_revision()
            .await
            .map_err(storage_to_app)
    }

    pub async fn get_evaluation_inputs(&self) -> Result<EvaluationInputs, AppError> {
        self.storage
            .get_evaluation_inputs()
            .await
            .map_err(storage_to_app)
    }

    /// Evaluate one consistent planner snapshot and persist newly discovered
    /// deterministic events. Only the current resident authority may commit;
    /// other devices receive an empty `EvaluationResult` and the resident's
    /// `next_eval_at` so they can wait or schedule the next alarm.
    pub async fn evaluate_and_commit_events(
        &self,
        device_id: &str,
    ) -> Result<takusu_agent::events::EvaluationResult, AppError> {
        let authority = self
            .storage
            .resolve_resident_authority(device_id)
            .await
            .map_err(storage_to_app)?;
        if !authority.is_resident {
            return Ok(takusu_agent::events::EvaluationResult {
                due_events: Vec::new(),
                next_eval_at: authority.next_eval_at,
            });
        }

        let now = takusu_types::Timestamp::now();
        let EvaluationInputs {
            schedule_revision,
            tasks,
            schedule: raw_schedule,
            progress,
            ledger,
            coverage,
        } = self
            .storage
            .get_evaluation_inputs()
            .await
            .map_err(storage_to_app)?;

        let schedule_entries: Vec<EvaluationScheduleEntry> = raw_schedule
            .iter()
            .map(|entry| EvaluationScheduleEntry {
                task_id: entry.task_id.clone(),
                start_at: entry.start_at,
                end_at: entry.end_at,
            })
            .collect();

        // Tasks the planner marked as scheduled but did not actually place in the
        // active schedule are the generation-failure signal for the evaluator.
        let scheduled_ids: HashSet<&str> = schedule_entries
            .iter()
            .map(|entry| entry.task_id.as_str())
            .collect();
        let unplaced_task_ids: Vec<String> = tasks
            .iter()
            .filter(|task| {
                task.status == TaskStatus::Scheduled && !scheduled_ids.contains(task.id.as_str())
            })
            .map(|task| task.id.clone())
            .collect();

        let settings = self.get_settings_or_default().await?;
        let tz = self.server_timezone().await?;
        let sleep_impact = sleep_impact_from_schedule(&settings, &schedule_entries, &tz);

        let progress_by_id: HashMap<String, _> = progress
            .into_iter()
            .map(|p| (p.task_id.clone(), p))
            .collect();

        let evaluation_tasks = tasks
            .iter()
            .map(|task| {
                let scheduled_at = schedule_entries
                    .iter()
                    .find(|entry| entry.task_id == task.id)
                    .map(|entry| entry.start_at)
                    .or(task.start_at);
                EvaluationTask {
                    id: task.id.clone(),
                    display_id: task.display_id,
                    title: task.title.clone(),
                    scheduled_at,
                    deadline_at: task.end_at,
                    status: task.status,
                    fixed: task.fixed,
                    quantity_total: task.quantity_total.map(i64::from),
                    quantity_done: task.quantity_done.into(),
                }
            })
            .collect::<Vec<_>>();

        let work: Vec<EvaluationWork> = tasks
            .iter()
            .filter(|task| task.status == TaskStatus::InProgress)
            .filter_map(|task| {
                let progress = progress_by_id.get(&task.id)?;
                let distribution = progress
                    .estimator
                    .map(|est| DurationDistribution::new(est.mean_minutes, est.sigma_minutes))
                    .unwrap_or_else(|| {
                        effective_distribution(
                            task.avg_minutes as f64,
                            task.sigma_minutes as f64,
                            None,
                        )
                    });
                Some(EvaluationWork {
                    task_id: task.id.clone(),
                    active_minutes: progress.total_active_minutes as f64,
                    distribution,
                    distribution_revision: progress.estimator.map(|est| est.revision).unwrap_or(0),
                    progress_band: progress
                        .estimator
                        .and_then(|est| est.band)
                        .map(InterventionBand::from),
                    next_crossing_at: progress.estimator.and_then(|est| est.next_crossing_time),
                })
            })
            .collect();

        let gaps = schedule_gaps(&schedule_entries, now, &tz)?;

        let target_start = takusu_types::Timestamp(
            takusu_types::parse_date_expression("today", &tz, false)
                .map_err(|error| AppError::Internal(format!("day start: {error}")))?,
        );
        let target_end = takusu_types::Timestamp(
            takusu_types::parse_date_expression("today", &tz, true)
                .map_err(|error| AppError::Internal(format!("day end: {error}")))?,
        );

        let mut coverage = coverage;
        if coverage.confirmations.is_empty()
            && coverage.unsettled_intervals.is_empty()
            && coverage.unclassified_gaps.is_empty()
        {
            coverage = bootstrap_evaluation(schedule_revision);
        }
        coverage.unclassified_gaps = gaps
            .iter()
            .filter(|gap| gap.kind == GapKind::Unclassified)
            .map(|gap| UnsettledIntervalRow {
                id: gap
                    .identity
                    .clone()
                    .unwrap_or_else(|| format!("gap:{}..{}", gap.start_at, gap.end_at)),
                start_at: gap.start_at,
                end_at: gap.end_at,
                classification: "unclassified".into(),
                source: "schedule".into(),
                created_at: now,
                settled_at: None,
                operation_id: None,
            })
            .collect();
        coverage.state = compute_coverage(&coverage, now, target_start, target_end);

        let snapshot = EvaluationSnapshot {
            schedule_revision,
            now,
            tasks: evaluation_tasks,
            schedule: schedule_entries,
            work,
            gaps,
            unplaced_task_ids,
            coverage,
            ledger: LedgerView {
                committed_event_ids: ledger.into_iter().map(|row| row.id).collect(),
            },
            sleep_impact,
        };
        let result = evaluate_events(&snapshot);
        let inserts: Vec<EventLedgerInsert> = result
            .due_events
            .iter()
            .map(planner_event_to_insert)
            .collect::<Result<Vec<_>, _>>()?;
        self.storage
            .commit_event_evaluation(snapshot.schedule_revision, &inserts)
            .await
            .map_err(storage_to_app)?;
        Ok(result)
    }

    pub async fn insert_event_ledger(
        &self,
        event: &EventLedgerInsert,
    ) -> Result<EventLedgerRow, AppError> {
        self.storage
            .insert_event_ledger(event)
            .await
            .map_err(storage_to_app)
    }

    pub async fn list_event_ledger(
        &self,
        device_id: Option<&str>,
    ) -> Result<Vec<EventLedgerRow>, AppError> {
        self.storage
            .list_event_ledger(device_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn claim_event_delivery(
        &self,
        device_id: &str,
        event_id: &str,
    ) -> Result<bool, AppError> {
        self.storage
            .claim_event_delivery(device_id, event_id)
            .await
            .map_err(storage_to_app)
    }

    pub async fn update_event_delivery_state(
        &self,
        event_id: &str,
        state: EventDeliveryState,
    ) -> Result<EventLedgerRow, AppError> {
        self.storage
            .update_event_delivery_state(event_id, state)
            .await
            .map_err(storage_to_app)
    }
}

fn planner_event_to_insert(event: &PlannerEvent) -> Result<EventLedgerInsert, AppError> {
    let presentation = serde_json::to_string(&event.presentation)
        .map_err(|error| AppError::Internal(format!("serialize event presentation: {error}")))?;
    Ok(EventLedgerInsert {
        id: event.id.clone(),
        kind: event.kind.as_str().to_string(),
        task_id: event.task_ref.as_ref().map(|task| task.task_id.clone()),
        presentation,
        urgency: urgency_label(event.urgency).to_string(),
        schedule_revision: event.schedule_revision,
        distribution_revision: event.distribution_revision,
        observation_kind: observation_kind(event).to_string(),
    })
}

fn urgency_label(urgency: takusu_agent::events::Urgency) -> &'static str {
    match urgency {
        takusu_agent::events::Urgency::Normal => "normal",
        takusu_agent::events::Urgency::High => "high",
        takusu_agent::events::Urgency::Emergency => "emergency",
    }
}

fn observation_kind(event: &PlannerEvent) -> &'static str {
    match event.kind {
        takusu_agent::events::PlannerEventKind::DistributionOverrun => "duration_distribution",
        takusu_agent::events::PlannerEventKind::TaskStartTimeReached => "start_time",
        takusu_agent::events::PlannerEventKind::TaskNonStartContinued => "non_start",
        takusu_agent::events::PlannerEventKind::UnclassifiedGapContinued => "gap",
        takusu_agent::events::PlannerEventKind::DeadlineViolation => "deadline",
        takusu_agent::events::PlannerEventKind::CarriedOverIncomplete => "carry_over",
        takusu_agent::events::PlannerEventKind::ScheduleGenerationFailure => "generation",
        takusu_agent::events::PlannerEventKind::SleepImpact => "sleep",
        takusu_agent::events::PlannerEventKind::CurrentTask => "current",
    }
}

fn schedule_gaps(
    entries: &[EvaluationScheduleEntry],
    now: takusu_types::Timestamp,
    tz: &jiff::tz::TimeZone,
) -> Result<Vec<EvaluationGap>, AppError> {
    let day_start = takusu_types::Timestamp(
        takusu_types::parse_date_expression("today", tz, false)
            .map_err(|error| AppError::Internal(format!("day start: {error}")))?,
    );
    let day_end = takusu_types::Timestamp(
        takusu_types::parse_date_expression("today", tz, true)
            .map_err(|error| AppError::Internal(format!("day end: {error}")))?,
    );

    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|entry| entry.start_at);

    let mut gaps = Vec::new();

    // Gap from the start of the current day up to the first scheduled entry.
    if let Some(first) = sorted.first() {
        if day_start < first.start_at {
            gaps.push(EvaluationGap {
                start_at: day_start,
                end_at: first.start_at,
                kind: GapKind::Unclassified,
                identity: Some(format!("day_start..{}", first.start_at)),
            });
        }
    } else {
        // No schedule at all for today: the whole day is a single unclassified gap
        // if the current time still falls inside it.
        if now <= day_end {
            gaps.push(EvaluationGap {
                start_at: day_start,
                end_at: day_end,
                kind: GapKind::Unclassified,
                identity: Some("day_start..day_end".into()),
            });
        }
    }

    for window in sorted.windows(2) {
        if window[0].end_at < window[1].start_at {
            gaps.push(EvaluationGap {
                start_at: window[0].end_at,
                end_at: window[1].start_at,
                kind: GapKind::Unclassified,
                identity: Some(format!("{}..{}", window[0].end_at, window[1].start_at)),
            });
        }
    }

    // Gap from the last scheduled entry to the end of the current day.
    if let Some(last) = sorted.last()
        && last.end_at < day_end
    {
        gaps.push(EvaluationGap {
            start_at: last.end_at,
            end_at: day_end,
            kind: GapKind::Unclassified,
            identity: Some(format!("{}..day_end", last.end_at)),
        });
    }

    Ok(gaps)
}

/// Detect whether the active schedule encroaches on the configured sleep window.
///
/// Sleep is a user-facing constraint; the evaluator only decides whether to fire
/// the stable `SleepImpact` event. A schedule entry that overlaps the sleep window
/// means the planner could not protect the user's sleep.
fn sleep_impact_from_schedule(
    settings: &SettingsRow,
    schedule: &[EvaluationScheduleEntry],
    tz: &jiff::tz::TimeZone,
) -> Option<SleepImpact> {
    let sleep_start_minutes = settings.sleep_start.to_minutes();
    let sleep_end_minutes = settings.sleep_end.to_minutes();

    for entry in schedule {
        let entry_start = entry.start_at.as_second();
        let entry_end = entry.end_at.as_second();

        // Check the sleep window for both the start and end dates of the entry.
        for boundary_ts in [entry.start_at, entry.end_at] {
            let boundary_zoned = boundary_ts.to_zoned(tz.clone());
            let date = boundary_zoned.date();

            let (sleep_start_zoned, sleep_end_zoned) = if sleep_end_minutes > sleep_start_minutes {
                let start = jiff::civil::DateTime::new(
                    date.year(),
                    date.month(),
                    date.day(),
                    settings.sleep_start.hour() as i8,
                    settings.sleep_start.minute() as i8,
                    0,
                    0,
                )
                .ok()?
                .to_zoned(tz.clone())
                .ok()?;
                let end = jiff::civil::DateTime::new(
                    date.year(),
                    date.month(),
                    date.day(),
                    settings.sleep_end.hour() as i8,
                    settings.sleep_end.minute() as i8,
                    0,
                    0,
                )
                .ok()?
                .to_zoned(tz.clone())
                .ok()?;
                (start, end)
            } else {
                // Sleep crosses midnight (e.g. 22:00–06:00).
                let start = jiff::civil::DateTime::new(
                    date.year(),
                    date.month(),
                    date.day(),
                    settings.sleep_start.hour() as i8,
                    settings.sleep_start.minute() as i8,
                    0,
                    0,
                )
                .ok()?
                .to_zoned(tz.clone())
                .ok()?;
                let next_date = date.tomorrow().ok()?;
                let end = jiff::civil::DateTime::new(
                    next_date.year(),
                    next_date.month(),
                    next_date.day(),
                    settings.sleep_end.hour() as i8,
                    settings.sleep_end.minute() as i8,
                    0,
                    0,
                )
                .ok()?
                .to_zoned(tz.clone())
                .ok()?;
                (start, end)
            };

            let sleep_start_sec = sleep_start_zoned.timestamp().as_second();
            let sleep_end_sec = sleep_end_zoned.timestamp().as_second();
            if entry_start < sleep_end_sec && entry_end > sleep_start_sec {
                return Some(SleepImpact { detected: true });
            }
        }
    }

    None
}
