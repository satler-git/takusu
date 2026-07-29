use std::str::FromStr;

use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::models::{CreateTask, UpdateTask};
use crate::util::parse_boolish;
use crate::validate::{validate_minutes, validate_title};
use takusu_storage::{Storage, TaskQuery};
use takusu_types::TaskStatusFilter;

pub async fn list(req: Request, env: Env) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let mut status: Option<TaskStatusFilter> = None;
    let mut from: Option<takusu_types::Timestamp> = None;
    let mut until: Option<takusu_types::Timestamp> = None;
    let mut no_overdue: Option<bool> = None;
    let mut habit_id: Option<String> = None;
    let mut ical_uid: Option<String> = None;
    let mut q: Option<String> = None;
    let mut limit: Option<i64> = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "status" => {
                status = Some(
                    TaskStatusFilter::from_str(v.as_ref())
                        .map_err(|e| WorkerError::BadRequest(format!("invalid status: {e}")))?,
                );
            }
            "from" => {
                from = Some(
                    takusu_types::Timestamp::from_str(v.as_ref())
                        .map_err(|e| WorkerError::BadRequest(format!("invalid from: {e}")))?,
                );
            }
            "until" => {
                until = Some(
                    takusu_types::Timestamp::from_str(v.as_ref())
                        .map_err(|e| WorkerError::BadRequest(format!("invalid until: {e}")))?,
                );
            }
            "no_overdue" => {
                if parse_boolish(&v) {
                    no_overdue = Some(true);
                }
            }
            "habit_id" => {
                habit_id = Some(v.into_owned());
            }
            "ical_uid" => {
                ical_uid = Some(v.into_owned());
            }
            "q" => {
                q = Some(v.into_owned());
            }
            "limit" => {
                if let Ok(n) = v.parse::<i64>() {
                    limit = Some(n);
                }
            }
            _ => continue,
        }
    }

    let query = TaskQuery {
        status,
        from,
        until,
        no_overdue,
        habit_id,
        ical_uid,
        q,
        limit,
    };

    let store = storage(&env)?;
    let rows = store.list_tasks(&query).await?;
    json_ok(&rows)
}

pub async fn create(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let body: CreateTask = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_title(&body.title)?;
    let store = storage(&env)?;
    let row = store.create_task(&body).await?;
    json_created(&row)
}

pub async fn get(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let row = store.get_task(id).await?;
    json_ok(&row)
}

pub async fn update(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: UpdateTask = parse_json(&mut req).await?;
    if let Some(avg) = body.avg_minutes {
        validate_minutes(avg, body.sigma_minutes)?;
    } else if let Some(sigma) = body.sigma_minutes {
        validate_minutes(0, Some(sigma))?;
    }
    if let Some(ref t) = body.title {
        validate_title(t)?;
    }
    let store = storage(&env)?;
    let row = store.update_task(id, &body).await?;
    json_ok(&row)
}

pub async fn replace(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: CreateTask = parse_json(&mut req).await?;
    validate_minutes(body.avg_minutes, body.sigma_minutes)?;
    validate_title(&body.title)?;
    let store = storage(&env)?;
    let row = store.replace_task(id, &body).await?;
    json_ok(&row)
}

pub async fn delete(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    store.delete_task(id).await?;
    Ok(Response::empty()?)
}
