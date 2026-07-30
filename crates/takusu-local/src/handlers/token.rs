use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use takusu_contracts::{TokenCreateResponse, TokenRow};
use takusu_local_lib::TokenClaims;

use crate::error::{HttpError, NoContent};
use crate::state::AppState;
use takusu_local_lib::error::AppError;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTokenRequest {
    pub label: Option<String>,
}

fn require_root(claims: &TokenClaims) -> Result<(), HttpError> {
    if !claims.is_root() {
        return Err(HttpError(AppError::Unauthorized));
    }
    Ok(())
}

pub async fn create_token(
    State(state): State<AppState>,
    Extension(claims): Extension<TokenClaims>,
    Json(body): Json<CreateTokenRequest>,
) -> Result<Json<TokenCreateResponse>, HttpError> {
    require_root(&claims)?;
    let resp = state.app.create_token(body.label.as_deref()).await?;
    Ok(Json(resp))
}

pub async fn list_tokens(
    State(state): State<AppState>,
    Extension(claims): Extension<TokenClaims>,
) -> Result<Json<Vec<TokenRow>>, HttpError> {
    require_root(&claims)?;
    let tokens = state.app.list_tokens().await?;
    Ok(Json(tokens))
}

pub async fn revoke_token(
    State(state): State<AppState>,
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<i64>,
) -> Result<NoContent, HttpError> {
    require_root(&claims)?;
    state.app.revoke_token(id).await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}
