use axum::Json;
use axum::extract::{Path, State};
use takusu_contracts::{
    CreateDevice, DeviceRow, RefreshEvaluatorHeartbeat, RefreshEvaluatorLease, ResidentAuthority,
    SpeechCapability, SuppressDevice, UpdateDevice,
};
use takusu_local_lib::error::{AppError, BadRequestKind};

use crate::error::HttpError;
use crate::state::AppState;

fn device_mismatch() -> HttpError {
    HttpError(AppError::BadRequest(BadRequestKind::Other(
        "device id in path and body must match".into(),
    )))
}

pub async fn create_device(
    State(state): State<AppState>,
    Json(body): Json<CreateDevice>,
) -> Result<Json<DeviceRow>, HttpError> {
    let row = state.app.register_device(&body).await?;
    Ok(Json(row))
}

pub async fn list_devices(
    State(state): State<AppState>,
) -> Result<Json<Vec<DeviceRow>>, HttpError> {
    let rows = state.app.list_devices().await?;
    Ok(Json(rows))
}

pub async fn get_device(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DeviceRow>, HttpError> {
    let row = state.app.get_device(&id).await?;
    Ok(Json(row))
}

pub async fn update_device(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDevice>,
) -> Result<Json<DeviceRow>, HttpError> {
    let row = state.app.update_device(&id, &body).await?;
    Ok(Json(row))
}

pub async fn delete_device(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(), HttpError> {
    state.app.delete_device(&id).await?;
    Ok(())
}

pub async fn refresh_heartbeat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RefreshEvaluatorHeartbeat>,
) -> Result<Json<DeviceRow>, HttpError> {
    if id != body.device_id {
        return Err(device_mismatch());
    }
    let row = state.app.refresh_evaluator_heartbeat(&body).await?;
    Ok(Json(row))
}

pub async fn refresh_lease(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RefreshEvaluatorLease>,
) -> Result<Json<DeviceRow>, HttpError> {
    if id != body.device_id {
        return Err(device_mismatch());
    }
    let row = state.app.refresh_evaluator_lease(&body).await?;
    Ok(Json(row))
}

pub async fn resolve_resident_authority(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ResidentAuthority>, HttpError> {
    let authority = state.app.resolve_resident_authority(&id).await?;
    Ok(Json(authority))
}

pub async fn get_speech_capability(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SpeechCapability>, HttpError> {
    let capability = state.app.get_speech_capability(&id).await?;
    Ok(Json(capability))
}

pub async fn suppress_device(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SuppressDevice>,
) -> Result<Json<DeviceRow>, HttpError> {
    let row = state.app.suppress_device(&id, body.minutes).await?;
    Ok(Json(row))
}
