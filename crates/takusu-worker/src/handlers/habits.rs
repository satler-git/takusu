use worker::Env;
use worker::Response;

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::models::{
    ApplyHabitEstimateRequest, CreateHabit, CreateHabitScheduledSpan, HabitDetail,
    UpdateHabit,
};
use crate::validate::{validate_minutes, validate_recurrence, validate_scheduled_span_dates};
use takusu_contracts::{Storage, Validate};

pub async fn list(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_habits().await?;
    json_ok(&rows)
}

pub async fn create(mut req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let body: CreateHabit = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_recurrence(&body.recurrence)?;
    let store = storage(&env)?;
    let row = store.create_habit(&body).await?;
    json_created(&row)
}

pub async fn get(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let habit = store.get_habit(id).await?;
    let steps = store.list_habit_steps(id).await?;
    json_ok(&HabitDetail { habit, steps })
}

pub async fn update(mut req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: UpdateHabit = parse_json(&mut req).await?;
    if let Some(avg) = body.avg_minutes {
        validate_minutes(avg, body.sigma_minutes)?;
    } else if let Some(sigma) = body.sigma_minutes {
        validate_minutes(0, Some(sigma))?;
    }
    if let Some(ref recurrence) = body.recurrence {
        validate_recurrence(recurrence)?;
    }
    let store = storage(&env)?;
    let row = store.update_habit(id, &body).await?;
    json_ok(&row)
}

pub async fn replace(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: CreateHabit = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_recurrence(&body.recurrence)?;
    let store = storage(&env)?;
    let row = store.replace_habit(id, &body).await?;
    json_ok(&row)
}

pub async fn delete(_req: worker::Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    store.delete_habit(id).await?;
    Ok(Response::empty()?)
}

// ── Habit scheduled spans (#303 / #503) ────────────────────────────────

pub async fn list_scheduled_spans(
    _req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_habit_scheduled_spans(id).await?;
    json_ok(&rows)
}

pub async fn list_all_scheduled_spans(
    _req: worker::Request,
    env: Env,
) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_all_habit_scheduled_spans().await?;
    json_ok(&rows)
}

pub async fn create_scheduled_span(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: CreateHabitScheduledSpan = parse_json(&mut req).await?;
    validate_scheduled_span_dates(&body.start_date, &body.end_date)?;
    let store = storage(&env)?;
    let row = store.create_habit_scheduled_span(id, &body).await?;
    json_created(&row)
}

pub async fn delete_scheduled_span(
    _req: worker::Request,
    env: Env,
    id: &str,
    span_id: &str,
) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    store.delete_habit_scheduled_span(id, span_id).await?;
    Ok(Response::empty()?)
}

// ── Habit steps (#95) ────────────────────────────────────────────────────

pub async fn list_steps(
    _req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_habit_steps(id).await?;
    json_ok(&rows)
}

pub async fn list_all_steps(_req: worker::Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_all_habit_steps().await?;
    json_ok(&rows)
}

pub async fn replace_steps(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: Vec<crate::models::HabitStepInput> = parse_json(&mut req).await?;
    body.validate()?;
    let store = storage(&env)?;
    let rows = store.replace_habit_steps(id, &body).await?;
    json_ok(&rows)
}

pub async fn apply_estimate(
    mut req: worker::Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: ApplyHabitEstimateRequest = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, Some(body.sigma_minutes))?;
    let store = storage(&env)?;
    store
        .apply_habit_estimate(id, body.avg_minutes, body.sigma_minutes, &body.steps)
        .await?;
    Ok(Response::empty()?)
}
