use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use takusu_local_lib::app::{
    GenerateScheduleInput, MoveEntryOutput, RescheduleInput, SchedulePreviewInput,
};
use takusu_storage::{SaveScheduleRequest, ScheduleRow};
use takusu_util::ScheduleMode;

use crate::error::HttpError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct GenerateSchedule {
    pub task_ids: Option<Vec<String>>,
    #[serde(default = "default_sleep")]
    pub sleep: String,
}

fn default_sleep() -> String {
    "recommended".to_string()
}

#[derive(Debug, Deserialize)]
pub struct Reschedule {
    pub mode: ScheduleMode,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: String,
}

#[derive(Debug, Deserialize)]
pub struct MoveEntry {
    pub start_at: String,
    #[serde(default)]
    pub force: bool,
}

pub async fn get_schedule(State(state): State<AppState>) -> Result<Json<ScheduleRow>, HttpError> {
    let row = state.app.get_schedule().await?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize)]
pub struct PreviewSchedule {
    #[serde(default = "default_mode")]
    pub mode: String,
    pub from: Option<String>,
    pub until: Option<String>,
    pub task_ids: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default = "default_sleep")]
    pub sleep: String,
}

fn default_mode() -> String {
    "full".to_string()
}

pub async fn preview_schedule(
    State(state): State<AppState>,
    Json(body): Json<PreviewSchedule>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let input = SchedulePreviewInput {
        mode: body.mode,
        from: body.from,
        until: body.until,
        task_ids: body.task_ids,
        pinned: body.pinned,
        sleep: body.sleep,
    };
    Ok(Json(
        serde_json::to_value(state.app.preview_schedule(&input).await?).unwrap(),
    ))
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
) -> Result<Json<serde_json::Value>, HttpError> {
    let output = state
        .app
        .move_entry(&task_id, &body.start_at, body.force)
        .await?;
    let MoveEntryOutput {
        task_id,
        start_at,
        end_at,
        warnings,
    } = output;
    if warnings.is_empty() {
        Ok(Json(
            serde_json::json!({ "task_id": task_id, "start_at": start_at, "end_at": end_at }),
        ))
    } else {
        Ok(Json(
            serde_json::json!({ "task_id": task_id, "start_at": start_at, "end_at": end_at, "warnings": warnings }),
        ))
    }
}

pub async fn clear_schedule(State(state): State<AppState>) -> Result<StatusCode, HttpError> {
    state.app.clear_schedule().await?;
    Ok(StatusCode::NO_CONTENT)
}
