use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use serde::{Deserialize, Serialize};
use takusu_local_lib::TokenClaims;
use takusu_storage::{SettingsRow, UpdateSettings};

use crate::error::HttpError;
use crate::state::AppState;
use takusu_local_lib::error::AppError;

pub async fn get_settings(State(state): State<AppState>) -> Result<Json<SettingsRow>, HttpError> {
    let row = state.app.get_settings().await?;
    Ok(Json(row))
}

pub async fn update_settings(
    State(state): State<AppState>,
    Json(body): Json<UpdateSettings>,
) -> Result<Json<SettingsRow>, HttpError> {
    let row = state.app.update_settings(&body).await?;
    Ok(Json(row))
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct HealthCheckResponse {
    pub status: String,
}

/// `GET /api/workers/health` — checks the storage backend (Cloudflare Worker
/// or local SQLite) is reachable. Used by the mobile settings page.
pub async fn workers_health(
    State(state): State<AppState>,
) -> Result<Json<HealthCheckResponse>, HttpError> {
    let status = state.app.health_check().await?;
    Ok(Json(HealthCheckResponse { status }))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct UpdateWorkersConfig {
    pub url: String,
    pub token: String,
}

/// Response for `PUT /api/workers/config`.
#[derive(Serialize, schemars::JsonSchema)]
pub struct WorkersConfigUpdateResponse {
    pub ok: bool,
}

/// `PUT /api/workers/config` — updates the Worker endpoint and root token
/// at runtime without restarting the server. Requires a root token.
pub async fn update_workers_config(
    State(state): State<AppState>,
    Extension(claims): Extension<TokenClaims>,
    Json(body): Json<UpdateWorkersConfig>,
) -> Result<Json<WorkersConfigUpdateResponse>, HttpError> {
    if !claims.is_root() {
        return Err(HttpError(AppError::Unauthorized));
    }
    let app = state.app.clone();
    app.update_workers_credentials(&body.url, &body.token)
        .await?;
    // Keep the local root-token bypass in sync with the newly saved worker
    // token so that subsequent root-only requests (e.g. further config
    // updates) can still succeed even if the new worker is unreachable.
    *state.root_token.write().await = Arc::from(body.token.into_boxed_str());
    Ok(Json(WorkersConfigUpdateResponse { ok: true }))
}
