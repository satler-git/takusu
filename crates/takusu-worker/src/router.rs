use worker::{Cors, Env, Method, Request, Response};

use crate::error::error_response;
use crate::handlers;

pub async fn handle(req: Request, env: Env) -> worker::Result<Response> {
    let start = worker::Date::now().as_millis();
    let method = req.method();
    let path = req.url()?.path().to_string();

    log::info!("=> {} {}", method, path);

    if method == Method::Options {
        log::info!(
            "<= {} {} -> 204 ({}ms)",
            method,
            path,
            worker::Date::now().as_millis() - start
        );
        return preflight(&req, &env);
    }
    let result = dispatch(req, env.clone()).await;
    let resp = match result {
        Ok(resp) => resp,
        Err(e) => error_response(e)?,
    };
    let status = resp.status_code();
    let resp = apply_cors(&env, resp);
    log::info!(
        "<= {} {} -> {} ({}ms)",
        method,
        path,
        status,
        worker::Date::now().as_millis() - start
    );
    resp
}

fn preflight(req: &Request, env: &Env) -> worker::Result<Response> {
    let cors = build_cors(env);
    let mut resp = Response::empty()?;
    cors.apply_headers(resp.headers_mut())?;
    let _ = req;
    Ok(resp)
}

fn build_cors(env: &Env) -> Cors {
    let mut cors = Cors::default()
        .with_origins(["*"])
        .with_methods([
            Method::Get,
            Method::Post,
            Method::Put,
            Method::Patch,
            Method::Delete,
        ])
        .with_allowed_headers(["authorization", "content-type", "idempotency-key"]);
    if let Ok(allowed) = env.var("TAKUSU_ALLOWED_ORIGIN") {
        let list: Vec<String> = allowed
            .to_string()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if !list.is_empty() {
            cors = cors.with_origins(list);
        }
    }
    cors
}

fn apply_cors(env: &Env, mut resp: Response) -> worker::Result<Response> {
    let cors = build_cors(env);
    cors.apply_headers(resp.headers_mut())?;
    Ok(resp)
}

async fn dispatch(req: Request, env: Env) -> Result<Response, crate::error::WorkerError> {
    let url = req.url()?;
    let path = url.path();
    let method = req.method();

    if path == "/health" {
        return Ok(handlers::health::health());
    }

    let api = path.strip_prefix("/api/").unwrap_or(path);
    let segs: Vec<&str> = api.split('/').filter(|s| !s.is_empty()).collect();

    if segs != ["auth", "verify"] {
        handlers::auth::require_auth(&req, &env).await?;
    }

    match (method.clone(), segs.as_slice()) {
        (Method::Get, ["auth", "verify"]) => handlers::auth::verify(req, env).await,
        (Method::Post, ["tokens"]) => handlers::tokens::create(req, env).await,
        (Method::Get, ["tokens"]) => handlers::tokens::list(req, env).await,
        (Method::Delete, ["tokens", id]) => handlers::tokens::revoke(req, env, id).await,
        (Method::Get, ["tasks"]) => handlers::tasks::list(req, env).await,
        (Method::Post, ["tasks"]) => handlers::tasks::create(req, env).await,
        (Method::Get, ["tasks", "similar"]) => handlers::memory::similar_tasks(req, env).await,
        (Method::Get, ["tasks", id]) => handlers::tasks::get(req, env, id).await,
        (Method::Patch, ["tasks", id]) => handlers::tasks::update(req, env, id).await,
        (Method::Put, ["tasks", id]) => handlers::tasks::replace(req, env, id).await,
        (Method::Delete, ["tasks", id]) => handlers::tasks::delete(req, env, id).await,
        (Method::Get, ["tasks", id, "progress"]) => handlers::progress::get_task_progress(req, env, id).await,
        (Method::Post, ["tasks", id, "split"]) => handlers::progress::split_task(req, env, id).await,
        (Method::Get, ["tasks", id, "comments"]) => handlers::comments::list(req, env, id).await,
        (Method::Post, ["tasks", id, "comments"]) => handlers::comments::create_user(req, env, id).await,
        (Method::Post, ["tasks", id, "comments", "agent"]) => handlers::comments::create_agent(req, env, id).await,
        (Method::Delete, ["comments", id]) => handlers::comments::delete(req, env, id).await,
        (Method::Post, ["work-sessions"]) => handlers::progress::create_work_session(req, env).await,
        (Method::Get, ["work-sessions"]) => handlers::progress::list_work_sessions(req, env).await,
        (Method::Get, ["work-sessions", id]) => handlers::progress::get_work_session(req, env, id).await,
        (Method::Post, ["work-sessions", id, "pause"]) => handlers::progress::pause_work_session(req, env, id).await,
        (Method::Post, ["work-sessions", id, "complete"]) => handlers::progress::complete_work_session(req, env, id).await,
        (Method::Post, ["work-sessions", id, "progress"]) => handlers::progress::record_work_session_progress(req, env, id).await,
        (Method::Post, ["work-sessions", id, "attach"]) => handlers::progress::attach_work_session(req, env, id).await,
        (Method::Post, ["work-sessions", id, "convert"]) => handlers::progress::convert_work_session(req, env, id).await,
        (Method::Get, ["habits"]) => handlers::habits::list(req, env).await,
        (Method::Post, ["habits"]) => handlers::habits::create(req, env).await,
        (Method::Get, ["habits", "scheduled-spans"]) => {
            handlers::habits::list_all_scheduled_spans(req, env).await
        }
        (Method::Get, ["habits", "steps"]) => handlers::habits::list_all_steps(req, env).await,
        (Method::Get, ["habits", id]) => handlers::habits::get(req, env, id).await,
        (Method::Patch, ["habits", id]) => handlers::habits::update(req, env, id).await,
        (Method::Put, ["habits", id]) => handlers::habits::replace(req, env, id).await,
        (Method::Delete, ["habits", id]) => handlers::habits::delete(req, env, id).await,
        (Method::Get, ["habits", id, "scheduled-spans"]) => {
            handlers::habits::list_scheduled_spans(req, env, id).await
        }
        (Method::Post, ["habits", id, "scheduled-spans"]) => {
            handlers::habits::create_scheduled_span(req, env, id).await
        }
        (Method::Delete, ["habits", id, "scheduled-spans", span_id]) => {
            handlers::habits::delete_scheduled_span(req, env, id, span_id).await
        }
        (Method::Get, ["habits", id, "steps"]) => handlers::habits::list_steps(req, env, id).await,
        (Method::Put, ["habits", id, "steps"]) => {
            handlers::habits::replace_steps(req, env, id).await
        }
        (Method::Post, ["habits", id, "estimate"]) => {
            handlers::habits::apply_estimate(req, env, id).await
        }
        (Method::Get, ["schedule"]) => handlers::schedule::get(req, env).await,
        (Method::Post, ["schedule", "save"]) => handlers::schedule::save(req, env).await,
        (Method::Delete, ["schedule"]) => handlers::schedule::clear(req, env).await,
        (Method::Get, ["settings"]) => handlers::settings::get(req, env).await,
        (Method::Put, ["settings"]) => handlers::settings::update(req, env).await,
        (Method::Get, ["skills"]) => handlers::skills::list(req, env).await,
        (Method::Post, ["skills"]) => handlers::skills::create(req, env).await,
        (Method::Get, ["skills", id]) => handlers::skills::get(req, env, id).await,
        (Method::Patch, ["skills", id]) => handlers::skills::update(req, env, id).await,
        (Method::Delete, ["skills", id]) => handlers::skills::delete(req, env, id).await,
        (Method::Post, ["memory"]) => handlers::memory::create(req, env).await,
        (Method::Get, ["memory", "search"]) => handlers::memory::search(req, env).await,
        (Method::Get, ["memory", id]) => handlers::memory::get(req, env, id).await,
        (Method::Patch, ["memory", id]) => handlers::memory::update(req, env, id).await,
        (Method::Delete, ["memory", id]) => handlers::memory::delete(req, env, id).await,
        (Method::Get, ["sync", "settings"]) => handlers::sync::get_settings(req, env).await,
        (Method::Put, ["sync", "settings"]) => handlers::sync::update_settings(req, env).await,
        (Method::Get, ["sync", "mappings"]) => handlers::sync::list_mappings(req, env).await,
        (Method::Post, ["sync", "mappings"]) => handlers::sync::upsert_mappings(req, env).await,
        (Method::Delete, ["sync", "mappings"]) => handlers::sync::delete_mappings(req, env).await,
        _ => Err(crate::error::WorkerError::NotFound(format!(
            "{} {}",
            method, path
        ))),
    }
}
