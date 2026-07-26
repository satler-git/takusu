use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::middleware;
use axum::routing::{delete, get, patch, post, put};
use sentry::integrations::tower::{NewSentryLayer, SentryHttpLayer};

use crate::auth;
use crate::handlers;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/tasks", post(handlers::task::create_task))
        .route("/tasks", get(handlers::task::list_tasks))
        .route("/tasks/batch", post(handlers::task::create_task_batch))
        .route("/tasks/complete", get(handlers::task::complete_task_query))
        .route("/tasks/import/ical", post(handlers::task::import_ical))
        .route(
            "/tasks/dependency-analysis",
            get(handlers::task::dependency_analysis),
        )
        // `/tasks/similar` must be declared before `/tasks/{id}` so axum
        // matches the literal segment instead of treating "similar" as an id.
        .route("/tasks/similar", get(handlers::memory::find_similar_tasks))
        .route("/tasks/{id}", get(handlers::task::get_task))
        .route("/tasks/{id}", put(handlers::task::replace_task))
        .route("/tasks/{id}", patch(handlers::task::update_task))
        .route("/tasks/{id}", delete(handlers::task::delete_task))
        .route(
            "/tasks/{id}/work/start",
            post(handlers::task::start_task_work),
        )
        .route(
            "/tasks/{id}/work/pause",
            post(handlers::task::pause_task_work),
        )
        .route(
            "/tasks/{id}/progress",
            post(handlers::task::record_progress),
        )
        .route(
            "/tasks/{id}/progress",
            get(handlers::task::get_task_progress),
        )
        .route(
            "/tasks/{id}/work/complete",
            post(handlers::task::complete_task_work),
        )
        .route("/tasks/{id}/split", post(handlers::task::split_task))
        .route("/habits", post(handlers::habit::create_habit))
        .route("/habits/batch", post(handlers::habit::create_habit_batch))
        .route("/habits/preview", post(handlers::habit::preview_habit))
        .route("/habits", get(handlers::habit::list_habits))
        // `/habits/scheduled-spans` and `/habits/steps` must be declared before
        // `/habits/{id}` so axum matches the literal segment instead of
        // treating "scheduled-spans" / "steps" as an id (#303 / #95).
        .route(
            "/habits/scheduled-spans",
            get(handlers::habit::list_all_habit_scheduled_spans),
        )
        .route("/habits/steps", get(handlers::habit::list_all_habit_steps))
        .route("/habits/{id}", get(handlers::habit::get_habit))
        .route("/habits/{id}", put(handlers::habit::replace_habit))
        .route("/habits/{id}", patch(handlers::habit::update_habit))
        .route("/habits/{id}", delete(handlers::habit::delete_habit))
        .route(
            "/habits/{id}/estimate",
            post(handlers::habit::estimate_habit),
        )
        .route(
            "/habits/{id}/scheduled-spans",
            get(handlers::habit::list_habit_scheduled_spans),
        )
        .route(
            "/habits/{id}/scheduled-spans",
            post(handlers::habit::create_habit_scheduled_span),
        )
        .route(
            "/habits/{id}/scheduled-spans/{span_id}",
            delete(handlers::habit::delete_habit_scheduled_span),
        )
        .route("/habits/{id}/steps", get(handlers::habit::list_habit_steps))
        .route(
            "/habits/{id}/steps",
            put(handlers::habit::replace_habit_steps),
        )
        .route(
            "/habits/{id}/steps/dependency-analysis",
            get(handlers::habit::step_dependency_analysis),
        )
        .route("/schedule", get(handlers::schedule::get_schedule))
        .route(
            "/schedule/generate",
            post(handlers::schedule::generate_schedule),
        )
        .route(
            "/schedule/preview",
            post(handlers::schedule::preview_schedule),
        )
        .route(
            "/schedule/replace",
            post(handlers::schedule::replace_schedule),
        )
        .route("/schedule/reschedule", post(handlers::schedule::reschedule))
        .route(
            "/schedule/entries/{task_id}",
            patch(handlers::schedule::move_entry),
        )
        .route("/schedule", delete(handlers::schedule::clear_schedule))
        .route("/tokens", post(handlers::token::create_token))
        .route("/tokens", get(handlers::token::list_tokens))
        .route("/tokens/{id}", delete(handlers::token::revoke_token))
        .route("/sync/settings", get(handlers::sync::get_settings))
        .route("/sync/settings", put(handlers::sync::update_settings))
        .route("/sync/oauth/url", post(handlers::sync::oauth_url))
        .route("/sync/oauth/callback", post(handlers::sync::oauth_callback))
        .route("/sync/trigger", post(handlers::sync::trigger_sync))
        .route(
            "/sync/delete-all",
            post(handlers::sync::delete_all_gcal_events),
        )
        .route("/sync/mappings", get(handlers::sync::list_mappings))
        .route("/settings", get(handlers::settings::get_settings))
        .route("/settings", put(handlers::settings::update_settings))
        .route(
            "/workers/config",
            put(handlers::settings::update_workers_config),
        )
        .route("/skills", get(handlers::skills::list_skills))
        .route("/skills", post(handlers::skills::create_skill))
        .route("/skills/{slug}", get(handlers::skills::get_skill))
        .route("/skills/{slug}", patch(handlers::skills::update_skill))
        .route("/skills/{slug}", delete(handlers::skills::delete_skill))
        .route("/memory", post(handlers::memory::create_memory))
        .route("/memory/search", get(handlers::memory::search_memory))
        .route("/memory/{id}", get(handlers::memory::get_memory))
        .route("/memory/{id}", patch(handlers::memory::update_memory))
        .route("/memory/{id}", delete(handlers::memory::delete_memory))
        .route("/workers/health", get(handlers::settings::workers_health))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));

    Router::new()
        .route("/health", get(health))
        .nest("/api", api)
        .with_state(state)
        .layer(SentryHttpLayer::new().enable_transaction())
        .layer(NewSentryLayer::<Request<Body>>::new_from_top())
}

async fn health() -> &'static str {
    "ok"
}
