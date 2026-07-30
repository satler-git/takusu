use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use takusu_contracts::{GoogleCalEventRow, UpdateGoogleCalSettings};
use takusu_local_lib::app::{
    DeleteAllGcalResult, GoogleCalSettingsOutput, OAuthCallbackResponse, OAuthUrlResponse,
    SyncTriggerResponse,
};
use takusu_local_lib::error::AppError;

use crate::error::HttpError;
use crate::state::AppState;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OAuthUrlRequest {
    pub redirect_uri: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OAuthCallbackRequest {
    pub code: String,
    pub redirect_uri: Option<String>,
}

pub async fn get_settings(
    State(state): State<AppState>,
) -> Result<Json<GoogleCalSettingsOutput>, HttpError> {
    let output = state.app.get_gcal_settings().await?;
    Ok(Json(output))
}

pub async fn update_settings(
    State(state): State<AppState>,
    Json(body): Json<UpdateGoogleCalSettings>,
) -> Result<Json<GoogleCalSettingsOutput>, HttpError> {
    let output = state.app.update_gcal_settings(&body).await?;
    Ok(Json(output))
}

pub async fn oauth_url(
    State(state): State<AppState>,
    Json(body): Json<OAuthUrlRequest>,
) -> Result<Json<OAuthUrlResponse>, HttpError> {
    let url = state.app.oauth_url(&body.redirect_uri).await?;
    Ok(Json(OAuthUrlResponse { url }))
}

pub async fn oauth_callback(
    State(state): State<AppState>,
    Json(body): Json<OAuthCallbackRequest>,
) -> Result<Json<OAuthCallbackResponse>, HttpError> {
    state
        .app
        .oauth_callback(&body.code, body.redirect_uri.as_deref())
        .await?;
    Ok(Json(OAuthCallbackResponse {
        refresh_token_set: true,
    }))
}

pub async fn trigger_sync(
    State(state): State<AppState>,
) -> Result<Json<SyncTriggerResponse>, HttpError> {
    state.app.do_sync().await.map_err(|e| {
        tracing::error!("google calendar sync failed: {e}");
        HttpError::from(AppError::Internal(format!("sync failed: {e}")))
    })?;
    Ok(Json(SyncTriggerResponse {
        status: "sync_triggered".to_string(),
    }))
}

pub async fn list_mappings(
    State(state): State<AppState>,
) -> Result<Json<Vec<GoogleCalEventRow>>, HttpError> {
    let rows = state.app.list_gcal_mappings().await?;
    Ok(Json(rows))
}

pub async fn delete_all_gcal_events(
    State(state): State<AppState>,
) -> Result<Json<DeleteAllGcalResult>, HttpError> {
    let result = state.app.delete_all_gcal_events().await?;
    Ok(Json(result))
}
