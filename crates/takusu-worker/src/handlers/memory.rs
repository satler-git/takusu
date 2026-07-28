use takusu_util::EnumLabel;
use wasm_bindgen::JsValue;
use worker::{Env, Request, Response};

use crate::auth;
use crate::error::WorkerError;
use crate::handlers::auth::db;
use crate::handlers::d1::safe_all;
use crate::handlers::tokens::{json_created, json_ok, parse_json};
use crate::memory;
use crate::models::{CreateMemory, MemoryRow, SimilarTaskRow, UpdateMemory};

const MEMORY_COLS: &str = "id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at";

fn memory_select() -> String {
    format!("SELECT {MEMORY_COLS} FROM memories")
}

fn operation_id(req: &Request) -> Option<String> {
    req.headers()
        .get("Idempotency-Key")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("idempotency-key").ok().flatten())
}

fn request_hash_create(body: &CreateMemory, operation_id: Option<&str>) -> String {
    let payload = serde_json::to_string(body).unwrap_or_default();
    auth::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

fn request_hash_update(id: &str, body: &UpdateMemory, operation_id: Option<&str>) -> String {
    let payload = serde_json::to_string(body).unwrap_or_default();
    auth::hash_token(&format!(
        "update:{id}:{payload}:{}",
        operation_id.unwrap_or("")
    ))
}

fn request_hash_delete(id: &str, observed_revision: i64, operation_id: Option<&str>) -> String {
    auth::hash_token(&format!(
        "delete:{id}:{observed_revision}:{}",
        operation_id.unwrap_or("")
    ))
}

async fn resolve_task_id_for_memory(
    database: &worker::D1Database,
    id: &str,
) -> Result<String, WorkerError> {
    #[derive(serde::Deserialize)]
    struct TaskIdRow {
        id: String,
    }

    // Allow agents to pass display ids with a leading `#`, e.g. `#42`.
    let stripped = id.strip_prefix('#').unwrap_or(id);

    // `h{habit_display_id}#{task_display_id}` → habit task lookup (#380).
    if let Some(rest) = stripped.strip_prefix(['h', 'H'])
        && let Some((hdisp, tdisp)) = rest.split_once('#')
        && let (Ok(hnum), Ok(tnum)) = (hdisp.parse::<i64>(), tdisp.parse::<i64>())
    {
        let stmt = database.prepare(
            "SELECT t.id FROM tasks t JOIN habits h ON t.habit_id = h.id WHERE h.display_id = ?1 AND t.display_id = ?2",
        );
        let rows: Vec<TaskIdRow> = safe_all(&stmt.bind(&[
            JsValue::from_f64(hnum as f64),
            JsValue::from_f64(tnum as f64),
        ])?)
        .await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::BadRequest(format!("task {id} not found")));
    }

    // Numeric input → display_id lookup for non-habit tasks only (#380).
    if let Ok(num) = stripped.parse::<i64>() {
        let stmt =
            database.prepare("SELECT id FROM tasks WHERE display_id = ?1 AND habit_id IS NULL");
        let rows: Vec<TaskIdRow> =
            safe_all(&stmt.bind(&[JsValue::from_f64(num as f64)])?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::BadRequest(format!("task {id} not found")));
    }

    // Full UUID — verify it exists. UUID prefixes are not accepted (#1251).
    if id.contains('-') {
        let stmt = database.prepare("SELECT id FROM tasks WHERE id = ?1");
        let rows: Vec<TaskIdRow> = safe_all(&stmt.bind(&[JsValue::from_str(id)])?).await?;
        return rows
            .into_iter()
            .next()
            .map(|r| r.id)
            .ok_or_else(|| WorkerError::BadRequest(format!("task {id} not found")));
    }

    // Anything else (e.g. a UUID prefix) is not a valid reference (#1251).
    Err(WorkerError::BadRequest(format!("task {id} not found")))
}

async fn select_one(database: &worker::D1Database, id: &str) -> Result<MemoryRow, WorkerError> {
    let stmt = database.prepare(format!("{select} WHERE id = ?1", select = memory_select()));
    let rows: Vec<MemoryRow> = safe_all(&stmt.bind(&[JsValue::from_str(id)])?).await?;
    rows.into_iter()
        .next()
        .ok_or_else(|| WorkerError::NotFound(format!("memory {id} not found")))
}

async fn check_idempotency(
    database: &worker::D1Database,
    op_id: &str,
    expected_hash: &str,
) -> Result<Option<String>, WorkerError> {
    let stmt = database.prepare(
        "SELECT request_hash, response_json FROM memory_operations WHERE operation_id = ?1",
    );
    #[derive(serde::Deserialize)]
    struct OpRow {
        request_hash: String,
        response_json: String,
    }
    let rows: Vec<OpRow> = safe_all(&stmt.bind(&[JsValue::from_str(op_id)])?).await?;
    if let Some(row) = rows.into_iter().next() {
        if row.request_hash != expected_hash {
            return Err(WorkerError::Conflict(
                "idempotency key reused with different request".into(),
            ));
        }
        return Ok(Some(row.response_json));
    }
    Ok(None)
}

async fn record_operation(
    database: &worker::D1Database,
    op_id: &str,
    request_hash: &str,
    response_json: &str,
) -> Result<(), WorkerError> {
    let stmt = database.prepare(
        "INSERT INTO memory_operations (operation_id, request_hash, response_json, created_at) VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
    );
    stmt.bind(&[
        JsValue::from_str(op_id),
        JsValue::from_str(request_hash),
        JsValue::from_str(response_json),
    ])?
    .run()
    .await
    .map_err(WorkerError::Worker)?;
    Ok(())
}

fn validate_create(body: &CreateMemory) -> Result<(), WorkerError> {
    if !matches!(
        body.kind,
        takusu_util::MemoryKind::ProperNoun
            | takusu_util::MemoryKind::Fact
            | takusu_util::MemoryKind::TaskNote
    ) {
        return Err(WorkerError::BadRequest(
            "kind must be 'proper_noun', 'fact', or 'task_note'".into(),
        ));
    }
    memory::normalize_key(&body.key)
        .map_err(|e| WorkerError::BadRequest(format!("invalid key: {e}")))?;
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
    if body.kind == takusu_util::MemoryKind::TaskNote {
        if body.subject_type != Some(takusu_util::SubjectType::Task) {
            return Err(WorkerError::BadRequest(
                "task_note requires subject_type='task'".into(),
            ));
        }
        if body.subject_id.as_ref().is_none_or(|s| s.is_empty()) {
            return Err(WorkerError::BadRequest(
                "task_note requires subject_id".into(),
            ));
        }
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
    let database = db(&env)?;

    let normalized_key = memory::normalize_key(&body.key)
        .map_err(|e| WorkerError::BadRequest(format!("invalid key: {e}")))?;
    let normalized_content = memory::normalize_content(&body.content)
        .map_err(|e| WorkerError::BadRequest(format!("invalid content: {e}")))?;
    let subject_type = body.subject_type.unwrap_or_default();
    let subject_id = if body.kind == takusu_util::MemoryKind::TaskNote {
        resolve_task_id_for_memory(&database, &body.subject_id.clone().unwrap_or_default()).await?
    } else {
        body.subject_id.clone().unwrap_or_default()
    };

    let hash = request_hash_create(&body, op.as_deref());
    if let Some(op_id) = op.as_deref()
        && let Some(json) = check_idempotency(&database, op_id, &hash).await?
    {
        let row: MemoryRow = serde_json::from_str(&json)
            .map_err(|e| WorkerError::Internal(format!("corrupt idempotency response: {e}")))?;
        return json_ok(&row);
    }

    let existing = find_existing(
        &database,
        body.kind.as_str(),
        &normalized_key,
        subject_type.as_str(),
        &subject_id,
    )
    .await?;

    if let Some(existing) = existing {
        if body.upsert {
            let row = update_existing(
                &database,
                &existing.id,
                &body.content,
                &normalized_content,
                op.as_deref(),
                &hash,
            )
            .await?;
            return json_ok(&row);
        }
        return Err(WorkerError::Conflict(format!(
            "memory {} already exists",
            body.key
        )));
    }

    let id = uuid::Uuid::now_v7().to_string();
    let insert = database.prepare(
        "INSERT INTO memories (id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'user_confirmed', 1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))"
    );
    let result = insert
        .bind(&[
            JsValue::from_str(&id),
            JsValue::from_str(body.kind.as_str()),
            JsValue::from_str(&body.key),
            JsValue::from_str(&normalized_key),
            JsValue::from_str(&body.content),
            JsValue::from_str(&normalized_content),
            JsValue::from_str(subject_type.as_str()),
            JsValue::from_str(&subject_id),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;
    let meta = result.meta().map_err(WorkerError::Worker)?;
    if meta.and_then(|m| m.rows_written).unwrap_or(0) == 0 {
        return Err(WorkerError::Internal(
            "memory insert did not write a row".into(),
        ));
    }

    let row = select_one(&database, &id).await?;
    if let Some(op_id) = op.as_deref() {
        let response_json = serde_json::to_string(&row)
            .map_err(|e| WorkerError::Internal(format!("serialize response: {e}")))?;
        record_operation(&database, op_id, &hash, &response_json).await?;
    }
    json_created(&row)
}

async fn find_existing(
    database: &worker::D1Database,
    kind: &str,
    normalized_key: &str,
    subject_type: &str,
    subject_id: &str,
) -> Result<Option<MemoryRow>, WorkerError> {
    let stmt = database.prepare(format!(
        "{} WHERE kind = ?1 AND normalized_key = ?2 AND subject_type = ?3 AND subject_id = ?4",
        memory_select()
    ));
    let rows: Vec<MemoryRow> = safe_all(&stmt.bind(&[
        JsValue::from_str(kind),
        JsValue::from_str(normalized_key),
        JsValue::from_str(subject_type),
        JsValue::from_str(subject_id),
    ])?)
    .await?;
    Ok(rows.into_iter().next())
}

async fn update_existing(
    database: &worker::D1Database,
    id: &str,
    content: &str,
    normalized_content: &str,
    op_id: Option<&str>,
    hash: &str,
) -> Result<MemoryRow, WorkerError> {
    let existing = select_one(database, id).await?;
    let new_revision = existing.revision + 1;

    if let Some(op_id) = op_id
        && let Some(json) = check_idempotency(database, op_id, hash).await?
    {
        let row: MemoryRow = serde_json::from_str(&json)
            .map_err(|e| WorkerError::Internal(format!("corrupt idempotency response: {e}")))?;
        return Ok(row);
    }

    let result = database
        .prepare(
            "UPDATE memories SET content = ?1, normalized_content = ?2, revision = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?4 AND revision = ?5",
        )
        .bind(&[
            JsValue::from_str(content),
            JsValue::from_str(normalized_content),
            JsValue::from_f64(new_revision as f64),
            JsValue::from_str(id),
            JsValue::from_f64(existing.revision as f64),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;
    let meta = result.meta().map_err(WorkerError::Worker)?;
    if meta.and_then(|m| m.rows_written).unwrap_or(0) == 0 {
        return Err(WorkerError::Conflict(
            "memory changed after proposal".into(),
        ));
    }
    let row = select_one(database, id).await?;

    if let Some(op_id) = op_id {
        let response_json = serde_json::to_string(&row)
            .map_err(|e| WorkerError::Internal(format!("serialize response: {e}")))?;
        record_operation(database, op_id, hash, &response_json).await?;
    }
    Ok(row)
}

pub async fn get(_req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let database = db(&env)?;
    let row = select_one(&database, id).await?;
    json_ok(&row)
}

pub async fn update(mut req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let body: UpdateMemory = parse_json(&mut req).await?;
    validate_update(&body)?;

    let database = db(&env)?;
    let existing = select_one(&database, id).await?;
    if existing.revision != body.observed_revision {
        return Err(WorkerError::Conflict(
            "memory changed after proposal".into(),
        ));
    }

    let op = operation_id(&req);
    let hash = request_hash_update(id, &body, op.as_deref());
    if let Some(op_id) = op.as_deref()
        && let Some(json) = check_idempotency(&database, op_id, &hash).await?
    {
        let row: MemoryRow = serde_json::from_str(&json)
            .map_err(|e| WorkerError::Internal(format!("corrupt idempotency response: {e}")))?;
        return json_ok(&row);
    }

    let content = body.content.unwrap_or_default();
    let normalized_content = memory::normalize_content(&content)
        .map_err(|e| WorkerError::BadRequest(format!("invalid content: {e}")))?;
    let new_revision = existing.revision + 1;

    let result = database
        .prepare(
            "UPDATE memories SET content = ?1, normalized_content = ?2, revision = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?4 AND revision = ?5",
        )
        .bind(&[
            JsValue::from_str(&content),
            JsValue::from_str(&normalized_content),
            JsValue::from_f64(new_revision as f64),
            JsValue::from_str(id),
            JsValue::from_f64(body.observed_revision as f64),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;
    let meta = result.meta().map_err(WorkerError::Worker)?;
    if meta.and_then(|m| m.rows_written).unwrap_or(0) == 0 {
        return Err(WorkerError::Conflict(
            "memory changed after proposal".into(),
        ));
    }
    let row = select_one(&database, id).await?;

    if let Some(op_id) = op.as_deref() {
        let response_json = serde_json::to_string(&row)
            .map_err(|e| WorkerError::Internal(format!("serialize response: {e}")))?;
        record_operation(&database, op_id, &hash, &response_json).await?;
    }
    json_ok(&row)
}

pub async fn delete(req: Request, env: Env, id: &str) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let observed_revision: i64 = url
        .query_pairs()
        .find(|(k, _)| k == "observed_revision")
        .and_then(|(_, v)| v.parse().ok())
        .ok_or_else(|| WorkerError::BadRequest("observed_revision is required".into()))?;

    let database = db(&env)?;
    let existing = select_one(&database, id).await?;
    if existing.revision != observed_revision {
        return Err(WorkerError::Conflict(
            "memory changed after proposal".into(),
        ));
    }

    let op = operation_id(&req);
    let hash = request_hash_delete(id, observed_revision, op.as_deref());
    if let Some(op_id) = op.as_deref()
        && let Some(_) = check_idempotency(&database, op_id, &hash).await?
    {
        return Ok(Response::empty()?);
    }

    let stmt = database.prepare("DELETE FROM memories WHERE id = ?1 AND revision = ?2");
    let result = stmt
        .bind(&[
            JsValue::from_str(id),
            JsValue::from_f64(observed_revision as f64),
        ])?
        .run()
        .await
        .map_err(WorkerError::Worker)?;

    let meta = result.meta().map_err(WorkerError::Worker)?;
    let affected = meta.and_then(|m| m.rows_written).unwrap_or(0);
    if affected == 0 {
        let current = select_one(&database, id).await?;
        if current.revision != observed_revision {
            return Err(WorkerError::Conflict(
                "memory changed after proposal".into(),
            ));
        }
        return Err(WorkerError::NotFound(format!("memory {id} not found")));
    }

    if let Some(op_id) = op.as_deref() {
        record_operation(&database, op_id, &hash, "null").await?;
    }
    Ok(Response::empty()?)
}

pub async fn search(req: Request, env: Env) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let mut q = None;
    let mut kind = None;
    let mut subject_type = None;
    let mut subject_id = None;
    let mut limit: i64 = 10;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "q" => q = Some(v.into_owned()),
            "kind" => kind = Some(v.into_owned()),
            "subject_type" => subject_type = Some(v.into_owned()),
            "subject_id" => subject_id = Some(v.into_owned()),
            "limit" => {
                if let Ok(n) = v.parse::<i64>() {
                    limit = n.clamp(1, 50);
                }
            }
            _ => {}
        }
    }
    let q = q.ok_or_else(|| WorkerError::BadRequest("q is required".into()))?;
    let terms = memory::tokenize_query(&q)
        .map_err(|e| WorkerError::BadRequest(format!("invalid query: {e}")))?;
    let patterns = memory::memory_like_patterns(&terms);
    let database = db(&env)?;

    let mut sql = String::from("SELECT * FROM memories WHERE ");
    let mut bindings: Vec<JsValue> = Vec::new();
    let mut idx = 1;

    for (i, pat) in patterns.iter().enumerate() {
        if i > 0 {
            sql.push_str(" AND ");
        }
        sql.push_str(&format!(
            "(normalized_key LIKE ?{idx} ESCAPE '\\' OR normalized_content LIKE ?{} ESCAPE '\\')",
            idx + 1
        ));
        bindings.push(JsValue::from_str(pat));
        bindings.push(JsValue::from_str(pat));
        idx += 2;
    }

    if let Some(ref k) = kind {
        let kinds: Vec<&str> = k
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if kinds.is_empty() {
            return json_ok(&Vec::<MemoryRow>::new());
        }
        if kinds.len() == 1 {
            sql.push_str(&format!(" AND kind = ?{idx}"));
            bindings.push(JsValue::from_str(kinds[0]));
            idx += 1;
        } else {
            let kind_count = kinds.len();
            let placeholders: Vec<String> =
                (0..kind_count).map(|i| format!("?{}", idx + i)).collect();
            sql.push_str(&format!(" AND kind IN ({})", placeholders.join(",")));
            for k in kinds {
                bindings.push(JsValue::from_str(k));
            }
            idx += kind_count;
        }
    }
    if let Some(ref st) = subject_type {
        sql.push_str(&format!(" AND subject_type = ?{idx}"));
        bindings.push(JsValue::from_str(st));
        idx += 1;
    }
    if let Some(ref sid) = subject_id {
        sql.push_str(&format!(" AND subject_id = ?{idx}"));
        bindings.push(JsValue::from_str(sid));
        idx += 1;
    }
    sql.push_str(&format!(" LIMIT ?{idx}"));
    bindings.push(JsValue::from_f64(1000.0));

    let stmt = database.prepare(sql).bind(&bindings)?;
    let mut rows: Vec<MemoryRow> = safe_all(&stmt).await?;

    memory::sort_memories(&q, &mut rows);
    rows.truncate(limit as usize);

    json_ok(&rows)
}

pub async fn similar_tasks(req: Request, env: Env) -> Result<Response, WorkerError> {
    let url = req.url()?;
    let mut title = String::new();
    let mut limit: i64 = 10;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "q" => title = v.into_owned(),
            "limit" => {
                if let Ok(n) = v.parse::<i64>() {
                    limit = n.clamp(1, 50);
                }
            }
            _ => {}
        }
    }
    if title.is_empty() {
        return Err(WorkerError::BadRequest("q is required".into()));
    }
    let normalized_title = memory::normalize_text(&title, Some(memory::MAX_QUERY_SCALARS))
        .map_err(|e| WorkerError::BadRequest(format!("invalid title: {e}")))?;

    let database = db(&env)?;

    // Pre-filter candidates in SQL by requiring the normalized title to contain
    // at least one query bigram (a strict superset of non-zero Dice matches, so
    // no true match is dropped — see similar_task_filter_patterns). Rows written
    // before migration 026 have a NULL normalized_title (D1 cannot run NFKC
    // during a migration); matching them against the normalized patterns in SQL
    // would miss case/NFKC variants (D1 LIKE is case-sensitive and not
    // NFKC-aware), so they are fetched unfiltered and scored in Rust, where both
    // sides are normalized correctly. The ORDER BY/LIMIT is a worst-case safety
    // bound on that legacy fetch. Known residual risk: if more than the cap of
    // NULL legacy rows exist and are the most recent, they can push a non-NULL
    // bigram match past the LIMIT; this self-heals as legacy rows are updated
    // (and thus gain a normalized_title), and the cap far exceeds personal-scale
    // data. All patterns are bound as parameters, never interpolated (#942).
    let patterns = memory::similar_task_filter_patterns(&normalized_title);
    if patterns.is_empty() {
        return json_ok(&Vec::<SimilarTaskRow>::new());
    }
    let mut clauses = Vec::with_capacity(patterns.len());
    let mut bindings: Vec<JsValue> = Vec::with_capacity(patterns.len());
    for (i, p) in patterns.iter().enumerate() {
        clauses.push(format!("t.normalized_title LIKE ?{} ESCAPE '\\'", i + 1));
        bindings.push(JsValue::from_str(p));
    }
    let filter = clauses.join(" OR ");
    let cap = memory::SIMILAR_TASK_CANDIDATE_CAP;
    let sql = format!(
        "SELECT t.id AS task_id, t.display_id, t.title, t.avg_minutes, t.sigma_minutes, tam.actual_minutes, t.completed_at, t.updated_at FROM tasks t LEFT JOIN task_actual_minutes tam ON tam.task_id = t.id WHERE t.status = 'completed' AND ((t.normalized_title IS NOT NULL AND ({filter})) OR t.normalized_title IS NULL) ORDER BY t.updated_at DESC LIMIT {cap}"
    );
    let stmt = database.prepare(sql).bind(&bindings)?;
    let rows: Vec<SimilarTaskRow> = safe_all(&stmt).await?;

    let mut scored: Vec<(f64, SimilarTaskRow)> = rows
        .into_iter()
        .filter_map(|row| {
            memory::similar_task_score_pre_normalized(&normalized_title, &row.title)
                .map(|score| (score, row))
        })
        .collect();

    scored.sort_by(|(sa, a), (sb, b)| {
        sa.total_cmp(sb)
            .reverse()
            .then_with(|| {
                let a_str = a.completed_at.map(|t| t.to_string());
                let b_str = b.completed_at.map(|t| t.to_string());
                memory::compare_optional_desc(&a_str, &b_str)
            })
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.task_id.cmp(&b.task_id))
    });

    let mut out: Vec<SimilarTaskRow> = scored
        .into_iter()
        .map(|(score, mut row)| {
            row.similarity = takusu_util::Similarity::dice(score);
            row
        })
        .collect();
    out.truncate(limit as usize);

    json_ok(&out)
}
