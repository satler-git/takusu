use aide::operation::OperationOutput;
use aide::openapi::{Response, StatusCode as OpenApiStatusCode};
use schemars::JsonSchema;
use serde::Serialize;
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

/// JSON body returned by all error responses.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ErrorMessage {
    pub message: String,
}

impl OperationOutput for HttpError {
    type Inner = ErrorMessage;

    fn inferred_responses(
        _ctx: &mut aide::generate::GenContext,
        _operation: &mut aide::openapi::Operation,
    ) -> Vec<(Option<OpenApiStatusCode>, Response)> {
        let json_schema = schemars::schema_for!(ErrorMessage);
        let response = Response {
            description: "Error".to_string(),
            content: indexmap::IndexMap::from([(
                "application/json".to_string(),
                aide::openapi::MediaType {
                    schema: Some(aide::openapi::SchemaObject {
                        json_schema,
                        external_docs: None,
                        example: None,
                    }),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };
        vec![
            (Some(OpenApiStatusCode::Code(400)), response.clone()),
            (Some(OpenApiStatusCode::Code(401)), response.clone()),
            (Some(OpenApiStatusCode::Code(404)), response.clone()),
            (Some(OpenApiStatusCode::Code(409)), response.clone()),
            (Some(OpenApiStatusCode::Code(500)), response),
        ]
    }
}

/// Wrapper around `StatusCode` that implements `OperationOutput` to document
/// 204 No Content responses (used by DELETE handlers).
#[derive(Debug)]
pub struct NoContent(pub axum::http::StatusCode);

impl axum::response::IntoResponse for NoContent {
    fn into_response(self) -> axum::response::Response {
        self.0.into_response()
    }
}

impl OperationOutput for NoContent {
    type Inner = ();

    fn inferred_responses(
        _ctx: &mut aide::generate::GenContext,
        _operation: &mut aide::openapi::Operation,
    ) -> Vec<(Option<OpenApiStatusCode>, Response)> {
        vec![(
            Some(OpenApiStatusCode::Code(204)),
            Response {
                description: "No Content".to_string(),
                ..Default::default()
            },
        )]
    }
}
