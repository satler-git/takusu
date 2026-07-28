use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use std::str::FromStr;
use takusu_storage::{
    CreateTask, CreateTaskBatch, CreateTaskBatchResult, ProgressResult, RecordProgress,
    SplitResult, SplitTask, TaskProgress, TaskQuery, TaskRow, UpdateTask,
};
use takusu_util::search::Completion;
use takusu_util::{TaskStatusFilter, Timestamp, parse_datetime_to_timestamp};

use crate::error::HttpError;
use crate::state::AppState;

fn operation_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("idempotency-key")
        .or_else(|| headers.get("Idempotency-Key"))
        .and_then(|v| v.to_str().ok())
}

#[derive(Debug, Deserialize)]
pub struct TaskQueryParams {
    pub status: Option<String>,
    pub from: Option<String>,
    pub until: Option<String>,
    pub no_overdue: Option<bool>,
    pub habit_id: Option<String>,
    pub ical_uid: Option<String>,
    pub q: Option<String>,
    pub limit: Option<i64>,
}

pub async fn create_task(
    State(state): State<AppState>,
    Json(body): Json<CreateTask>,
) -> Result<(StatusCode, Json<TaskRow>), HttpError> {
    let task = state.app.create_task(&body).await?;
    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn create_task_batch(
    State(state): State<AppState>,
    Json(body): Json<CreateTaskBatch>,
) -> Result<(StatusCode, Json<Vec<CreateTaskBatchResult>>), HttpError> {
    let tasks = state.app.create_task_batch(&body).await?;
    Ok((StatusCode::CREATED, Json(tasks)))
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<TaskQueryParams>,
) -> Result<Json<Vec<TaskRow>>, HttpError> {
    let tz = state.app.server_timezone().await?;
    let q = TaskQuery {
        status: query
            .status
            .map(|s| TaskStatusFilter::from_str(&s))
            .transpose()
            .map_err(|e| HttpError(takusu_local_lib::error::AppError::BadRequest(e.to_string())))?,
        from: query
            .from
            .map(|s| parse_datetime_to_timestamp(&s, &tz).map(Timestamp::from))
            .transpose()
            .map_err(|e| HttpError(takusu_local_lib::error::AppError::BadRequest(e)))?,
        until: query
            .until
            .map(|s| parse_datetime_to_timestamp(&s, &tz).map(Timestamp::from))
            .transpose()
            .map_err(|e| HttpError(takusu_local_lib::error::AppError::BadRequest(e)))?,
        no_overdue: query.no_overdue,
        habit_id: query.habit_id,
        ical_uid: query.ical_uid,
        q: query.q,
        limit: query.limit,
    };
    let tasks = state.app.list_tasks(&q).await?;
    Ok(Json(tasks))
}

#[derive(Debug, Deserialize)]
pub struct CompleteQuery {
    pub q: String,
    pub limit: Option<usize>,
}

pub async fn complete_task_query(
    State(state): State<AppState>,
    Query(query): Query<CompleteQuery>,
) -> Result<Json<Vec<Completion>>, HttpError> {
    let completions = state.app.complete_task_query(&query.q, query.limit).await?;
    Ok(Json(completions))
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskRow>, HttpError> {
    let task = state.app.get_task(&id).await?;
    Ok(Json(task))
}

pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTask>,
) -> Result<Json<TaskRow>, HttpError> {
    let task = state.app.update_task(&id, &body).await?;
    Ok(Json(task))
}

pub async fn replace_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateTask>,
) -> Result<Json<TaskRow>, HttpError> {
    let task = state.app.replace_task(&id, &body).await?;
    Ok(Json(task))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, HttpError> {
    state.app.delete_task(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn import_ical(
    State(state): State<AppState>,
    body: String,
) -> Result<Json<serde_json::Value>, HttpError> {
    let result = state.app.import_ical(&body).await?;
    Ok(Json(serde_json::json!({
        "imported": result.imported,
        "task_ids": result.task_ids,
    })))
}

pub async fn dependency_analysis(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, HttpError> {
    let redundant = state.app.analyze_task_dependencies().await?;
    Ok(Json(serde_json::json!({ "redundant": redundant })))
}

pub async fn start_task_work(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<TaskRow>, HttpError> {
    let task = state
        .app
        .start_task_work(&id, operation_id(&headers))
        .await?;
    Ok(Json(task))
}

pub async fn pause_task_work(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<TaskRow>, HttpError> {
    let task = state
        .app
        .pause_task_work(&id, operation_id(&headers))
        .await?;
    Ok(Json(task))
}

pub async fn record_progress(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RecordProgress>,
) -> Result<Json<ProgressResult>, HttpError> {
    let result = state
        .app
        .record_progress(&id, &body, operation_id(&headers))
        .await?;
    Ok(Json(result))
}

pub async fn complete_task_work(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<TaskRow>, HttpError> {
    let task = state
        .app
        .complete_task_work(&id, operation_id(&headers))
        .await?;
    Ok(Json(task))
}

pub async fn get_task_progress(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskProgress>, HttpError> {
    let progress = state.app.get_task_progress(&id).await?;
    Ok(Json(progress))
}

pub async fn split_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SplitTask>,
) -> Result<Json<SplitResult>, HttpError> {
    let result = state
        .app
        .split_task(&id, &body, operation_id(&headers))
        .await?;
    Ok(Json(result))
}
