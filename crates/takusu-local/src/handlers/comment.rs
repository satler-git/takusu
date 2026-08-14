use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use takusu_contracts::{CommentRow, CreateComment};
use takusu_types::CommentAuthor;

use crate::error::{HttpError, NoContent};
use crate::handlers::common::operation_id;
use crate::state::AppState;

pub async fn list_comments(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<CommentRow>>, HttpError> {
    let comments = state.app.list_comments(&task_id).await?;
    Ok(Json(comments))
}

/// Public comment creation: always records `author = 'user'` (invariant 2).
///
/// Returns `200` (not `201`) to match the worker backend and the wider API
/// convention (memory/habit/task creates all use `200 Ok`), so the OpenAPI
/// success body is documented as `CommentRow`.
pub async fn create_comment(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateComment>,
) -> Result<Json<CommentRow>, HttpError> {
    let comment = state
        .app
        .create_comment(&task_id, CommentAuthor::User, &body, operation_id(&headers))
        .await?;
    Ok(Json(comment))
}

/// Agent comment creation: records `author = 'agent'`. Called only by
/// `takusu-agent` (invariant 2). Access is separation-by-convention, not
/// authentication, until principal-scoped tokens land.
pub async fn create_agent_comment(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateComment>,
) -> Result<Json<CommentRow>, HttpError> {
    let comment = state
        .app
        .create_comment(
            &task_id,
            CommentAuthor::Agent,
            &body,
            operation_id(&headers),
        )
        .await?;
    Ok(Json(comment))
}

/// Delete a comment (user operation, invariant 4).
///
/// Any valid token may delete by id. "User-only" is currently enforced by
/// convention — `takusu-agent` has no delete-comment tool and this endpoint is
/// not called by it — because the token model has no principal scoping yet.
/// Revisit when principal-scoped tokens land (design invariant 2 / WI-7).
pub async fn delete_comment(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<NoContent, HttpError> {
    state.app.delete_comment(&id).await?;
    Ok(NoContent(StatusCode::NO_CONTENT))
}
