-- Monotonic schedule revisions make a rescheduled boundary a new event key.
CREATE TABLE IF NOT EXISTS schedule_revisions (
    id       TEXT PRIMARY KEY,
    revision INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO schedule_revisions (id, revision) VALUES ('active', 0);

CREATE TABLE IF NOT EXISTS event_ledger (
    id                    TEXT PRIMARY KEY,
    kind                  TEXT NOT NULL,
    task_id               TEXT,
    presentation          TEXT NOT NULL,
    urgency               TEXT NOT NULL,
    schedule_revision     INTEGER NOT NULL,
    distribution_revision INTEGER,
    observation_kind      TEXT NOT NULL,
    delivery_state        TEXT NOT NULL DEFAULT 'pending_delivery',
    created_at            TEXT NOT NULL,
    delivered_at          TEXT,
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_event_ledger_delivery ON event_ledger(delivery_state, created_at);
CREATE INDEX IF NOT EXISTS idx_event_ledger_task ON event_ledger(task_id);

CREATE TABLE IF NOT EXISTS event_delivery_claims (
    event_id   TEXT NOT NULL REFERENCES event_ledger(id) ON DELETE CASCADE,
    device_id  TEXT NOT NULL,
    claimed_at TEXT NOT NULL,
    PRIMARY KEY (event_id, device_id)
);
CREATE INDEX IF NOT EXISTS idx_event_delivery_claims_device
    ON event_delivery_claims(device_id, claimed_at);
