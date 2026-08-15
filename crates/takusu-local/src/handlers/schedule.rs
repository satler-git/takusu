use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use takusu_contracts::{
    GenerateSchedule, MoveEntry, MoveEntryResponse, Reschedule, SaveScheduleRequest,
    SchedulePreviewRequest, SchedulePreviewResponse, ScheduleRow,
};

use crate::error::{HttpError, NoContent};
use crate::handlers::common::operation_id;
use crate::state::AppState;

pub async fn get_schedule(State(state): State<AppState>) -> Result<Json<ScheduleRow>, HttpError> {
    let row = state.app.get_schedule().await?;
    Ok(Json(row))
}

pub async fn preview_schedule(
    State(state): State<AppState>,
    Json(body): Json<SchedulePreviewRequest>,
) -> Result<Json<SchedulePreviewResponse>, HttpError> {
    Ok(Json(state.app.preview_schedule(&body).await?))
}

pub async fn replace_schedule(
    State(state): State<AppState>,
    Json(body): Json<SaveScheduleRequest>,
) -> Result<Json<ScheduleRow>, HttpError> {
    Ok(Json(state.app.replace_schedule(&body).await?))
}

pub async fn generate_schedule(
    State(state): State<AppState>,
    Json(body): Json<GenerateSchedule>,
) -> Result<Json<ScheduleRow>, HttpError> {
    let result = state.app.generate_schedule(&body).await?;
    Ok(Json(result))
}

pub async fn reschedule(
    State(state): State<AppState>,
    Json(body): Json<Reschedule>,
) -> Result<Json<ScheduleRow>, HttpError> {
    let result = state.app.reschedule(&body).await?;
    Ok(Json(result))
}

pub async fn move_entry(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<MoveEntry>,
) -> Result<Json<MoveEntryResponse>, HttpError> {
    let output = state
        .app
        .move_entry(&task_id, body.start_at, body.force, operation_id(&headers))
        .await?;
    Ok(Json(output))
}

pub async fn clear_schedule(State(state): State<AppState>) -> Result<NoContent, HttpError> {
    state.app.clear_schedule().await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}
