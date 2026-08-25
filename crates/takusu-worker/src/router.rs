use percent_encoding::percent_decode_str;
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

/// Decode a percent-encoded URL path segment so IDs containing `:` or other
/// `pchar` characters match the values stored in D1.
fn decode_path(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

async fn dispatch(req: Request, env: Env) -> Result<Response, crate::error::WorkerError> {
    let url = req.url()?;
    let path = url.path();
    let method = req.method();

    if path == "/health" {
        return Ok(handlers::health::health());
    }

    let api = path.strip_prefix("/api/").unwrap_or(path);
    let segs: Vec<String> = api
        .split('/')
        .filter(|s| !s.is_empty())
        .map(decode_path)
        .collect();

    if segs != ["auth", "verify"] {
        handlers::auth::require_auth(&req, &env).await?;
    }

    match (method.clone(), segs.as_slice()) {
        (Method::Get, [a, b]) if a == "auth" && b == "verify" => {
            handlers::auth::verify(req, env).await
        }
        (Method::Post, [a]) if a == "tokens" => handlers::tokens::create(req, env).await,
        (Method::Get, [a]) if a == "tokens" => handlers::tokens::list(req, env).await,
        (Method::Delete, [a, id]) if a == "tokens" => {
            handlers::tokens::revoke(req, env, id).await
        }
        (Method::Get, [a]) if a == "tasks" => handlers::tasks::list(req, env).await,
        (Method::Post, [a]) if a == "tasks" => handlers::tasks::create(req, env).await,
        (Method::Get, [a, b]) if a == "tasks" && b == "similar" => {
            handlers::memory::similar_tasks(req, env).await
        }
        (Method::Get, [a, id]) if a == "tasks" => handlers::tasks::get(req, env, id).await,
        (Method::Patch, [a, id]) if a == "tasks" => {
            handlers::tasks::update(req, env, id).await
        }
        (Method::Put, [a, id]) if a == "tasks" => {
            handlers::tasks::replace(req, env, id).await
        }
        (Method::Delete, [a, id]) if a == "tasks" => {
            handlers::tasks::delete(req, env, id).await
        }
        (Method::Get, [a, id, b]) if a == "tasks" && b == "progress" => {
            handlers::progress::get_task_progress(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "tasks" && b == "split" => {
            handlers::progress::split_task(req, env, id).await
        }
        (Method::Get, [a, id, b]) if a == "tasks" && b == "comments" => {
            handlers::comments::list(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "tasks" && b == "comments" => {
            handlers::comments::create_user(req, env, id).await
        }
        (Method::Post, [a, id, b, c]) if a == "tasks" && b == "comments" && c == "agent" => {
            handlers::comments::create_agent(req, env, id).await
        }
        (Method::Delete, [a, id]) if a == "comments" => {
            handlers::comments::delete(req, env, id).await
        }
        (Method::Post, [a]) if a == "work-sessions" => {
            handlers::progress::create_work_session(req, env).await
        }
        (Method::Get, [a]) if a == "work-sessions" => {
            handlers::progress::list_work_sessions(req, env).await
        }
        (Method::Post, [a, b]) if a == "work-sessions" && b == "undo" => {
            handlers::progress::undo_work_session(req, env).await
        }
        (Method::Get, [a, id]) if a == "work-sessions" => {
            handlers::progress::get_work_session(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "work-sessions" && b == "pause" => {
            handlers::progress::pause_work_session(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "work-sessions" && b == "complete" => {
            handlers::progress::complete_work_session(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "work-sessions" && b == "progress" => {
            handlers::progress::record_work_session_progress(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "work-sessions" && b == "attach" => {
            handlers::progress::attach_work_session(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "work-sessions" && b == "convert" => {
            handlers::progress::convert_work_session(req, env, id).await
        }
        (Method::Get, [a]) if a == "habits" => handlers::habits::list(req, env).await,
        (Method::Post, [a]) if a == "habits" => handlers::habits::create(req, env).await,
        (Method::Get, [a, b]) if a == "habits" && b == "scheduled-spans" => {
            handlers::habits::list_all_scheduled_spans(req, env).await
        }
        (Method::Get, [a, b]) if a == "habits" && b == "steps" => {
            handlers::habits::list_all_steps(req, env).await
        }
        (Method::Get, [a, id]) if a == "habits" => handlers::habits::get(req, env, id).await,
        (Method::Patch, [a, id]) if a == "habits" => {
            handlers::habits::update(req, env, id).await
        }
        (Method::Put, [a, id]) if a == "habits" => {
            handlers::habits::replace(req, env, id).await
        }
        (Method::Delete, [a, id]) if a == "habits" => {
            handlers::habits::delete(req, env, id).await
        }
        (Method::Get, [a, id, b]) if a == "habits" && b == "scheduled-spans" => {
            handlers::habits::list_scheduled_spans(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "habits" && b == "scheduled-spans" => {
            handlers::habits::create_scheduled_span(req, env, id).await
        }
        (Method::Delete, [a, id, b, span_id])
            if a == "habits" && b == "scheduled-spans" =>
        {
            handlers::habits::delete_scheduled_span(req, env, id, span_id).await
        }
        (Method::Get, [a, id, b]) if a == "habits" && b == "steps" => {
            handlers::habits::list_steps(req, env, id).await
        }
        (Method::Put, [a, id, b]) if a == "habits" && b == "steps" => {
            handlers::habits::replace_steps(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "habits" && b == "estimate" => {
            handlers::habits::apply_estimate(req, env, id).await
        }
        (Method::Get, [a]) if a == "schedule" => handlers::schedule::get(req, env).await,
        (Method::Post, [a, b]) if a == "schedule" && b == "save" => {
            handlers::schedule::save(req, env).await
        }
        (Method::Delete, [a]) if a == "schedule" => handlers::schedule::clear(req, env).await,
        (Method::Get, [a]) if a == "settings" => handlers::settings::get(req, env).await,
        (Method::Put, [a]) if a == "settings" => handlers::settings::update(req, env).await,
        (Method::Post, [a]) if a == "devices" => handlers::devices::create(req, env).await,
        (Method::Get, [a]) if a == "devices" => handlers::devices::list(req, env).await,
        (Method::Get, [a, id]) if a == "devices" => handlers::devices::get(req, env, id).await,
        (Method::Patch, [a, id]) if a == "devices" => {
            handlers::devices::update(req, env, id).await
        }
        (Method::Delete, [a, id]) if a == "devices" => {
            handlers::devices::delete(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "devices" && b == "heartbeat" => {
            handlers::devices::heartbeat(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "devices" && b == "lease" => {
            handlers::devices::lease(req, env, id).await
        }
        (Method::Get, [a, id, b]) if a == "devices" && b == "resident" => {
            handlers::devices::resident(req, env, id).await
        }
        (Method::Get, [a, id, b]) if a == "devices" && b == "speech" => {
            handlers::devices::speech(req, env, id).await
        }
        (Method::Get, [a]) if a == "skills" => handlers::skills::list(req, env).await,
        (Method::Post, [a]) if a == "skills" => handlers::skills::create(req, env).await,
        (Method::Get, [a, id]) if a == "skills" => handlers::skills::get(req, env, id).await,
        (Method::Patch, [a, id]) if a == "skills" => {
            handlers::skills::update(req, env, id).await
        }
        (Method::Delete, [a, id]) if a == "skills" => {
            handlers::skills::delete(req, env, id).await
        }
        (Method::Post, [a]) if a == "memory" => handlers::memory::create(req, env).await,
        (Method::Get, [a, b]) if a == "memory" && b == "search" => {
            handlers::memory::search(req, env).await
        }
        (Method::Post, [a, b]) if a == "memory" && b == "inject" => {
            handlers::memory::inject(req, env).await
        }
        (Method::Get, [a, id]) if a == "memory" => handlers::memory::get(req, env, id).await,
        (Method::Patch, [a, id]) if a == "memory" => {
            handlers::memory::update(req, env, id).await
        }
        (Method::Delete, [a, id]) if a == "memory" => {
            handlers::memory::delete(req, env, id).await
        }
        (Method::Get, [a]) if a == "events" => handlers::events::list(req, env).await,
        (Method::Post, [a]) if a == "events" => handlers::events::insert(req, env).await,
        (Method::Post, [a, b]) if a == "events" && b == "commit" => {
            handlers::events::commit(req, env).await
        }
        (Method::Get, [a, b]) if a == "events" && b == "revision" => {
            handlers::events::revision(req, env).await
        }
        (Method::Get, [a, b]) if a == "events" && b == "snapshot" => {
            handlers::events::snapshot(req, env).await
        }
        (Method::Post, [a, b]) if a == "events" && b == "evaluate" => {
            handlers::events::evaluate(req, env).await
        }
        (Method::Post, [a, b]) if a == "coverage" && b == "confirmations" => {
            handlers::coverage::create_confirmation(req, env).await
        }
        (Method::Post, [a, b]) if a == "coverage" && b == "unsettled-intervals" => {
            handlers::coverage::create_unsettled_interval(req, env).await
        }
        (Method::Post, [a, b]) if a == "coverage" && b == "settle" => {
            handlers::coverage::settle(req, env).await
        }
        (Method::Post, [a, id, b]) if a == "events" && b == "claim" => {
            handlers::events::claim(req, env, id).await
        }
        (Method::Post, [a, id, b]) if a == "events" && b == "acknowledge" => {
            handlers::events::acknowledge(req, env, id).await
        }
        (Method::Put, [a, id, b]) if a == "events" && b == "state" => {
            handlers::events::update_state(req, env, id).await
        }
        (Method::Get, [a, b]) if a == "sync" && b == "settings" => {
            handlers::sync::get_settings(req, env).await
        }
        (Method::Put, [a, b]) if a == "sync" && b == "settings" => {
            handlers::sync::update_settings(req, env).await
        }
        (Method::Get, [a, b]) if a == "sync" && b == "mappings" => {
            handlers::sync::list_mappings(req, env).await
        }
        (Method::Post, [a, b]) if a == "sync" && b == "mappings" => {
            handlers::sync::upsert_mappings(req, env).await
        }
        (Method::Delete, [a, b]) if a == "sync" && b == "mappings" => {
            handlers::sync::delete_mappings(req, env).await
        }
        _ => Err(crate::error::WorkerError::NotFound(format!(
            "{} {}",
            method, path
        ))),
    }
}
