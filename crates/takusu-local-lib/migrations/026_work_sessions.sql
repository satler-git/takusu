-- Migration 026: work sessions become top-level entities.
-- Tasks are optional attachments to a work session, and the old
-- task_work_sessions / progress_events tables are migrated.

BEGIN;

-- 1. Rename and recreate work sessions.
ALTER TABLE task_work_sessions RENAME TO old_task_work_sessions;

CREATE TABLE work_sessions (
    id              TEXT PRIMARY KEY,
    task_id         TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    title           TEXT,
    note            TEXT,
    quantity_total  INTEGER,
    quantity_done   INTEGER NOT NULL DEFAULT 0,
    quantity_unit   TEXT,
    started_at      TEXT NOT NULL,
    ended_at        TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_work_sessions_task ON work_sessions(task_id);

-- Only one open work session is allowed globally.
CREATE UNIQUE INDEX IF NOT EXISTS idx_work_sessions_open
    ON work_sessions ((0)) WHERE ended_at IS NULL;

INSERT INTO work_sessions (
    id, task_id, title, note, quantity_total, quantity_done, quantity_unit,
    started_at, ended_at, created_at
)
SELECT
    id, task_id, NULL, NULL, NULL, 0, NULL,
    started_at, ended_at, created_at
FROM old_task_work_sessions;

-- 2. Match old progress events to the work session that was open when the
--    event was recorded.
CREATE TEMP TABLE event_session_match AS
SELECT
    e.id AS event_id,
    s.id AS session_id,
    ROW_NUMBER() OVER (
        PARTITION BY e.id
        ORDER BY s.started_at DESC, s.id DESC
    ) AS rn
FROM progress_events e
LEFT JOIN work_sessions s
    ON s.task_id = e.task_id
    AND datetime(s.started_at) <= datetime(e.at)
    AND (s.ended_at IS NULL OR datetime(e.at) <= datetime(s.ended_at));

-- 3. Create synthetic sessions for progress events that could not be matched
--    to a real work session (e.g. corrections without an active session).
INSERT INTO work_sessions (
    id, task_id, title, note, quantity_total, quantity_done, quantity_unit,
    started_at, ended_at, created_at
)
SELECT
    'legacy-' || e.id,
    e.task_id,
    'legacy session',
    'migrated from old progress event with no matching work session',
    NULL,
    0,
    NULL,
    datetime(e.at, '-' || MAX(e.active_minutes, 0) || ' minutes'),
    e.at,
    e.at
FROM progress_events e
LEFT JOIN event_session_match m
    ON e.id = m.event_id AND m.rn = 1
WHERE m.session_id IS NULL;

-- 4. Recreate progress_events with work_session_id as the primary parent.
ALTER TABLE progress_events RENAME TO old_progress_events;

CREATE TABLE progress_events (
    id              TEXT PRIMARY KEY,
    work_session_id TEXT NOT NULL REFERENCES work_sessions(id) ON DELETE CASCADE,
    task_id         TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    at              TEXT NOT NULL DEFAULT (datetime('now')),
    quantity_done   INTEGER,
    delta_quantity  INTEGER,
    active_minutes  INTEGER NOT NULL,
    note            TEXT
);

CREATE INDEX IF NOT EXISTS idx_progress_events_session ON progress_events(work_session_id);
CREATE INDEX IF NOT EXISTS idx_progress_events_task ON progress_events(task_id);

INSERT INTO progress_events (
    id, work_session_id, task_id, at, quantity_done, delta_quantity,
    active_minutes, note
)
SELECT
    e.id,
    COALESCE(m.session_id, 'legacy-' || e.id),
    e.task_id,
    e.at,
    e.quantity_done,
    e.delta_quantity,
    e.active_minutes,
    e.note
FROM old_progress_events e
LEFT JOIN event_session_match m
    ON e.id = m.event_id AND m.rn = 1;

-- 5. Drop old tables and recreate the task_actual_minutes view.
DROP TABLE old_task_work_sessions;
DROP TABLE old_progress_events;
DROP VIEW IF EXISTS task_actual_minutes;

CREATE VIEW task_actual_minutes AS
SELECT
    task_id,
    SUM(MAX((strftime('%s', COALESCE(ended_at, 'now')) - strftime('%s', started_at)) / 60, 1)) AS actual_minutes
FROM work_sessions
WHERE task_id IS NOT NULL
GROUP BY task_id;

-- 6. Backfill session metadata from the events and the linked task.
UPDATE work_sessions
SET quantity_done = (
    SELECT COALESCE(MAX(quantity_done), 0)
    FROM progress_events
    WHERE progress_events.work_session_id = work_sessions.id
);

UPDATE work_sessions
SET
    title = COALESCE(title, (SELECT title FROM tasks WHERE tasks.id = work_sessions.task_id)),
    quantity_total = COALESCE(quantity_total, (SELECT quantity_total FROM tasks WHERE tasks.id = work_sessions.task_id)),
    quantity_unit = COALESCE(quantity_unit, (SELECT quantity_unit FROM tasks WHERE tasks.id = work_sessions.task_id))
WHERE task_id IS NOT NULL;

COMMIT;
