-- Multi-device arbitration for resident authority (WI-11).
-- Device registry table, guarded with IF NOT EXISTS so it can be applied
-- repeatedly without error (e.g. when a previous migration run only added
-- the settings column).

CREATE TABLE IF NOT EXISTS devices (
    id                        TEXT PRIMARY KEY,
    name                      TEXT NOT NULL,
    platform                  TEXT NOT NULL CHECK(platform IN ('desktop', 'android')),
    priority                  INTEGER NOT NULL DEFAULT 0,
    evaluator_heartbeat_until TEXT,
    evaluator_lease_until     TEXT,
    next_eval_at              TEXT,
    audio_service_running     INTEGER NOT NULL DEFAULT 0,
    private_output_route      INTEGER NOT NULL DEFAULT 0,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_devices_platform
    ON devices(platform);
CREATE INDEX IF NOT EXISTS idx_devices_priority
    ON devices(priority);
