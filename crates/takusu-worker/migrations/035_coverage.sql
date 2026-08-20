-- Coverage trust state for planner-event authority (WI-10).
CREATE TABLE IF NOT EXISTS coverage_confirmations (
    id                TEXT PRIMARY KEY,
    start_at          TEXT NOT NULL,
    end_at            TEXT NOT NULL,
    timezone          TEXT NOT NULL,
    source            TEXT NOT NULL,
    schedule_revision INTEGER NOT NULL,
    calendar_health   TEXT NOT NULL,
    created_at        TEXT NOT NULL,
    settled_at        TEXT,
    operation_id      TEXT
);
CREATE INDEX IF NOT EXISTS idx_coverage_confirmations_created
    ON coverage_confirmations(created_at);

CREATE TABLE IF NOT EXISTS unsettled_intervals (
    id            TEXT PRIMARY KEY,
    start_at      TEXT NOT NULL,
    end_at        TEXT NOT NULL,
    classification TEXT NOT NULL,
    source        TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    settled_at    TEXT,
    operation_id  TEXT
);
CREATE INDEX IF NOT EXISTS idx_unsettled_intervals_settled
    ON unsettled_intervals(settled_at, start_at);
