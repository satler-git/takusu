use aide::axum::ApiRouter;
use aide::axum::routing as api;
use aide::openapi::{Info, OpenApi, Server};
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::middleware;
use axum::routing::get;
use sentry::integrations::tower::{NewSentryLayer, SentryHttpLayer};
use std::sync::Arc;

use crate::auth;
use crate::handlers;
use crate::state::AppState;

/// Create the base `OpenApi` with shared info and server metadata.
fn base_open_api() -> OpenApi {
    OpenApi {
        info: Info {
            title: "takusu API".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        },
        servers: vec![Server {
            url: "/api".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Build the API routes with all documented routes and populate `open_api`.
///
/// This is shared between [`router`] (runtime) and [`generate_openapi`] (build
/// tool) so the generated spec always matches the served routes.
fn build_api_router(open_api: &mut OpenApi) -> Router<AppState> {
    ApiRouter::new()
        .api_route("/tasks", api::post(handlers::task::create_task))
        .api_route("/tasks", api::get(handlers::task::list_tasks))
        .api_route("/tasks/batch", api::post(handlers::task::create_task_batch))
        .api_route(
            "/tasks/complete",
            api::get(handlers::task::complete_task_query),
        )
        .api_route("/tasks/import/ical", api::post(handlers::task::import_ical))
        .api_route(
            "/tasks/dependency-analysis",
            api::get(handlers::task::dependency_analysis),
        )
        // `/tasks/similar` must be declared before `/tasks/{id}` so axum
        // matches the literal segment instead of treating "similar" as an id.
        .api_route(
            "/tasks/similar",
            api::get(handlers::memory::find_similar_tasks),
        )
        .api_route("/tasks/{id}", api::get(handlers::task::get_task))
        .api_route("/tasks/{id}", api::put(handlers::task::replace_task))
        .api_route("/tasks/{id}", api::patch(handlers::task::update_task))
        .api_route("/tasks/{id}", api::delete(handlers::task::delete_task))
        .api_route("/tasks/{id}/split", api::post(handlers::task::split_task))
        .api_route(
            "/tasks/{id}/comments",
            api::get(handlers::comment::list_comments),
        )
        .api_route(
            "/tasks/{id}/comments",
            api::post(handlers::comment::create_comment),
        )
        .api_route(
            "/tasks/{id}/comments/agent",
            api::post(handlers::comment::create_agent_comment),
        )
        .api_route(
            "/comments/{id}",
            api::delete(handlers::comment::delete_comment),
        )
        .api_route(
            "/work-sessions",
            api::post(handlers::work_session::create_work_session),
        )
        .api_route(
            "/work-sessions",
            api::get(handlers::work_session::list_work_sessions),
        )
        .api_route(
            "/work-sessions/{id}",
            api::get(handlers::work_session::get_work_session),
        )
        .api_route(
            "/work-sessions/{id}/pause",
            api::post(handlers::work_session::pause_work_session),
        )
        .api_route(
            "/work-sessions/{id}/complete",
            api::post(handlers::work_session::complete_work_session),
        )
        .api_route(
            "/work-sessions/{id}/progress",
            api::post(handlers::work_session::record_work_session_progress),
        )
        .api_route(
            "/work-sessions/{id}/attach",
            api::post(handlers::work_session::attach_work_session),
        )
        .api_route(
            "/work-sessions/{id}/convert",
            api::post(handlers::work_session::convert_work_session),
        )
        .api_route("/habits", api::post(handlers::habit::create_habit))
        .api_route(
            "/habits/batch",
            api::post(handlers::habit::create_habit_batch),
        )
        .api_route("/habits/preview", api::post(handlers::habit::preview_habit))
        .api_route("/habits", api::get(handlers::habit::list_habits))
        // `/habits/scheduled-spans` and `/habits/steps` must be declared before
        // `/habits/{id}` so axum matches the literal segment instead of
        // treating "scheduled-spans" / "steps" as an id (#303 / #95).
        .api_route(
            "/habits/scheduled-spans",
            api::get(handlers::habit::list_all_habit_scheduled_spans),
        )
        .api_route(
            "/habits/steps",
            api::get(handlers::habit::list_all_habit_steps),
        )
        .api_route("/habits/{id}", api::get(handlers::habit::get_habit))
        .api_route("/habits/{id}", api::put(handlers::habit::replace_habit))
        .api_route("/habits/{id}", api::patch(handlers::habit::update_habit))
        .api_route("/habits/{id}", api::delete(handlers::habit::delete_habit))
        .api_route(
            "/habits/{id}/estimate",
            api::post(handlers::habit::estimate_habit),
        )
        .api_route(
            "/habits/{id}/scheduled-spans",
            api::get(handlers::habit::list_habit_scheduled_spans),
        )
        .api_route(
            "/habits/{id}/scheduled-spans",
            api::post(handlers::habit::create_habit_scheduled_span),
        )
        .api_route(
            "/habits/{id}/scheduled-spans/{span_id}",
            api::delete(handlers::habit::delete_habit_scheduled_span),
        )
        .api_route(
            "/habits/{id}/steps",
            api::get(handlers::habit::list_habit_steps),
        )
        .api_route(
            "/habits/{id}/steps",
            api::put(handlers::habit::replace_habit_steps),
        )
        .api_route(
            "/habits/{id}/steps/dependency-analysis",
            api::get(handlers::habit::step_dependency_analysis),
        )
        .api_route("/schedule", api::get(handlers::schedule::get_schedule))
        .api_route(
            "/schedule/generate",
            api::post(handlers::schedule::generate_schedule),
        )
        .api_route(
            "/schedule/preview",
            api::post(handlers::schedule::preview_schedule),
        )
        .api_route(
            "/schedule/replace",
            api::post(handlers::schedule::replace_schedule),
        )
        .api_route(
            "/schedule/reschedule",
            api::post(handlers::schedule::reschedule),
        )
        .api_route(
            "/schedule/entries/{task_id}",
            api::patch(handlers::schedule::move_entry),
        )
        .api_route("/schedule", api::delete(handlers::schedule::clear_schedule))
        .api_route("/tokens", api::post(handlers::token::create_token))
        .api_route("/tokens", api::get(handlers::token::list_tokens))
        .api_route("/tokens/{id}", api::delete(handlers::token::revoke_token))
        .api_route("/sync/settings", api::get(handlers::sync::get_settings))
        .api_route("/sync/settings", api::put(handlers::sync::update_settings))
        .api_route("/sync/oauth/url", api::post(handlers::sync::oauth_url))
        .api_route(
            "/sync/oauth/callback",
            api::post(handlers::sync::oauth_callback),
        )
        .api_route("/sync/trigger", api::post(handlers::sync::trigger_sync))
        .api_route(
            "/sync/delete-all",
            api::post(handlers::sync::delete_all_gcal_events),
        )
        .api_route("/sync/mappings", api::get(handlers::sync::list_mappings))
        .api_route("/settings", api::get(handlers::settings::get_settings))
        .api_route("/settings", api::put(handlers::settings::update_settings))
        .api_route(
            "/workers/config",
            api::put(handlers::settings::update_workers_config),
        )
        .api_route("/skills", api::get(handlers::skills::list_skills))
        .api_route("/skills", api::post(handlers::skills::create_skill))
        .api_route("/skills/{slug}", api::get(handlers::skills::get_skill))
        .api_route("/skills/{slug}", api::patch(handlers::skills::update_skill))
        .api_route(
            "/skills/{slug}",
            api::delete(handlers::skills::delete_skill),
        )
        .api_route("/memory", api::post(handlers::memory::create_memory))
        .api_route("/memory/search", api::get(handlers::memory::search_memory))
        .api_route(
            "/memory/inject",
            api::post(handlers::memory::injectable_memory),
        )
        .api_route("/memory/{id}", api::get(handlers::memory::get_memory))
        .api_route("/memory/{id}", api::patch(handlers::memory::update_memory))
        .api_route("/memory/{id}", api::delete(handlers::memory::delete_memory))
        .api_route("/events", api::get(handlers::events::list_events))
        .api_route("/events", api::post(handlers::events::insert_event))
        .api_route("/events/revision", api::get(handlers::events::revision))
        .api_route("/events/snapshot", api::get(handlers::events::snapshot))
        .api_route(
            "/events/evaluate",
            api::post(handlers::events::evaluate_events),
        )
        .api_route(
            "/events/{event_id}/claim",
            api::post(handlers::events::claim_event),
        )
        .api_route(
            "/events/{event_id}/acknowledge",
            api::post(handlers::events::acknowledge_event),
        )
        .api_route(
            "/events/{event_id}/state",
            api::put(handlers::events::update_event_state),
        )
        .api_route(
            "/workers/health",
            api::get(handlers::settings::workers_health),
        )
        .api_route("/devices", api::post(handlers::device::create_device))
        .api_route("/devices", api::get(handlers::device::list_devices))
        .api_route("/devices/{id}", api::get(handlers::device::get_device))
        .api_route("/devices/{id}", api::patch(handlers::device::update_device))
        .api_route("/devices/{id}", api::delete(handlers::device::delete_device))
        .api_route(
            "/devices/{id}/heartbeat",
            api::post(handlers::device::refresh_heartbeat),
        )
        .api_route(
            "/devices/{id}/lease",
            api::post(handlers::device::refresh_lease),
        )
        .api_route(
            "/devices/{id}/resident",
            api::get(handlers::device::resolve_resident_authority),
        )
        .api_route(
            "/devices/{id}/speech",
            api::get(handlers::device::get_speech_capability),
        )
        // Serve the generated OpenAPI document. This route is not documented
        // in the spec itself (it uses `route`, not `api_route`).
        .route("/openapi.json", get(serve_openapi))
        .finish_api_with(open_api, |api| {
            api.default_response::<crate::error::HttpError>()
        })
}

pub fn router(state: AppState) -> Router {
    let mut open_api = base_open_api();

    let api = build_api_router(&mut open_api).layer(middleware::from_fn_with_state(
        state.clone(),
        auth::auth_middleware,
    ));

    // Agent routes for the desktop resident daemon (WI-7). They live under
    // `/api/agent/v1/` and use the in-process `AgentApiState`.
    let agent_router = takusu_agent::transport::router(state.agent.clone());

    Router::new()
        .route("/health", get(health))
        .nest("/api", api)
        .nest_service("/api/agent/v1", agent_router)
        .with_state(state)
        .layer(Extension(Arc::new(open_api)))
        .layer(SentryHttpLayer::new().enable_transaction())
        .layer(NewSentryLayer::<Request<Body>>::new_from_top())
}

/// Generate the OpenAPI document without starting a server.
///
/// Used by the `generate-openapi` binary and CI to produce the spec file
/// that `openapi-typescript` consumes.
pub fn generate_openapi() -> OpenApi {
    let mut open_api = base_open_api();
    // Build the router to populate the spec, then discard the router.
    let _ = build_api_router(&mut open_api);
    open_api
}

async fn health() -> &'static str {
    "ok"
}

async fn serve_openapi(Extension(api): Extension<Arc<OpenApi>>) -> axum::response::Response {
    // Serialize once and return the bytes directly to avoid cloning the
    // full ~360 KB OpenApi document on every request.
    let body = serde_json::to_vec(api.as_ref()).unwrap_or_default();
    axum::response::IntoResponse::into_response((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Body::from(body),
    ))
}
