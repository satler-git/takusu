use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_ok, parse_json};
use crate::models::{RecordProgress, SplitTask};
use takusu_contracts::Storage;

fn operation_id(req: &Request) -> Option<String> {
    req.headers()
        .get("Idempotency-Key")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("idempotency-key").ok().flatten())
}

pub async fn start_task_work(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let task = store.start_task_work(id, op_id.as_deref()).await?;
    json_ok(&task)
}

pub async fn pause_task_work(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let task = store.pause_task_work(id, op_id.as_deref()).await?;
    json_ok(&task)
}

pub async fn record_progress(
    mut req: Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: RecordProgress = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let result = store
        .record_progress(id, &body, op_id.as_deref())
        .await?;
    json_ok(&result)
}

pub async fn complete_task_work(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let task = store.complete_task_work(id, op_id.as_deref()).await?;
    json_ok(&task)
}

pub async fn get_task_progress(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let progress = store.get_task_progress(id).await?;
    json_ok(&progress)
}

pub async fn split_task(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: SplitTask = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let result = store.split_task(id, &body, op_id.as_deref()).await?;
    json_ok(&result)
}
