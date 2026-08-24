use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use takusu_contracts::{CreateCoverageConfirmation, CreateUnsettledInterval, SettleRequest};

use crate::error::HttpError;
use crate::handlers::common::operation_id;
use crate::state::AppState;

pub async fn create_coverage_confirmation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<CreateCoverageConfirmation>,
) -> Result<Json<takusu_contracts::CoverageConfirmationRow>, HttpError> {
    if body.operation_id.is_none() {
        body.operation_id = operation_id(&headers).map(|s| s.to_string());
    }
    let confirmation = state.app.create_coverage_confirmation(&body).await?;
    Ok(Json(confirmation))
}

pub async fn create_unsettled_interval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<CreateUnsettledInterval>,
) -> Result<Json<takusu_contracts::UnsettledIntervalRow>, HttpError> {
    if body.operation_id.is_none() {
        body.operation_id = operation_id(&headers).map(|s| s.to_string());
    }
    let interval = state.app.create_unsettled_interval(&body).await?;
    Ok(Json(interval))
}

pub async fn list_unsettled_intervals(
    State(state): State<AppState>,
) -> Result<Json<Vec<takusu_contracts::UnsettledIntervalRow>>, HttpError> {
    let intervals = state.app.list_unsettled_intervals().await?;
    Ok(Json(intervals))
}

pub async fn settle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<SettleRequest>,
) -> Result<Json<takusu_contracts::SettleResponse>, HttpError> {
    if body.operation_id.is_none() {
        body.operation_id = operation_id(&headers).map(|s| s.to_string());
    }
    let response = state.app.settle(&body).await?;
    Ok(Json(response))
}
