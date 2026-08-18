//! OpenAPI spec generation for the agent HTTP API.
//!
//! This module builds a standalone `OpenApi` document for the versioned
//! `/api/agent/v1/*` endpoints. It uses stub handlers and the same request /
//! response types as `crate::transport` so the generated schemas stay in sync
//! with the wire format without requiring a full `aide`-based runtime router.

use std::convert::Infallible;
use std::marker::PhantomData;

use aide::axum::ApiRouter;
use aide::axum::routing as api;
use aide::generate::GenContext;
use aide::openapi::{
    Info, MediaType, OpenApi, Operation, RequestBody, Response as OpenApiResponse, SchemaObject,
    Server, StatusCode,
};
use aide::operation::{OperationInput, OperationOutput, set_body};
use axum::body::Body;
use axum::extract::{FromRequest, Json, Path, Query, Request};
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, NoContent, Response};
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Presentation;
use crate::ToolStatsSnapshot;
use crate::capability::{ActionCapability, CapabilityRequest};
use crate::notification::{StartTimeNotificationList, StartTimeNotificationRequest};
use crate::surface::{SurfaceCommandResponse, SurfaceEvent, SurfaceSnapshot};
use crate::transport::{
    API_VERSION, ApprovalDecisionRequest, ApprovalResultDto, CapabilitiesResponse,
    CreateSessionRequest, CreateSessionResponse, EditTurnRequest, HealthResponse, PlannerEvent,
    ResumeSessionRequest, ResumeSessionResponse, RevertRequest, SseEvent, SurfaceAudioRequest,
    SurfaceCommandRequest, TurnRequest, TurnResultDto, UpdateAgentSettings, UpdateSessionSettings,
    UserInputResolutionRequest, Versioned,
};

/// Generic `{ "ok": true }` body used by several agent endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OkResponse {
    pub ok: bool,
}

/// Default error body returned by agent endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ErrorResponse {
    pub version: u8,
    pub error: String,
}

/// An extractor that documents an optional JSON request body.
pub struct MaybeJson<T>(pub Option<T>);

impl<T, S> FromRequest<S> for MaybeJson<T>
where
    T: JsonSchema + Send,
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request(_req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        // This function is only used during OpenAPI generation; the real router
        // uses axum's own extractors.
        Ok(MaybeJson(None))
    }
}

impl<T: JsonSchema> OperationInput for MaybeJson<T> {
    fn operation_input(ctx: &mut GenContext, operation: &mut Operation) {
        let json_schema = ctx.schema.subschema_for::<T>();
        let resolved_schema = ctx.resolve_schema(&json_schema);

        set_body(
            ctx,
            operation,
            RequestBody {
                description: resolved_schema
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(String::from),
                content: IndexMap::from_iter([(
                    "application/json".into(),
                    MediaType {
                        schema: Some(SchemaObject {
                            json_schema,
                            example: None,
                            external_docs: None,
                        }),
                        ..Default::default()
                    },
                )]),
                required: false,
                extensions: IndexMap::default(),
            },
        );
    }

    fn inferred_early_responses(
        ctx: &mut GenContext,
        operation: &mut Operation,
    ) -> Vec<(Option<StatusCode>, OpenApiResponse)> {
        <Json<T> as OperationInput>::inferred_early_responses(ctx, operation)
    }
}

/// A response type that documents an SSE (`text/event-stream`) endpoint. The
/// schema describes the shape of each individual event payload.
pub struct SseStream<T>(PhantomData<T>);

impl<T> Default for SseStream<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: JsonSchema> IntoResponse for SseStream<T> {
    fn into_response(self) -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::empty())
            .unwrap()
    }
}

impl<T: JsonSchema> OperationOutput for SseStream<T> {
    type Inner = T;

    fn operation_response(
        ctx: &mut GenContext,
        _operation: &mut Operation,
    ) -> Option<OpenApiResponse> {
        let json_schema = ctx.schema.subschema_for::<T>();
        let resolved_schema = ctx.resolve_schema(&json_schema);

        Some(OpenApiResponse {
            description: resolved_schema
                .get("description")
                .and_then(|d| d.as_str())
                .map(String::from)
                .unwrap_or_else(|| "Server-Sent Events stream".to_string()),
            content: IndexMap::from_iter([(
                "text/event-stream".into(),
                MediaType {
                    schema: Some(SchemaObject {
                        json_schema,
                        example: None,
                        external_docs: None,
                    }),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        })
    }

    fn inferred_responses(
        ctx: &mut GenContext,
        operation: &mut Operation,
    ) -> Vec<(Option<StatusCode>, OpenApiResponse)> {
        Self::operation_response(ctx, operation)
            .map(|r| vec![(Some(StatusCode::Code(200)), r)])
            .unwrap_or_default()
    }
}

fn versioned<T: Serialize>(value: T) -> Json<Versioned<T>> {
    Json(Versioned {
        version: API_VERSION,
        value,
    })
}

async fn health() -> Json<Versioned<HealthResponse>> {
    versioned(HealthResponse { ok: true })
}

async fn capabilities() -> Json<Versioned<CapabilitiesResponse>> {
    versioned(CapabilitiesResponse {
        audio_input: true,
        tts: true,
        approvals: true,
        user_input: true,
    })
}

async fn update_settings(
    Json(_): Json<Versioned<UpdateAgentSettings>>,
) -> Json<Versioned<OkResponse>> {
    versioned(OkResponse { ok: true })
}

async fn create_session(
    MaybeJson(_): MaybeJson<Versioned<CreateSessionRequest>>,
) -> Json<Versioned<CreateSessionResponse>> {
    versioned(CreateSessionResponse {
        session_id: String::new(),
    })
}

async fn resume_session(
    Json(_): Json<Versioned<ResumeSessionRequest>>,
) -> Json<Versioned<ResumeSessionResponse>> {
    versioned(ResumeSessionResponse {
        session_id: String::new(),
    })
}

async fn run_turn(
    Path(_id): Path<String>,
    Json(_): Json<Versioned<TurnRequest>>,
) -> Json<Versioned<TurnResultDto>> {
    versioned(TurnResultDto {
        text: String::new(),
        changes: vec![],
        schedule_dirty: false,
        approval_request: None,
        presentation: None,
    })
}

async fn run_turn_stream(
    Path(_id): Path<String>,
    Json(_): Json<Versioned<TurnRequest>>,
) -> SseStream<SseEvent> {
    SseStream::default()
}

async fn edit_turn_stream(
    Path((_id, _turn_index)): Path<(String, usize)>,
    Json(_): Json<Versioned<EditTurnRequest>>,
) -> SseStream<SseEvent> {
    SseStream::default()
}

async fn events_stream() -> SseStream<PlannerEvent> {
    SseStream::default()
}

async fn surface_snapshot() -> Json<Versioned<SurfaceSnapshot>> {
    versioned(SurfaceSnapshot::default())
}

async fn surface_events() -> SseStream<SurfaceEvent> {
    SseStream::default()
}

async fn surface_command(
    Json(body): Json<Versioned<SurfaceCommandRequest>>,
) -> Json<Versioned<SurfaceCommandResponse>> {
    versioned(SurfaceCommandResponse {
        command: body.value.command,
        accepted: false,
        reason: Some("schema stub".into()),
        snapshot: SurfaceSnapshot::default(),
    })
}

async fn surface_audio(
    Json(_body): Json<Versioned<SurfaceAudioRequest>>,
) -> Json<Versioned<SurfaceSnapshot>> {
    versioned(SurfaceSnapshot::default())
}

async fn revert_turn(
    Path((_id, _turn_index)): Path<(String, usize)>,
    Json(_): Json<Versioned<RevertRequest>>,
) -> Json<Versioned<OkResponse>> {
    versioned(OkResponse { ok: true })
}

async fn update_session_settings(
    Path(_id): Path<String>,
    Json(_): Json<Versioned<UpdateSessionSettings>>,
) -> Json<Versioned<OkResponse>> {
    versioned(OkResponse { ok: true })
}

async fn get_approval(Path(_id): Path<String>) -> Json<Versioned<Option<crate::ApprovalRequest>>> {
    versioned(None)
}

async fn resolve_approval(
    Path((_id, _approval_id)): Path<(String, String)>,
    Json(_): Json<Versioned<ApprovalDecisionRequest>>,
) -> Json<Versioned<ApprovalResultDto>> {
    versioned(ApprovalResultDto {
        id: String::new(),
        approved: false,
        changes: vec![],
        schedule_dirty: false,
    })
}

async fn resolve_user_input(
    Path((_id, _call_id)): Path<(String, String)>,
    Json(_): Json<Versioned<UserInputResolutionRequest>>,
) -> Json<Versioned<OkResponse>> {
    versioned(OkResponse { ok: true })
}

async fn delete_session(Path(_id): Path<String>) -> NoContent {
    NoContent
}

async fn get_tool_stats() -> Json<Versioned<ToolStatsSnapshot>> {
    versioned(ToolStatsSnapshot::default())
}

async fn clear_tool_stats() -> NoContent {
    NoContent
}

/// Schema-only stub for `POST /agent/v1/capabilities`. The real handler is in
/// `transport.rs`; this function exists only to register the OpenAPI shape.
async fn mint_capability_schema(
    Json(_body): Json<Versioned<CapabilityRequest>>,
) -> Json<Versioned<ActionCapability>> {
    versioned(ActionCapability {
        id: "stub".to_string(),
        event_id: None,
        device_id: "stub".to_string(),
        action: "start".to_string(),
        input_path: crate::capability::InputPath::ScreenCapability,
        expires_at: jiff::Timestamp::now(),
        one_shot: true,
        task_id: "stub".to_string(),
        snooze_minutes: None,
        snooze_target: None,
        quantity_done: None,
        note: None,
        scheduled_at: None,
        request: None,
    })
}

/// Schema-only stub for `POST /agent/v1/actions`. The real handler is in
/// `transport.rs`; this function exists only to register the OpenAPI shape.
async fn authorize_action_schema(
    Json(_body): Json<Versioned<ActionCapability>>,
) -> Json<Versioned<Presentation>> {
    versioned(Presentation::Text {
        text: "stub".to_string(),
    })
}

/// Schema-only stub for `GET /agent/v1/notifications/start-time`.
async fn start_time_notifications_schema(
    Query(_request): Query<StartTimeNotificationRequest>,
) -> Json<Versioned<StartTimeNotificationList>> {
    versioned(StartTimeNotificationList {
        notifications: vec![],
    })
}

fn build_api_router(open_api: &mut OpenApi) -> axum::Router {
    ApiRouter::new()
        .api_route("/agent/v1/health", api::get(health))
        .api_route("/agent/v1/events", api::get(events_stream))
        .api_route("/agent/v1/capabilities", api::get(capabilities))
        .api_route("/agent/v1/capabilities", api::post(mint_capability_schema))
        .api_route("/agent/v1/actions", api::post(authorize_action_schema))
        .api_route(
            "/agent/v1/notifications/start-time",
            api::get(start_time_notifications_schema),
        )
        .api_route("/agent/v1/settings", api::put(update_settings))
        .api_route("/agent/v1/sessions", api::post(create_session))
        .api_route("/agent/v1/sessions/resume", api::post(resume_session))
        .api_route("/agent/v1/sessions/{id}/turns", api::post(run_turn))
        .api_route(
            "/agent/v1/sessions/{id}/turns/stream",
            api::post(run_turn_stream),
        )
        .api_route("/agent/v1/surface", api::get(surface_snapshot))
        .api_route("/agent/v1/surface/events", api::get(surface_events))
        .api_route("/agent/v1/surface/commands", api::post(surface_command))
        .api_route("/agent/v1/surface/audio", api::post(surface_audio))
        .api_route(
            "/agent/v1/sessions/{id}/turns/{turn_index}/edit/stream",
            api::post(edit_turn_stream),
        )
        .api_route(
            "/agent/v1/sessions/{id}/turns/{turn_index}/revert",
            api::post(revert_turn),
        )
        .api_route(
            "/agent/v1/sessions/{id}/settings",
            api::put(update_session_settings),
        )
        .api_route("/agent/v1/sessions/{id}/approval", api::get(get_approval))
        .api_route(
            "/agent/v1/sessions/{id}/approvals/{approval_id}",
            api::post(resolve_approval),
        )
        .api_route(
            "/agent/v1/sessions/{id}/tool-calls/{call_id}/user-input",
            api::post(resolve_user_input),
        )
        .api_route("/agent/v1/sessions/{id}", api::delete(delete_session))
        .api_route("/agent/v1/stats/tools", api::get(get_tool_stats))
        .api_route("/agent/v1/stats/tools", api::delete(clear_tool_stats))
        .finish_api_with(open_api, |api| {
            api.default_response::<Json<ErrorResponse>>()
        })
}

/// Generate the `OpenApi` document for the agent API.
///
/// Paths are prefixed with `/agent/v1` so the document can be merged into the
/// main takusu API spec whose server URL is `/api`.
pub fn generate_openapi() -> OpenApi {
    let mut open_api = OpenApi {
        openapi: std::borrow::Cow::Borrowed("3.1.0"),
        info: Info {
            title: "takusu agent API".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        },
        servers: vec![Server {
            url: "/api".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let _ = build_api_router(&mut open_api);
    open_api
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_openapi_includes_core_paths() {
        let spec = generate_openapi();
        let paths = &spec.paths.unwrap().paths;
        assert!(paths.contains_key("/agent/v1/health"));
        assert!(paths.contains_key("/agent/v1/events"));
        assert!(paths.contains_key("/agent/v1/surface"));
        assert!(paths.contains_key("/agent/v1/surface/events"));
        assert!(paths.contains_key("/agent/v1/surface/commands"));
        assert!(paths.contains_key("/agent/v1/surface/audio"));
        assert!(paths.contains_key("/agent/v1/sessions"));
        assert!(paths.contains_key("/agent/v1/sessions/{id}/turns/stream"));
    }

    #[test]
    fn generate_openapi_is_valid_json() {
        let spec = generate_openapi();
        let json = serde_json::to_value(&spec).expect("serialize openapi");
        assert_eq!(json["openapi"], "3.1.0");
        assert!(json["components"]["schemas"].as_object().is_some());
    }
}
