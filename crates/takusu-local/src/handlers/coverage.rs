use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use takusu_contracts::{CreateCoverageConfirmation, CreateUnsettledInterval};

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
