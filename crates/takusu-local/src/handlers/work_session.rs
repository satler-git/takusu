use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use takusu_contracts::{
    AttachWorkSession, ConvertWorkSession, RecordWorkSessionProgress, StartWorkSession, TaskRow,
    WorkSessionProgressResult, WorkSessionRow,
};

use crate::error::HttpError;
use crate::handlers::common::operation_id;
use crate::state::AppState;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WorkSessionListQuery {
    pub task_id: Option<String>,
}

pub async fn create_work_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<StartWorkSession>,
) -> Result<Json<WorkSessionRow>, HttpError> {
    let session = state
        .app
        .start_work_session(&body, operation_id(&headers))
        .await?;
    Ok(Json(session))
}

pub async fn get_work_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WorkSessionRow>, HttpError> {
    let session = state.app.get_work_session(&id).await?;
    Ok(Json(session))
}

pub async fn list_work_sessions(
    State(state): State<AppState>,
    Query(query): Query<WorkSessionListQuery>,
) -> Result<Json<Vec<WorkSessionRow>>, HttpError> {
    let sessions = state
        .app
        .list_work_sessions(query.task_id.as_deref())
        .await?;
    Ok(Json(sessions))
}

pub async fn pause_work_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<WorkSessionRow>, HttpError> {
    let session = state
        .app
        .pause_work_session(&id, operation_id(&headers))
        .await?;
    Ok(Json(session))
}

pub async fn complete_work_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<WorkSessionRow>, HttpError> {
    let session = state
        .app
        .complete_work_session(&id, operation_id(&headers))
        .await?;
    Ok(Json(session))
}

pub async fn record_work_session_progress(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RecordWorkSessionProgress>,
) -> Result<Json<WorkSessionProgressResult>, HttpError> {
    let result = state
        .app
        .record_work_session_progress(&id, &body, operation_id(&headers))
        .await?;
    Ok(Json(result))
}

pub async fn attach_work_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AttachWorkSession>,
) -> Result<Json<WorkSessionRow>, HttpError> {
    let session = state
        .app
        .attach_work_session(&id, &body, operation_id(&headers))
        .await?;
    Ok(Json(session))
}

pub async fn convert_work_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ConvertWorkSession>,
) -> Result<Json<TaskRow>, HttpError> {
    let task = state
        .app
        .convert_work_session(&id, &body, operation_id(&headers))
        .await?;
    Ok(Json(task))
}
