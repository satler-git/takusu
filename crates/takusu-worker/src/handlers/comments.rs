use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::json_created;
use crate::handlers::tokens::{json_ok, parse_json};
use takusu_contracts::{CreateComment, Storage};
use takusu_types::CommentAuthor;

fn operation_id(req: &Request) -> Option<String> {
    req.headers()
        .get("Idempotency-Key")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("idempotency-key").ok().flatten())
}

fn validate_create(body: &CreateComment) -> Result<(), WorkerError> {
    const MAX_COMMENT_CHARS: usize = 4096;
    if body.content.trim().is_empty() {
        return Err(WorkerError::BadRequest(
            "comment content must not be empty".into(),
        ));
    }
    if body.content.chars().count() > MAX_COMMENT_CHARS {
        return Err(WorkerError::BadRequest(format!(
            "comment content must be at most {MAX_COMMENT_CHARS} characters"
        )));
    }
    Ok(())
}

async fn create(req: &mut Request, env: &Env, task_id: &str, author: CommentAuthor) -> Result<Response, WorkerError> {
    let body: CreateComment = parse_json(req).await?;
    validate_create(&body)?;
    let op = operation_id(req);
    let store = storage(env)?;
    let row = store
        .create_comment(task_id, author, &body.content, op.as_deref())
        .await?;
    json_created(&row)
}

pub async fn create_user(mut req: Request, env: Env, task_id: &str) -> Result<Response, WorkerError> {
    create(&mut req, &env, task_id, CommentAuthor::User).await
}

pub async fn create_agent(mut req: Request, env: Env, task_id: &str) -> Result<Response, WorkerError> {
    create(&mut req, &env, task_id, CommentAuthor::Agent).await
}

pub async fn list(_req: Request, env: Env, task_id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows = store.list_comments(task_id).await?;
    json_ok(&rows)
}

/// Delete a comment (user operation, invariant 4). Any valid token may delete
/// by id; "user-only" is enforced by convention (the agent has no delete tool)
/// until principal-scoped tokens land (design invariant 2 / WI-7).
pub async fn delete(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    store.delete_comment(id).await?;
    Ok(Response::empty()?)
}