use thiserror::Error;
use takusu_local_lib::error::AppError;

#[derive(Debug, Error)]
#[error(transparent)]
pub struct HttpError(#[from] pub AppError);

impl axum::response::IntoResponse for HttpError {
    fn into_response(self) -> axum::response::Response {
        use axum::Json;
        use axum::http::StatusCode;
        let (status, body) = match &self.0 {
            AppError::NotFound(m) => (StatusCode::NOT_FOUND, serde_json::json!({ "message": m })),
            AppError::BadRequest(m) => {
                (StatusCode::BAD_REQUEST, serde_json::json!({ "message": m }))
            }
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                serde_json::json!({ "message": "unauthorized" }),
            ),
            AppError::Conflict { message } => (
                StatusCode::CONFLICT,
                serde_json::json!({ "message": message }),
            ),
            AppError::Internal(m) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({ "message": m }),
            ),
        };
        (status, Json(body)).into_response()
    }
}
