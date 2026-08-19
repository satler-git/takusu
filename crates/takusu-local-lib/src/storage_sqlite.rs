use async_trait::async_trait;
use jiff::tz::TimeZone;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use takusu_contracts::{
    AttachWorkSession, CommentRow, ConvertWorkSession, CreateHabit, CreateHabitScheduledSpan,
    CreateMemory, CreateSkill, CreateTask, EstimatorStateRow, GoogleCalEventRow,
    GoogleCalSettingsRow, HabitRow, HabitScheduledSpanRow, HabitStepEstimateInput, HabitStepInput,
    HabitStepRow, MemoryInjectionQuery, MemoryInjectionResult, MemoryKindCounts, MemoryQuery,
    MemoryRow, MoveEntryResponse, RecordWorkSessionProgress, SaveScheduleRequest, ScheduleEntry,
    ScheduleRow, SettingsRow, SimilarTaskQuery, SimilarTaskRow, SkillRow, SplitResult, SplitTask,
    StartWorkSession, Storage, StorageError, TaskProgress, TaskQuery, TaskRow, TokenCreateResponse,
    TokenRow, UpdateGoogleCalSettings, UpdateHabit, UpdateMemory, UpdateSettings, UpdateSkill,
    UpdateTask, WorkSessionProgressResult, WorkSessionRow, storage::StorageResult,
};
use takusu_search::search::{EvalContext, filter_tasks};
use takusu_types::Minutes;
use takusu_types::{
    CommentAuthor, EnumLabel, Quantity, TaskStatus, TaskStatusFilter, Timestamp, WindowMode,
};
use takusu_types::{DEFAULT_AUD, SCOPE_READ_WRITE};

use crate::config::LocalConfig;
use takusu_contracts::validate::{validate_quantity, validate_scheduled_span_dates};

/// SQL predicate for tasks whose deadline has passed but are not finished.
const OVERDUE_SQL: &str =
    "status NOT IN ('completed', 'skipped') AND datetime(end_at) < datetime('now')";
/// SQL predicate that excludes overdue tasks (completed/skipped or end_at is now or later).
const NOT_OVERDUE_SQL: &str =
    "(status IN ('completed', 'skipped') OR datetime(end_at) >= datetime('now'))";
/// SQL predicate for actionable tasks (pending, scheduled, or in_progress).
const ACTIONABLE_SQL: &str = "status IN ('pending', 'scheduled', 'in_progress')";
/// Static `SELECT ... FROM tasks` fragments for queries that require an
/// audited `&'static str` (`SqlSafeStr`) and avoid `SELECT *` brittleness.
const SELECT_TASKS: &str = "SELECT id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, habit_id, ical_uid, user_edited, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at, tam.actual_minutes FROM tasks LEFT JOIN task_actual_minutes tam ON tam.task_id = tasks.id";
pub(crate) const SELECT_TASK_BY_ID: &str = "SELECT id, display_id, title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, habit_id, ical_uid, user_edited, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at, tam.actual_minutes FROM tasks LEFT JOIN task_actual_minutes tam ON tam.task_id = tasks.id WHERE tasks.id = ?";

const MIGRATION_001: &str = include_str!("../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_google_cal.sql");
const MIGRATION_003: &str = include_str!("../migrations/003_settings.sql");
const MIGRATION_004: &str = include_str!("../migrations/004_indexes.sql");
const MIGRATION_005: &str = include_str!("../migrations/005_task_display_id.sql");
const MIGRATION_006: &str = include_str!("../migrations/006_user_edited.sql");
const MIGRATION_007: &str = include_str!("../migrations/007_task_display_id_seq.sql");
const MIGRATION_010: &str = include_str!("../migrations/010_habit_pauses.sql");
const MIGRATION_012: &str = include_str!("../migrations/012_window_mode.sql");
const MIGRATION_013: &str = include_str!("../migrations/013_habit_task_display_id.sql");
const MIGRATION_014: &str = include_str!("../migrations/014_workload.sql");
const MIGRATION_015: &str = include_str!("../migrations/015_skills.sql");
const MIGRATION_016: &str = include_str!("../migrations/016_memory.sql");
const MIGRATION_017: &str = include_str!("../migrations/017_solver.sql");
const MIGRATION_018: &str = include_str!("../migrations/018_progress.sql");
const MIGRATION_019: &str = include_str!("../migrations/019_jwt.sql");
const MIGRATION_020: &str = include_str!("../migrations/020_task_actual_minutes_view.sql");
const MIGRATION_021: &str = include_str!("../migrations/021_solver_default_sa.sql");
const MIGRATION_022: &str = include_str!("../migrations/022_split_from_task_index.sql");
const MIGRATION_023: &str = include_str!("../migrations/023_timestamp_format.sql");
const MIGRATION_024: &str = include_str!("../migrations/024_zero_quantity_to_null.sql");
const MIGRATION_025: &str = include_str!("../migrations/025_task_normalized_title.sql");
const MIGRATION_026: &str = include_str!("../migrations/026_work_sessions.sql");
const MIGRATION_027: &str = include_str!("../migrations/027_horizon.sql");
const MIGRATION_028: &str = include_str!("../migrations/028_task_comments.sql");
const MIGRATION_029: &str = include_str!("../migrations/029_task_note_comments.sql");
const MIGRATION_030: &str = include_str!("../migrations/030_gcal_reminder_minutes.sql");
const MIGRATION_031: &str = include_str!("../migrations/031_gcal_event_defaults.sql");
const MIGRATION_032: &str = include_str!("../migrations/032_estimator_state.sql");

mod work_session;

// Migration 013 one-time backfill: drops the old global unique index, renumbers
// existing habit tasks to start from 1 per habit, and seeds the per-habit
// sequences. Non-idempotent (DROP + UPDATE renumber) — guarded by a check
// in `init` that only runs this when habit_task_display_id_seq is empty.
const MIGRATION_013_BACKFILL: &str = "
-- Drop the old global unique index so habit tasks can use per-habit sequences.
DROP INDEX IF EXISTS idx_tasks_display_id;

-- Renumber existing habit tasks so each habit starts from 1, ordered by
-- creation time (then id as tiebreaker). This gives the clean h1#1, h1#2, ...
-- numbering instead of retaining old global-sequence values (e.g. h1#47).
UPDATE tasks SET display_id = (
    SELECT COUNT(*) + 1 FROM tasks t2
    WHERE t2.habit_id = tasks.habit_id
      AND (t2.created_at < tasks.created_at
           OR (t2.created_at = tasks.created_at AND t2.id < tasks.id))
) WHERE habit_id IS NOT NULL;

-- Initialize sequences for existing habits based on max display_id.
-- Uses MAX (not COUNT) to avoid reusing display_ids after task deletion (#186).
INSERT OR IGNORE INTO habit_task_display_id_seq (habit_id, next_id)
SELECT
    habit_id,
    COALESCE(MAX(display_id), 0) + 1
FROM tasks
WHERE habit_id IS NOT NULL
GROUP BY habit_id;
";
// Migration 011 creates the habit_steps table (idempotent — uses IF NOT EXISTS
// for both the table and the index). The `ALTER TABLE tasks ADD COLUMN
// habit_step_id` is not idempotent (SQLite has no IF NOT EXISTS for ADD
// COLUMN), so it is run conditionally in `init` below.
const MIGRATION_011_TABLE: &str = "CREATE TABLE IF NOT EXISTS habit_steps (
    id              TEXT PRIMARY KEY,
    habit_id        TEXT NOT NULL REFERENCES habits(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL,
    title           TEXT NOT NULL,
    description     TEXT,
    start_time      TEXT NOT NULL,
    end_time        TEXT NOT NULL,
    avg_minutes     INTEGER NOT NULL,
    sigma_minutes   INTEGER NOT NULL DEFAULT 0,
    parallelizable  BOOLEAN NOT NULL DEFAULT 0,
    allows_parallel BOOLEAN NOT NULL DEFAULT 0,
    abandonability  REAL NOT NULL DEFAULT 0.0,
    fixed           BOOLEAN NOT NULL DEFAULT 0,
    depends_on      TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_habit_steps_habit ON habit_steps(habit_id);";

// Migration 009 adds habits.display_id. The ALTER TABLE is not idempotent
// (SQLite has no IF NOT EXISTS for ADD COLUMN), so it is run conditionally
// in `init`. The backfill, index, and sequence-table statements below are
// idempotent and run unconditionally. See `MIGRATION_009_BACKFILL`.
const MIGRATION_009_BACKFILL: &str = "
-- Backfill existing rows with sequential numbers ordered by creation time.
UPDATE habits SET display_id = (
    SELECT COUNT(*) + 1 FROM habits h2
    WHERE h2.created_at < habits.created_at
       OR (h2.created_at = habits.created_at AND h2.id < habits.id)
) WHERE display_id = 0;

-- Unique only for real (non-zero) display_ids.
CREATE UNIQUE INDEX IF NOT EXISTS idx_habits_display_id ON habits(display_id) WHERE display_id != 0;

-- Monotonic display_id sequence — prevents reuse after habit deletion.
CREATE TABLE IF NOT EXISTS habit_display_id_seq (
    next_id INTEGER NOT NULL
);

-- Initialize from the current maximum display_id (or 1 if no habits exist).
INSERT INTO habit_display_id_seq (next_id)
SELECT COALESCE(MAX(display_id), 0) + 1 FROM habits
WHERE (SELECT COUNT(*) FROM habit_display_id_seq) = 0;
";

pub struct SqliteStorage {
    pool: SqlitePool,
    jwt_secret: String,
}

impl SqliteStorage {
    pub async fn init(cfg: &LocalConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let url = ensure_create_mode(cfg.db_url());

        let jwt_secret = if !cfg.jwt_secret.is_empty() {
            cfg.jwt_secret.clone()
        } else if let Some(v) = std::env::var("TAKUSU_JWT_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
        {
            v
        } else {
            return Err(
                "TAKUSU_JWT_SECRET (or jwt_secret in LocalConfig) is required for sqlite storage"
                    .into(),
            );
        };

        if let Some(path) = extract_db_path(&url)
            && let Some(parent) = std::path::Path::new(&path).parent()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create db directory {}: {e}", parent.display()))?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::raw_sql("PRAGMA foreign_keys = ON")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await?;

        sqlx::raw_sql(MIGRATION_001).execute(&pool).await?;
        sqlx::raw_sql(MIGRATION_002).execute(&pool).await?;
        sqlx::raw_sql(MIGRATION_003).execute(&pool).await?;
        sqlx::raw_sql(MIGRATION_004).execute(&pool).await?;

        // Migration 005 uses ALTER TABLE ADD COLUMN which is not idempotent
        // (SQLite has no IF NOT EXISTS for ADD COLUMN). Check whether the
        // display_id column already exists before running it.
        let has_display_id: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('tasks') WHERE name = 'display_id'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_display_id {
            sqlx::raw_sql(MIGRATION_005).execute(&pool).await?;
        }
        let has_user_edited: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('tasks') WHERE name = 'user_edited'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_user_edited {
            sqlx::raw_sql(MIGRATION_006).execute(&pool).await?;
        }

        // Migration 007 creates the display_id sequence table (idempotent).
        sqlx::raw_sql(MIGRATION_007).execute(&pool).await?;

        // Migration 008 adds fixed column to habits and tasks (not idempotent).
        // SQLite has no IF NOT EXISTS for ADD COLUMN, so check each table
        // separately and run only the missing ALTER. This recovers from a
        // partial migration 008 failure where one table was altered but the
        // other was not (e.g. a crash mid-migration).
        let has_tasks_fixed: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('tasks') WHERE name = 'fixed'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_tasks_fixed {
            sqlx::query("ALTER TABLE tasks ADD COLUMN fixed BOOLEAN NOT NULL DEFAULT 0")
                .execute(&pool)
                .await?;
        }
        let has_habits_fixed: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('habits') WHERE name = 'fixed'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_habits_fixed {
            sqlx::query("ALTER TABLE habits ADD COLUMN fixed BOOLEAN NOT NULL DEFAULT 0")
                .execute(&pool)
                .await?;
        }

        // Migration 009 adds habits.display_id (not idempotent — SQLite has no
        // IF NOT EXISTS for ADD COLUMN). The backfill, index, and sequence
        // table statements in 009 are idempotent, but the ALTER is not, so we
        // split the migration: run the ALTER only when the column is missing,
        // then run the rest of 009 unconditionally.
        let has_habits_display_id: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('habits') WHERE name = 'display_id'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_habits_display_id {
            sqlx::query("ALTER TABLE habits ADD COLUMN display_id INTEGER NOT NULL DEFAULT 0")
                .execute(&pool)
                .await?;
        }
        // Backfill + index + sequence table (idempotent statements).
        sqlx::raw_sql(MIGRATION_009_BACKFILL).execute(&pool).await?;

        // Migration 010 creates the legacy habit_pauses table (idempotent — uses
        // IF NOT EXISTS for both the table and the index).
        sqlx::raw_sql(MIGRATION_010).execute(&pool).await?;

        // Rename the legacy habit_pauses table to habit_scheduled_spans (#503).
        // This runs once per database and is idempotent: it only acts when the
        // old table exists and the new one does not.
        let has_old_habit_pauses: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='habit_pauses'",
        )
        .fetch_one(&pool)
        .await?;
        let has_new_habit_scheduled_spans: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='habit_scheduled_spans'",
        )
        .fetch_one(&pool)
        .await?;
        if has_old_habit_pauses && !has_new_habit_scheduled_spans {
            sqlx::query("ALTER TABLE habit_pauses RENAME TO habit_scheduled_spans")
                .execute(&pool)
                .await?;
            sqlx::query(
                "CREATE INDEX IF NOT EXISTS idx_habit_scheduled_spans_habit ON habit_scheduled_spans(habit_id)",
            )
            .execute(&pool)
            .await?;
            sqlx::query("DROP INDEX IF EXISTS idx_habit_pauses_habit")
                .execute(&pool)
                .await?;
        }

        // Migration 011 creates the habit_steps table (idempotent) and adds
        // tasks.habit_step_id (not idempotent — guarded by a column check).
        sqlx::raw_sql(MIGRATION_011_TABLE).execute(&pool).await?;
        let has_habit_step_id: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('tasks') WHERE name = 'habit_step_id'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_habit_step_id {
            sqlx::query("ALTER TABLE tasks ADD COLUMN habit_step_id TEXT")
                .execute(&pool)
                .await?;
        }

        // Migration 012 adds habits.window_mode (not idempotent — SQLite has
        // no IF NOT EXISTS for ADD COLUMN). The column defaults to 'day' so
        // existing habits keep the legacy per-day window behavior.
        let has_window_mode: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('habits') WHERE name = 'window_mode'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_window_mode {
            sqlx::raw_sql(MIGRATION_012).execute(&pool).await?;
        }

        // Migration 013 creates habit_task_display_id_seq table and scoped
        // indexes (idempotent). The one-time backfill (drop old index, renumber
        // habit tasks, seed sequences) is non-idempotent and guarded by a
        // check: only run when the seq table exists but has no rows.
        sqlx::raw_sql(MIGRATION_013).execute(&pool).await?;
        let seq_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM habit_task_display_id_seq")
            .fetch_one(&pool)
            .await?;
        if seq_count == 0 {
            sqlx::raw_sql(MIGRATION_013_BACKFILL).execute(&pool).await?;
        }

        // Migration 014 adds workload columns to settings (idempotent).
        let has_workload: bool = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('settings') WHERE name = 'comfortable_minutes'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_workload {
            sqlx::raw_sql(MIGRATION_014).execute(&pool).await?;
        }

        // Migration 015 creates the skills table (idempotent).
        sqlx::raw_sql(MIGRATION_015).execute(&pool).await?;

        // Migration 016 creates the memory tables (idempotent).
        sqlx::raw_sql(MIGRATION_016).execute(&pool).await?;

        // Migration 017 adds solver/time_budget_ms/seed/warm_start to settings
        // (idempotent, guarded by a column check).
        let has_solver: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('settings') WHERE name = 'solver'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_solver {
            sqlx::raw_sql(MIGRATION_017).execute(&pool).await?;
        }

        // Migration 018 adds progress columns and tables (idempotent, guarded
        // by a column check).
        let has_quantity_total: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('tasks') WHERE name = 'quantity_total'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_quantity_total {
            sqlx::raw_sql(MIGRATION_018).execute(&pool).await?;
        }

        // Migration 019 migrates tokens to JWT metadata (jti, scope, expires_at).
        let has_jti: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('tokens') WHERE name = 'jti'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_jti {
            sqlx::raw_sql(MIGRATION_019).execute(&pool).await?;
        }

        // Migration 020 creates a view that pre-computes per-task active work
        // minutes from work_sessions (idempotent).
        sqlx::raw_sql(MIGRATION_020).execute(&pool).await?;

        // Migration 021 changes the default solver from 'auto' to 'sa' for
        // existing rows (idempotent).
        sqlx::raw_sql(MIGRATION_021).execute(&pool).await?;

        // Migration 022 creates a partial index on tasks.split_from_task_id
        // for efficient split-task cleanup (idempotent).
        sqlx::raw_sql(MIGRATION_022).execute(&pool).await?;

        // Migration 023 normalizes legacy timestamp strings to whole-second RFC 3339.
        sqlx::raw_sql(MIGRATION_023).execute(&pool).await?;

        // Migration 024 normalizes quantity_total == 0 to NULL (same as unset).
        sqlx::raw_sql(MIGRATION_024).execute(&pool).await?;

        // Migration 025 adds tasks.normalized_title for similar-task pre-filtering
        // (#942). The ALTER is not idempotent (SQLite has no IF NOT EXISTS for ADD
        // COLUMN), so guard with a column check.
        let has_normalized_title: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('tasks') WHERE name = 'normalized_title'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_normalized_title {
            sqlx::raw_sql(MIGRATION_025).execute(&pool).await?;
            // Backfill normalized_title for rows that existed before migration 025.
            // NFKC normalization cannot run in SQL, so it happens here in Rust,
            // once, inside a single transaction (atomic, and no per-row commit
            // overhead on large databases). Titles that fail to normalize (e.g.
            // control-character only) are left NULL, which excludes them from
            // similar-task search rather than storing a misleading empty string.
            // This runs only at migration time, so NULL rows are not re-scanned on
            // every startup.
            let stale: Vec<(String, String)> =
                sqlx::query_as("SELECT id, title FROM tasks WHERE normalized_title IS NULL")
                    .fetch_all(&pool)
                    .await?;
            if !stale.is_empty() {
                let mut tx = pool.begin().await?;
                for (id, title) in stale {
                    if let Ok(nt) = takusu_search::memory::normalize_text(
                        &title,
                        Some(takusu_search::memory::MAX_CONTENT_SCALARS),
                    ) {
                        sqlx::query("UPDATE tasks SET normalized_title = ? WHERE id = ?")
                            .bind(&nt)
                            .bind(&id)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
                tx.commit().await?;
            }
        }

        // Migration 026: work sessions become top-level entities with optional
        // task attachments. Guarded by the presence of the new work_sessions table.
        let has_work_sessions: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='work_sessions'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_work_sessions {
            sqlx::raw_sql(MIGRATION_026).execute(&pool).await?;
        }

        // Migration 027: horizon support — plan_length_days setting and
        // horizon_task_ids column on schedules.
        let has_plan_length: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('settings') WHERE name='plan_length_days'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_plan_length {
            sqlx::raw_sql(MIGRATION_027).execute(&pool).await?;
        }

        // Migration 028 creates the task_comments and comment_operations
        // tables (idempotent).
        sqlx::raw_sql(MIGRATION_028).execute(&pool).await?;

        // Migration 029 (WI-2): migrate task_note memories to comments and
        // drop 'task_note' from the memories kind CHECK constraint. The guard
        // is the CHECK constraint itself: once the table is rebuilt it no
        // longer mentions task_note, so this runs exactly once per database.
        // The whole migration runs inside a transaction so a failure mid-way
        // cannot leave a partial comment backfill or a half-rebuilt table.
        let memories_sql: Option<String> = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='memories'",
        )
        .fetch_optional(&pool)
        .await?;
        if memories_sql
            .as_deref()
            .is_some_and(|sql| sql.contains("task_note"))
        {
            let mut tx = pool.begin().await?;
            sqlx::raw_sql(MIGRATION_029).execute(&mut *tx).await?;
            tx.commit().await?;
        }

        // Migration 030 adds google_cal_settings.reminder_minutes (#1470).
        // SQLite has no IF NOT EXISTS for ADD COLUMN, so guard by column check.
        let has_reminder_minutes: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('google_cal_settings') WHERE name = 'reminder_minutes'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_reminder_minutes {
            sqlx::raw_sql(MIGRATION_030).execute(&pool).await?;
        }

        // Migration 031 adds google_cal_settings.color_id, visibility, and
        // transparency (#1470). SQLite has no IF NOT EXISTS for ADD COLUMN,
        // so guard by the color_id column.
        let has_color_id: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('google_cal_settings') WHERE name = 'color_id'",
        )
        .fetch_one(&pool)
        .await?;
        if !has_color_id {
            sqlx::raw_sql(MIGRATION_031).execute(&pool).await?;
        }

        sqlx::raw_sql(MIGRATION_032).execute(&pool).await?;

        Ok(Self { pool, jwt_secret })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

fn ensure_create_mode(db_url: &str) -> String {
    if !db_url.contains("mode=") {
        let separator = if db_url.contains('?') { '&' } else { '?' };
        format!("{db_url}{separator}mode=rwc")
    } else {
        db_url.to_string()
    }
}

fn extract_db_path(db_url: &str) -> Option<String> {
    let path = db_url.strip_prefix("sqlite:")?;
    if path.is_empty() || path.starts_with(':') {
        return None;
    }
    let path = path.split('?').next().unwrap();
    Some(path.to_string())
}

pub(crate) fn map_err(e: sqlx::Error) -> StorageError {
    StorageError::Internal(e.to_string())
}

#[async_trait]
impl Storage for SqliteStorage {
    /// Verify a JWT and, for non-root tokens, check the jti is not revoked.
    async fn verify_token(&self, token: &str) -> StorageResult<Option<takusu_types::TokenClaims>> {
        let claims = match takusu_types::jwt::verify(&self.jwt_secret, token, DEFAULT_AUD) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("JWT verification failed: {e}");
                return Ok(None);
            }
        };

        // Root tokens are self-authenticating via the signature and scope claim.
        if claims.is_root() {
            return Ok(Some(claims));
        }

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tokens WHERE jti = ? AND revoked_at IS NULL")
                .bind(&claims.jti)
                .fetch_one(&self.pool)
                .await
                .map_err(map_err)?;
        Ok(if count > 0 { Some(claims) } else { None })
    }

    async fn list_tasks(&self, query: &TaskQuery) -> StorageResult<Vec<TaskRow>> {
        let mut sql = format!("{SELECT_TASKS} WHERE 1=1");
        let mut bindings: Vec<String> = Vec::new();
        if let Some(status) = query.status {
            if status == TaskStatusFilter::Overdue {
                sql.push_str(" AND ");
                sql.push_str(OVERDUE_SQL);
            } else if status == TaskStatusFilter::Actionable {
                sql.push_str(" AND ");
                sql.push_str(ACTIONABLE_SQL);
            } else {
                sql.push_str(" AND status = ?");
                bindings.push(status.as_str().to_string());
            }
        }
        if let Some(v) = query.from {
            sql.push_str(" AND end_at >= ?");
            bindings.push(v.to_string());
        }
        if let Some(v) = query.until {
            // start_at is nullable: NULL <= value evaluates to NULL
            // (excluded). Include tasks with no explicit start time so
            // range queries don't silently drop them.
            sql.push_str(" AND (start_at IS NULL OR start_at <= ?)");
            bindings.push(v.to_string());
        }
        if query.no_overdue == Some(true) {
            sql.push_str(" AND ");
            sql.push_str(NOT_OVERDUE_SQL);
        }
        if let Some(ref v) = query.habit_id {
            sql.push_str(" AND habit_id = ?");
            bindings.push(v.clone());
        }
        if let Some(ref v) = query.ical_uid {
            sql.push_str(" AND ical_uid = ?");
            bindings.push(v.clone());
        }
        sql.push_str(" ORDER BY created_at DESC");

        // Apply a SQL-level limit only when no post-fetch query filter is needed;
        // otherwise we must filter all candidates before truncating.
        let post_filter_limit = if query.q.is_some() {
            query.limit
        } else {
            if let Some(limit) = query.limit {
                sql.push_str(" LIMIT ?");
                bindings.push(limit.to_string());
            }
            None
        };

        let mut q = sqlx::query_as::<_, TaskRow>(sqlx::AssertSqlSafe(sql.as_str()));
        for b in &bindings {
            q = q.bind(b);
        }
        let mut rows = q.fetch_all(&self.pool).await.map_err(map_err)?;

        if let Some(ref qstr) = query.q {
            rows = filter_rows_with_query(self, rows, qstr).await?;
        }

        if let Some(limit) = post_filter_limit {
            rows.truncate(limit as usize);
        }

        Ok(rows)
    }

    async fn task_exists_by_ical_uid(&self, uid: &str) -> StorageResult<bool> {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM tasks WHERE ical_uid = ? LIMIT 1")
                .bind(uid)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_err)?;
        Ok(exists.is_some())
    }

    async fn get_task(&self, id: &str) -> StorageResult<TaskRow> {
        let full = resolve_task_id(&self.pool, id).await?;
        sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(&full)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound(format!("task {id} not found")),
                other => StorageError::Internal(other.to_string()),
            })
    }

    async fn create_task(&self, body: &CreateTask) -> StorageResult<TaskRow> {
        // Treat quantity_total / original_quantity_total 0 as unset (same as None) server-side.
        let quantity_total = body.quantity_total.filter(|t| *t != 0);
        let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
        validate_quantity(quantity_total, body.quantity_done, original_quantity_total)?;
        let id = uuid::Uuid::now_v7().to_string();
        let resolved_depends = resolve_depends(&self.pool, body.depends.as_deref()).await?;
        let depends = takusu_types::DependencyList::new(resolved_depends);
        // sigma 未指定時は avg の 20% をデフォルトにする (確定タスクでない限りある程度バッファを見込む)
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let fixed = body.fixed.unwrap_or(false);
        // Atomically reserve a monotonic display_id from the sequence table.
        // This prevents display_id reuse after task deletion (#186).
        // For habit tasks, use a habit-specific sequence (#380).
        let display_id: i64 = if let Some(ref habit_id) = body.habit_id {
            // Use habit-specific sequence. Ensure the sequence entry exists first.
            sqlx::query(
                "INSERT OR IGNORE INTO habit_task_display_id_seq (habit_id, next_id) VALUES (?1, 1)",
            )
            .bind(habit_id)
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
            sqlx::query_scalar(
                "UPDATE habit_task_display_id_seq SET next_id = next_id + 1 WHERE habit_id = ?1 RETURNING next_id - 1",
            )
            .bind(habit_id)
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?
        } else {
            // Use global task sequence
            sqlx::query_scalar(
                "UPDATE task_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1",
            )
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?
        };
        let quantity_done = body.quantity_done.unwrap_or_default();
        let quantity_unit = body.quantity_unit.as_deref();
        // A title that fails NFKC normalization (e.g. control-character only)
        // stores NULL, excluding the task from similar-task search rather than
        // matching on a misleading empty string (#942).
        let normalized_title = takusu_search::memory::normalize_text(
            &body.title,
            Some(takusu_search::memory::MAX_CONTENT_SCALARS),
        )
        .ok();
        sqlx::query(
            "INSERT INTO tasks (id, display_id, title, normalized_title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))"
        )
        .bind(&id)
        .bind(display_id)
        .bind(&body.title)
        .bind(&normalized_title)
        .bind(&body.description)
        .bind(body.start_at)
        .bind(body.end_at)
        .bind(body.avg_minutes)
        .bind(sigma)
        .bind(&depends)
        .bind(parallelizable)
        .bind(allows_parallel)
        .bind(abandonability)
        .bind(&body.ical_uid)
        .bind(&body.habit_id)
        .bind(fixed)
        .bind(&body.habit_step_id)
        .bind(quantity_total)
        .bind(quantity_done)
        .bind(quantity_unit)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(original_quantity_total)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn update_task(&self, id: &str, body: &UpdateTask) -> StorageResult<TaskRow> {
        let depends = if let Some(ref deps) = body.depends {
            let resolved = resolve_depends(&self.pool, Some(deps)).await?;
            Some(takusu_types::DependencyList::new(resolved))
        } else {
            None
        };

        let mut tx = self.pool.begin().await.map_err(map_err)?;
        let full = resolve_task_id(&mut *tx, id).await?;
        let existing = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(&full)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound(format!("task {id} not found")),
                other => StorageError::Internal(other.to_string()),
            })?;

        let plan = validate_task_update(body, &existing)?;
        update_task_fields(&mut *tx, &full, body, &plan, &depends).await?;
        if body.status.is_some() {
            handle_status_transition(&mut *tx, &full, &existing, plan.status).await?;
            // #1044: moving to a terminal status should close any open work
            // session so active time is not left dangling.
            if plan.status == TaskStatus::Skipped || plan.status == TaskStatus::Completed {
                work_session::cleanup_work_sessions(&mut *tx, &full).await?;
            }
        }

        let task: TaskRow = sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(&full)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        tx.commit().await.map_err(map_err)?;
        Ok(task)
    }

    async fn replace_task(&self, id: &str, body: &CreateTask) -> StorageResult<TaskRow> {
        // Treat quantity_total / original_quantity_total 0 as unset (same as None) server-side.
        let quantity_total = body.quantity_total.filter(|t| *t != 0);
        let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
        // Full replacement resets quantity_done to 0, so validate against that.
        validate_quantity(
            quantity_total,
            Some(Quantity::default()),
            original_quantity_total,
        )?;
        let full = resolve_task_id(&self.pool, id).await?;
        let resolved_depends = resolve_depends(&self.pool, body.depends.as_deref()).await?;
        let depends = takusu_types::DependencyList::new(resolved_depends);
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let fixed = body.fixed.unwrap_or(false);
        let quantity_unit = body.quantity_unit.as_deref();
        // A title that fails NFKC normalization (e.g. control-character only)
        // stores NULL, excluding the task from similar-task search rather than
        // matching on a misleading empty string (#942).
        let normalized_title = takusu_search::memory::normalize_text(
            &body.title,
            Some(takusu_search::memory::MAX_CONTENT_SCALARS),
        )
        .ok();
        sqlx::query(
            "UPDATE tasks SET title=?, normalized_title=?, description=?, start_at=?, end_at=?, avg_minutes=?, sigma_minutes=?, depends=?, parallelizable=?, allows_parallel=?, abandonability=?, habit_id=COALESCE(?,habit_id), fixed=?, habit_step_id=?, quantity_total=COALESCE(?, quantity_total), quantity_done=0, quantity_unit=COALESCE(?, quantity_unit), completed_at=NULL, split_from_task_id=COALESCE(?, split_from_task_id), original_quantity_total=COALESCE(?, original_quantity_total), status='pending', user_edited = CASE WHEN habit_id IS NOT NULL THEN true ELSE user_edited END, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?"
        )
        .bind(&body.title)
        .bind(&normalized_title)
        .bind(&body.description)
        .bind(body.start_at)
        .bind(body.end_at)
        .bind(body.avg_minutes)
        .bind(sigma)
        .bind(&depends)
        .bind(parallelizable)
        .bind(allows_parallel)
        .bind(abandonability)
        .bind(&body.habit_id)
        .bind(fixed)
        .bind(&body.habit_step_id)
        .bind(quantity_total)
        .bind(quantity_unit)
        .bind(None::<String>)
        .bind(original_quantity_total)
        .bind(&full)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        sqlx::query_as::<_, TaskRow>(SELECT_TASK_BY_ID)
            .bind(&full)
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn delete_task(&self, id: &str) -> StorageResult<()> {
        let full = resolve_task_id(&self.pool, id).await?;
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        // Break split-task self-references so deleting a parent does not fail
        // on the tasks.split_from_task_id foreign key.
        sqlx::query("UPDATE tasks SET split_from_task_id = NULL WHERE split_from_task_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        // Delete child rows explicitly so deletion is deterministic even if the
        // foreign_keys pragma is temporarily disabled.
        sqlx::query("DELETE FROM google_cal_events WHERE task_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("DELETE FROM task_comments WHERE task_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        work_session::cleanup_work_sessions(&mut *tx, &full).await?;
        sqlx::query("UPDATE work_sessions SET task_id = NULL WHERE task_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("UPDATE progress_events SET task_id = NULL WHERE task_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    // ── Task comments (WI-1) ────────────────────────────────

    async fn list_comments(&self, task_id: &str) -> StorageResult<Vec<CommentRow>> {
        let full = resolve_task_id(&self.pool, task_id).await?;
        sqlx::query_as::<_, CommentRow>(
            "SELECT * FROM task_comments WHERE task_id = ? ORDER BY seq ASC",
        )
        .bind(&full)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn create_comment(
        &self,
        task_id: &str,
        author: CommentAuthor,
        content: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<CommentRow> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        let full = resolve_task_id(&mut *tx, task_id).await?;

        let payload = format!("create:{full}:{}:{content}", author.as_str());
        let hash = comment_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) = Self::check_comment_idempotency(&mut *tx, op_id, &hash).await?
        {
            return stored;
        }

        let id = uuid::Uuid::now_v7().to_string();
        let seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM task_comments WHERE task_id = ?",
        )
        .bind(&full)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

        sqlx::query(
            "INSERT INTO task_comments (id, task_id, author, content, seq, created_at) VALUES (?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(&id)
        .bind(&full)
        .bind(author.as_str())
        .bind(content)
        .bind(seq)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

        let row: CommentRow = sqlx::query_as("SELECT * FROM task_comments WHERE id = ?")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        if let Some(op_id) = operation_id {
            Self::record_comment_operation(&mut *tx, op_id, &hash, &row).await?;
        }
        tx.commit().await.map_err(map_err)?;
        Ok(row)
    }

    async fn delete_comment(&self, id: &str) -> StorageResult<()> {
        let result = sqlx::query("DELETE FROM task_comments WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!("comment {id} not found")));
        }
        Ok(())
    }

    async fn list_habits(&self) -> StorageResult<Vec<HabitRow>> {
        sqlx::query_as::<_, HabitRow>("SELECT * FROM habits ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn get_habit(&self, id: &str) -> StorageResult<HabitRow> {
        let full = resolve_habit_id(&self.pool, id).await?;
        sqlx::query_as::<_, HabitRow>("SELECT * FROM habits WHERE id = ?")
            .bind(&full)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound(format!("habit {id} not found")),
                other => StorageError::Internal(other.to_string()),
            })
    }

    async fn create_habit(&self, body: &CreateHabit) -> StorageResult<HabitRow> {
        let id = uuid::Uuid::now_v7().to_string();
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let fixed = body.fixed.unwrap_or(false);
        let window_mode = body.window_mode.unwrap_or(WindowMode::Day);
        // Atomically reserve a monotonic display_id from the sequence table
        // (mirrors tasks.display_id, issue #186 / #305).
        let display_id: i64 = sqlx::query_scalar(
            "UPDATE habit_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(map_err)?;
        sqlx::query(
            "INSERT INTO habits (id, display_id, title, description, recurrence, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, active, fixed, window_mode, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))"
        )
        .bind(&id)
        .bind(display_id)
        .bind(&body.title)
        .bind(&body.description)
        .bind(&body.recurrence)
        .bind(body.start_time)
        .bind(body.end_time)
        .bind(body.avg_minutes)
        .bind(sigma)
        .bind(parallelizable)
        .bind(allows_parallel)
        .bind(abandonability)
        .bind(fixed)
        .bind(window_mode.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        sqlx::query_as::<_, HabitRow>("SELECT * FROM habits WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn update_habit(&self, id: &str, body: &UpdateHabit) -> StorageResult<HabitRow> {
        let full = resolve_habit_id(&self.pool, id).await?;
        sqlx::query(
            "UPDATE habits SET title=COALESCE(?,title), description=COALESCE(?,description), recurrence=COALESCE(?,recurrence), start_time=COALESCE(?,start_time), end_time=COALESCE(?,end_time), avg_minutes=COALESCE(?,avg_minutes), sigma_minutes=COALESCE(?,sigma_minutes), parallelizable=COALESCE(?,parallelizable), allows_parallel=COALESCE(?,allows_parallel), abandonability=COALESCE(?,abandonability), active=COALESCE(?,active), fixed=COALESCE(?,fixed), window_mode=COALESCE(?,window_mode), updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?"
        )
        .bind(body.title.as_ref())
        .bind(body.description.as_ref())
        .bind(body.recurrence.as_ref())
        .bind(body.start_time.as_ref())
        .bind(body.end_time.as_ref())
        .bind(body.avg_minutes)
        .bind(body.sigma_minutes)
        .bind(body.parallelizable)
        .bind(body.allows_parallel)
        .bind(body.abandonability)
        .bind(body.active)
        .bind(body.fixed)
        .bind(body.window_mode.map(|w| w.to_string()))
        .bind(&full)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        sqlx::query_as::<_, HabitRow>("SELECT * FROM habits WHERE id = ?")
            .bind(&full)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound(format!("habit {id} not found")),
                other => StorageError::Internal(other.to_string()),
            })
    }

    async fn replace_habit(&self, id: &str, body: &CreateHabit) -> StorageResult<HabitRow> {
        let full = resolve_habit_id(&self.pool, id).await?;
        let sigma = body
            .sigma_minutes
            .unwrap_or(Minutes(body.avg_minutes).to_slots().0.max(1));
        let parallelizable = body.parallelizable.unwrap_or(false);
        let allows_parallel = body.allows_parallel.unwrap_or(false);
        let abandonability = body.abandonability.unwrap_or(0.5.into());
        let fixed = body.fixed.unwrap_or(false);
        let window_mode = body.window_mode.unwrap_or(WindowMode::Day);
        sqlx::query(
            "UPDATE habits SET title=?, description=?, recurrence=?, start_time=?, end_time=?, avg_minutes=?, sigma_minutes=?, parallelizable=?, allows_parallel=?, abandonability=?, fixed=?, window_mode=?, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?"
        )
        .bind(&body.title)
        .bind(&body.description)
        .bind(&body.recurrence)
        .bind(body.start_time)
        .bind(body.end_time)
        .bind(body.avg_minutes)
        .bind(sigma)
        .bind(parallelizable)
        .bind(allows_parallel)
        .bind(abandonability)
        .bind(fixed)
        .bind(window_mode.to_string())
        .bind(&full)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        sqlx::query_as::<_, HabitRow>("SELECT * FROM habits WHERE id = ?")
            .bind(&full)
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn delete_habit(&self, id: &str) -> StorageResult<()> {
        // Delete tasks referencing this habit before deleting the habit,
        // so the foreign-key constraint (enforced on D1 and now on SQLite)
        // does not block deletion of habits that have already generated tasks
        // (#240). The client confirms with the user before issuing the delete
        // when there are associated tasks. All statements run in a single
        // transaction so a partial failure cannot leave the database with
        // tasks deleted but the habit still present.
        let full = resolve_habit_id(&self.pool, id).await?;
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        // Break split-task self-references that point to any task about to be
        // deleted, including split-off tasks that live outside this habit.
        sqlx::query(
            "UPDATE tasks SET split_from_task_id = NULL WHERE split_from_task_id IN (SELECT id FROM tasks WHERE habit_id = ?)",
        )
        .bind(&full)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;
        // Delete child rows explicitly so deletion is deterministic even if the
        // foreign_keys pragma is temporarily disabled.
        sqlx::query("DELETE FROM google_cal_events WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?)")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        let now = takusu_types::now_rfc3339();
        sqlx::query("UPDATE work_sessions SET ended_at = ? WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?) AND ended_at IS NULL")
            .bind(&now)
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("UPDATE work_sessions SET task_id = NULL WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?)")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("UPDATE progress_events SET task_id = NULL WHERE task_id IN (SELECT id FROM tasks WHERE habit_id = ?)")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("DELETE FROM tasks WHERE habit_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        // habit_scheduled_spans and habit_steps have ON DELETE CASCADE, but keep
        // the explicit deletes for parity with D1 and in case the pragma is off
        // (#303 / #95).
        sqlx::query("DELETE FROM habit_scheduled_spans WHERE habit_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("DELETE FROM habit_steps WHERE habit_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        // habit_task_display_id_seq has no foreign key, so it must be deleted
        // explicitly (#380).
        sqlx::query("DELETE FROM habit_task_display_id_seq WHERE habit_id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        sqlx::query("DELETE FROM habits WHERE id = ?")
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn list_habit_scheduled_spans(
        &self,
        habit_id: &str,
    ) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        let full = resolve_habit_id(&self.pool, habit_id).await?;
        sqlx::query_as::<_, HabitScheduledSpanRow>(
            "SELECT * FROM habit_scheduled_spans WHERE habit_id = ? ORDER BY start_date ASC, created_at ASC",
        )
        .bind(&full)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn list_all_habit_scheduled_spans(&self) -> StorageResult<Vec<HabitScheduledSpanRow>> {
        sqlx::query_as::<_, HabitScheduledSpanRow>(
            "SELECT * FROM habit_scheduled_spans ORDER BY habit_id, start_date ASC, created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn create_habit_scheduled_span(
        &self,
        habit_id: &str,
        body: &CreateHabitScheduledSpan,
    ) -> StorageResult<HabitScheduledSpanRow> {
        validate_scheduled_span_dates(&body.start_date, &body.end_date)?;
        let full = resolve_habit_id(&self.pool, habit_id).await?;
        let id = uuid::Uuid::now_v7().to_string();
        let now = takusu_types::now_rfc3339();
        sqlx::query(
            "INSERT INTO habit_scheduled_spans (id, habit_id, start_date, end_date, reason, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&full)
        .bind(body.start_date)
        .bind(body.end_date)
        .bind(body.reason.as_deref())
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        sqlx::query_as::<_, HabitScheduledSpanRow>(
            "SELECT * FROM habit_scheduled_spans WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn delete_habit_scheduled_span(
        &self,
        habit_id: &str,
        span_id: &str,
    ) -> StorageResult<()> {
        let full = resolve_habit_id(&self.pool, habit_id).await?;
        let result = sqlx::query("DELETE FROM habit_scheduled_spans WHERE id = ? AND habit_id = ?")
            .bind(span_id)
            .bind(&full)
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "scheduled span {span_id} not found for habit {habit_id}"
            )));
        }
        Ok(())
    }

    // ── Habit steps (#95) ─────────────────────────────────────────────────

    async fn list_habit_steps(&self, habit_id: &str) -> StorageResult<Vec<HabitStepRow>> {
        let full = resolve_habit_id(&self.pool, habit_id).await?;
        sqlx::query_as::<_, HabitStepRow>(
            "SELECT * FROM habit_steps WHERE habit_id = ? ORDER BY position ASC, created_at ASC",
        )
        .bind(&full)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn list_all_habit_steps(&self) -> StorageResult<Vec<HabitStepRow>> {
        sqlx::query_as::<_, HabitStepRow>(
            "SELECT * FROM habit_steps ORDER BY habit_id, position ASC, created_at ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn replace_habit_steps(
        &self,
        habit_id: &str,
        steps: &[HabitStepInput],
    ) -> StorageResult<Vec<HabitStepRow>> {
        let full = resolve_habit_id(&self.pool, habit_id).await?;
        let mut tx = self.pool.begin().await.map_err(map_err)?;

        // Fetch existing step ids for this habit.
        let existing_ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM habit_steps WHERE habit_id = ?")
                .bind(&full)
                .fetch_all(&mut *tx)
                .await
                .map_err(map_err)?;
        let existing_set: std::collections::HashSet<&String> = existing_ids.iter().collect();

        // Track ids present in the input so we can delete the rest.
        let mut input_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let now = takusu_types::now_rfc3339();

        for s in steps {
            let id =
                s.id.clone()
                    .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
            input_ids.insert(id.clone());
            let sigma = s
                .sigma_minutes
                .unwrap_or(Minutes(s.avg_minutes).to_slots().0.max(1));
            let parallelizable = s.parallelizable.unwrap_or(false);
            let allows_parallel = s.allows_parallel.unwrap_or(false);
            let abandonability = s.abandonability.unwrap_or(0.5.into());
            let fixed = s.fixed.unwrap_or(false);
            let depends_on = takusu_types::DependencyList::new(s.depends_on.clone());

            if existing_set.contains(&id) {
                sqlx::query(
                    "UPDATE habit_steps SET position=?, title=?, description=?, start_time=?, end_time=?, avg_minutes=?, sigma_minutes=?, parallelizable=?, allows_parallel=?, abandonability=?, fixed=?, depends_on=? WHERE id = ? AND habit_id = ?",
                )
                .bind(s.position)
                .bind(&s.title)
                .bind(s.description.as_ref())
                .bind(s.start_time)
                .bind(s.end_time)
                .bind(s.avg_minutes)
                .bind(sigma)
                .bind(parallelizable)
                .bind(allows_parallel)
                .bind(abandonability)
                .bind(fixed)
                .bind(&depends_on)
                .bind(&id)
                .bind(&full)
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
            } else {
                sqlx::query(
                    "INSERT INTO habit_steps (id, habit_id, position, title, description, start_time, end_time, avg_minutes, sigma_minutes, parallelizable, allows_parallel, abandonability, fixed, depends_on, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(&full)
                .bind(s.position)
                .bind(&s.title)
                .bind(s.description.as_ref())
                .bind(s.start_time)
                .bind(s.end_time)
                .bind(s.avg_minutes)
                .bind(sigma)
                .bind(parallelizable)
                .bind(allows_parallel)
                .bind(abandonability)
                .bind(fixed)
                .bind(&depends_on)
                .bind(&now)
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
            }
        }

        // Delete existing steps not present in the input. Tasks referencing
        // them (via habit_step_id) are left in place; sync_habit_tasks cleans
        // them up on the next sync (pending + unedited only).
        for old_id in &existing_ids {
            if !input_ids.contains(old_id) {
                sqlx::query("DELETE FROM habit_steps WHERE id = ? AND habit_id = ?")
                    .bind(old_id)
                    .bind(&full)
                    .execute(&mut *tx)
                    .await
                    .map_err(map_err)?;
            }
        }

        tx.commit().await.map_err(map_err)?;

        sqlx::query_as::<_, HabitStepRow>(
            "SELECT * FROM habit_steps WHERE habit_id = ? ORDER BY position ASC, created_at ASC",
        )
        .bind(&full)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn apply_habit_estimate(
        &self,
        habit_id: &str,
        avg_minutes: i64,
        sigma_minutes: i64,
        step_estimates: &[HabitStepEstimateInput],
    ) -> StorageResult<()> {
        let full = resolve_habit_id(&self.pool, habit_id).await?;
        let mut tx = self.pool.begin().await.map_err(map_err)?;

        let fixed: bool = sqlx::query_scalar("SELECT fixed FROM habits WHERE id = ?")
            .bind(&full)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        if fixed {
            return Err(StorageError::BadRequest(
                "cannot apply estimate to fixed habit".into(),
            ));
        }

        for step in step_estimates {
            // Only update non-fixed steps; fixed steps are intentionally preserved.
            sqlx::query(
                "UPDATE habit_steps SET avg_minutes = ?, sigma_minutes = ? WHERE id = ? AND habit_id = ? AND fixed = 0",
            )
            .bind(step.avg_minutes)
            .bind(step.sigma_minutes)
            .bind(&step.step_id)
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        }

        sqlx::query("UPDATE habits SET avg_minutes = ?, sigma_minutes = ? WHERE id = ?")
            .bind(avg_minutes)
            .bind(sigma_minutes)
            .bind(&full)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;

        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn get_schedule(&self) -> StorageResult<Option<ScheduleRow>> {
        sqlx::query_as::<_, ScheduleRow>("SELECT * FROM schedules WHERE id = 'active'")
            .fetch_optional(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn save_schedule(&self, req: &SaveScheduleRequest) -> StorageResult<ScheduleRow> {
        let schedule = takusu_contracts::ScheduleData::new(req.entries.clone());
        let horizon_ids = takusu_types::JsonString::new(req.horizon_task_ids.clone());
        let now = takusu_types::now_rfc3339();
        // Wrap the schedule upsert and the task status updates in a single
        // transaction so a failure mid-way cannot leave the schedule saved
        // but some tasks still marked pending (#289).
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        sqlx::query(
            "INSERT INTO schedules (id, created_at, updated_at, schedule, horizon_task_ids) VALUES ('active', ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET schedule=excluded.schedule, horizon_task_ids=excluded.horizon_task_ids, updated_at=excluded.updated_at"
        )
        .bind(&now)
        .bind(&now)
        .bind(&schedule)
        .bind(&horizon_ids)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;
        for id in &req.mark_scheduled_task_ids {
            sqlx::query(
                "UPDATE tasks SET status = 'scheduled', updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
            )
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        }
        tx.commit().await.map_err(map_err)?;
        sqlx::query_as::<_, ScheduleRow>("SELECT * FROM schedules WHERE id = 'active'")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn clear_schedule(&self) -> StorageResult<()> {
        sqlx::query("DELETE FROM schedules WHERE id = 'active'")
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn create_token(&self, label: Option<&str>) -> StorageResult<TokenCreateResponse> {
        let label_opt = label.filter(|s| !s.is_empty());
        let (new_token, jti) = takusu_types::jwt::generate_token_jwt(
            &self.jwt_secret,
            SCOPE_READ_WRITE,
            label_opt,
            None,
        )
        .map_err(|e| StorageError::Internal(e.to_string()))?;
        let expires_at = token_expires_at(takusu_types::jwt::DEFAULT_TOKEN_TTL_SECONDS);
        sqlx::query(
            "INSERT INTO tokens (jti, scope, label, created_by, created_at, expires_at) VALUES (?, ?, ?, 'authenticated', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), ?)",
        )
        .bind(&jti)
        .bind(SCOPE_READ_WRITE)
        .bind(label_opt)
        .bind(&expires_at)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        let row: TokenRow = sqlx::query_as::<_, TokenRow>("SELECT * FROM tokens WHERE jti = ?")
            .bind(&jti)
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(TokenCreateResponse {
            id: row.id,
            token: new_token,
            scope: row.scope,
            label: row.label,
            created_at: row.created_at,
            expires_at: row.expires_at,
        })
    }

    async fn list_tokens(&self) -> StorageResult<Vec<TokenRow>> {
        sqlx::query_as::<_, TokenRow>("SELECT * FROM tokens ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn revoke_token(&self, id: i64) -> StorageResult<()> {
        let result = sqlx::query(
            "UPDATE tokens SET revoked_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "token {id} not found or already revoked"
            )));
        }
        Ok(())
    }

    async fn get_settings(&self) -> StorageResult<SettingsRow> {
        sqlx::query_as::<_, SettingsRow>("SELECT * FROM settings WHERE id = 'active'")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound("settings not found".into()),
                other => StorageError::Internal(other.to_string()),
            })
    }

    async fn update_settings(&self, body: &UpdateSettings) -> StorageResult<SettingsRow> {
        let existing = self.get_settings().await?;
        let tz = body.tz.clone().unwrap_or(existing.tz);
        let sleep_start = body.sleep_start.unwrap_or(existing.sleep_start);
        let sleep_end = body.sleep_end.unwrap_or(existing.sleep_end);
        let comfortable_minutes = body.comfortable_minutes.or(existing.comfortable_minutes);
        let maximum_minutes = body.maximum_minutes.or(existing.maximum_minutes);
        let solver = body.solver.unwrap_or(existing.solver).to_string();
        let time_budget_ms = body.time_budget_ms.or(existing.time_budget_ms);
        let seed = body.seed.or(existing.seed);
        let warm_start = body.warm_start.unwrap_or(existing.warm_start);
        let plan_length_days = body.plan_length_days.unwrap_or(existing.plan_length_days);
        sqlx::query(
            "UPDATE settings SET tz = ?, sleep_start = ?, sleep_end = ?, comfortable_minutes = ?, maximum_minutes = ?, solver = ?, time_budget_ms = ?, seed = ?, warm_start = ?, plan_length_days = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = 'active'",
        )
        .bind(&tz)
        .bind(sleep_start)
        .bind(sleep_end)
        .bind(comfortable_minutes)
        .bind(maximum_minutes)
        .bind(&solver)
        .bind(time_budget_ms)
        .bind(seed)
        .bind(warm_start)
        .bind(plan_length_days)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        // Re-query to return the actual updated_at the DB set (#290).
        sqlx::query_as::<_, SettingsRow>("SELECT * FROM settings WHERE id = 'active'")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn get_gcal_settings(&self) -> StorageResult<GoogleCalSettingsRow> {
        let row = sqlx::query_as::<_, GoogleCalSettingsRow>(
            "SELECT * FROM google_cal_settings WHERE id = 'active'",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(row.unwrap_or_else(|| GoogleCalSettingsRow {
            id: "active".to_string(),
            enabled: false,
            calendar_id: "primary".to_string(),
            client_id: String::new(),
            client_secret: String::new(),
            refresh_token: None,
            reminder_minutes: None,
            color_id: None,
            visibility: None,
            transparency: None,
            created_at: takusu_types::Timestamp::default(),
            updated_at: takusu_types::Timestamp::default(),
        }))
    }

    async fn update_gcal_settings(
        &self,
        body: &UpdateGoogleCalSettings,
    ) -> StorageResult<GoogleCalSettingsRow> {
        let existing = self.get_gcal_settings().await?;
        let enabled = body.enabled.unwrap_or(existing.enabled);
        let calendar_id = body
            .calendar_id
            .clone()
            .unwrap_or_else(|| existing.calendar_id.clone());
        let client_id = body
            .client_id
            .clone()
            .unwrap_or_else(|| existing.client_id.clone());
        let client_secret = body
            .client_secret
            .clone()
            .unwrap_or_else(|| existing.client_secret.clone());
        let refresh_token = body
            .refresh_token
            .clone()
            .or_else(|| existing.refresh_token.clone());
        let reminder_minutes = body
            .reminder_minutes
            .as_ref()
            .map_or(existing.reminder_minutes, |x| *x);
        let color_id = body.color_id.as_ref().map_or(existing.color_id, |x| *x);
        let visibility = body
            .visibility
            .as_ref()
            .map_or(existing.visibility.clone(), |x| x.clone());
        let transparency = body
            .transparency
            .as_ref()
            .map_or(existing.transparency.clone(), |x| x.clone());

        sqlx::query(
            "INSERT INTO google_cal_settings (id, enabled, calendar_id, client_id, client_secret, refresh_token, reminder_minutes, color_id, visibility, transparency, created_at, updated_at) VALUES ('active', ?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled, calendar_id=excluded.calendar_id, client_id=excluded.client_id, client_secret=excluded.client_secret, refresh_token=excluded.refresh_token, reminder_minutes=excluded.reminder_minutes, color_id=excluded.color_id, visibility=excluded.visibility, transparency=excluded.transparency, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"
        )
        .bind(enabled)
        .bind(&calendar_id)
        .bind(&client_id)
        .bind(&client_secret)
        .bind(&refresh_token)
        .bind(reminder_minutes)
        .bind(color_id)
        .bind(&visibility)
        .bind(&transparency)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;

        // Re-query to return the actual updated_at the DB set (#290).
        sqlx::query_as::<_, GoogleCalSettingsRow>(
            "SELECT * FROM google_cal_settings WHERE id = 'active'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(map_err)
    }

    async fn list_gcal_mappings(&self) -> StorageResult<Vec<GoogleCalEventRow>> {
        sqlx::query_as::<_, GoogleCalEventRow>("SELECT * FROM google_cal_events")
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn upsert_gcal_mappings(&self, mappings: &[(String, String)]) -> StorageResult<()> {
        for (task_id, event_id) in mappings {
            sqlx::query(
                "INSERT INTO google_cal_events (task_id, google_event_id, updated_at) VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now')) ON CONFLICT(task_id) DO UPDATE SET google_event_id=excluded.google_event_id, updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now')"
            )
            .bind(task_id)
            .bind(event_id)
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        }
        Ok(())
    }

    async fn delete_gcal_mappings(&self, task_ids: &[String]) -> StorageResult<()> {
        for task_id in task_ids {
            sqlx::query("DELETE FROM google_cal_events WHERE task_id = ?")
                .bind(task_id)
                .execute(&self.pool)
                .await
                .map_err(map_err)?;
        }
        Ok(())
    }

    async fn clear_gcal_mappings(&self) -> StorageResult<()> {
        sqlx::query("DELETE FROM google_cal_events")
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn list_skills(&self) -> StorageResult<Vec<SkillRow>> {
        sqlx::query_as::<_, SkillRow>("SELECT slug, name, description, body, built_in, created_at, updated_at FROM skills ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn get_skill(&self, slug: &str) -> StorageResult<SkillRow> {
        sqlx::query_as::<_, SkillRow>("SELECT slug, name, description, body, built_in, created_at, updated_at FROM skills WHERE slug = ?")
            .bind(slug)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound(format!("skill {slug} not found")),
                other => StorageError::Internal(other.to_string()),
            })
    }

    async fn create_skill(&self, body: &CreateSkill) -> StorageResult<SkillRow> {
        let built_in = body.built_in.unwrap_or(false);
        sqlx::query(
            "INSERT INTO skills (slug, name, description, body, built_in, created_at, updated_at) VALUES (?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(&body.slug)
        .bind(&body.name)
        .bind(&body.description)
        .bind(&body.body)
        .bind(built_in)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        self.get_skill(&body.slug).await
    }

    async fn update_skill(&self, slug: &str, body: &UpdateSkill) -> StorageResult<SkillRow> {
        sqlx::query(
            "UPDATE skills SET name=COALESCE(?,name), description=COALESCE(?,description), body=COALESCE(?,body), updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE slug = ?"
        )
        .bind(body.name.as_ref())
        .bind(body.description.as_ref())
        .bind(body.body.as_ref())
        .bind(slug)
        .execute(&self.pool)
        .await
        .map_err(map_err)?;
        self.get_skill(slug).await
    }

    async fn delete_skill(&self, slug: &str) -> StorageResult<()> {
        sqlx::query("DELETE FROM skills WHERE slug = ?")
            .bind(slug)
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn get_memory(&self, id: &str) -> StorageResult<MemoryRow> {
        sqlx::query_as::<_, MemoryRow>("SELECT * FROM memories WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => {
                    StorageError::NotFound(format!("memory {id} not found"))
                }
                other => StorageError::Internal(other.to_string()),
            })
    }

    async fn create_memory(
        &self,
        body: &CreateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow> {
        let normalized_key = takusu_search::memory::normalize_key(&body.key)
            .map_err(|e| StorageError::BadRequest(format!("invalid key: {e}")))?;
        let normalized_content = takusu_search::memory::normalize_content(&body.content)
            .map_err(|e| StorageError::BadRequest(format!("invalid content: {e}")))?;
        let subject_type = body.subject_type.unwrap_or_default();
        let subject_id = body.subject_id.clone().unwrap_or_default();

        let mut tx = self.pool.begin().await.map_err(map_err)?;
        let payload = serde_json::to_string(body)
            .map_err(|e| StorageError::Io(format!("serialize memory body: {e}")))?;
        let hash = memory_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) = Self::check_idempotency(&mut *tx, op_id, &hash).await?
        {
            return stored;
        }

        let existing: Option<MemoryRow> = sqlx::query_as::<_, MemoryRow>(
            "SELECT * FROM memories WHERE kind = ? AND normalized_key = ? AND subject_type = ? AND subject_id = ?",
        )
        .bind(body.kind.as_str())
        .bind(&normalized_key)
        .bind(subject_type.as_str())
        .bind(&subject_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_err)?;

        if let Some(existing) = existing {
            if body.upsert {
                let id = existing.id;
                let new_revision = existing.revision + 1;
                let result = sqlx::query(
                    "UPDATE memories SET content = ?, normalized_content = ?, revision = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ? AND revision = ?",
                )
                .bind(&body.content)
                .bind(&normalized_content)
                .bind(new_revision)
                .bind(&id)
                .bind(existing.revision)
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
                if result.rows_affected() == 0 {
                    return Err(StorageError::Conflict(
                        "memory changed after proposal".into(),
                    ));
                }
                let row: MemoryRow =
                    sqlx::query_as::<_, MemoryRow>("SELECT * FROM memories WHERE id = ?")
                        .bind(&id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(map_err)?;
                if let Some(op_id) = operation_id {
                    Self::record_operation(&mut *tx, op_id, &hash, &row).await?;
                }
                tx.commit().await.map_err(map_err)?;
                return Ok(row);
            }
            return Err(StorageError::Conflict(format!(
                "memory {} already exists",
                body.key
            )));
        }

        let id = uuid::Uuid::now_v7().to_string();
        let source = takusu_types::MemorySource::UserConfirmed;
        sqlx::query(
            "INSERT INTO memories (id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(&id)
        .bind(body.kind.as_str())
        .bind(&body.key)
        .bind(&normalized_key)
        .bind(&body.content)
        .bind(&normalized_content)
        .bind(subject_type.as_str())
        .bind(&subject_id)
        .bind(source.as_str())
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

        let row: MemoryRow = sqlx::query_as::<_, MemoryRow>("SELECT * FROM memories WHERE id = ?")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        if let Some(op_id) = operation_id {
            Self::record_operation(&mut *tx, op_id, &hash, &row).await?;
        }
        tx.commit().await.map_err(map_err)?;
        Ok(row)
    }

    async fn update_memory(
        &self,
        id: &str,
        body: &UpdateMemory,
        operation_id: Option<&str>,
    ) -> StorageResult<MemoryRow> {
        let content = body
            .content
            .as_ref()
            .ok_or_else(|| StorageError::BadRequest("content is required".into()))?;
        let normalized_content = takusu_search::memory::normalize_content(content)
            .map_err(|e| StorageError::BadRequest(format!("invalid content: {e}")))?;

        let mut tx = self.pool.begin().await.map_err(map_err)?;
        let payload = format!(
            "update:{id}:{}:{}",
            body.observed_revision,
            body.content.as_deref().unwrap_or("")
        );
        let hash = memory_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) = Self::check_idempotency(&mut *tx, op_id, &hash).await?
        {
            return stored;
        }

        let existing: Option<MemoryRow> =
            sqlx::query_as::<_, MemoryRow>("SELECT * FROM memories WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_err)?;
        let existing =
            existing.ok_or_else(|| StorageError::NotFound(format!("memory {id} not found")))?;
        if existing.revision != body.observed_revision {
            return Err(StorageError::Conflict(
                "memory changed after proposal".into(),
            ));
        }
        let new_revision = existing.revision + 1;

        let result = sqlx::query(
            "UPDATE memories SET content = ?, normalized_content = ?, revision = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ? AND revision = ?",
        )
        .bind(content)
        .bind(&normalized_content)
        .bind(new_revision)
        .bind(id)
        .bind(body.observed_revision)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Conflict(
                "memory changed after proposal".into(),
            ));
        }

        let row: MemoryRow = sqlx::query_as::<_, MemoryRow>("SELECT * FROM memories WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        if let Some(op_id) = operation_id {
            Self::record_operation(&mut *tx, op_id, &hash, &row).await?;
        }
        tx.commit().await.map_err(map_err)?;
        Ok(row)
    }

    async fn delete_memory(
        &self,
        id: &str,
        observed_revision: i64,
        operation_id: Option<&str>,
    ) -> StorageResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        let payload = format!("delete:{id}:{observed_revision}");
        let hash = memory_request_hash(&payload, operation_id);
        if let Some(op_id) = operation_id
            && let Some(stored) = Self::check_idempotency::<_, ()>(&mut *tx, op_id, &hash).await?
        {
            let _ = stored?;
            return Ok(());
        }

        let existing: Option<MemoryRow> =
            sqlx::query_as::<_, MemoryRow>("SELECT * FROM memories WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(map_err)?;
        let existing =
            existing.ok_or_else(|| StorageError::NotFound(format!("memory {id} not found")))?;
        if existing.revision != observed_revision {
            return Err(StorageError::Conflict(
                "memory changed after proposal".into(),
            ));
        }

        let result = sqlx::query("DELETE FROM memories WHERE id = ? AND revision = ?")
            .bind(id)
            .bind(observed_revision)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Conflict(
                "memory changed after proposal".into(),
            ));
        }

        if let Some(op_id) = operation_id {
            Self::record_operation_raw(&mut *tx, op_id, &hash, "null").await?;
        }
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn search_memories(&self, query: &MemoryQuery) -> StorageResult<Vec<MemoryRow>> {
        let terms = takusu_search::memory::tokenize_query(&query.q)
            .map_err(|e| StorageError::BadRequest(format!("invalid query: {e}")))?;
        let patterns = takusu_search::memory::memory_like_patterns(&terms);

        let mut sql = String::from("SELECT * FROM memories WHERE ");
        let mut bindings: Vec<String> = Vec::new();

        for (i, pat) in patterns.iter().enumerate() {
            if i > 0 {
                sql.push_str(" AND ");
            }
            sql.push_str(
                "(normalized_key LIKE ? ESCAPE '\\' OR normalized_content LIKE ? ESCAPE '\\')",
            );
            bindings.push(pat.clone());
            bindings.push(pat.clone());
        }

        if let Some(kind) = query.kind {
            sql.push_str(" AND kind = ?");
            bindings.push(kind.as_str().to_string());
        }
        if let Some(subject_type) = query.subject_type {
            sql.push_str(" AND subject_type = ?");
            bindings.push(subject_type.as_str().to_string());
        }
        if let Some(ref subject_id) = query.subject_id {
            sql.push_str(" AND subject_id = ?");
            bindings.push(subject_id.clone());
        }
        sql.push_str(" LIMIT 1000");

        let mut q = sqlx::query_as::<_, MemoryRow>(sqlx::AssertSqlSafe(sql.as_str()));
        for b in &bindings {
            q = q.bind(b);
        }
        let mut rows: Vec<MemoryRow> = q.fetch_all(&self.pool).await.map_err(map_err)?;

        takusu_search::memory::sort_memories(&query.q, &mut rows);

        let limit = query.limit.map_or(10, |n| n.clamp(1, 50) as usize);
        rows.truncate(limit);
        Ok(rows)
    }

    async fn injectable_memories(
        &self,
        query: &MemoryInjectionQuery,
    ) -> StorageResult<MemoryInjectionResult> {
        let normalized = takusu_search::memory::normalize_text(
            &query.text,
            Some(takusu_search::memory::MAX_INJECTION_UTTERANCE_SCALARS),
        )
        .map_err(|e| StorageError::BadRequest(format!("invalid text: {e}")))?;

        let counts = self.memory_counts().await?;

        if normalized.is_empty() {
            return Ok(MemoryInjectionResult {
                memories: Vec::new(),
                counts,
            });
        }

        // The SQL pre-filter already narrows candidates by the reverse lookup
        // (key occurs as a substring of the utterance) and orders them by the
        // injection ranking (key length desc, recency desc) so the LIMIT bounds
        // the number of rows read from the database per turn — the bigram-free
        // `instr` match on a ubiquitous short key would otherwise load an
        // unbounded row set (review #1003).
        let sql_limit = query
            .limit
            .unwrap_or(5)
            .clamp(1, takusu_search::memory::MAX_INJECTION_RESULTS as u32)
            as i32;

        let mut rows: Vec<MemoryRow> = sqlx::query_as(
            "SELECT * FROM memories
            WHERE kind IN ('proper_noun', 'fact')
              AND instr(?, normalized_key) > 0
            ORDER BY length(normalized_key) DESC, updated_at DESC, id ASC
            LIMIT ?",
        )
        .bind(&normalized)
        .bind(sql_limit)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let matched = takusu_search::memory::rank_memories_for_injection(&normalized, &mut rows);
        let limit = query.limit.map_or(5, |n| {
            n.min(takusu_search::memory::MAX_INJECTION_RESULTS as u32) as usize
        });
        rows.truncate(matched.min(limit));
        Ok(MemoryInjectionResult {
            memories: rows,
            counts,
        })
    }

    async fn find_similar_tasks(
        &self,
        query: &SimilarTaskQuery,
    ) -> StorageResult<Vec<SimilarTaskRow>> {
        let normalized_title = takusu_search::memory::normalize_text(
            &query.title,
            Some(takusu_search::memory::MAX_QUERY_SCALARS),
        )
        .map_err(|e| StorageError::BadRequest(format!("invalid title: {e}")))?;

        // Pre-filter candidates in SQL by requiring the normalized title to
        // contain at least one query bigram (a strict superset of non-zero Dice
        // matches, so no true match is dropped — see similar_task_filter_patterns).
        // All patterns are bound as parameters, never interpolated (#942).
        let patterns = takusu_search::memory::similar_task_filter_patterns(&normalized_title);
        if patterns.is_empty() {
            return Ok(Vec::new());
        }
        let filter = vec!["t.normalized_title LIKE ? ESCAPE '\\'"; patterns.len()].join(" OR ");
        // The bigram pre-filter already narrows candidates sharply; the
        // ORDER BY/LIMIT is only a worst-case safety bound so a very common
        // bigram (e.g. a single frequent kanji) cannot transfer an unbounded row
        // set. The cap is far above any personal-scale completed-task count, so
        // it never drops a relevant match in practice (#942).
        let cap = takusu_search::memory::SIMILAR_TASK_CANDIDATE_CAP;
        let sql = format!(
            "SELECT t.id AS task_id, t.display_id, t.title, t.avg_minutes, t.sigma_minutes, tam.actual_minutes, t.completed_at, t.updated_at FROM tasks t LEFT JOIN task_actual_minutes tam ON tam.task_id = t.id WHERE t.status = 'completed' AND ({filter}) ORDER BY t.updated_at DESC LIMIT {cap}"
        );
        let mut q = sqlx::query_as::<_, SimilarTaskRow>(sqlx::AssertSqlSafe(sql.as_str()));
        for p in &patterns {
            q = q.bind(p);
        }
        let rows: Vec<SimilarTaskRow> = q.fetch_all(&self.pool).await.map_err(map_err)?;

        let mut scored: Vec<(f64, SimilarTaskRow)> = rows
            .into_iter()
            .filter_map(|row| {
                takusu_search::memory::similar_task_score_pre_normalized(
                    &normalized_title,
                    &row.title,
                )
                .map(|score| (score, row))
            })
            .collect();

        scored.sort_by(|(sa, a), (sb, b)| {
            sa.total_cmp(sb)
                .reverse()
                .then_with(|| b.completed_at.cmp(&a.completed_at))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.task_id.cmp(&b.task_id))
        });

        let limit = query.limit.map_or(10, |n| n.clamp(1, 50)) as usize;
        let mut out: Vec<SimilarTaskRow> = scored
            .into_iter()
            .map(|(score, mut row)| {
                row.similarity = takusu_types::Similarity::dice(score);
                row
            })
            .collect();
        out.truncate(limit);
        Ok(out)
    }

    async fn start_work_session(
        &self,
        body: &StartWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        work_session::start_work_session(self, body, operation_id).await
    }

    async fn pause_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        work_session::pause_work_session(self, id, operation_id).await
    }

    async fn complete_work_session(
        &self,
        id: &str,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        work_session::complete_work_session(self, id, operation_id).await
    }

    async fn record_work_session_progress(
        &self,
        id: &str,
        body: &RecordWorkSessionProgress,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionProgressResult> {
        work_session::record_work_session_progress(self, id, body, operation_id).await
    }

    async fn get_work_session(&self, id: &str) -> StorageResult<WorkSessionRow> {
        work_session::get_work_session(self, id).await
    }

    async fn list_work_sessions(
        &self,
        task_id: Option<&str>,
    ) -> StorageResult<Vec<WorkSessionRow>> {
        work_session::list_work_sessions(self, task_id).await
    }

    async fn attach_work_session(
        &self,
        id: &str,
        body: &AttachWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<WorkSessionRow> {
        work_session::attach_work_session(self, id, body, operation_id).await
    }

    async fn convert_work_session(
        &self,
        id: &str,
        body: &ConvertWorkSession,
        operation_id: Option<&str>,
    ) -> StorageResult<TaskRow> {
        work_session::convert_work_session(self, id, body, operation_id).await
    }

    async fn get_task_progress(&self, id: &str) -> StorageResult<TaskProgress> {
        work_session::get_task_progress(self, id).await
    }

    async fn get_estimator_state(&self, task_id: &str) -> StorageResult<Option<EstimatorStateRow>> {
        work_session::get_estimator_state(self, task_id).await
    }

    async fn split_task(
        &self,
        id: &str,
        body: &SplitTask,
        operation_id: Option<&str>,
    ) -> StorageResult<SplitResult> {
        if body.retained_quantity < 0 {
            return Err(StorageError::BadRequest(
                "retained_quantity cannot be negative".into(),
            ));
        }

        let payload = serde_json::json!({"op": "split", "id": id, "body": body}).to_string();
        let request_hash = progress_request_hash(&payload, operation_id);

        let mut tx = self.pool.begin().await.map_err(map_err)?;
        if let Some(op_id) = operation_id
            && let Some(stored) =
                Self::check_progress_idempotency(&mut *tx, op_id, &request_hash).await?
        {
            return stored;
        }

        let full = resolve_task_id(&mut *tx, id).await?;

        let original: TaskRow = sqlx::query_as(SELECT_TASK_BY_ID)
            .bind(&full)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => StorageError::NotFound(format!("task {id} not found")),
                other => StorageError::Internal(other.to_string()),
            })?;

        if original.status == TaskStatus::Completed || original.status == TaskStatus::Skipped {
            return Err(StorageError::BadRequest(format!(
                "cannot split a {} task",
                original.status
            )));
        }

        let total = original.quantity_total.ok_or_else(|| {
            StorageError::BadRequest("cannot split a task with no quantity_total".into())
        })?;
        if body.retained_quantity <= 0 {
            return Err(StorageError::BadRequest(
                "retained_quantity must be greater than 0".into(),
            ));
        }
        if body.retained_quantity > total {
            return Err(StorageError::BadRequest(
                "retained_quantity cannot exceed quantity_total".into(),
            ));
        }
        if body.retained_quantity == total {
            return Err(StorageError::BadRequest(
                "retained_quantity must be less than quantity_total".into(),
            ));
        }
        if body.retained_quantity < original.quantity_done {
            return Err(StorageError::BadRequest(
                "retained_quantity cannot be less than quantity_done".into(),
            ));
        }
        let remainder_quantity = Quantity::new(total.get() - body.retained_quantity.get())
            .expect("retained_quantity is less than total, so remainder is non-negative");
        let original_quantity_total = original
            .original_quantity_total
            .filter(|t| *t != 0)
            .unwrap_or(total);

        // Allocate a display_id for the remainder task.
        let display_id: i64 = sqlx::query_scalar(
            "UPDATE task_display_id_seq SET next_id = next_id + 1 RETURNING next_id - 1",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

        let remainder_id = uuid::Uuid::now_v7().to_string();
        let depends = if body.set_dependency.unwrap_or(false) {
            vec![full.clone()]
        } else {
            Vec::new()
        };
        let depends = takusu_types::DependencyList::new(depends);

        let remainder_title = body.title.as_ref().unwrap_or(&original.title);
        // A title that fails NFKC normalization (e.g. control-character only)
        // stores NULL, excluding the task from similar-task search rather than
        // matching on a misleading empty string (#942).
        let normalized_title = takusu_search::memory::normalize_text(
            remainder_title,
            Some(takusu_search::memory::MAX_CONTENT_SCALARS),
        )
        .ok();
        sqlx::query(
            "INSERT INTO tasks (id, display_id, title, normalized_title, description, start_at, end_at, avg_minutes, sigma_minutes, depends, parallelizable, allows_parallel, abandonability, status, ical_uid, habit_id, fixed, habit_step_id, quantity_total, quantity_done, quantity_unit, completed_at, split_from_task_id, original_quantity_total, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(&remainder_id)
        .bind(display_id)
        .bind(remainder_title)
        .bind(&normalized_title)
        .bind(body.description.as_ref().or(original.description.as_ref()))
        .bind(original.start_at)
        .bind(body.end_at.as_ref().unwrap_or(&original.end_at))
        .bind(original.avg_minutes)
        .bind(original.sigma_minutes)
        .bind(&depends)
        .bind(original.parallelizable)
        .bind(original.allows_parallel)
        .bind(original.abandonability)
        .bind(None::<String>) // ical_uid
        .bind(None::<String>) // habit_id
        .bind(original.fixed)
        .bind(None::<String>) // habit_step_id
        .bind(remainder_quantity)
        .bind(0i64)
        .bind(original.quantity_unit.as_ref())
        .bind(None::<String>) // completed_at
        .bind(&full)
        .bind(Some(original_quantity_total))
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

        let new_done = original.quantity_done.min(body.retained_quantity);
        sqlx::query(
            "UPDATE tasks SET quantity_total = ?, quantity_done = ?, original_quantity_total = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
        )
        .bind(body.retained_quantity)
        .bind(new_done)
        .bind(Some(original_quantity_total))
        .bind(&full)
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

        let original: TaskRow = sqlx::query_as(SELECT_TASK_BY_ID)
            .bind(&full)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
        let remainder: TaskRow = sqlx::query_as(SELECT_TASK_BY_ID)
            .bind(&remainder_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;

        let result = SplitResult {
            original,
            remainder,
        };
        if let Some(op_id) = operation_id {
            Self::record_progress_operation(&mut *tx, op_id, &request_hash, &result).await?;
        }
        tx.commit().await.map_err(map_err)?;
        Ok(result)
    }

    async fn get_move_idempotency(
        &self,
        operation_id: &str,
        request_hash: &str,
    ) -> StorageResult<Option<MoveEntryResponse>> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        let stored = Self::check_progress_idempotency::<_, MoveEntryResponse>(
            &mut *tx,
            operation_id,
            request_hash,
        )
        .await?;
        tx.commit().await.map_err(map_err)?;
        Ok(match stored {
            Some(Ok(response)) => Some(response),
            Some(Err(e)) => return Err(e),
            None => None,
        })
    }

    async fn record_move_idempotency(
        &self,
        operation_id: &str,
        request_hash: &str,
        response: &MoveEntryResponse,
    ) -> StorageResult<()> {
        let mut tx = self.pool.begin().await.map_err(map_err)?;
        Self::record_progress_operation(&mut *tx, operation_id, request_hash, response).await?;
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    async fn health_check(&self) -> StorageResult<String> {
        // A cheap round-trip to the DB confirms the connection is alive.
        let v: String = sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(format!("sqlite ok (v{v})"))
    }
}

async fn filter_rows_with_query(
    storage: &SqliteStorage,
    rows: Vec<TaskRow>,
    q: &str,
) -> StorageResult<Vec<TaskRow>> {
    let tz_str = match storage.get_settings().await {
        Ok(s) => s.tz,
        Err(StorageError::NotFound(_)) => "UTC".to_string(),
        Err(e) => return Err(e),
    };
    let tz = takusu_types::parse_timezone(&tz_str).unwrap_or(TimeZone::UTC);
    let now = takusu_types::now_timestamp()
        .map_err(|e| StorageError::Internal(format!("current time unavailable: {e}")))?;

    let habits = storage.list_habits().await?;

    let schedule_entries: Vec<ScheduleEntry> = match storage.get_schedule().await? {
        Some(row) => row.schedule.as_inner().clone(),
        None => Vec::new(),
    };
    let schedule: Vec<(String, (String, String))> = schedule_entries
        .into_iter()
        .map(|e| (e.task_id, (e.start_at.to_string(), e.end_at.to_string())))
        .collect();

    let ctx = EvalContext::new(tz, now, schedule, &rows, &habits);
    filter_tasks(rows, q, &ctx).map_err(StorageError::BadRequest)
}

impl SqliteStorage {
    async fn memory_counts(&self) -> StorageResult<MemoryKindCounts> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT kind, COUNT(*) AS n FROM memories WHERE kind IN ('proper_noun', 'fact') GROUP BY kind",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        let mut counts = MemoryKindCounts {
            proper_noun: 0,
            fact: 0,
        };
        for (kind, n) in rows {
            match kind.as_str() {
                "proper_noun" => counts.proper_noun = n,
                "fact" => counts.fact = n,
                _ => {}
            }
        }
        Ok(counts)
    }

    async fn check_idempotency<'c, E, T: serde::de::DeserializeOwned>(
        executor: E,
        operation_id: &str,
        request_hash: &str,
    ) -> StorageResult<Option<StorageResult<T>>>
    where
        E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    {
        #[derive(sqlx::FromRow)]
        struct OpRow {
            request_hash: String,
            response_json: String,
        }
        let row: Option<OpRow> = sqlx::query_as(
            "SELECT request_hash, response_json FROM memory_operations WHERE operation_id = ?",
        )
        .bind(operation_id)
        .fetch_optional(executor)
        .await
        .map_err(map_err)?;

        if let Some(row) = row {
            if row.request_hash != request_hash {
                return Err(StorageError::Conflict(
                    "idempotency key reused with different request".into(),
                ));
            }
            let value: T = serde_json::from_str(&row.response_json).map_err(|e| {
                StorageError::Internal(format!("corrupt idempotency response: {e}"))
            })?;
            return Ok(Some(Ok(value)));
        }
        Ok(None)
    }

    async fn record_operation<'c, E, T: serde::Serialize>(
        executor: E,
        operation_id: &str,
        request_hash: &str,
        value: &T,
    ) -> StorageResult<()>
    where
        E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    {
        let response_json = serde_json::to_string(value)
            .map_err(|e| StorageError::Internal(format!("serialize idempotency response: {e}")))?;
        Self::record_operation_raw(executor, operation_id, request_hash, &response_json).await
    }

    async fn record_operation_raw<'c, E>(
        executor: E,
        operation_id: &str,
        request_hash: &str,
        response_json: &str,
    ) -> StorageResult<()>
    where
        E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    {
        sqlx::query(
            "INSERT INTO memory_operations (operation_id, request_hash, response_json, created_at) VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(operation_id)
        .bind(request_hash)
        .bind(response_json)
        .execute(executor)
        .await
        .map_err(map_err)?;
        Ok(())
    }

    pub(crate) async fn check_progress_idempotency<'c, E, T: serde::de::DeserializeOwned>(
        executor: E,
        operation_id: &str,
        request_hash: &str,
    ) -> StorageResult<Option<StorageResult<T>>>
    where
        E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    {
        #[derive(sqlx::FromRow)]
        struct OpRow {
            request_hash: String,
            response_json: String,
        }
        let row: Option<OpRow> = sqlx::query_as(
            "SELECT request_hash, response_json FROM progress_operations WHERE operation_id = ?",
        )
        .bind(operation_id)
        .fetch_optional(executor)
        .await
        .map_err(map_err)?;

        if let Some(row) = row {
            if row.request_hash != request_hash {
                return Err(StorageError::Conflict(
                    "idempotency key reused with different request".into(),
                ));
            }
            let value: T = serde_json::from_str(&row.response_json).map_err(|e| {
                StorageError::Internal(format!("corrupt idempotency response: {e}"))
            })?;
            return Ok(Some(Ok(value)));
        }
        Ok(None)
    }

    pub(crate) async fn record_progress_operation<'c, E, T: serde::Serialize>(
        executor: E,
        operation_id: &str,
        request_hash: &str,
        value: &T,
    ) -> StorageResult<()>
    where
        E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    {
        let response_json = serde_json::to_string(value)
            .map_err(|e| StorageError::Internal(format!("serialize idempotency response: {e}")))?;
        sqlx::query(
            "INSERT INTO progress_operations (operation_id, request_hash, response_json, created_at) VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(operation_id)
        .bind(request_hash)
        .bind(response_json)
        .execute(executor)
        .await
        .map_err(map_err)?;
        Ok(())
    }

    /// Idempotency replay for comment creation (WI-1). Mirrors
    /// `check_idempotency` but reads the `comment_operations` receipt table.
    async fn check_comment_idempotency<'c, E>(
        executor: E,
        operation_id: &str,
        request_hash: &str,
    ) -> StorageResult<Option<StorageResult<CommentRow>>>
    where
        E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    {
        #[derive(sqlx::FromRow)]
        struct OpRow {
            request_hash: String,
            response_json: String,
        }
        let row: Option<OpRow> = sqlx::query_as(
            "SELECT request_hash, response_json FROM comment_operations WHERE operation_id = ?",
        )
        .bind(operation_id)
        .fetch_optional(executor)
        .await
        .map_err(map_err)?;

        if let Some(row) = row {
            if row.request_hash != request_hash {
                return Err(StorageError::Conflict(
                    "idempotency key reused with different request".into(),
                ));
            }
            let value: CommentRow = serde_json::from_str(&row.response_json).map_err(|e| {
                StorageError::Internal(format!("corrupt idempotency response: {e}"))
            })?;
            return Ok(Some(Ok(value)));
        }
        Ok(None)
    }

    async fn record_comment_operation<'c, E>(
        executor: E,
        operation_id: &str,
        request_hash: &str,
        value: &CommentRow,
    ) -> StorageResult<()>
    where
        E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    {
        let response_json = serde_json::to_string(value)
            .map_err(|e| StorageError::Internal(format!("serialize idempotency response: {e}")))?;
        sqlx::query(
            "INSERT INTO comment_operations (operation_id, request_hash, response_json, created_at) VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
        )
        .bind(operation_id)
        .bind(request_hash)
        .bind(response_json)
        .execute(executor)
        .await
        .map_err(map_err)?;
        Ok(())
    }
}

fn memory_request_hash(payload: &str, operation_id: Option<&str>) -> String {
    crate::auth::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

pub(crate) fn progress_request_hash(payload: &str, operation_id: Option<&str>) -> String {
    crate::auth::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

fn comment_request_hash(payload: &str, operation_id: Option<&str>) -> String {
    crate::auth::hash_token(&format!("{}:{}", payload, operation_id.unwrap_or("")))
}

/// Computed, validated values needed to apply an `UpdateTask` to an existing
/// row. Produced by `validate_task_update` so the pure validation step is
/// independently testable without a database.
#[derive(Debug)]
struct TaskUpdatePlan {
    original_quantity_total: Option<Quantity>,
    status: TaskStatus,
    /// Recomputed normalized title; `None` keeps the stored value via COALESCE.
    normalized_title: Option<String>,
    /// 0 = leave `start_at` unchanged, 1 = apply `start_val`.
    upd_start: i64,
    start_val: Option<Timestamp>,
}

/// Validate the mutable fields of an `UpdateTask` against the existing row and
/// precompute everything the field/status update queries need. Pure: no DB
/// access, so it can be unit-tested in isolation.
fn validate_task_update(body: &UpdateTask, existing: &TaskRow) -> StorageResult<TaskUpdatePlan> {
    // Treat original_quantity_total 0 as unset (same as None) server-side.
    // quantity_total 0 is a clear sentinel handled below.
    let existing_total = existing.quantity_total.filter(|t| *t != 0);
    let original_quantity_total = body.original_quantity_total.filter(|t| *t != 0);
    validate_quantity(
        body.quantity_total.or(existing_total),
        body.quantity_done.or(Some(existing.quantity_done)),
        original_quantity_total,
    )?;

    let status = body.status.unwrap_or(existing.status);
    let validated = [
        TaskStatus::Pending,
        TaskStatus::Scheduled,
        TaskStatus::InProgress,
        TaskStatus::Completed,
        TaskStatus::Skipped,
    ];
    if !validated.contains(&status) {
        return Err(StorageError::BadRequest(format!(
            "invalid status: {status}"
        )));
    }

    // Recompute the normalized title only when the title actually changes;
    // bind None otherwise (or when normalization fails) so COALESCE keeps the
    // stored value (#942).
    let normalized_title = body.title.as_ref().and_then(|t| {
        takusu_search::memory::normalize_text(t, Some(takusu_search::memory::MAX_CONTENT_SCALARS))
            .ok()
    });

    // Unpack Option<Option<Timestamp>> for start_at.
    // None = no change, Some(None) = clear to NULL, Some(Some(ts)) = set value.
    // end_at is NOT NULL so it stays Option<Timestamp> with COALESCE.
    let (upd_start, start_val) = match body.start_at {
        None => (0i64, None),
        Some(inner) => (1i64, inner),
    };

    Ok(TaskUpdatePlan {
        original_quantity_total,
        status,
        normalized_title,
        upd_start,
        start_val,
    })
}

/// Apply the field-level `UPDATE tasks SET ...` for an update. Binds every
/// column from `body` plus the precomputed `plan`/`depends` values.
async fn update_task_fields<'c, E>(
    executor: E,
    full: &str,
    body: &UpdateTask,
    plan: &TaskUpdatePlan,
    depends: &Option<takusu_types::DependencyList>,
) -> StorageResult<()>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    sqlx::query(
        "UPDATE tasks SET \
         title=COALESCE(?,title), \
         normalized_title=COALESCE(?,normalized_title), \
         description=CASE WHEN ?= '' THEN NULL ELSE COALESCE(?,description) END, \
         start_at=CASE WHEN ?=0 THEN start_at ELSE ? END, \
         end_at=COALESCE(?,end_at), \
         avg_minutes=COALESCE(?,avg_minutes), \
         sigma_minutes=COALESCE(?,sigma_minutes), \
         depends=COALESCE(?,depends), \
         parallelizable=COALESCE(?,parallelizable), \
         allows_parallel=COALESCE(?,allows_parallel), \
         abandonability=COALESCE(?,abandonability), \
         status=?, \
         habit_id=COALESCE(?,habit_id), \
         user_edited=COALESCE(?,user_edited), \
         fixed=COALESCE(?,fixed), \
         habit_step_id=COALESCE(?,habit_step_id), \
         quantity_total=CASE WHEN ?= 0 THEN NULL ELSE COALESCE(?,quantity_total) END, \
         quantity_done=COALESCE(?,quantity_done), \
         quantity_unit=CASE WHEN ?= '' THEN NULL ELSE COALESCE(?,quantity_unit) END, \
         original_quantity_total=COALESCE(?,original_quantity_total), \
         updated_at=strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?",
    )
    .bind(body.title.as_ref())
    .bind(&plan.normalized_title)
    .bind(body.description.as_ref())
    .bind(body.description.as_ref())
    .bind(plan.upd_start)
    .bind(plan.start_val)
    .bind(body.end_at)
    .bind(body.avg_minutes)
    .bind(body.sigma_minutes)
    .bind(depends.as_ref())
    .bind(body.parallelizable)
    .bind(body.allows_parallel)
    .bind(body.abandonability)
    .bind(plan.status.to_string())
    .bind(body.habit_id.as_ref())
    .bind(body.user_edited)
    .bind(body.fixed)
    .bind(body.habit_step_id.as_ref())
    .bind(body.quantity_total)
    .bind(body.quantity_total)
    .bind(body.quantity_done)
    .bind(body.quantity_unit.as_ref())
    .bind(body.quantity_unit.as_ref())
    .bind(plan.original_quantity_total)
    .bind(full)
    .execute(executor)
    .await
    .map_err(map_err)?;
    Ok(())
}

/// Drive `completed_at` for an explicit status transition: set on completion,
/// cleared when leaving `Completed`, left untouched otherwise. Work-session
/// cleanup on terminal transitions is handled by the caller via
/// `cleanup_work_sessions` (#1044).
async fn handle_status_transition<'c, E>(
    executor: E,
    full: &str,
    existing: &TaskRow,
    new_status: TaskStatus,
) -> StorageResult<()>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    let completed_at = if new_status == TaskStatus::Completed {
        existing
            .completed_at
            .or(Some(takusu_types::Timestamp::now()))
    } else if existing.status == TaskStatus::Completed {
        None
    } else {
        existing.completed_at
    };
    sqlx::query("UPDATE tasks SET completed_at = ? WHERE id = ?")
        .bind(completed_at)
        .bind(full)
        .execute(executor)
        .await
        .map_err(map_err)?;
    Ok(())
}

pub(crate) async fn resolve_task_id<'c, E>(executor: E, id: &str) -> StorageResult<String>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    let parsed = takusu_types::TaskRef::try_from(id)
        .map_err(|_| StorageError::NotFound(format!("task {id} not found")))?;
    match parsed {
        takusu_types::TaskRef::HabitTask { habit, task } => sqlx::query_scalar::<_, String>(
            "SELECT t.id FROM tasks t JOIN habits h ON t.habit_id = h.id \
                 WHERE h.display_id = ? AND t.display_id = ?",
        )
        .bind(habit)
        .bind(task)
        .fetch_optional(executor)
        .await
        .map_err(map_err)?
        .ok_or_else(|| StorageError::NotFound(format!("task {id} not found"))),
        takusu_types::TaskRef::Display(num) => sqlx::query_scalar::<_, String>(
            "SELECT id FROM tasks WHERE display_id = ? AND habit_id IS NULL",
        )
        .bind(num)
        .fetch_optional(executor)
        .await
        .map_err(map_err)?
        .ok_or_else(|| StorageError::NotFound(format!("task {id} not found"))),
        takusu_types::TaskRef::Uuid(uuid) => {
            let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM tasks WHERE id = ?")
                .bind(&uuid)
                .fetch_one(executor)
                .await
                .map_err(map_err)?;
            if exists {
                Ok(uuid)
            } else {
                Err(StorageError::NotFound(format!("task {id} not found")))
            }
        }
    }
}

/// Resolve a habit reference to its full UUID.
/// Accepts `h<N>` (habit display_id, e.g. `h1`) or a full UUID. UUID prefixes
/// are not accepted (#1251).
async fn resolve_habit_id(pool: &SqlitePool, id: &str) -> StorageResult<String> {
    let parsed = takusu_types::HabitRef::try_from(id)
        .map_err(|_| StorageError::NotFound(format!("habit {id} not found")))?;
    match parsed {
        takusu_types::HabitRef::Display(num) => {
            sqlx::query_scalar::<_, String>("SELECT id FROM habits WHERE display_id = ?")
                .bind(num)
                .fetch_optional(pool)
                .await
                .map_err(map_err)?
                .ok_or_else(|| StorageError::NotFound(format!("habit {id} not found")))
        }
        takusu_types::HabitRef::Uuid(uuid) => {
            let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM habits WHERE id = ?")
                .bind(&uuid)
                .fetch_one(pool)
                .await
                .map_err(map_err)?;
            if exists {
                Ok(uuid)
            } else {
                Err(StorageError::NotFound(format!("habit {id} not found")))
            }
        }
    }
}

/// Resolve a list of dependency references (display_id numbers or full UUIDs)
/// to full UUID strings. Entries that are already full UUIDs are passed through.
async fn resolve_depends(pool: &SqlitePool, deps: Option<&[String]>) -> StorageResult<Vec<String>> {
    let Some(deps) = deps else {
        return Ok(Vec::new());
    };
    let mut resolved = Vec::with_capacity(deps.len());
    for d in deps {
        resolved.push(resolve_task_id(pool, d).await?);
    }
    Ok(resolved)
}

/// Compute the ISO-8601 expiration timestamp `ttl_seconds` from now.
fn token_expires_at(ttl_seconds: i64) -> Option<String> {
    let now = jiff::Timestamp::now().as_second();
    let exp = now.saturating_add(ttl_seconds);
    jiff::Timestamp::from_second(exp)
        .ok()
        .map(|t| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use takusu_types::Abandonability;

    #[tokio::test]
    async fn foreign_keys_are_enabled_after_init() {
        let cfg = LocalConfig {
            db: "sqlite::memory:".into(),
            jwt_secret: "test-secret".into(),
            ..Default::default()
        };
        let storage = SqliteStorage::init(&cfg).await.unwrap();
        let enabled: i32 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(storage.pool())
            .await
            .unwrap();
        assert_eq!(
            enabled, 1,
            "foreign keys should be enabled for every connection"
        );
    }

    #[tokio::test]
    async fn progress_persists_estimator_revision_in_the_same_transaction() {
        let cfg = LocalConfig {
            db: "sqlite::memory:".into(),
            jwt_secret: "test-secret".into(),
            ..Default::default()
        };
        let storage = SqliteStorage::init(&cfg).await.unwrap();
        let now = Timestamp::now();
        let task = storage
            .create_task(&CreateTask {
                title: "estimate me".into(),
                description: None,
                start_at: None,
                end_at: Timestamp::from_second(now.as_second() + 3600).unwrap(),
                avg_minutes: 60,
                sigma_minutes: Some(20),
                depends: None,
                parallelizable: None,
                allows_parallel: None,
                abandonability: None,
                ical_uid: None,
                habit_id: None,
                fixed: Some(false),
                habit_step_id: None,
                quantity_total: Some(Quantity::new(10).unwrap()),
                quantity_done: None,
                quantity_unit: Some("items".into()),
                original_quantity_total: None,
            })
            .await
            .unwrap();
        let session = storage
            .start_work_session(
                &StartWorkSession {
                    task_id: Some(task.id.clone()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE work_sessions SET started_at = ? WHERE id = ?")
            .bind(Timestamp::from_second(now.as_second() - 1800).unwrap())
            .bind(&session.id)
            .execute(storage.pool())
            .await
            .unwrap();

        let result = storage
            .record_work_session_progress(
                &session.id,
                &RecordWorkSessionProgress {
                    quantity_done: Quantity::new(5).unwrap(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.estimator.as_ref().unwrap().revision, 1);
        let state = storage
            .get_estimator_state(&task.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.revision, 1);
        let observations: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM estimator_observations WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(storage.pool())
                .await
                .unwrap();
        assert_eq!(observations, 1);

        let correction = storage
            .record_work_session_progress(
                &session.id,
                &RecordWorkSessionProgress {
                    quantity_done: Quantity::new(4).unwrap(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(correction.estimator.as_ref().unwrap().revision, 2);
        let compensation: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM estimator_observations WHERE task_id = ? AND compensates_observation_id IS NOT NULL",
        )
        .bind(&task.id)
        .fetch_one(storage.pool())
        .await
        .unwrap();
        assert_eq!(compensation, 1);
    }

    /// Minimal `TaskRow` for `validate_task_update` unit tests. Only the
    /// fields the validator inspects are populated meaningfully; the rest get
    /// neutral defaults.
    fn sample_task_row(status: TaskStatus) -> TaskRow {
        let now = Timestamp::from_second(1_700_000_000).unwrap();
        TaskRow {
            id: "task-1".into(),
            display_id: 1,
            title: "sample".into(),
            description: None,
            start_at: None,
            end_at: now,
            avg_minutes: 30,
            sigma_minutes: 5,
            depends: Vec::new().into(),
            parallelizable: false,
            allows_parallel: false,
            abandonability: Abandonability::default(),
            status,
            habit_id: None,
            ical_uid: None,
            user_edited: false,
            fixed: false,
            habit_step_id: None,
            quantity_total: None,
            quantity_done: Quantity::default(),
            quantity_unit: None,
            completed_at: None,
            split_from_task_id: None,
            original_quantity_total: None,
            actual_minutes: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn validate_task_update_keeps_existing_status_when_omitted() {
        let existing = sample_task_row(TaskStatus::InProgress);
        let body = UpdateTask::default();
        let plan = validate_task_update(&body, &existing).unwrap();
        assert_eq!(plan.status, TaskStatus::InProgress);
        // No start_at change requested.
        assert_eq!(plan.upd_start, 0);
        assert!(plan.start_val.is_none());
        // No title change → no normalized title recomputed.
        assert!(plan.normalized_title.is_none());
    }

    #[test]
    fn validate_task_update_rejects_done_exceeding_total() {
        let mut existing = sample_task_row(TaskStatus::Pending);
        existing.quantity_total = Some(Quantity::new(10).unwrap());
        existing.quantity_done = Quantity::new(3).unwrap();
        let body = UpdateTask {
            quantity_done: Some(Quantity::new(11).unwrap()),
            ..Default::default()
        };
        let err = validate_task_update(&body, &existing).unwrap_err();
        assert!(matches!(err, StorageError::BadRequest(_)));
    }

    #[test]
    fn validate_task_update_accepts_all_valid_statuses() {
        // `UpdateTask.status` is typed (`Option<TaskStatus>` deserialized via
        // `enum_serde::option`), so an out-of-range label is rejected at the
        // serde boundary before reaching the validator. The `validated`
        // allow-list inside `validate_task_update` is a defense-in-depth guard
        // that is unreachable through the typed API; this test confirms the
        // validator accepts every legitimate status value.
        let existing = sample_task_row(TaskStatus::Pending);
        for status in [
            TaskStatus::Pending,
            TaskStatus::Scheduled,
            TaskStatus::InProgress,
            TaskStatus::Completed,
            TaskStatus::Skipped,
        ] {
            let body = UpdateTask {
                status: Some(status),
                ..Default::default()
            };
            let plan = validate_task_update(&body, &existing).unwrap();
            assert_eq!(plan.status, status);
        }
    }

    #[test]
    fn validate_task_update_unpacks_start_at_clear() {
        let existing = sample_task_row(TaskStatus::Pending);
        // Some(None) = explicit clear to NULL.
        let body = UpdateTask {
            start_at: Some(None),
            ..Default::default()
        };
        let plan = validate_task_update(&body, &existing).unwrap();
        assert_eq!(plan.upd_start, 1);
        assert!(plan.start_val.is_none());
    }

    #[test]
    fn validate_task_update_unpacks_start_at_set() {
        let existing = sample_task_row(TaskStatus::Pending);
        let ts = Timestamp::from_second(1_700_000_500).unwrap();
        let body = UpdateTask {
            start_at: Some(Some(ts)),
            ..Default::default()
        };
        let plan = validate_task_update(&body, &existing).unwrap();
        assert_eq!(plan.upd_start, 1);
        assert_eq!(plan.start_val, Some(ts));
    }

    #[test]
    fn validate_task_update_recomputes_normalized_title() {
        let existing = sample_task_row(TaskStatus::Pending);
        let body = UpdateTask {
            title: Some("  hello world  ".into()),
            ..Default::default()
        };
        let plan = validate_task_update(&body, &existing).unwrap();
        // Normalization trims and lower-cases (or similar); just assert it
        // produced a non-empty value distinct from the raw input.
        let normalized = plan.normalized_title.expect("title changed");
        assert!(!normalized.is_empty());
        assert_ne!(normalized, "  hello world  ");
    }

    #[test]
    fn validate_task_update_treats_zero_original_as_unset() {
        let mut existing = sample_task_row(TaskStatus::Pending);
        existing.quantity_total = Some(Quantity::new(10).unwrap());
        // original_quantity_total = 0 is a sentinel for "unset" and must not
        // trip the `> 0` validation.
        let body = UpdateTask {
            original_quantity_total: Some(Quantity::default()),
            ..Default::default()
        };
        let plan = validate_task_update(&body, &existing).unwrap();
        assert!(plan.original_quantity_total.is_none());
    }

    /// WI-2: `task_note` memories migrate to task comments with the correct
    /// source→author mapping, dead-subject rows are dropped, and the memories
    /// CHECK constraint no longer accepts `task_note`.
    #[tokio::test]
    async fn migrate_task_note_memories_to_comments() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Build the pre-029 schema: base tables + memories (with task_note) +
        // task_comments.
        sqlx::raw_sql(MIGRATION_001).execute(&pool).await.unwrap();
        sqlx::raw_sql(MIGRATION_016).execute(&pool).await.unwrap();
        sqlx::raw_sql(MIGRATION_028).execute(&pool).await.unwrap();

        // A live task that two of the notes reference.
        let task_id = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO tasks (id, title, end_at, avg_minutes, sigma_minutes, status, created_at, updated_at) VALUES (?, 'mig task', '2030-01-01T00:00:00Z', 30, 0, 'pending', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
        )
        .bind(&task_id)
        .execute(&pool)
        .await
        .unwrap();

        // A pre-existing comment claims seq = 1; migrated task_note rows must
        // be offset past it so they do not collide on (task_id, seq).
        sqlx::query(
            "INSERT INTO task_comments (id, task_id, author, content, seq, created_at) VALUES ('pre-existing', ?, 'user', 'before migration', 1, '2024-01-01T00:00:00Z')",
        )
        .bind(&task_id)
        .execute(&pool)
        .await
        .unwrap();

        // Insert task_note memories: user_confirmed and agent_inferred on the
        // live task, imported on a dead (non-existent) task.
        async fn insert_note(pool: &SqlitePool, key: &str, source: &str, subject: &str) {
            let id = uuid::Uuid::now_v7().to_string();
            sqlx::query(
                "INSERT INTO memories (id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at) VALUES (?, 'task_note', ?, ?, ?, ?, 'task', ?, ?, 1, ?, ?)",
            )
            .bind(&id)
            .bind(key)
            .bind(key)
            .bind(key)
            .bind(key)
            .bind(subject)
            .bind(source)
            .bind("2025-01-01T00:00:00Z")
            .bind("2025-01-01T00:00:00Z")
            .execute(pool)
            .await
            .unwrap();
        }
        insert_note(&pool, "live-user", "user_confirmed", &task_id).await;
        insert_note(&pool, "live-agent", "agent_inferred", &task_id).await;
        insert_note(&pool, "dead-imported", "imported", "no-such-task").await;

        // Run the migration.
        sqlx::raw_sql(MIGRATION_029).execute(&pool).await.unwrap();

        // All three task_note rows are gone.
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM memories")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);

        // Only the live tasks produced comments. The pre-existing comment
        // keeps seq 1; migrated rows are offset to seq 2 and 3, in seq order,
        // with the correct author mapping and preserved created_at.
        let comments: Vec<CommentRow> =
            sqlx::query_as("SELECT * FROM task_comments WHERE task_id = ? ORDER BY seq ASC")
                .bind(&task_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[0].seq, 1);
        assert_eq!(comments[0].author, CommentAuthor::User);
        assert_eq!(comments[0].content, "before migration");
        assert_eq!(comments[1].seq, 2);
        assert_eq!(comments[1].author, CommentAuthor::User);
        assert_eq!(comments[1].content, "live-user");
        assert_eq!(comments[1].created_at.to_string(), "2025-01-01T00:00:00Z");
        assert_eq!(comments[2].seq, 3);
        assert_eq!(comments[2].author, CommentAuthor::Agent);
        assert_eq!(comments[2].content, "live-agent");

        // The rebuilt CHECK constraint rejects task_note.
        let rejected = sqlx::query(
            "INSERT INTO memories (id, kind, key, normalized_key, content, normalized_content, subject_type, subject_id, source, revision, created_at, updated_at) VALUES ('x', 'task_note', 'k','k','c','c','task', 't', 'user_confirmed', 1, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await;
        assert!(
            rejected.is_err(),
            "task_note should be rejected after migration"
        );
    }
}
