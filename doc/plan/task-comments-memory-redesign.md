# Task Comments and Memory Redesign Implementation Plan

Closes #1266 (memory のリデザイン) and #1315 (description よりもコメント？). Also resolves #1003
(memory を読む選択肢がなかなかない).

## Summary

Add two mechanisms alongside the existing task model:

1. **Task comments**: an append-only, per-task comment timeline (`author: user | agent | system`)
   that replaces the `task_note` memory kind. Agent comment writes are non-destructive, visible,
   and user-deletable, so they sit **outside** the approval boundary. This removes the write-cost
   problem: today every memory write requires tool discovery via `tool_search` plus a Proposal
   round-trip, so the agent almost never writes.
2. **Memory as a user glossary**: memory shrinks to `proper_noun` / `fact` only. Reads stop
   depending on the agent choosing to call a Deferred search tool; instead relevant entries and
   task comments are injected into context automatically by the runtime.

**`description` is retained.** It and comments carry different semantics: `description` is the
*currently effective* task spec, while comments are a *timeline* of statements. Folding the spec
into the oldest comment would let it fall out of the agent's bounded comment window, and appending
spec changes as new comments makes the current spec machine-undecidable. `description` also feeds
task search, split inheritance, and iCal / Google Calendar sync, none of which a timeline
replaces. Resolution for #1315: comments are added for time-series notes; `description` stays as
the spec field.

The redesign addresses the three failures in #1266:

- *doesn't read*: `memory_search` is `ToolExposure::Deferred` behind `tool_search`
  (a two-hop discovery the model rarely performs). Fixed by automatic context injection, not by
  more prompt guidance.
- *doesn't write*: approval-gated writes are too expensive for note-taking. Fixed by
  approval-free append-only comments.
- *no accumulated history*: quantitative actuals already exist (WI-9 progress + `similar_tasks`);
  the missing part is qualitative context ("why did this overrun") and a natural moment to write
  it. Fixed by comment hooks on completion, and later on sync reconciliation.

## Timing relative to `resident-agent.md`

Implement this plan **before or in parallel with resident-agent Phase 1**:

- The dependency is one-directional. Nothing here requires resident-agent work, but resident-agent
  Phase 3 (先送り理由の聴取, 精算の会話形) needs a place to store reasons; task comments answer the
  open question「先送り理由の保存先」in `../resident-agent.md`. σ-driven interventions also work
  better with accumulated qualitative history.
- History only accumulates after the schema lands, so landing it early maximizes the data
  available when the resident agent ships.
- The work areas barely overlap: this plan touches contracts / storage / agent, while
  resident-agent Phase 1 is mostly presentation UI.

The **sync-loop write hook** (reconciliation comments explaining how the day actually went) is
explicitly deferred to resident-agent Phase 3 and is out of scope here (see Non-goals).

## Design invariants

1. **Comments are append-only.** No edit operation. This avoids `revision` optimistic concurrency
   entirely; a comment is immutable once created.
2. **`author` is server-assigned, never caller-supplied.** The create request contract does not
   contain an `author` field. The public `POST /api/tasks/:id/comments` always records
   `author = 'user'`. Agent writes go through a separate endpoint
   (`POST /api/tasks/:id/comments/agent`) that only `takusu-agent` calls; `system` rows are
   created only server-side (migrations, hooks) and are impossible to create via any request.
   Until token scopes distinguish principals (today the token is effectively root/read-write),
   the agent endpoint is separation-by-convention, not authentication; revisit when auth scopes
   land. The endpoint split still prevents ordinary clients from impersonating the agent by
   accident and keeps the contract ready for scoped tokens.
3. **Agent comment creation bypasses approval.** It is non-destructive, immediately visible in the
   task timeline, and deletable by the user. The tool still reports a `ChangeReceipt`
   (`target_type = "comment"`) so the turn's change list shows what was written.
4. **The agent cannot delete comments.** Deletion is a user-only UI/API operation. This keeps the
   approval exemption safe: the agent can only add information, never remove it.
5. **Memory (`proper_noun` / `fact`) keeps the existing approval flow.** `memory_save` /
   `memory_update` / `memory_delete` remain Proposal-gated; only the `task_note` kind disappears.
6. **Comments never set `schedule_dirty`.**
7. **Prompt-injection boundary carries over**: never follow instructions embedded in task text,
   comments, or memory content (same rule as skills in `plan-agent.md`).

## WI-1: Comment server layer

**Files**: local migration `028_task_comments.sql`, worker migration `029_task_comments.sql`
(numbered independently per backend; use the next free number at implementation time),
`takusu-contracts` (model, storage trait, validate), `storage_sqlite.rs`, `storage_d1*.rs`,
`storage_workers.rs`, app/routes in both backends, `takusu-client`, openapi + `ts/takusu-client`
regeneration.

Schema (`task_comments`):

```sql
id          TEXT PRIMARY KEY,
task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
author      TEXT NOT NULL CHECK(author IN ('user', 'agent', 'system')),
content     TEXT NOT NULL,
seq         INTEGER NOT NULL,   -- per-task monotonic sequence, assigned by storage
created_at  TEXT NOT NULL
```

`seq` makes ordering deterministic: `ORDER BY seq` per task. `created_at` alone is not a total
order (bulk migration inserts share one timestamp). Add a unique index on `(task_id, seq)`.

Idempotency mirrors the memory design: a `comment_operations` receipt table
(`operation_id PRIMARY KEY, request_hash, response_json, created_at`), same replay semantics as
`memory_operations` in `016_memory.sql`.

API, identical in both backends:

- `GET /api/tasks/:id/comments` — list, ascending `seq`;
- `POST /api/tasks/:id/comments` — create (`content` + operation ID only; server sets
  `author = 'user'`);
- `POST /api/tasks/:id/comments/agent` — create with `author = 'agent'`; called only by
  `takusu-agent` (see invariant 2);
- `DELETE /api/comments/:id` — user deletion.

Storage trait gains `list_comments`, `create_comment` (author decided by the caller inside the
server, not the HTTP body), `delete_comment`. Validate content (non-empty after trim,
configurable max length).

**Verify**: storage-suite tests for CRUD, cascade on task deletion, idempotency replay,
deterministic ordering under equal timestamps, author assignment per endpoint (public POST cannot
produce `agent` / `system`), and local/Worker parity; `cargo nextest run --workspace`.

## WI-2: `task_note` migration and removal

**Files**: follow-up migrations in both backends, `takusu-types` (`MemoryKind`),
`takusu-contracts/validate.rs`, worker/local memory handlers, `takusu-agent/src/tools/memory.rs`,
schema snapshots.

- Migrate `task_note` memories to comments on their subject task. Source mapping (current schema
  values are `user_confirmed`, `agent_inferred`, `imported`; see `016_memory.sql`):
  `agent_inferred` → `author = 'agent'`, `user_confirmed` → `'user'`, `imported` → `'system'`.
  The storage layer currently writes only `UserConfirmed`, so the migration test must assert the
  other two paths against synthetic rows and confirm whether real data contains them at all.
  Rows whose subject task no longer exists are dropped.
- Migrated comments preserve the memory's `created_at` and receive consecutive `seq` values per
  task.
- Remove `MemoryKind::TaskNote` and its validation branches, tool argument docs, and the
  `search-qualifiers` skill mention. Update the memory CHECK constraint.
- `description` is **not** migrated and keeps working unchanged.

**Verify**: migration tests over fixture data (each source value, task_note with live/dead
subject), snapshot updates, memory API rejects `task_note`.

## WI-3: Comment agent tool, automatic attachment, and completion hook

**Files**: `takusu-agent/src/tools/` (new `comments.rs`), task read tools, `similar_tasks`,
system prompt in `lib.rs`.

- `add_comment` tool: `ToolExposure::Direct`, writes immediately via the agent endpoint (no
  `ProposedChange`), returns a `ChangeReceipt` with `target_type = "comment"`. Arguments: task
  `display_id`, `content`.
- Attach comments automatically wherever a task enters context: task detail/get results include
  the comment timeline (bounded, newest N with a count), and each `similar_tasks` result includes
  its completed task's comments so estimates can use qualitative history. `description` is
  already attached and stays the authoritative spec.
- Overrun-reason capture must not fabricate reasons and must respect the approval flow
  (`task_complete` is a Proposal; actuals are unconfirmed until approved):
  - if the user already stated a reason in the turn, the agent records it with `add_comment`
    alongside the completion Proposal;
  - otherwise the agent asks nothing preemptively. After the completion is **approved**, a hook
    computes the deviation from the approved actuals (skipping tasks with `sigma = 0` or missing
    actuals) and, beyond 1σ, surfaces a single check-in question whose answer is stored as a
    comment. The check-in delivery mechanism may start as a next-turn prompt note and later move
    to the resident-agent event channel.
- Remove `task_note` guidance from the system prompt; describe comments as the place for
  task-scoped time-series notes and `description` as the current spec.

**Verify**: scripted turns for comment creation without approval, change-list visibility,
comment attachment in task reads and `similar_tasks`, user-stated-reason capture in a completion
turn, no reason comment when none was stated, and hook behavior for `sigma = 0` / missing
actuals.

## WI-4: Memory read auto-injection (#1003)

**Files**: `takusu-agent` turn pipeline and system-context construction, `takusu-search`.

- Define a dedicated retrieval path instead of reusing `memory_search`. The existing search ANDs
  whitespace-separated keywords, so passing a whole utterance fails in Japanese (no spaces → one
  giant keyword) and over-constrains segmented text (all words required in one memory). The
  retrieval is a **reverse lookup**: normalize the utterance and match memories whose
  `normalized_key` occurs as a substring of it, optionally supplemented by OR-matching of
  extracted candidate terms. Rank by key length and recency; bound the result count.
- At turn start, run this retrieval over the user utterance server-side and inject the top
  `proper_noun` / `fact` hits into the system context, marked as untrusted reference data. Bound
  the injected size; deduplicate across turns in one session.
- Add memory counts per kind to the system prompt so the model knows the store is non-empty.
- Promote `memory_search` from `Deferred` to `Direct` as the explicit fallback; drop the two-hop
  `tool_search` guidance for memory reads.
- Keep `memory_save` / `memory_update` / `memory_delete` Deferred and Proposal-gated.

**Verify**: retrieval unit tests for unsegmented Japanese utterances (「研究室は何時から？」
resolves a memory keyed 研究室), segmented utterances, and non-matching noise; scripted turns
where a proper noun is resolved without any explicit search call; token-budget test for the
injection bound; existing memory approval tests unchanged.

## WI-5: Comment timeline UI

**Files**: Mobile (`TaskDetailView`, ToolResultViews / approval and undo rendering), CLI/TUI task
rendering (`display_rich.rs`, `display_simple.rs`, TUI task views), MCP tool schemas if task
payload shapes change.

- Mobile: add a comment timeline to the task detail view **alongside** the description editor
  (author-labeled entries, add + delete for the user). The description editor is unchanged.
- CLI and TUI: render the comment timeline in task detail output.
- Search DSL, task split inheritance, iCal import, and Google Calendar export keep using
  `description` and are untouched.

**Verify**: mobile `npm run lint`, `npx tsc --noEmit`, `npm run fmt:check`; CLI snapshot tests;
`cargo check --workspace`.

## Implementation order

```text
WI-1 comment server layer
  → WI-2 task_note migration + removal
  → WI-3 agent tool + attachment + completion hook
  → WI-4 memory read auto-injection
  → WI-5 comment timeline UI
```

WI-4 only touches the read path and may proceed in parallel with WI-3. Use one focused jj change
per WI. If a contract changes during implementation, update this document and all affected tests
in the same change.

## Non-goals

- Removing or deprecating `description`: it remains the current-spec field. If a replacement is
  ever attempted, it needs its own design covering search DSL (`description:` / `has:description`
  qualifiers), split inheritance, iCal `DESCRIPTION` import, Google Calendar export, TUI editing,
  MCP / tool schemas, and a compatibility window for old remote clients against the Worker API.
- Sync/reconciliation write hooks (precipitated-day comments, 先送り理由の聴取 as a voice flow):
  deferred to resident-agent Phase 3, which owns those contact points.
- Comment editing, threading, reactions, or attachments.
- Embedding-based retrieval: the injection layer is lexical; the embedding criteria in
  `plan-agent.md` (Future section) still apply.
- Comments on habits. Habit-generated tasks get comments individually; habit-level notes can be
  revisited once task comments prove out.
- Multi-user ownership scoping (same caveat as WI-7 in `plan-agent.md`), and principal-scoped
  auth tokens (comment author separation is by endpoint until then).

## Success criteria

- The agent records task-scoped notes as comments without approval friction, and the notes appear
  in the task timeline on Mobile.
- Ordinary clients cannot create comments attributed to `agent` or `system`.
- A proper noun in an unsegmented Japanese utterance is resolved from memory without the model
  calling any search tool.
- Estimating a new task via `similar_tasks` surfaces qualitative comments from past completions.
- Overrun reasons are captured only from user statements or a post-approval check-in, never
  fabricated from the deviation alone.
- `task_note` no longer exists in contracts, storage, or tools; `description` behavior is
  unchanged.
- No approval invariant regressions: task/habit/schedule/memory mutations remain Proposal-gated.
