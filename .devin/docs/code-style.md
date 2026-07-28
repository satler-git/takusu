# Code Style

- **Add a comment whenever the reason for writing code a certain way is
  non-obvious.** If a future reader might ask "why is this done this way?",
  add a comment explaining the rationale. This is especially important for
  performance optimizations, workarounds for external library quirks, safety
  invariants that aren't type-checked, and cases where the seemingly "cleaner"
  approach would be wrong.
- Uses `thiserror` for error types
- Module-level docs (`//!`) in each source file describe algorithm details
- Modules organized by domain (core, local, audio, google-cal)
- Workspace-level dependency versions defined in root `Cargo.toml`
- `FxHashSet` over `HashSet` for performance-critical collections
- Edition 2024: `gen` is a reserved keyword → use `r#gen` for rand trait methods
- **reqwest**: Use `rustls-tls` feature (not default native-tls) to avoid
  OpenSSL dependency

# Hacks / Brittle Code — Do Not Remove Casually

These patterns look suspicious but exist for real reasons. If you need to
change them, understand the context first:

## `sqlx::AssertSqlSafe` in dynamic SQL

**Files:** `takusu-local/src/handlers/task.rs`, `schedule.rs`, `sync.rs`;
`takusu-local-lib/src/storage_sqlite.rs`, etc.

Dynamic SQL with parameterized `?` placeholders suppresses sqlx's compile-time
verification. Safe today because all user values go through `?` bindings, but
removes sqlx's guard against future accidental string interpolation. If
refactoring, replace with `sqlx::query_builder` or array binding.

## ~~`TAKUSU_WORKERS_URL` `|` split hack~~ FIXED

**Files:** `takusu-local/src/main.rs`, `takusu-cli/src/main.rs`,
`takusu-web/src/lib.rs`

The `split('|').next()` workaround has been removed from all three call sites.
`worker_url` is now treated as a single URL string. The 2nd+ `|`-separated
segments were never used or documented, so dropping the split fixes URLs that
contain `|` without losing any real functionality.

## ~~Fire-and-forget Google Calendar sync~~ FIXED

**Files:** `takusu-local/src/handlers/schedule.rs`, `sync.rs`;
`takusu-local-lib/src/app.rs`

`tokio::spawn` was replaced with awaited `do_sync()` calls. Sync now runs
inline during the request.

## `COALESCE` prevents clearing fields to NULL

**File:** `takusu-worker/src/handlers/tasks.rs`, `habits.rs`

```sql
UPDATE tasks SET title=COALESCE(?1,title), ...
```

`Option::None` (from `serde_json::Value::Null` → `Option::None`) binds as
`JsValue::NULL`, so `COALESCE(NULL, title)` keeps the old value. There is no way
to clear a field. Fixing this requires distinguishing "not provided" from
"explicitly set to null".

## ~~`LIKE` prefix matching for short IDs~~ REMOVED

**Files:** `takusu-local/src/handlers/task.rs`, `habit.rs`;
`takusu-local-lib/src/storage_sqlite.rs`

UUID prefix matching (`SELECT id FROM tasks WHERE id LIKE ? || '%'` and the
client-side `starts_with` fallback) was removed in #1251. It forced a full
table scan and was vulnerable to `_`/`%` pattern injection. Task/habit
references now accept only display IDs (`#42`, `h1#3`, numeric) and full
UUIDs.

## `point_to_iso` hardcoded 5-minute slots

**Files:** `takusu-local-lib/src/app.rs` and ~8 other locations

Magic number `5` (slot length in minutes) is duplicated across crates with no
shared constant. Changing slot granularity requires updating every site.

## ~~Duplicated integration test patterns~~ NOTED

**Files:** `takusu-local/tests/integration.rs`, `phase4.rs`, `workers_e2e.rs`

Integration tests share code patterns. Full deduplication into a shared
test-utils crate is planned.

## ~~`_unused_jsvalue_marker` dead code~~ REMOVED

**File:** `takusu-worker/src/handlers/tokens.rs`

Removed the dead code function and unused `JsValue` import.

## ~~410 (Gone) treated as success in Google Calendar delete~~ FIXED

**File:** `google-cal/src/lib.rs`

Magic number `410` is now a named constant `ALREADY_DELETED`.

## ~~`get_settings_or_default` swallows DB errors~~ FIXED

**Files:** `takusu-local-lib/src/app.rs`

Now returns `Result<SettingsRow, AppError>`. DB errors propagate correctly;
`NotFound` still falls back to defaults.

## ~~Sync .ok() silently drops DB errors~~ FIXED

**Files:** `takusu-local-lib/src/app.rs`

All DB operations propagate errors via `?` with `.map_err()`.

## ~~Unsafe `set_len` in audio recording~~ REMOVED

**File:** `takusu-audio/src/record.rs`

Replaced with simple `buf.push()` loop. The unsafe optimization was unnecessary
overhead.

## ~~`generate_neighbor_partial` only uses 3 of 5 neighbor types~~ FIXED

**File:** `takusu-core/src/anneal.rs`

Added `neighbor_reorder_partial` and `neighbor_lns_partial` operators. The
partial variant now uses the same 5 neighbor types with identical probability
distribution as the full variant (shift 25%/swap 25%/duration 20%/reorder 15%/
lns 15%).

## ~~Auth middleware not applied to CRUD endpoints~~ FIXED

**File:** `takusu-worker/src/router.rs`

`require_auth()` is now called for every `/api/*` route except
`/api/auth/verify` in the `dispatch` function. Tokens (`tasks`, `habits`,
`schedule`, `settings`, `sync`, `tokens`) all require
`Authorization: Bearer <token>`.

## `freeness()` name is counterintuitive

**File:** `takusu-core/src/lib.rs`

High "freeness" means the task has slack time and is deprioritized. Low
freeness → prioritized first. The name suggests the opposite convention.
