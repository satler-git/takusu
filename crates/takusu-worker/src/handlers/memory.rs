use std::str::FromStr;

use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::storage;
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::memory;
use crate::models::{CreateMemory, UpdateMemory};
use takusu_contracts::{
    MemoryInjectionQuery, MemoryQuery, SimilarTaskQuery, Storage,
};
use takusu_types::{EnumLabel, MemoryKind, SubjectType};

fn operation_id(req: &Request) -> Option<String> {
    req.headers()
        .get("Idempotency-Key")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("idempotency-key").ok().flatten())
}

fn validate_create(body: &CreateMemory) -> Result<(), WorkerError> {
    if !matches!(
        body.kind,
        MemoryKind::ProperNoun | MemoryKind::Fact
    ) {
        return Err(WorkerError::BadRequest(
            "kind must be 'proper_noun' or 'fact'".into(),
        ));
    }
    memory::normalize_key(&body.key)
        .map_err(|e| WorkerError::BadRequest(format!("invalid key: {e}")))?;
    // Bounding the raw key (not just the normalized length) keeps injected
    // keys small in the system prompt even when they are whitespace-heavy
    // (WI-4 / #1003).
    if body.key.chars().count() > takusu_search::memory::MAX_KEY_SCALARS {
        return Err(WorkerError::BadRequest("key too long".into()));
    }
    memory::normalize_content(&body.content)
        .map_err(|e| WorkerError::BadRequest(format!("invalid content: {e}")))?;
    if body
        .subject_type
        .as_ref()
        .is_some_and(|s| s.as_str().len() > 64)
    {
        return Err(WorkerError::BadRequest("subject_type too long".into()));
    }
    if body.subject_id.as_ref().is_some_and(|s| s.len() > 64) {
        return Err(WorkerError::BadRequest("subject_id too long".into()));
    }
    Ok(())
}

fn validate_update(body: &UpdateMemory) -> Result<(), WorkerError> {
    let content = body
        .content
        .as_ref()
        .ok_or_else(|| WorkerError::BadRequest("content is required".into()))?;
    if content.is_empty() {
        return Err(WorkerError::BadRequest("content is required".into()));
    }
    memory::normalize_content(content)
        .map_err(|e| WorkerError::BadRequest(format!("invalid content: {e}")))?;
    Ok(())
}

pub async fn create(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let body: CreateMemory = parse_json(&mut req).await?;
    validate_create(&body)?;
    let op = operation_id(&req);
    let store = storage(&env)?;
    let row = store.create_memory(&body, op.as_deref()).await?;
    json_created(&row)
}

pub async fn get(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let row = store.get_memory(id).await?;
    json_ok(&row)
}

pub async fn update(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: UpdateMemory = parse_json(&mut req).await?;
    validate_update(&body)?;
    let op = operation_id(&req);
    let store = storage(&env)?;
    let row = store.update_memory(id, &body, op.as_deref()).await?;
    json_ok(&row)
}

pub async fn delete(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let observed_revision: i64 = url
        .query_pairs()
        .find(|(k, _)| k == "observed_revision")
        .and_then(|(_, v)| v.parse().ok())
        .ok_or_else(|| WorkerError::BadRequest("observed_revision is required".into()))?;
    let op = operation_id(&req);
    let store = storage(&env)?;
    store
        .delete_memory(id, observed_revision, op.as_deref())
        .await?;
    Ok(Response::empty()?)
}

pub async fn search(req: Request, env: Env) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let mut q = None;
    let mut kind = None;
    let mut subject_type = None;
    let mut subject_id = None;
    let mut limit: Option<i64> = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "q" => q = Some(v.into_owned()),
            "kind" => {
                kind = Some(
                    MemoryKind::from_str(v.as_ref())
                        .map_err(|e| WorkerError::BadRequest(format!("invalid kind: {e}")))?,
                );
            }
            "subject_type" => {
                subject_type =
                    Some(SubjectType::from_str(v.as_ref()).map_err(|e| {
                        WorkerError::BadRequest(format!("invalid subject_type: {e}"))
                    })?);
            }
            "subject_id" => subject_id = Some(v.into_owned()),
            "limit" => {
                if let Ok(n) = v.parse::<i64>() {
                    limit = Some(n);
                }
            }
            _ => {}
        }
    }
    let q = q.ok_or_else(|| WorkerError::BadRequest("q is required".into()))?;
    let query = MemoryQuery {
        q,
        kind,
        subject_type,
        subject_id,
        limit,
    };
    let store = storage(&env)?;
    let rows = store.search_memories(&query).await?;
    json_ok(&rows)
}

pub async fn inject(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let query: MemoryInjectionQuery = parse_json(&mut req).await?;
    if query.text.trim().is_empty() {
        return Err(WorkerError::BadRequest("text is required".into()));
    }
    let store = storage(&env)?;
    let result = store.injectable_memories(&query).await?;
    json_ok(&result)
}

pub async fn similar_tasks(req: Request, env: Env) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let mut title = String::new();
    let mut limit: Option<i64> = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "q" => title = v.into_owned(),
            "limit" => {
                if let Ok(n) = v.parse::<i64>() {
                    limit = Some(n);
                }
            }
            _ => {}
        }
    }
    if title.is_empty() {
        return Err(WorkerError::BadRequest("q is required".into()));
    }
    // Validate the title normalizes before delegating to storage.
    memory::normalize_text(&title, Some(memory::MAX_QUERY_SCALARS))
        .map_err(|e| WorkerError::BadRequest(format!("invalid title: {e}")))?;
    let query = SimilarTaskQuery { title, limit };
    let store = storage(&env)?;
    let rows = store.find_similar_tasks(&query).await?;
    json_ok(&rows)
}
