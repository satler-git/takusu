use takusu_local_lib::error::AppError;
use thiserror::Error;

#[derive(Debug, Error)]
#[error(transparent)]
pub struct HttpError(#[from] pub AppError);

impl axum::response::IntoResponse for HttpError {
    fn into_response(self) -> axum::response::Response {
        use axum::Json;
        use axum::http::StatusCode;
        let (status, body) = match &self.0 {
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, serde_json::json!({ "message": m })),
            AppError::BadRequest(kind) => (
                StatusCode::BAD_REQUEST,
                serde_json::json!({ "message": kind.to_string() }),
            ),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                serde_json::json!({ "message": "unauthorized" }),
            ),
            AppError::Conflict(kind) => (
                StatusCode::CONFLICT,
                serde_json::json!({ "message": kind.to_string() }),
            ),
            AppError::Internal(m) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "message": m }),
            ),
        };
        (status, Json(body)).into_response()
    }
}
