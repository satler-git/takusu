use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use takusu_local_lib::app::{
    GenerateScheduleInput, MoveEntryOutput, RescheduleInput, SchedulePreviewInput,
    SchedulePreviewOutput,
};
use takusu_storage::{SaveScheduleRequest, ScheduleRow};
use takusu_types::{ScheduleMode, SleepInput};

use crate::error::{HttpError, NoContent};
use crate::state::AppState;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GenerateSchedule {
    pub task_ids: Option<Vec<String>>,
    #[serde(default = "default_sleep")]
    pub sleep: SleepInput,
}

fn default_sleep() -> SleepInput {
    SleepInput::Recommended
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct Reschedule {
    pub mode: ScheduleMode,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: SleepInput,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveEntry {
    pub start_at: String,
    #[serde(default)]
    #[schemars(default)]
    pub force: bool,
}

pub async fn get_schedule(State(state): State<AppState>) -> Result<Json<ScheduleRow>, HttpError> {
    let row = state.app.get_schedule().await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PreviewSchedule {
    #[serde(default = "default_mode")]
    pub mode: ScheduleMode,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: SleepInput,
}

fn default_mode() -> ScheduleMode {
    ScheduleMode::Full
}

pub async fn preview_schedule(
    State(state): State<AppState>,
    Json(body): Json<PreviewSchedule>,
) -> Result<Json<SchedulePreviewOutput>, HttpError> {
    let input = SchedulePreviewInput {
        mode: body.mode,
        from: body.from,
        until: body.until,
        task_ids: body.task_ids,
        pinned: body.pinned,
        sleep: body.sleep,
    };
    Ok(Json(state.app.preview_schedule(&input).await?))
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
    let input = GenerateScheduleInput {
        task_ids: body.task_ids,
        sleep: body.sleep,
    };
    let result = state.app.generate_schedule(&input).await?;
    Ok(Json(result))
}

pub async fn reschedule(
    State(state): State<AppState>,
    Json(body): Json<Reschedule>,
) -> Result<Json<ScheduleRow>, HttpError> {
    let input = RescheduleInput {
        mode: body.mode,
        from: body.from,
        until: body.until,
        task_ids: body.task_ids,
        pinned: body.pinned,
        sleep: body.sleep,
    };
    let result = state.app.reschedule(&input).await?;
    Ok(Json(result))
}

pub async fn move_entry(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(body): Json<MoveEntry>,
) -> Result<Json<MoveEntryOutput>, HttpError> {
    let output = state
        .app
        .move_entry(&task_id, &body.start_at, body.force)
        .await?;
    Ok(Json(output))
}

pub async fn clear_schedule(State(state): State<AppState>) -> Result<NoContent, HttpError> {
    state.app.clear_schedule().await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}
