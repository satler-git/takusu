-- Idempotency receipts for settlement operations (WI-18).
CREATE TABLE IF NOT EXISTS settle_operations (
    operation_id     TEXT PRIMARY KEY,
    request_hash     TEXT NOT NULL,
    response_json    TEXT NOT NULL,
    created_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_settle_operations_created_at
    ON settle_operations(created_at);
