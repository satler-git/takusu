//! End-to-end test: spin up a mock worker (axum + in-memory SQLite) that
//! mirrors the takusu-worker HTTP API, then point WorkersStorage at it
//! and exercise the full storage trait. This is the integration test for
//! the WorkersStorage client without needing wasm32 or a deployed Worker.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::LazyLock;
use takusu_local_lib::{
    DEFAULT_AUD, SCOPE_READ_WRITE, TokenClaims, generate_root_jwt, jwt,
    storage_workers::WorkersStorage,
};
use takusu_storage::{
    CreateHabit, CreateHabitScheduledSpan, CreateMemory, CreateTask, HabitRow,
    HabitScheduledSpanRow, MemoryQuery, MemoryRow, SimilarTaskQuery, SimilarTaskRow, Storage,
    TaskQuery, TaskRow, TokenCreateResponse, TokenRow, UpdateHabit, UpdateMemory, UpdateTask,
};
use takusu_util::{EnumLabel, Minutes, Quantity};
use tokio::net::TcpListener;

const JWT_SECRET: &str = "test-secret-do-not-use-in-production";
static ROOT_TOKEN: LazyLock<String> = LazyLock::new(|| {
    generate_root_jwt(JWT_SECRET, None).expect("root token generation should not fail")
});

#[derive(Clone)]
struct MockState {
    pool: SqlitePool,
    jwt_secret: String,
}

async fn setup_mock_db() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let sqls: &[&str] = &[
        include_str!("../../takusu-local-lib/migrations/001_init.sql"),
        include_str!("../../takusu-local-lib/migrations/002_google_cal.sql"),
        include_str!("../../takusu-local-lib/migrations/003_settings.sql"),
        include_str!("../../takusu-local-lib/migrations/004_indexes.sql"),
        include_str!("../../takusu-local-lib/migrations/005_task_display_id.sql"),
        include_str!("../../takusu-local-lib/migrations/006_user_edited.sql"),
        include_str!("../../takusu-local-lib/migrations/007_task_display_id_seq.sql"),
        include_str!("../../takusu-local-lib/migrations/008_fixed.sql"),
        include_str!("../../takusu-local-lib/migrations/009_habit_display_id.sql"),
        include_str!("../../takusu-local-lib/migrations/010_habit_pauses.sql"),
        "ALTER TABLE habit_pauses RENAME TO habit_scheduled_spans; DROP INDEX IF EXISTS idx_habit_pauses_habit; CREATE INDEX IF NOT EXISTS idx_habit_scheduled_spans_habit ON habit_scheduled_spans(habit_id);",
        include_str!("../../takusu-local-lib/migrations/011_habit_steps.sql"),
        include_str!("../../takusu-local-lib/migrations/012_window_mode.sql"),
        include_str!("../../takusu-local-lib/migrations/013_habit_task_display_id.sql"),
        include_str!("../../takusu-local-lib/migrations/014_workload.sql"),
        include_str!("../../takusu-local-lib/migrations/015_skills.sql"),
        include_str!("../../takusu-local-lib/migrations/016_memory.sql"),
        include_str!("../../takusu-local-lib/migrations/017_solver.sql"),
        include_str!("../../takusu-local-lib/migrations/018_progress.sql"),
        include_str!("../../takusu-local-lib/migrations/019_jwt.sql"),
        include_str!("../../takusu-local-lib/migrations/020_task_actual_minutes_view.sql"),
        include_str!("../../takusu-local-lib/migrations/021_solver_default_sa.sql"),
        include_str!("../../takusu-local-lib/migrations/022_split_from_task_index.sql"),
        include_str!("../../takusu-local-lib/migrations/023_timestamp_format.sql"),
        include_str!("../../takusu-local-lib/migrations/024_zero_quantity_to_null.sql"),
        include_str!("../../takusu-local-lib/migrations/025_task_normalized_title.sql"),
    ];
    for s in sqls {
        sqlx::raw_sql(*s).execute(&pool).await.unwrap();
    }
    pool
}

fn mock_router(state: MockState) -> Router {
    Router::new()
        .route("/api/auth/verify", get(verify))
        // Habits + scheduled spans: literal segments before parameterized routes
        // so `scheduled-spans` / `steps` are not treated as a habit id.
        .route("/api/habits", get(list_habits).post(create_habit))
        .route(
            "/api/habits/scheduled-spans",
            get(list_all_habit_scheduled_spans),
        )
        .route(
            "/api/habits/{id}",
            get(get_habit).patch(update_habit).delete(delete_habit),
        )
        .route(
            "/api/habits/{id}/scheduled-spans",
            get(list_habit_scheduled_spans).post(create_habit_scheduled_span),
        )
        .route(
            "/api/habits/{id}/scheduled-spans/{span_id}",
            delete(delete_habit_scheduled_span),
        )
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route(
            "/api/tasks/{id}",
            get(get_task)
                .put(replace_task)
                .patch(update_task)
                .delete(delete_task),
        )
        .route("/api/tokens", get(list_tokens).post(create_token))
        .route("/api/tokens/{id}", delete(revoke_token))
        .route("/api/memory", post(create_memory))
        .route("/api/memory/search", get(list_memories))
        .route(
            "/api/memory/{id}",
            get(get_memory).patch(update_memory).delete(delete_memory),
        )
        .route("/api/tasks/similar", get(similar_tasks))
        .with_state(state)
}

async fn verify(
    State(state): State<MockState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<TokenClaims>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let token = match token {
        Some(t) => t.to_string(),
        None => return Err(StatusCode::UNAUTHORIZED),
    };
    let claims = jwt::verify(&state.jwt_secret, &token, DEFAULT_AUD)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    if claims.is_root() {
        return Ok(Json(claims));
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tokens WHERE jti = ? AND revoked_at IS NULL")
            .bind(&claims.jti)
            .fetch_one(&state.pool)
            .await
            .unwrap_or(0);
    if count == 0 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(claims))
}

/// SQL predicate for tasks whose deadline has passed but are not finished.
const OVERDUE_SQL: &str =
    "status NOT IN ('completed', 'skipped') AND datetime(end_at) < datetime('now')";
/// SQL predicate that excludes overdue tasks (completed/skipped or end_at is now or later).
const NOT_OVERDUE_SQL: &str =
    "(status IN ('completed', 'skipped') OR datetime(end_at) >= datetime('now'))";
/// Shared task SELECT fragment (mirrors takusu-worker TASK_COLS).
const TASK_SELECT: &str = "SELECT id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, habit_id, ical_uid, user_edited, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at, tam.actual_minutes FROM tasks LEFT JOIN task_actual_minutes tam ON tam.task_id = tasks.id";

async fn list_tasks(
    State(state): State<MockState>,
    axum::extract::Query(q): axum::extract::Query<TaskQuery>,
) -> Json<Vec<TaskRow>> {
    let mut sql = format!("{TASK_SELECT} WHERE 1=1");
    if let Some(ref status) = q.status {
        if status == "overdue" {
            sql.push_str(" AND ");
            sql.push_str(OVERDUE_SQL);
        } else {
            sql.push_str(" AND status = ?");
        }
    }
    if q.from.is_some() {
        sql.push_str(" AND end_at >= ?");
    }
    if q.until.is_some() {
        // start_at is nullable: NULL <= value evaluates to NULL
        // (excluded). Include tasks with no explicit start time so
        // range queries don't silently drop them.
        sql.push_str(" AND (start_at IS NULL OR start_at <= ?)");
    }
    if q.no_overdue == Some(true) {
        sql.push_str(" AND ");
        sql.push_str(NOT_OVERDUE_SQL);
    }
    if q.habit_id.is_some() {
        sql.push_str(" AND habit_id = ?");
    }
    if q.ical_uid.is_some() {
        sql.push_str(" AND ical_uid = ?");
    }
    sql.push_str(" ORDER BY created_at DESC");
    let mut query = sqlx::query_as::<_, TaskRow>(sqlx::AssertSqlSafe(sql.as_str()));
    if let Some(s) = &q.status
        && s != "overdue"
    {
        query = query.bind(s);
    }
    if let Some(f) = &q.from {
        query = query.bind(f);
    }
    if let Some(u) = &q.until {
        query = query.bind(u);
    }
    if let Some(h) = &q.habit_id {
        query = query.bind(h);
    }
    if let Some(u) = &q.ical_uid {
        query = query.bind(u);
    }
    let rows = query.fetch_all(&state.pool).await.unwrap_or_default();
    Json(rows)
}

async fn create_task(
    State(state): State<MockState>,
    Json(body): Json<CreateTask>,
) -> Result<(StatusCode, Json<TaskRow>), StatusCode> {
    let id = uuid::Uuid::now_v7().to_string();
    let depends_json = serde_json::to_string(&body.depends.clone().unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_string());
    let sigma = body
        .sigma_minutes
        .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
    let parallelizable = body.parallelizable.unwrap_or(false);
    let allows_parallel = body.allows_parallel.unwrap_or(false);
    let abandonability = body.abandonability.unwrap_or(0.5.into());
    let fixed = body.fixed.unwrap_or(false);

    let quantity_done = body.quantity_done.unwrap_or_default();
    // Treat quantity_total / original_quantity_total 0 as unset (same as None) server-side.
    let quantity_total = body.quantity_total.filter(|t| *t != 0);
    let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
    sqlx::query(
        "INSERT INTO tasks (id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total) VALUES (?, (SELECT COALESCE(MAX(display_id), 0) + 1 FROM tasks), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(&body.start_at)
    .bind(&body.end_at)
    .bind(body.avg_minutes)
    .bind(sigma)
    .bind(&depends_json)
    .bind(parallelizable)
    .bind(allows_parallel)
    .bind(abandonability)
    .bind(&body.ical_uid)
    .bind(fixed)
    .bind(&body.habit_step_id)
    .bind(quantity_total)
    .bind(quantity_done)
    .bind(&body.quantity_unit)
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(original_quantity_total)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let select_sql = format!("{TASK_SELECT} WHERE tasks.id = ?");
    let row: TaskRow = sqlx::query_as::<_, TaskRow>(sqlx::AssertSqlSafe(select_sql.as_str()))
        .bind(&id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn get_task(
    State(state): State<MockState>,
    Path(id): Path<String>,
) -> Result<Json<TaskRow>, StatusCode> {
    let sql = format!("{TASK_SELECT} WHERE tasks.id = ?");
    let row: Option<TaskRow> = sqlx::query_as::<_, TaskRow>(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(&id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    row.map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn update_task(
    State(state): State<MockState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTask>,
) -> Result<Json<TaskRow>, StatusCode> {
    let validated = [
        "pending",
        "scheduled",
        "in_progress",
        "completed",
        "skipped",
    ];
    if let Some(ref s) = body.status
        && !validated.contains(&s.as_str())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let depends_json = body
        .depends
        .as_ref()
        .map(|d| serde_json::to_string(d).unwrap_or_else(|_| "[]".into()));

    let existing_sql = format!("{TASK_SELECT} WHERE tasks.id = ?");
    let existing: TaskRow =
        sqlx::query_as::<_, TaskRow>(sqlx::AssertSqlSafe(existing_sql.as_str()))
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
    let final_status = body.status.unwrap_or(existing.status).to_string();

    // Treat quantity_total / original_quantity_total 0 as unset (same as None) server-side.
    let quantity_total = body.quantity_total.filter(|t| *t != 0);
    let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
    sqlx::query(
        "UPDATE tasks SET title=COALESCE(?1,title), description=COALESCE(?2,description), start_at=COALESCE(?3,start_at), end_at=COALESCE(?4,end_at), avg_minutes=COALESCE(?5,avg_minutes), sigma_minutes=COALESCE(?6,sigma_minutes), depends=COALESCE(?7,depends), parallelizable=COALESCE(?8,parallelizable), allows_parallel=COALESCE(?9,allows_parallel), abandonability=COALESCE(?10,abandonability), status=?11, user_edited=COALESCE(?13,user_edited), quantity_total=COALESCE(?14,quantity_total), quantity_done=COALESCE(?15,quantity_done), quantity_unit=COALESCE(?16,quantity_unit), original_quantity_total=COALESCE(?17,original_quantity_total), updated_at=datetime('now') WHERE id = ?12"
    )
    .bind(body.title.as_deref())
    .bind(body.description.as_deref())
    .bind(body.start_at.as_deref())
    .bind(body.end_at.as_deref())
    .bind(body.avg_minutes)
    .bind(body.sigma_minutes)
    .bind(depends_json.as_deref())
    .bind(body.parallelizable)
    .bind(body.allows_parallel)
    .bind(body.abandonability)
    .bind(&final_status)
    .bind(&id)
    .bind(body.user_edited)
    .bind(quantity_total)
    .bind(body.quantity_done)
    .bind(body.quantity_unit.as_deref())
    .bind(original_quantity_total)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let select_sql = format!("{TASK_SELECT} WHERE tasks.id = ?");
    let row: TaskRow = sqlx::query_as::<_, TaskRow>(sqlx::AssertSqlSafe(select_sql.as_str()))
        .bind(&id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(row))
}

async fn replace_task(
    State(state): State<MockState>,
    Path(id): Path<String>,
    Json(body): Json<CreateTask>,
) -> Result<Json<TaskRow>, StatusCode> {
    let depends_json = serde_json::to_string(&body.depends.clone().unwrap_or_default())
        .unwrap_or_else(|_| "[]".to_string());
    let sigma = body
        .sigma_minutes
        .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
    let parallelizable = body.parallelizable.unwrap_or(false);
    let allows_parallel = body.allows_parallel.unwrap_or(false);
    let abandonability = body.abandonability.unwrap_or(0.5.into());
    let quantity_done = body.quantity_done.unwrap_or_default();
    // Treat quantity_total / original_quantity_total 0 as unset (same as None) server-side.
    let quantity_total = body.quantity_total.filter(|t| *t != 0);
    let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);

    sqlx::query(
        "UPDATE tasks SET title=?, description=?, start_at=?, end_at=?, avg_minutes=?, sigma_minutes=?, depends=?, parallelizable=?, allows_parallel=?, abandonability=?, quantity_total=?, quantity_done=?, quantity_unit=?, completed_at=?, split_from_task_id=?, original_quantity_total=?, updated_at=datetime('now') WHERE id = ?"
    )
    .bind(&body.title)
    .bind(&body.description)
    .bind(&body.start_at)
    .bind(&body.end_at)
    .bind(body.avg_minutes)
    .bind(sigma)
    .bind(&depends_json)
    .bind(parallelizable)
    .bind(allows_parallel)
    .bind(abandonability)
    .bind(quantity_total)
    .bind(quantity_done)
    .bind(&body.quantity_unit)
    .bind(None::<String>)
    .bind(None::<String>)
    .bind(original_quantity_total)
    .bind(&id)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let select_sql = format!("{TASK_SELECT} WHERE tasks.id = ?");
    let row: TaskRow = sqlx::query_as::<_, TaskRow>(sqlx::AssertSqlSafe(select_sql.as_str()))
        .bind(&id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(row))
}

async fn delete_task(State(state): State<MockState>, Path(id): Path<String>) -> StatusCode {
    let r = sqlx::query("DELETE FROM tasks WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await
        .unwrap();
    if r.rows_affected() == 0 {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::NO_CONTENT
    }
}

async fn list_tokens(State(state): State<MockState>) -> Json<Vec<TokenRow>> {
    let rows: Vec<TokenRow> = sqlx::query_as::<_, TokenRow>(
        "SELECT id, jti, scope, label, created_by, created_at, revoked_at, expires_at FROM tokens ORDER BY created_at DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    Json(rows)
}

async fn create_token(
    State(state): State<MockState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Result<(StatusCode, Json<TokenCreateResponse>), StatusCode> {
    let label = body
        .get("label")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let label_opt = label.as_deref().filter(|s| !s.is_empty());
    let (new_token, jti) =
        jwt::generate_token_jwt(&state.jwt_secret, SCOPE_READ_WRITE, label_opt, None)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    sqlx::query(
        "INSERT INTO tokens (jti, scope, label, created_by) VALUES (?, 'read-write', ?, 'authenticated')",
    )
    .bind(&jti)
    .bind(label_opt)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row: TokenRow = sqlx::query_as::<_, TokenRow>(
        "SELECT id, jti, scope, label, created_by, created_at, revoked_at, expires_at FROM tokens WHERE jti = ?",
    )
    .bind(&jti)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        StatusCode::CREATED,
        Json(TokenCreateResponse {
            id: row.id,
            token: new_token,
            scope: row.scope,
            label: row.label,
            created_at: row.created_at,
            expires_at: row.expires_at,
        }),
    ))
}

async fn revoke_token(
    State(state): State<MockState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let id_num: i64 = id.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    let result = sqlx::query(
        "UPDATE tokens SET revoked_at = datetime('now') WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(id_num)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Resolve a habit reference (`h<N>`, full UUID, or UUID prefix) to its full
/// UUID, mirroring the local storage helper.
async fn resolve_habit_id(pool: &SqlitePool, id: &str) -> Result<String, StatusCode> {
    if let Some(rest) = id.strip_prefix(['h', 'H'])
        && let Ok(num) = rest.parse::<i64>()
    {
        return sqlx::query_scalar::<_, String>("SELECT id FROM habits WHERE display_id = ?")
            .bind(num)
            .fetch_optional(pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND);
    }
    if id.contains('-') {
        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM habits WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if exists {
            return Ok(id.to_string());
        }
    }
    let matches: Vec<String> = sqlx::query_scalar("SELECT id FROM habits WHERE id LIKE ? || '%'")
        .bind(id)
        .fetch_all(pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match matches.len() {
        0 => Err(StatusCode::NOT_FOUND),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

async fn list_habits(State(state): State<MockState>) -> Json<Vec<HabitRow>> {
    let rows: Vec<HabitRow> = sqlx::query_as::<_, HabitRow>(
        "SELECT id, display_id, title, description, recurrence, start_time, end_time, \
         avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, \
         active, fixed, window_mode, created_at, updated_at \
         FROM habits ORDER BY display_id, created_at DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    Json(rows)
}

async fn get_habit(
    State(state): State<MockState>,
    Path(id): Path<String>,
) -> Result<Json<HabitRow>, StatusCode> {
    let full = resolve_habit_id(&state.pool, &id).await?;
    let row: Option<HabitRow> = sqlx::query_as::<_, HabitRow>(
        "SELECT id, display_id, title, description, recurrence, start_time, end_time, \
         avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, \
         active, fixed, window_mode, created_at, updated_at \
         FROM habits WHERE id = ?",
    )
    .bind(&full)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    row.map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn create_habit(
    State(state): State<MockState>,
    Json(body): Json<CreateHabit>,
) -> Result<(StatusCode, Json<HabitRow>), StatusCode> {
    let id = uuid::Uuid::now_v7().to_string();
    let sigma = body
        .sigma_minutes
        .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
    let parallelizable = body.parallelizable.unwrap_or(false);
    let allows_parallel = body.allows_parallel.unwrap_or(false);
    let abandonability = body.abandonability.unwrap_or(0.5.into());
    let fixed = body.fixed.unwrap_or(false);
    let window_mode = body
        .window_mode
        .as_ref()
        .map(|w| w.as_str())
        .unwrap_or("day");
    sqlx::query(
        "INSERT INTO habits (id, title, description, recurrence, start_time, end_time, \
         avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, \
         active, fixed, window_mode, display_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
         (SELECT COALESCE(MAX(display_id), 0) + 1 FROM habits))",
    )
    .bind(&id)
    .bind(&body.title)
    .bind(&body.description)
    .bind(&body.recurrence)
    .bind(&body.start_time)
    .bind(&body.end_time)
    .bind(body.avg_minutes)
    .bind(sigma)
    .bind(parallelizable)
    .bind(allows_parallel)
    .bind(abandonability)
    .bind(true)
    .bind(fixed)
    .bind(window_mode)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    get_habit(State(state.clone()), Path(id.clone()))
        .await
        .map(|row| (StatusCode::CREATED, row))
}

async fn update_habit(
    State(state): State<MockState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateHabit>,
) -> Result<Json<HabitRow>, StatusCode> {
    let full = resolve_habit_id(&state.pool, &id).await?;
    sqlx::query(
        "UPDATE habits SET \
         title=COALESCE(?1,title), description=COALESCE(?2,description), \
         recurrence=COALESCE(?3,recurrence), start_time=COALESCE(?4,start_time), \
         end_time=COALESCE(?5,end_time), avg_minutes=COALESCE(?6,avg_minutes), \
         sigma_minutes=COALESCE(?7,sigma_minutes), \
         parallelizable=COALESCE(?8,parallelizable), \
         allows_parallel=COALESCE(?9,allows_parallel), \
         abandonability=COALESCE(?10,abandonability), active=COALESCE(?11,active), \
         fixed=COALESCE(?12,fixed), window_mode=COALESCE(?13,window_mode), \
         updated_at=datetime('now') WHERE id=?14",
    )
    .bind(body.title.as_deref())
    .bind(body.description.as_deref())
    .bind(body.recurrence.as_deref())
    .bind(body.start_time.as_deref())
    .bind(body.end_time.as_deref())
    .bind(body.avg_minutes)
    .bind(body.sigma_minutes)
    .bind(body.parallelizable)
    .bind(body.allows_parallel)
    .bind(body.abandonability)
    .bind(body.active)
    .bind(body.fixed)
    .bind(body.window_mode.as_ref().map(|w| w.as_str()))
    .bind(&full)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    get_habit(State(state), Path(full)).await
}

async fn delete_habit(
    State(state): State<MockState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let full = resolve_habit_id(&state.pool, &id).await?;
    let result = sqlx::query("DELETE FROM habits WHERE id = ?")
        .bind(&full)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_all_habit_scheduled_spans(
    State(state): State<MockState>,
) -> Json<Vec<HabitScheduledSpanRow>> {
    let rows: Vec<HabitScheduledSpanRow> = sqlx::query_as::<_, HabitScheduledSpanRow>(
        "SELECT * FROM habit_scheduled_spans ORDER BY habit_id, start_date, created_at",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    Json(rows)
}

async fn list_habit_scheduled_spans(
    State(state): State<MockState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<HabitScheduledSpanRow>>, StatusCode> {
    let full = resolve_habit_id(&state.pool, &id).await?;
    let rows: Vec<HabitScheduledSpanRow> = sqlx::query_as::<_, HabitScheduledSpanRow>(
        "SELECT * FROM habit_scheduled_spans WHERE habit_id = ? ORDER BY start_date, created_at",
    )
    .bind(&full)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows))
}

async fn create_habit_scheduled_span(
    State(state): State<MockState>,
    Path(id): Path<String>,
    Json(body): Json<CreateHabitScheduledSpan>,
) -> Result<(StatusCode, Json<HabitScheduledSpanRow>), StatusCode> {
    let full = resolve_habit_id(&state.pool, &id).await?;
    let span_id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO habit_scheduled_spans \
         (id, habit_id, start_date, end_date, reason, created_at) \
         VALUES (?, ?, ?, ?, ?, datetime('now'))",
    )
    .bind(&span_id)
    .bind(&full)
    .bind(&body.start_date)
    .bind(&body.end_date)
    .bind(&body.reason)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row: HabitScheduledSpanRow = sqlx::query_as::<_, HabitScheduledSpanRow>(
        "SELECT * FROM habit_scheduled_spans WHERE id = ?",
    )
    .bind(&span_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn delete_habit_scheduled_span(
    State(state): State<MockState>,
    Path((id, span_id)): Path<(String, String)>,
) -> Result<StatusCode, StatusCode> {
    let full = resolve_habit_id(&state.pool, &id).await?;
    let result = sqlx::query("DELETE FROM habit_scheduled_spans WHERE id = ? AND habit_id = ?")
        .bind(&span_id)
        .bind(&full)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn spawn_mock_worker() -> String {
    let pool = setup_mock_db().await;
    let state = MockState {
        pool,
        jwt_secret: JWT_SECRET.to_string(),
    };
    let app = mock_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn workers_storage_e2e() {
    let base_url = spawn_mock_worker().await;
    let storage = WorkersStorage::new_with(base_url.clone(), ROOT_TOKEN.to_string());

    assert!(
        storage
            .verify_token(ROOT_TOKEN.as_str())
            .await
            .unwrap()
            .is_some()
    );
    assert!(storage.verify_token("tsk_bogus").await.unwrap().is_none());

    let create_body = CreateTask {
        title: "e2e task".to_string(),
        description: Some("integration test".to_string()),
        start_at: Some("2026-06-05T09:00:00+09:00".to_string()),
        end_at: "2026-06-05T18:00:00+09:00".to_string(),
        avg_minutes: 60,
        sigma_minutes: Some(15),
        depends: Some(vec![]),
        parallelizable: Some(false),
        allows_parallel: Some(false),
        abandonability: Some(0.3.into()),
        ical_uid: None,
        habit_id: None,
        fixed: None,
        habit_step_id: None,
        quantity_total: None,
        quantity_done: None,
        quantity_unit: None,
        original_quantity_total: None,
    };
    let task = storage.create_task(&create_body).await.unwrap();
    assert_eq!(task.title, "e2e task");
    assert_eq!(task.status, takusu_util::TaskStatus::Pending);
    let id = task.id.clone();

    let tasks = storage
        .list_tasks(&TaskQuery {
            status: Some("pending".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, id);

    let fetched = storage.get_task(&id).await.unwrap();
    assert_eq!(fetched.id, id);

    let err = storage
        .get_task("00000000-0000-0000-0000-000000000000")
        .await;
    assert!(matches!(
        err,
        Err(takusu_storage::StorageError::NotFound(_))
    ));

    let update_body = UpdateTask {
        title: Some("e2e task updated".to_string()),
        ..Default::default()
    };
    let updated = storage.update_task(&id, &update_body).await.unwrap();
    assert_eq!(updated.title, "e2e task updated");

    storage.delete_task(&id).await.unwrap();
    let after = storage.get_task(&id).await;
    assert!(matches!(
        after,
        Err(takusu_storage::StorageError::NotFound(_))
    ));

    let resp = storage.create_token(Some("e2e")).await.unwrap();
    assert!(resp.token.starts_with("eyJ"));
    let tokens = storage.list_tokens().await.unwrap();
    assert_eq!(tokens.len(), 1);
    storage.revoke_token(resp.id).await.unwrap();
    let tokens_after = storage.list_tokens().await.unwrap();
    assert!(tokens_after[0].revoked_at.is_some());
}

// ── Memory handlers (mock worker) ──

async fn create_memory(
    State(state): State<MockState>,
    Json(body): Json<CreateMemory>,
) -> Result<(StatusCode, Json<MemoryRow>), StatusCode> {
    let id = uuid::Uuid::now_v7().to_string();
    let subject_type = body.subject_type.unwrap_or_default();
    let subject_id = body.subject_id.clone().unwrap_or_default();
    let normalized_key = body.key.clone();
    let normalized_content = body.content.clone();
    sqlx::query(
        "INSERT INTO memories (id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'user_confirmed', 1)"
    )
    .bind(&id)
    .bind(body.kind.as_str())
    .bind(&body.key)
    .bind(&normalized_key)
    .bind(&body.content)
    .bind(&normalized_content)
    .bind(subject_type.as_str())
    .bind(&subject_id)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::CONFLICT)?;

    let row: MemoryRow = sqlx::query_as::<_, MemoryRow>(
        "SELECT id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at FROM memories WHERE id = ?",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn get_memory(
    State(state): State<MockState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryRow>, StatusCode> {
    let row: Option<MemoryRow> = sqlx::query_as::<_, MemoryRow>(
        "SELECT id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at FROM memories WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    row.map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn update_memory(
    State(state): State<MockState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateMemory>,
) -> Result<Json<MemoryRow>, StatusCode> {
    let content = body.content.as_deref().unwrap_or_default();
    let normalized_content = content.to_string();
    let row: Option<MemoryRow> = sqlx::query_as::<_, MemoryRow>(
        "UPDATE memories SET content = ?, normalized_content = ?, revision = revision + 1, updated_at = datetime('now') WHERE id = ? AND revision = ? RETURNING id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at",
    )
    .bind(content)
    .bind(&normalized_content)
    .bind(&id)
    .bind(body.observed_revision)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    row.map(Json).ok_or(StatusCode::CONFLICT)
}

async fn delete_memory(
    State(state): State<MockState>,
    Path(id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> StatusCode {
    let observed = params
        .get("observed_revision")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let result = sqlx::query("DELETE FROM memories WHERE id = ? AND revision = ?")
        .bind(&id)
        .bind(observed)
        .execute(&state.pool)
        .await;
    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT,
        _ => StatusCode::NOT_FOUND,
    }
}

async fn list_memories(
    State(state): State<MockState>,
    axum::extract::Query(q): axum::extract::Query<MemoryQuery>,
) -> Json<Vec<MemoryRow>> {
    let pattern = format!("%{}%", q.q);
    let mut sql = String::from(
        "SELECT id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at, last_used_at FROM memories WHERE (normalized_key LIKE ? OR normalized_content LIKE ?)",
    );
    let mut binds: Vec<String> = vec![pattern.clone(), pattern];
    if let Some(ref kind) = q.kind {
        sql.push_str(" AND kind = ?");
        binds.push(kind.to_string());
    }
    if let Some(ref st) = q.subject_type {
        sql.push_str(" AND subject_type = ?");
        binds.push(st.to_string());
    }
    if let Some(ref sid) = q.subject_id {
        sql.push_str(" AND subject_id = ?");
        binds.push(sid.clone());
    }
    sql.push_str(" ORDER BY updated_at DESC");
    let limit = q.limit.unwrap_or(10).clamp(1, 50) as usize;
    sql.push_str(&format!(" LIMIT {limit}"));

    let mut query = sqlx::query_as::<_, MemoryRow>(sqlx::AssertSqlSafe(sql.as_str()));
    for b in binds {
        query = query.bind(b);
    }
    let rows = query.fetch_all(&state.pool).await.unwrap_or_default();
    Json(rows)
}

async fn similar_tasks(
    State(state): State<MockState>,
    axum::extract::Query(q): axum::extract::Query<SimilarTaskQuery>,
) -> Json<Vec<SimilarTaskRow>> {
    let limit = q.limit.unwrap_or(10).clamp(1, 50) as usize;
    // Mirror the real worker contract: score completed tasks with the Dice
    // coefficient and report it as `dice:{score:.3}` (#942). The mock scores in
    // Rust from the raw title (normalizing both sides) instead of using the
    // normalized_title SQL pre-filter, which is equivalent for test purposes.
    let Ok(normalized) =
        takusu_util::memory::normalize_text(&q.title, Some(takusu_util::memory::MAX_QUERY_SCALARS))
    else {
        return Json(Vec::new());
    };
    // Match the real worker's worst-case candidate cap so the mock behaves the
    // same on large datasets (#942).
    let cap = takusu_util::memory::SIMILAR_TASK_CANDIDATE_CAP;
    let sql = format!(
        "SELECT t.id AS task_id, t.display_id, t.title, t.avg_minutes, t.sigma_minutes, tam.actual_minutes, t.completed_at, t.updated_at, '' AS similarity FROM tasks t LEFT JOIN task_actual_minutes tam ON tam.task_id = t.id WHERE t.status = 'completed' ORDER BY t.updated_at DESC LIMIT {cap}"
    );
    let rows: Vec<SimilarTaskRow> =
        sqlx::query_as::<_, SimilarTaskRow>(sqlx::AssertSqlSafe(sql.as_str()))
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();

    let mut scored: Vec<(f64, SimilarTaskRow)> = rows
        .into_iter()
        .filter_map(|row| {
            takusu_util::memory::similar_task_score_pre_normalized(&normalized, &row.title)
                .map(|score| (score, row))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sa.total_cmp(sb)
            .reverse()
            .then_with(|| {
                takusu_util::memory::compare_optional_desc(&a.completed_at, &b.completed_at)
            })
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
    let mut out: Vec<SimilarTaskRow> = scored
        .into_iter()
        .map(|(score, mut row)| {
            row.similarity = format!("dice:{score:.3}");
            row
        })
        .collect();
    out.truncate(limit);
    Json(out)
}

#[tokio::test]
async fn workers_storage_memory_e2e() {
    let base_url = spawn_mock_worker().await;
    let storage = WorkersStorage::new_with(base_url, ROOT_TOKEN.to_string());

    let create = CreateMemory {
        kind: takusu_util::MemoryKind::ProperNoun,
        key: "研究室".into(),
        content: "大学の研究室".into(),
        subject_type: None,
        subject_id: None,
        upsert: false,
    };
    let row = storage.create_memory(&create, None).await.unwrap();
    let id = row.id.clone();
    assert_eq!(row.key, "研究室");

    let fetched = storage.get_memory(&id).await.unwrap();
    assert_eq!(fetched.id, id);

    let update = UpdateMemory {
        observed_revision: row.revision,
        content: Some("大学の研究室（更新）".into()),
    };
    let updated = storage.update_memory(&id, &update, None).await.unwrap();
    assert_eq!(updated.content, "大学の研究室（更新）");

    let found = storage
        .search_memories(&MemoryQuery {
            q: "研究室".into(),
            kind: None,
            subject_type: None,
            subject_id: None,
            limit: Some(10),
        })
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, id);

    storage
        .delete_memory(&id, updated.revision, None)
        .await
        .unwrap();
    let after = storage.get_memory(&id).await;
    assert!(matches!(
        after,
        Err(takusu_storage::StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn workers_storage_list_tasks_no_overdue_filter() {
    let base_url = spawn_mock_worker().await;
    let storage = WorkersStorage::new_with(base_url, ROOT_TOKEN.to_string());

    let overdue = CreateTask {
        title: "overdue task".into(),
        description: None,
        start_at: None,
        end_at: "2020-01-01T00:00:00+00:00".into(),
        avg_minutes: 10,
        sigma_minutes: Some(2),
        depends: Some(vec![]),
        parallelizable: Some(false),
        allows_parallel: Some(false),
        abandonability: Some(0.5.into()),
        ical_uid: None,
        habit_id: None,
        fixed: None,
        habit_step_id: None,
        quantity_total: None,
        quantity_done: None,
        quantity_unit: None,
        original_quantity_total: None,
    };
    let future = CreateTask {
        title: "future task".into(),
        description: None,
        start_at: None,
        end_at: "2030-01-01T00:00:00+00:00".into(),
        ..overdue.clone()
    };
    storage.create_task(&overdue).await.unwrap();
    storage.create_task(&future).await.unwrap();

    let tasks = storage
        .list_tasks(&TaskQuery {
            no_overdue: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "future task");
}

#[tokio::test]
async fn workers_storage_e2e_zero_quantity() {
    let base_url = spawn_mock_worker().await;
    let storage = WorkersStorage::new_with(base_url, ROOT_TOKEN.to_string());

    let task = storage
        .create_task(&CreateTask {
            title: "zero-total".into(),
            description: None,
            start_at: None,
            end_at: "2030-01-01T00:00:00+00:00".into(),
            avg_minutes: 30,
            sigma_minutes: None,
            depends: None,
            parallelizable: None,
            allows_parallel: None,
            abandonability: None,
            ical_uid: None,
            habit_id: None,
            fixed: None,
            habit_step_id: None,
            quantity_total: Some(Quantity::default()),
            quantity_done: None,
            quantity_unit: None,
            original_quantity_total: Some(Quantity::default()),
        })
        .await
        .unwrap();
    assert!(task.quantity_total.is_none());
    assert!(task.original_quantity_total.is_none());
    assert_eq!(task.quantity_done, 0);

    let updated = storage
        .update_task(
            &task.id,
            &UpdateTask {
                title: None,
                description: None,
                start_at: None,
                end_at: None,
                avg_minutes: None,
                sigma_minutes: None,
                depends: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                status: None,
                habit_id: None,
                user_edited: None,
                fixed: None,
                habit_step_id: None,
                quantity_total: Some(Quantity::default()),
                quantity_done: None,
                quantity_unit: None,
                original_quantity_total: Some(Quantity::default()),
            },
        )
        .await
        .unwrap();
    assert!(updated.quantity_total.is_none());
    assert!(updated.original_quantity_total.is_none());
}
