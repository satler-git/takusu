use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use takusu_contracts::{
    CreateMemory, MemoryInjectionQuery, MemoryQuery, SimilarTaskQuery, UpdateMemory,
};

use crate::error::{HttpError, NoContent};
use crate::handlers::common::operation_id;
use crate::state::AppState;

pub async fn create_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateMemory>,
) -> Result<Json<takusu_contracts::MemoryRow>, HttpError> {
    let memory = state
        .app
        .create_memory(&body, operation_id(&headers))
        .await?;
    Ok(Json(memory))
}

pub async fn get_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<takusu_contracts::MemoryRow>, HttpError> {
    let memory = state.app.get_memory(&id).await?;
    Ok(Json(memory))
}

pub async fn update_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateMemory>,
) -> Result<Json<takusu_contracts::MemoryRow>, HttpError> {
    let memory = state
        .app
        .update_memory(&id, &body, operation_id(&headers))
        .await?;
    Ok(Json(memory))
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DeleteMemoryParams {
    pub observed_revision: i64,
}

pub async fn delete_memory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<DeleteMemoryParams>,
) -> Result<NoContent, HttpError> {
    state
        .app
        .delete_memory(&id, params.observed_revision, operation_id(&headers))
        .await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}

pub async fn search_memory(
    State(state): State<AppState>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Vec<takusu_contracts::MemoryRow>>, HttpError> {
    let memories = state.app.search_memories(&query).await?;
    Ok(Json(memories))
}

pub async fn injectable_memory(
    State(state): State<AppState>,
    Json(query): Json<MemoryInjectionQuery>,
) -> Result<Json<takusu_contracts::MemoryInjectionResult>, HttpError> {
    let result = state.app.injectable_memories(&query).await?;
    Ok(Json(result))
}

pub async fn find_similar_tasks(
    State(state): State<AppState>,
    Query(query): Query<SimilarTaskQuery>,
) -> Result<Json<Vec<takusu_contracts::SimilarTaskRow>>, HttpError> {
    let tasks = state.app.find_similar_tasks(&query).await?;
    Ok(Json(tasks))
}
