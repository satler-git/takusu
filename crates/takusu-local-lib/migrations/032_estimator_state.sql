CREATE TABLE IF NOT EXISTS estimator_state (
    task_id         TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    revision        INTEGER NOT NULL DEFAULT 0,
    mean_minutes    REAL NOT NULL,
    sigma_minutes   REAL NOT NULL,
    source          TEXT NOT NULL DEFAULT 'task',
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS estimator_observations (
    id                              TEXT PRIMARY KEY,
    task_id                        TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    revision                       INTEGER NOT NULL,
    kind                           TEXT NOT NULL,
    active_minutes                 REAL NOT NULL,
    quantity_fraction              REAL,
    projection_minutes             REAL,
    prior_mean_minutes             REAL NOT NULL,
    prior_sigma_minutes            REAL NOT NULL,
    posterior_mean_minutes         REAL NOT NULL,
    posterior_sigma_minutes        REAL NOT NULL,
    compensates_observation_id     TEXT REFERENCES estimator_observations(id),
    created_at                     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_estimator_observations_task_revision
    ON estimator_observations(task_id, revision);

CREATE TABLE IF NOT EXISTS estimator_task_priors (
    kind            TEXT PRIMARY KEY,
    revision        INTEGER NOT NULL DEFAULT 0,
    mean_minutes    REAL NOT NULL,
    sigma_minutes   REAL NOT NULL,
    source          TEXT NOT NULL,
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
