use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use takusu_contracts::{
    CoverageEvaluation, EvaluationInputs, EventDeliveryState, EventLedgerInsert, EventLedgerRow,
    ScheduleRevisionResponse,
};
use takusu_local_lib::TokenClaims;
use takusu_local_lib::error::{AppError, BadRequestKind};

use crate::error::HttpError;
use crate::state::AppState;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct EvaluateEventsRequest {
    #[serde(default)]
    pub device_id: String,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct EventListQuery {
    pub device_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClaimEventRequest {
    pub device_id: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ClaimEventResponse {
    pub claimed: bool,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct EventEvaluationResponse {
    pub due_events: Vec<serde_json::Value>,
    #[schemars(with = "Option<String>")]
    pub next_eval_at: Option<takusu_types::Timestamp>,
}

pub async fn revision(
    State(state): State<AppState>,
) -> Result<Json<ScheduleRevisionResponse>, HttpError> {
    Ok(Json(ScheduleRevisionResponse {
        revision: state.app.get_schedule_revision().await?,
    }))
}

pub async fn snapshot(
    State(state): State<AppState>,
) -> Result<Json<EvaluationInputs>, HttpError> {
    let mut inputs = state.app.get_evaluation_inputs().await?;
    let settings = state.app.get_settings().await?;
    let tz = takusu_types::parse_timezone(&settings.tz)
        .map_err(|e| HttpError::from(AppError::BadRequest(BadRequestKind::InvalidTime(format!("invalid timezone: {e}")))))?;
    let now = takusu_types::now_timestamp()
        .map_err(|e| HttpError::from(AppError::Internal(e)))?;
    let (target_start, target_end) = target_period_for(&tz).map_err(HttpError::from)?;
    let state = takusu_agent::coverage::compute_coverage(
        &inputs.coverage,
        now.into(),
        target_start,
        target_end,
    );
    inputs.coverage = CoverageEvaluation {
        state,
        ..inputs.coverage
    };
    Ok(Json(inputs))
}

fn target_period_for(
    tz: &jiff::tz::TimeZone,
) -> Result<(takusu_types::Timestamp, takusu_types::Timestamp), AppError> {
    let start = takusu_types::parse_date_expression("today", tz, false)
        .map_err(|e| AppError::Internal(format!("day start: {e}")))?;
    let end = takusu_types::parse_date_expression("today", tz, true)
        .map_err(|e| AppError::Internal(format!("day end: {e}")))?;
    Ok((takusu_types::Timestamp(start), takusu_types::Timestamp(end)))
}

pub async fn evaluate_events(
    State(state): State<AppState>,
    Json(body): Json<EvaluateEventsRequest>,
) -> Result<Json<EventEvaluationResponse>, HttpError> {
    let result = state
        .app
        .evaluate_and_commit_events(&body.device_id)
        .await?;
    let due_events = result
        .due_events
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).map_err(|error| {
                HttpError::from(takusu_local_lib::error::AppError::Internal(format!(
                    "serialize evaluated event: {error}"
                )))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(EventEvaluationResponse {
        due_events,
        next_eval_at: result.next_eval_at,
    }))
}

pub async fn insert_event(
    State(state): State<AppState>,
    Extension(claims): Extension<TokenClaims>,
    Json(event): Json<EventLedgerInsert>,
) -> Result<Json<EventLedgerRow>, HttpError> {
    if !claims.is_root() {
        return Err(HttpError(AppError::Unauthorized));
    }
    Ok(Json(state.app.insert_event_ledger(&event).await?))
}

pub async fn list_events(
    State(state): State<AppState>,
    Query(query): Query<EventListQuery>,
) -> Result<Json<Vec<EventLedgerRow>>, HttpError> {
    Ok(Json(
        state
            .app
            .list_event_ledger(query.device_id.as_deref())
            .await?,
    ))
}

pub async fn claim_event(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
    Json(body): Json<ClaimEventRequest>,
) -> Result<Json<ClaimEventResponse>, HttpError> {
    Ok(Json(ClaimEventResponse {
        claimed: state
            .app
            .claim_event_delivery(&body.device_id, &event_id)
            .await?,
    }))
}

pub async fn acknowledge_event(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
) -> Result<Json<EventLedgerRow>, HttpError> {
    Ok(Json(
        state
            .app
            .update_event_delivery_state(&event_id, EventDeliveryState::Acknowledged)
            .await?,
    ))
}

pub async fn update_event_state(
    State(state): State<AppState>,
    Path(event_id): Path<String>,
    Json(state_value): Json<EventDeliveryState>,
) -> Result<Json<EventLedgerRow>, HttpError> {
    Ok(Json(
        state
            .app
            .update_event_delivery_state(&event_id, state_value)
            .await?,
    ))
}
