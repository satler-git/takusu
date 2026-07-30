use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use takusu_contracts::{
    CreateHabit, CreateHabitBatch, CreateHabitBatchResult, CreateHabitScheduledSpan, HabitDetail,
    HabitEstimateRequest, HabitEstimateResult, HabitPreviewRequest, HabitPreviewTask, HabitRow,
    HabitScheduledSpanRow, HabitStepInput, HabitStepRow, UpdateHabit,
};
use takusu_local_lib::app::DependencyAnalysisResponse;

use crate::error::{HttpError, NoContent};
use crate::state::AppState;

pub async fn create_habit(
    State(state): State<AppState>,
    Json(body): Json<CreateHabit>,
) -> Result<Json<HabitRow>, HttpError> {
    let habit = state.app.create_habit(&body).await?;
    Ok(Json(habit))
}

pub async fn create_habit_batch(
    State(state): State<AppState>,
    Json(body): Json<CreateHabitBatch>,
) -> Result<Json<Vec<CreateHabitBatchResult>>, HttpError> {
    let habits = state.app.create_habit_batch(&body).await?;
    Ok(Json(habits))
}

pub async fn preview_habit(
    State(state): State<AppState>,
    Json(body): Json<HabitPreviewRequest>,
) -> Result<Json<Vec<HabitPreviewTask>>, HttpError> {
    let tasks = state.app.preview_habit(&body).await?;
    Ok(Json(tasks))
}

pub async fn list_habits(State(state): State<AppState>) -> Result<Json<Vec<HabitRow>>, HttpError> {
    let habits = state.app.list_habits().await?;
    Ok(Json(habits))
}

pub async fn get_habit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<HabitDetail>, HttpError> {
    let habit = state.app.get_habit(&id).await?;
    Ok(Json(habit))
}

pub async fn estimate_habit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<HabitEstimateRequest>,
) -> Result<Json<HabitEstimateResult>, HttpError> {
    let result = state.app.estimate_habit(&id, &body).await?;
    Ok(Json(result))
}

pub async fn update_habit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateHabit>,
) -> Result<Json<HabitRow>, HttpError> {
    let habit = state.app.update_habit(&id, &body).await?;
    Ok(Json(habit))
}

pub async fn replace_habit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateHabit>,
) -> Result<Json<HabitRow>, HttpError> {
    let habit = state.app.replace_habit(&id, &body).await?;
    Ok(Json(habit))
}

pub async fn delete_habit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<NoContent, HttpError> {
    state.app.delete_habit(&id).await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}

// ── Habit scheduled spans (#303 / #503) ────────────────────────────────

pub async fn list_habit_scheduled_spans(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<HabitScheduledSpanRow>>, HttpError> {
    let spans = state.app.list_habit_scheduled_spans(&id).await?;
    Ok(Json(spans))
}

pub async fn list_all_habit_scheduled_spans(
    State(state): State<AppState>,
) -> Result<Json<Vec<HabitScheduledSpanRow>>, HttpError> {
    let spans = state
        .app
        .list_all_habit_scheduled_spans()
        .await
        .map_err(HttpError::from)?;
    Ok(Json(spans))
}

pub async fn create_habit_scheduled_span(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateHabitScheduledSpan>,
) -> Result<Json<HabitScheduledSpanRow>, HttpError> {
    let span = state.app.create_habit_scheduled_span(&id, &body).await?;
    Ok(Json(span))
}

pub async fn delete_habit_scheduled_span(
    State(state): State<AppState>,
    Path((id, span_id)): Path<(String, String)>,
) -> Result<NoContent, HttpError> {
    state.app.delete_habit_scheduled_span(&id, &span_id).await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}

// ── Habit steps (#95) ────────────────────────────────────────────────────

pub async fn list_habit_steps(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<HabitStepRow>>, HttpError> {
    let steps = state.app.list_habit_steps(&id).await?;
    Ok(Json(steps))
}

pub async fn list_all_habit_steps(
    State(state): State<AppState>,
) -> Result<Json<Vec<HabitStepRow>>, HttpError> {
    let steps = state.app.list_all_habit_steps().await?;
    Ok(Json(steps))
}

pub async fn replace_habit_steps(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Vec<HabitStepInput>>,
) -> Result<Json<Vec<HabitStepRow>>, HttpError> {
    let steps = state.app.replace_habit_steps(&id, &body).await?;
    Ok(Json(steps))
}

pub async fn step_dependency_analysis(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DependencyAnalysisResponse>, HttpError> {
    let redundant = state.app.analyze_habit_step_dependencies(&id).await?;
    Ok(Json(DependencyAnalysisResponse { redundant }))
}
