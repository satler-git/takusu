-- Task comment timeline (WI-1). Append-only per-task timeline replacing the
-- `task_note` memory kind. `author` is server-assigned via endpoint split.
CREATE TABLE IF NOT EXISTS task_comments (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    author      TEXT NOT NULL CHECK(author IN ('user', 'agent', 'system')),
    content     TEXT NOT NULL,
    seq         INTEGER NOT NULL,   -- per-task monotonic sequence, assigned by storage
    created_at  TEXT NOT NULL
);

-- seq makes ordering deterministic per task even when multiple rows share one
-- created_at timestamp (e.g. a bulk migration insert).
CREATE UNIQUE INDEX IF NOT EXISTS uq_task_comments_task_seq
    ON task_comments(task_id, seq);
CREATE INDEX IF NOT EXISTS idx_task_comments_task
    ON task_comments(task_id);

-- Idempotency receipts for comment creation. Same replay semantics as
-- `memory_operations` in 017_memory.sql.
CREATE TABLE IF NOT EXISTS comment_operations (
    operation_id     TEXT PRIMARY KEY,
    request_hash     TEXT NOT NULL,
    response_json    TEXT NOT NULL,
    created_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_comment_operations_created_at
    ON comment_operations(created_at);