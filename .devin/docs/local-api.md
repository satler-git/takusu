# takusu-local API and Architecture

## Authentication

- Root token: `TAKUSU_ROOT_TOKEN` env var (format: `tsk_` + UUID v7)
- Issued tokens: stored as SHA-256 hash in `tokens` table
- Any valid token can issue new tokens (trust chain)
- Revocation is per-token, no cascade
- All `/api/*` endpoints require `Authorization: Bearer <token>`
- `/health` requires no auth

## Endpoints

- **Task**: CRUD + batch create + iCal import (`/api/tasks`, `/api/tasks/batch`, `/api/tasks/import/ical`)
- **Habit**: CRUD + batch create (`/api/habits`, `/api/habits/batch`)
- **Schedule**: get/generate/reschedule/move/clear (`/api/schedule/*`)
- **Settings**: get/update (`/api/settings`) — tz, sleep_start, sleep_end
- **Token**: issue/list/revoke (`/api/tokens`)
- **Sync**: Google Calendar settings/OAuth/trigger (`/api/sync/*`)

## Testing

Integration tests use `axum::Router::oneshot()` with in-memory SQLite. No
external HTTP server needed. Run with `cargo nextest run -p takusu-local`.

## Key Architecture Decisions

- **takusu-local-lib** is the core business logic, used by both `takusu-local`
  (server) and `takusu-cli` (client).
- **Pluggable storage**: `takusu-storage` provides the `Storage` trait. Two
  implementations: `SqliteStorage` (direct sqlx) and `WorkersStorage`
  (HTTP → Cloudflare Worker).
- **CLI uses takusu-local-lib directly**: No network round-trip; `takusu-cli`
  initializes `TakusuApp` with a storage backend (`TAKUSU_STORAGE=sqlite|workers`).
- **Single active schedule**: `schedules` table has one row (`id = 'active'`),
  UPSERT on generate
- **Task CRUD does not auto-reschedule**: responses include `unscheduled_count`
- **Move entry with validation**: `PATCH /api/schedule/entries/:task_id` returns
  409 with warnings on violations; `force: true` overrides
- **iCal import skips duplicates**: by `ical_uid` column (unique index)
- **Token hashing**: tokens stored as SHA-256, full token only returned on
  creation
- **Google Calendar sync**: schedule generate/reschedule/move/clear triggers
  sync inline (no fire-and-forget). `google-cal` crate does diff-based sync.
- **Generate uses `now` as start**: `POST /api/schedule/generate` no longer
  accepts `from` or `until`; the start time is always the current time and the
  horizon is derived from task deadlines. The planner schedules all eligible
  tasks regardless of an upper bound.
- **Task status tracking**: tasks have a `status` column with 5 states:
  `pending` (not yet scheduled), `scheduled` (in current schedule),
  `in_progress` (being worked on), `completed` (done), `skipped` (explicitly
  skipped). Status is changeable via `task status <id> <value>` or
  `task update --status <value>`.
  - **Generate includes pending/scheduled only**: query is
    `status IN ('pending', 'scheduled')`. `in_progress`, `completed`, and
    `skipped` tasks are excluded from schedule generation.
  - **Generate sets scheduled**: all tasks included in a generate become
    `status='scheduled'`.
  - **Reschedule**: queries tasks with `status IN ('pending', 'scheduled')`.
  - **Clear schedule**: does NOT reset task status (tasks stay `scheduled`; must
    be manually set to `pending`).
