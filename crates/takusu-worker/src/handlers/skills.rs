use worker::{Env, Request, Response};

use crate::error::WorkerError;
use crate::handlers::auth::{is_root, storage};
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::models::{CreateSkill, SkillRow, UpdateSkill};
use takusu_storage::Storage;

fn validate_slug(slug: &str) -> Result<(), WorkerError> {
    if slug.is_empty() || slug.len() > 64 {
        return Err(WorkerError::BadRequest(
            "slug must be 1..64 characters".into(),
        ));
    }
    if slug.starts_with('.') || slug.contains('/') || slug.contains("..") {
        return Err(WorkerError::BadRequest(
            "slug must not contain path components".into(),
        ));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(WorkerError::BadRequest(
            "slug must contain only ASCII letters, digits, '-', '_'".into(),
        ));
    }
    Ok(())
}

fn validate_create(body: &CreateSkill) -> Result<(), WorkerError> {
    validate_slug(&body.slug)?;
    if body.name.is_empty() || body.name.len() > 100 {
        return Err(WorkerError::BadRequest(
            "name must be 1..100 characters".into(),
        ));
    }
    if body.description.len() > 500 {
        return Err(WorkerError::BadRequest(
            "description must be at most 500 characters".into(),
        ));
    }
    if body.body.is_empty() || body.body.len() > 64 * 1024 {
        return Err(WorkerError::BadRequest(
            "body must be 1..65536 characters".into(),
        ));
    }
    Ok(())
}

pub async fn list(_req: Request, env: Env) -> Result<Response, WorkerError> {
    let store = storage(&env)?;
    let rows: Vec<SkillRow> = store.list_skills().await?;
    json_ok(&rows)
}

pub async fn create(mut req: Request, env: Env) -> Result<Response, WorkerError> {
    let body: CreateSkill = parse_json(&mut req).await?;
    validate_create(&body)?;
    if body.built_in == Some(true) && !is_root(&req, &env)? {
        return Err(WorkerError::Unauthorized);
    }
    let store = storage(&env)?;
    match store.get_skill(&body.slug).await {
        Err(takusu_storage::StorageError::NotFound(_)) => {}
        Ok(_) => {
            return Err(WorkerError::Conflict(format!(
                "skill {} already exists",
                body.slug
            )));
        }
        Err(e) => return Err(e.into()),
    }
    let row = store.create_skill(&body).await?;
    json_created(&row)
}

pub async fn get(_req: Request, env: Env, slug: &str) -> Result<Response, WorkerError> {
    validate_slug(slug)?;
    let store = storage(&env)?;
    let row: SkillRow = store.get_skill(slug).await?;
    json_ok(&row)
}

pub async fn update(mut req: Request, env: Env, slug: &str) -> Result<Response, WorkerError> {
    let body: UpdateSkill = parse_json(&mut req).await?;
    validate_slug(slug)?;
    if body
        .name
        .as_ref()
        .is_some_and(|n| n.is_empty() || n.len() > 100)
    {
        return Err(WorkerError::BadRequest(
            "name must be 1..100 characters".into(),
        ));
    }
    if body.description.as_ref().is_some_and(|d| d.len() > 500) {
        return Err(WorkerError::BadRequest(
            "description must be at most 500 characters".into(),
        ));
    }
    if body
        .body
        .as_ref()
        .is_some_and(|b| b.is_empty() || b.len() > 64 * 1024)
    {
        return Err(WorkerError::BadRequest("body length is invalid".into()));
    }

    let store = storage(&env)?;
    let existing = store.get_skill(slug).await?;
    if existing.built_in {
        return Err(WorkerError::Conflict(format!(
            "built-in skill {slug} cannot be edited"
        )));
    }

    let row = store.update_skill(slug, &body).await?;
    json_ok(&row)
}

pub async fn delete(_req: Request, env: Env, slug: &str) -> Result<Response, WorkerError> {
    validate_slug(slug)?;
    let store = storage(&env)?;
    let existing = store.get_skill(slug).await?;
    if existing.built_in {
        return Err(WorkerError::Conflict(format!(
            "built-in skill {slug} cannot be deleted"
        )));
    }
    store.delete_skill(slug).await?;
    Ok(Response::empty()?)
}
