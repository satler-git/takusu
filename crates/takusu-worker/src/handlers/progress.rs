use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_ok, parse_json};
use crate::models::SplitTask;
use takusu_contracts::{
    AttachWorkSession, ConvertWorkSession, RecordWorkSessionProgress, StartWorkSession, Storage,
    UndoWorkSession,
};

fn operation_id(req: &Request) -> Option<String> {
    req.headers()
        .get("Idempotency-Key")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("idempotency-key").ok().flatten())
}

pub async fn create_work_session(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let body: StartWorkSession = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let session = store.start_work_session(&body, op_id.as_deref()).await?;
    json_ok(&session)
}

pub async fn list_work_sessions(req: Request, env: Env) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let task_id = url
        .query_pairs()
        .find(|(k, _)| k == "task_id")
        .map(|(_, v)| v.into_owned());
    let store = storage(&env)?;
    let sessions = store.list_work_sessions(task_id.as_deref()).await?;
    json_ok(&sessions)
}

pub async fn get_work_session(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let session = store.get_work_session(id).await?;
    json_ok(&session)
}

pub async fn pause_work_session(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let session = store.pause_work_session(id, op_id.as_deref()).await?;
    json_ok(&session)
}

pub async fn complete_work_session(
    req: Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let session = store.complete_work_session(id, op_id.as_deref()).await?;
    json_ok(&session)
}

pub async fn record_work_session_progress(
    mut req: Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: RecordWorkSessionProgress = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let result = store
        .record_work_session_progress(id, &body, op_id.as_deref())
        .await?;
    json_ok(&result)
}

pub async fn attach_work_session(
    mut req: Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: AttachWorkSession = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let session = store
        .attach_work_session(id, &body, op_id.as_deref())
        .await?;
    json_ok(&session)
}

pub async fn convert_work_session(
    mut req: Request,
    env: Env,
    id: &str,
) -> Result<Response, WorkerError> {
    let body: ConvertWorkSession = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let task = store
        .convert_work_session(id, &body, op_id.as_deref())
        .await?;
    json_ok(&task)
}

pub async fn split_task(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: SplitTask = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let result = store.split_task(id, &body, op_id.as_deref()).await?;
    json_ok(&result)
}

pub async fn get_task_progress(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let progress = store.get_task_progress(id).await?;
    json_ok(&progress)
}

pub async fn undo_work_session(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let body: UndoWorkSession = parse_json(&mut req).await?;
    let op_id = operation_id(&req);
    let store = storage(&env)?;
    let result = store.undo_work_session(&body, op_id.as_deref()).await?;
    json_ok(&result)
}
