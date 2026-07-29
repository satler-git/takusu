use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use takusu_local_lib::TokenClaims;
use takusu_storage::{CreateSkill, SkillRow, UpdateSkill};

use crate::error::{HttpError, NoContent};
use crate::state::AppState;
use takusu_local_lib::error::AppError;

pub async fn create_skill(
    State(state): State<AppState>,
    Extension(claims): Extension<TokenClaims>,
    Json(body): Json<CreateSkill>,
) -> Result<Json<SkillRow>, HttpError> {
    if body.built_in == Some(true) && !claims.is_root() {
        return Err(AppError::Unauthorized.into());
    }
    let skill = state.app.create_skill(&body).await?;
    Ok(Json(skill))
}

pub async fn list_skills(State(state): State<AppState>) -> Result<Json<Vec<SkillRow>>, HttpError> {
    let skills = state.app.list_skills().await?;
    Ok(Json(skills))
}

pub async fn get_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<SkillRow>, HttpError> {
    let skill = state.app.get_skill(&slug).await?;
    Ok(Json(skill))
}

pub async fn update_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<UpdateSkill>,
) -> Result<Json<SkillRow>, HttpError> {
    let skill = state.app.update_skill(&slug, &body).await?;
    Ok(Json(skill))
}

pub async fn delete_skill(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<NoContent, HttpError> {
    state.app.delete_skill(&slug).await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}
