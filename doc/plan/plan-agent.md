# takusu LLM Agent + Memory / Progress Management Plan

## Summary

Implement the voice-assistant half of the takusu vision (`../proposal.typ` §音声アシスタント):
an LLM agent that operates the planner through tool calling, plus the memory, safe change
management, schedule orchestration, and progress features it depends on.

The implementation proceeds sequentially with one agent. Each work item (WI) should leave the
repository working and tested before the next begins. Contracts in this document may be refined
when implementation reveals a problem, but the document and all affected clients must be updated
in the same change.

The LLM-facing tool set is:

- planner reads: task, habit, schedule, and settings lookup;
- planner mutations: task/habit CRUD proposed for client approval before execution;
- schedule preview and commit;
- memory search/save/update/delete and similar-task lookup;
- progress start/pause/report/complete/split;
- skills list/read and confirmed add/edit.

Audio capture and playback are application I/O, not LLM tools. The CLI or Android/server adapter
records audio, transcribes it, calls the agent for one turn, and speaks the returned text.
The LLM must never activate the microphone by itself.

## Product invariants from `../proposal.typ`

The implementation must preserve these behaviors across all work items:

1. **Explain inference**: when the LLM fills a missing deadline, estimate, sigma, dependency, or
   abandonability, the response and approval request say what was inferred and why.
2. **Approve planner changes in the client**: before the agent changes a task, habit, or schedule,
   the client shows why the change is needed, a concrete list of changes, and Approve/Deny actions.
   Denial performs no write and lets the user correct the request.
3. **Confirm persistent effects**: planner mutations, persistent skill edits, and schedule commits
   require explicit user approval before execution.
4. **Batch scheduling work**: consecutive task edits mark the schedule dirty; they do not run the
   planner independently. Recompute after an explicit schedule request, an explicit user request,
   or at the end of a configurable quiet period.
5. **Search before guessing**: for an unknown proper noun, search memory first, ask the user if it
   is still unknown, then save the answer. For a missing estimate, inspect similar completed tasks
   before using model knowledge.
6. **Use active work time**: progress estimates and actual duration use explicit active work
   sessions, not wall-clock time between first start and completion.
7. **Stable references**: user-facing task references use `display_id`. Habit-generated task IDs
   are scoped by habit and must use an unambiguous form such as `h<habit_display_id>#<task_display_id>`.

## Architecture

```text
takusu-agent (library + CLI binary)
├── src/lib.rs        # AgentSession, tool loop, history, pending approvals
├── src/tool.rs       # Tool trait, ToolRegistry, structured tool results
├── src/llm.rs        # OpenAI chat-completions adapter
├── src/tools/
│   ├── takusu.rs     # planner reads, mutation proposals, schedule preview/commit
│   ├── skills.rs     # skills list/read/add/edit with confirmation
│   ├── memory.rs     # memory CRUD and similar tasks
│   └── progress.rs   # work sessions, progress, completion, splitting
├── src/audio.rs      # application-level STT/TTS adapter; not registered as tools
└── src/bin/agent.rs  # text mode and push-to-talk CLI

takusu server/storage/client extensions
├── backend-specific memory migration
├── migrations/015_progress.sql
├── /api/schedule/preview
├── /api/memory/* and /api/tasks/similar
└── /api/tasks/:id/work/*, /progress, and /split
```

Schedule preview is an application/planner concern, not a storage concern. The local server and
`takusu-local-lib` run `takusu-core` to calculate a candidate without replacing the active schedule.
The Cloudflare Worker is a storage backend for D1: it exposes schedule read/save operations used by
`WorkersStorage`, but it must not run planner preview logic. The approval flow sends the accepted
candidate back through the local application layer, which atomically saves it through the selected
storage backend.

Migration numbers are maintained independently by the local SQLite and D1/Worker backends. The
memory design uses the next available number in each backend rather than assuming a shared sequence.
SQLite/local and D1/Worker must expose the same storage behavior, while planner-only operations
remain in the application layer.

### Runtime data flow

```text
CLI/Android input
  → record/VAD
  → STT
  → AgentSession::run_turn(text)
  → LLM/tool loop
  → structured TurnResult { text, approval_request, schedule_state }
  → UI renders text and, when present, Why / changes / Approve or Deny
  → AgentSession::resolve_approval(id, decision)
  → approved operations execute; denied operations perform no writes
  → optional TTS/playback
```

`AgentSession` is independent of the transport. The first executable is the CLI, but a later
HTTP/WebSocket adapter must be able to own sessions without moving planner or audio logic into the
binary. Only one turn may mutate a session at a time; concurrent calls are serialized per session.

## Shared contracts

### Tool contract

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError>;
}

pub struct ToolOutput {
    /// JSON or text returned to the LLM.
    pub content: String,
    /// Planner writes proposed for application-level approval.
    pub proposed_changes: Vec<ProposedChange>,
    pub schedule_dirty: bool,
}
```

Recoverable errors such as invalid arguments, not found, and optimistic-conflict errors are added
as tool-result messages so the LLM can correct its request. Authentication failures, transport
failures after retry, malformed provider responses, and session cancellation fail the turn.

The per-turn limit counts actual tool calls, including multiple calls returned in one LLM response.
Mutating calls are sequential even when the provider returns them together. Read-only calls may be
parallelized later.

### Agent configuration

Read `$XDG_CONFIG_HOME/takusu/agent.toml`, then apply
`TAKUSU_AGENT__<SECTION>__<KEY>` overrides.

```toml
[llm]
base_url = "https://api.openai.com/v1"
model = "gpt-4.1-mini"
api_key_env = "TAKUSU_LLM_API_KEY"
max_context_tokens = 32000
max_tool_calls = 64
request_timeout_seconds = 60

[server]
url = "http://127.0.0.1:3000"
token = "tsk_..."

[audio]
# future audio settings will live here once TTS/STT integration is wired into takusu-agent

[skills]
dir = "~/.local/share/takusu/skills"

[schedule]
quiet_period_seconds = 30
```

The system context is rebuilt for each turn from server settings and contains the user's timezone,
current zoned time, dirty-schedule state, available skills index, and task-reference rules. It must
not use the host timezone as a substitute after settings have loaded.

### Client approval contract

Agent tools do not write task, habit, or schedule mutations immediately. They return proposed
operations, which `AgentSession` groups into one short-lived `ApprovalRequest` for the current turn:

```rust
pub struct ApprovalRequest {
    pub id: String,
    pub why: String,
    pub changes: Vec<ProposedChange>,
    pub inferred_fields: Vec<InferredField>,
    pub warnings: Vec<String>,
    pub expires_at: Timestamp,
}

pub struct ProposedChange {
    pub operation: PlannerOperation,
    pub target_label: String,
    pub description: String,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}
```

`why` is a short user-facing explanation, not hidden model reasoning. `changes` contains concrete,
individually readable task/habit/schedule differences. `inferred_fields` states each value supplied
by the model and its source. `warnings` contains consequences such as displaced tasks or reduced
sleep. These fields map directly to the client screen:

```text
Why?
  <why>

Changes
  <change 1>
  <change 2>

[Deny] [Approve]
```

Pending approvals live only in the owning `AgentSession`; they do not add database tables, an audit
log, a change-set API, or a general undo system. A session keeps a bounded number of expiring
requests. Restarting the agent invalidates them without applying anything. Only the exact owning
session may resolve an ID, and each request may be resolved once.

`AgentSession::resolve_approval(id, Approve)` executes the stored operations through the ordinary
`takusu-client` CRUD methods in source order. It never accepts replacement operations from the
client. Update and delete proposals retain the target's observed `updated_at` (or equivalent
version) and are rejected if the target changed before approval. A create ID is allocated when the
proposal is built so retrying transport delivery cannot create a second object. If a multi-operation
approval partially fails, execution stops, the result identifies applied and unapplied operations,
and the agent refreshes state before offering a follow-up proposal; this lightweight flow does not
promise cross-operation atomicity.

`Deny` discards the request and performs no planner write. The client may include an optional short
reason as the next user message so the agent can revise its proposal. While an approval is pending,
the session rejects another mutating turn or requires the client to deny the old request first;
read-only turns remain allowed.

### Schedule orchestration contract

A successful task/habit change sets `schedule_dirty = true`; it does not automatically call the
planner. Before answering schedule questions, or after the configured quiet period, the agent calls:

```json
POST /api/schedule/preview
{
  "mode":"full|partial",
  "from":null,
  "until":null,
  "task_ids":null,
  "pinned":[],
  "sleep":"recommended"
}
```

The endpoint runs the planner without replacing the active schedule and returns:

```json
{
  "entries": [],
  "unscheduled_task_ids": [],
  "displaced_task_ids": [],
  "sleep_minutes_before": 480,
  "sleep_minutes_after": 420,
  "warnings": []
}
```

The `AgentSession` keeps the candidate only as part of its pending approval. The approval screen
summarizes moved and unscheduled tasks and any sleep reduction; the user does not have to infer
impact from raw entries. On approval, the agent sends the stored candidate to a schedule replace
operation that validates every referenced task and atomically replaces the active schedule. On
denial or expiry, the candidate is discarded. This requires no persisted preview table. After any
intervening approved task/habit mutation, the candidate becomes stale and a fresh preview must be
shown for approval. A successful replacement clears the session's dirty flag.

### Mobile integration contract

The Android app is the primary approval client. Users configure its Agent connection under the
existing categorized Settings screen. On Home, tapping the central add button opens the Agent;
sliding that button upward continues to open manual task creation as specified in `mobile-ui.md`.
The agent transport exposes the same session API as the library rather than putting planner logic in
Kotlin:

- send text or an STT transcript to a session and receive `TurnResult`;
- receive an optional `ApprovalRequest` alongside the assistant text;
- approve or deny that request by ID and receive an `ApprovalResult`;
- resume a still-live session and recover its unresolved request.

The initial transport may use request/response HTTP; WebSocket streaming can be added without
changing these messages. Authentication scopes every session and approval ID to the current user.
The server must serialize turn and approval resolution for a session, and repeated delivery of the
same decision returns the previous result rather than executing it again.

When `approval_request` is present, Mobile opens a sheet or screen with these sections in order:

1. **Why?** — `why` and any inferred values with their reasons;
2. **Changes** — one readable row per proposed operation, expandable to before/after fields;
3. **Warnings** — shown only when non-empty, with schedule impact emphasized;
4. **Deny / Approve** — both remain explicit; dismissing the sheet does not approve.

While approval is being submitted, both actions are disabled. Success updates the local task and
schedule views from `ApprovalResult`; failure keeps the proposal visible with a retryable error when
safe. Deny may optionally collect a correction and send it as the next turn. Expired or lost-session
requests show that no change was made and ask the user to request a new proposal. Notifications and
voice replies may announce that approval is needed, but they must deep-link to this screen rather
than treating a spoken acknowledgement outside the active session as approval.

The CLI implements the same contract as a reference client by printing Why and the change list and
requiring an explicit `approve` or `deny`; `--yes` is permitted only for tests or deliberate
non-interactive use and must never be the Mobile default.

## Sequential work items

### WI-0: Existing scaffold and core loop

**Status**: implemented by the existing takusu-agent scaffold and core-loop changes.

Keep the current `AgentConfig`, `ToolRegistry`, `LlmClient` abstraction, message types, and scripted
mock tests. Before later WIs rely on the loop, align it with the shared contracts above:

- count actual tool calls;
- feed recoverable tool errors back to the model;
- serialize turns per `AgentSession`;
- return a `TurnResult` containing response text and an optional grouped approval request;
- trim by estimated/provider token count and preserve complete tool-call groups;
- inject server timezone and the real skills index rather than empty placeholders.

**Verify**: scripted tests for correction after invalid arguments, multiple calls respecting the
limit, concurrent-turn serialization, history trimming, and proposal collection.

### WI-1: OpenAI chat-completions adapter

**Files**: `crates/takusu-agent/src/llm.rs`.

Implement a hand-written `reqwest` adapter for `POST {base_url}/chat/completions`, including tool
schemas, multiple tool calls, nullable content, and provider error bodies. Retry 429 and retryable
5xx responses with bounded exponential backoff and jitter. Provider retries may rebuild a proposal
but must never resolve an approval or repeat a planner write. Apply request timeouts and support
cancellation.

“OpenAI-compatible” is a tested baseline, not a guarantee that every OpenRouter, llama.cpp, or
vLLM version has identical behavior. Keep provider capabilities behind `LlmClient`; document and
test each supported endpoint using captured fixtures.

**Verify**: fixture-based serialization/deserialization tests, retry classification tests, and an
ignored real-endpoint smoke test when the configured API key exists.

### WI-2: Text CLI and application-level audio

**Files**: `crates/takusu-agent/src/audio.rs`, `src/bin/agent.rs`, and a playback module in
`takusu-audio`. Remove the obsolete `src/tools/audio.rs` stub and its module registration when the
application-level adapter replaces it.

Implement `takusu-agent --text "今日の予定は?"` first. Then add push-to-talk:

1. the CLI records and transcribes one utterance;
2. it calls one agent turn;
3. it prints the response and any Why/change approval prompt;
4. it resolves that prompt only after explicit `approve` or `deny` input;
5. unless `--no-tts` is set, it synthesizes and plays the response;
6. it waits for the next user-initiated recording.

Playback validates WAV headers, sample format, channel count, and sample rate before opening a `cpal` output stream. Audio/STT/TTS failures do not corrupt the conversation or retry planner mutations. Add timeouts and cancellation around all network and device operations.

Do not register `listen` or `speak` as LLM tools. VAD/noise suppression and streaming can be added
behind the same audio adapter later.

**Verify**: text-mode E2E with a mock LLM, WAV parser tests, and manual STT/TTS smoke tests.

### WI-3: Planner read tools and task references

**Files**: `crates/takusu-agent/src/tools/takusu.rs` plus focused client additions if needed.

Implement read-only tools first:

- `list_tasks`, `get_task`;
- `list_habits`, `get_habit`;
- `get_schedule`;
- `get_settings`.

Resolve global task `display_id` directly and habit task references only with their habit scope.
Do not resolve by fetching an arbitrary filtered page. Add a dedicated server/client lookup if the
existing API cannot resolve an identifier uniquely. Datetime interpretation uses server timezone
and `takusu-util`; tool responses include normalized absolute timestamps so the LLM can explain its
interpretation.

**Verify**: argument-schema, timezone boundary, global ID, habit-scoped ID, missing ID, and
ambiguous-reference tests.

### WI-4: Approved mutations and schedule orchestration

**Files**: `crates/takusu-agent/src/lib.rs`, `takusu-client`, `tools/takusu.rs`, plus focused
local/Worker schedule routes if preview or atomic replacement is missing.

Implement the client-approval and schedule-orchestration contracts above. Add proposal-producing
tools for task/habit create/update/delete and schedule preview. Add `ApprovalRequest`,
`ApprovalResult`, bounded pending state, expiry, and `resolve_approval`. The ordinary planner CRUD
calls are private execution details and are invoked only by approved stored operations, never by a
second LLM turn or client-supplied payload.

Mutation arguments include the model's `inferred_fields`; the grouped request renders those values
and reasons with concrete before/after changes. Keep schedule dirty state in the session. Multiple
approved conversational edits trigger at most one schedule preview after the quiet period or when a
schedule answer requires fresh data. A denied schedule proposal leaves the dirty flag set so the
user may revise tasks or request another preview.

**Verify**: proposal grouping, no write before approval, approve, deny, expiry, one-shot resolution,
stale update/delete rejection, stable create IDs, partial-failure reporting, preview without
persistence, impact warnings, stale preview invalidation, and dirty-state clearing. Run focused
local and Worker tests for any added schedule endpoint behavior.

### WI-5: Mobile Agent integration

**Files**: agent transport/session adapter, `mobile/src/views/SettingsView.tsx`,
`mobile/src/api/settingsStore.ts`, `mobile/src/components/AddButton.tsx`, `mobile/src/views/HomeView.tsx`,
a focused Agent API/provider module under `mobile/src/api/`, and Agent/approval UI under
`mobile/src/components/` or `mobile/src/views/`.

Expose a minimal authenticated Agent transport with connection check, session creation, `run_turn`,
unresolved-approval recovery, and `resolve_approval`. The Mobile client and Rust transport share
versioned request/response fixtures. Authentication scopes sessions and approval IDs to the
configured user, and repeated delivery of a decision returns the previous result rather than
executing it again.

Add `agent` to the existing Settings category list. Its detail screen configures the Agent service
URL and token, checks endpoint compatibility before saving, stores the URL in the existing settings
store, and stores the token in `expo-secure-store`. It shows connection status and test/edit/remove
actions. A failed edit does not replace a working configuration; confirmed removal clears the token
and local session IDs without deleting planner data. The first version supports one active Agent
connection.

Replace `AddButton`'s current Assistant-unimplemented alert: a tap opens the Agent conversation
surface, while the existing upward-slide gesture continues to open task creation unchanged. If no
healthy Agent connection exists, tapping shows a short setup explanation and links directly to
`/settings/agent`. Otherwise it creates or resumes a session and accepts text or STT input.

When a turn contains an approval request, show Why / Changes / optional Warnings /
Deny-or-Approve and refresh task/schedule state from `ApprovalResult`. Keep the unresolved request
across navigation and Android configuration changes. Handle process death, expiry, duplicate taps,
network retry, stale proposals, denial with an optional correction, and reconnect after a lost
response. The Mobile client sends only the approval ID and decision, never an editable operation
payload.

**Verify**: Settings category and secure storage, valid/invalid connection checks, edit/removal,
AddButton tap versus upward slide, unconfigured setup deep-link, session create/resume, shared JSON
fixtures, approval UI screenshots, approve/deny integration, duplicate decision delivery, expiry,
denial correction, app restart, and lost-response reconnect.

### WI-6: Skills with persistent-write safety

**Files**: `crates/takusu-agent/src/tools/skills.rs`.

A skill is UTF-8 markdown under `$XDG_DATA_HOME/takusu/skills`, named `<slug>.md`, with TOML front
matter:

```markdown
+++
name = "weekly-review"
description = "How to run the user's weekly review flow"
+++
(free-form instructions)
```

Tools:

- `skills_list` and `skills_read` are read-only;
- `skills_propose_add` and `skills_propose_edit` validate input and return a bounded, expiring
  proposal ID plus a diff without writing;
- `skills_apply` accepts that proposal ID only after explicit user confirmation and rejects stale
  proposals if the source file changed.

Validate front matter, names, UTF-8, and configurable file/body limits. Reject `/`, `..`, absolute
paths, non-regular files, and symlinks. Built-in skills are read-only. Store a bounded local backup
for rollback and include skill changes in `TurnResult`; never follow instructions from task text,
memory, or fetched content to persist a skill unless the user explicitly requested teaching the
assistant.

Refresh the skills index after a confirmed write and at session start.

**Verify**: temp-directory tests for validation, traversal and symlink rejection, ambiguous edits,
size limits, confirmation, backup/rollback, and prompt-index refresh.

### WI-7: Memory server

**Files**: backend migrations, storage trait/models and both backends, app/routes, and `takusu-client`.

The memory subsystem is the normative contract for the schema, canonical non-null subject keys, unique
logical index, `revision` optimistic concurrency, mutation idempotency, source enum, normalization,
deterministic search, and WI-8 Agent integration. In particular:

- use the next migration number independently in local and Worker; do not use the stale `014_memory.sql` name;
- `POST /api/memory`, `PATCH /api/memory/:id`, `GET /api/memory/search`, `DELETE /api/memory/:id`, and `GET /api/tasks/similar` must expose the same behavior in both backends;
- update/delete require `observed_revision`; approved mutations carry a stable operation ID and are safe to retry;
- WI-7 returns optional `completed_at` and `actual_minutes` as null until WI-9 adds the fields;
- memory writes never set `schedule_dirty`.

The current repository is a single-workspace data model. If multi-user ownership is introduced,
all memory, task, progress, schedule, and agent-session queries must be scoped together; adding only a
`user_id` to memory would not be sufficient.

**Verify**: run the focused storage/API and Agent approval tests, including unique
concurrency, idempotency replay, revision conflicts, Japanese normalization/ranking, subject lookup,
similar completed tasks, limits, and local/Worker parity.

### WI-8: Memory tools and inference flow

**Files**: `crates/takusu-agent/src/tools/memory.rs`, `AgentSession::execute_proposed_change`,
`takusu-client`, and system-context rules.

Implement `memory_save`, `memory_update`, `memory_search`, `memory_delete`, and `similar_tasks`,
and add the corresponding memory create/update/delete dispatch to the approval executor. Memory
proposals must produce `ChangeReceipt.target_type = "memory"`, preserve `observed_revision`, use a
stable operation ID for retries, and leave `schedule_dirty` unchanged.
The system flow is:

1. search a proper noun before treating it as unknown;
2. if no adequate result exists, ask the user rather than inventing its meaning;
3. save only after the user supplies or confirms the meaning;
4. before creating a task without an estimate, inspect similar completed tasks;
5. state whether the estimate came from history, the user, or model knowledge.

Memory deletion is destructive and requires confirmation. Do not automatically persist arbitrary
conversation content as memory.

**Verify**: scripted turns for memory hit, miss→question→save, estimate from similar task, fallback
estimate disclosure, and confirmed deletion.

### WI-9: Active-session progress management

**Files**: `015_progress.sql`, storage trait/models and both backends, app/routes, and
`takusu-client`.

Add quantitative fields and split lineage to tasks:

```sql
ALTER TABLE tasks ADD COLUMN quantity_total INTEGER;
ALTER TABLE tasks ADD COLUMN quantity_done  INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN quantity_unit  TEXT;
ALTER TABLE tasks ADD COLUMN completed_at   TEXT;
ALTER TABLE tasks ADD COLUMN split_from_task_id TEXT REFERENCES tasks(id);
ALTER TABLE tasks ADD COLUMN original_quantity_total INTEGER;

CREATE TABLE task_work_sessions (
    id         TEXT PRIMARY KEY,
    task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    started_at TEXT NOT NULL,
    ended_at   TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE progress_events (
    id                TEXT PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    at                TEXT NOT NULL DEFAULT (datetime('now')),
    quantity_done     INTEGER,
    delta_quantity    INTEGER,
    active_minutes    INTEGER NOT NULL,
    note              TEXT
);
```

Only one open work session is allowed per task. Operations are idempotent and validated:

- `POST /api/tasks/:id/work/start`;
- `POST /api/tasks/:id/work/pause`;
- `POST /api/tasks/:id/progress` with cumulative `quantity_done`;
- `POST /api/tasks/:id/work/complete`;
- `GET /api/tasks/:id/progress`;
- `POST /api/tasks/:id/split` with the retained and remainder quantities.

A lower cumulative quantity is treated as an explicit correction, not a new speed observation.
Reject negative quantities and require confirmation when a correction would materially change an
estimate. `quantity_done >= quantity_total` may propose completion, but should not silently close
an open session due only to a transcription error.

For each interval with `delta_quantity > 0` and positive new active time:

```text
minutes_per_unit = delta_active_minutes / delta_quantity
projected_total = minutes_per_unit * quantity_total
bounded_projection = clamp(projected_total, 5 minutes, 24 hours)
new_avg = round(0.5 * old_avg + 0.5 * bounded_projection)
```

With at least two valid interval observations, derive sigma from the sample standard deviation of
projected totals, cap it to the same bounds, and document the exact rounding. With fewer than two,
leave sigma unchanged. Completion `actual_minutes` is the sum of closed active work sessions, not
`completed_at - first_started_at`.

The split endpoint atomically preserves the original task history, records lineage, creates the
remainder, and establishes dependency if requested; the LLM must not emulate splitting by shrinking
historical totals through unrelated CRUD calls.

**Verify**: start/pause/resume, duplicate requests, cumulative progress, zero/decreasing/corrected
values, estimate and sigma math, completion active time, split lineage, and local/Worker parity.

### WI-10: Progress tools and schedule flow

**Files**: `crates/takusu-agent/src/tools/progress.rs`.

Implement `task_start`, `task_pause`, `task_progress`, `task_complete`, and `task_split`. Resolve
task references through WI-3. Report estimate changes and their evidence in the response. These
persistent progress changes use the same client approval request as WI-4. Once approved, changes
that affect planning mark the schedule dirty rather than rescheduling inside the progress tool.

If the user says only “着手した” or “完了した” and multiple tasks are plausible, ask a focused
question before producing a proposal. Quantity updates that imply completion, corrections, and
splits are always summarized in its change list.

**Verify**: scripted E2E scenarios for start→pause→resume→progress, no write before approval,
estimate update, dirty schedule, schedule approval, completion, ambiguous references, correction,
and split.

### WI-11: End-to-end hardening

Run complete text and voice scenarios against `takusu-local`, a mock LLM, and optionally a real LLM:

- “演習30題追加” → Why/inference/change list → approve or deny;
- unknown proper noun → memory miss → question → save → propose create;
- consecutive approved edits → one schedule preview and approval;
- urgent task → sleep/displacement warning → approve or deny;
- edit target after proposal → stale approval rejected without overwriting it;
- start/pause/progress/complete using active time and explicit approvals;
- split a task while preserving history;
- Mobile session resume, expiry, denial correction, and lost-response retry;
- STT/TTS unavailable while text mode and planner state remain usable;
- repeated provider/transport request without duplicate mutation.

Add structured tracing with secret redaction, per-provider timeouts, graceful Ctrl-C cancellation,
and clear CLI rendering of tool activity at `-v`. Do not log tokens, raw audio, private memory
content, or complete LLM prompts by default.

**Verify**: `cargo fmt`, `cargo clippy --workspace`, `cargo nextest run --workspace`, Worker tests,
and documented manual audio smoke tests.

## Implementation order

```text
WI-0 harden existing core
  → WI-1 LLM adapter
  → WI-2 text CLI + audio adapter
  → WI-3 planner reads and references
  → WI-4 approved mutations + schedule preview
  → WI-5 Mobile Agent integration (Settings + AddButton + approval UI)
  → WI-6 safe skills
  → WI-7 memory server
  → WI-8 memory tools
  → WI-9 active-session progress server
  → WI-10 progress tools
  → WI-11 E2E hardening
```

Use one focused jj change per WI when practical. Because one agent implements the sequence, prefer
updating shared model/storage/router/client contracts once in the server WI and consuming them in
the immediately following agent WI. Do not develop against speculative parallel branches.

Before each push, rebase the current change onto `main`, run focused tests during development, then
run formatting, clippy, and the relevant crate/integration tests. If a contract changes, update this
document and all affected tests in the same change.

## Future: embedding-based memory search

Start with deterministic lexical search and measure failures before adding embeddings. If semantic
recall is justified:

- store a versioned multilingual embedding and model identifier with each memory;
- keep a common storage-trait method for SQLite and D1;
- generate embeddings locally when deployment constraints allow it;
- avoid a dedicated vector database at personal scale;
- account for D1 network transfer and WASM compute rather than assuming a remote full-table scan is
  sub-millisecond;
- rebuild embeddings when the model/version changes;
- retain lexical search as fallback.

`sqlite-vec` may be a local-only optimization behind the same trait. Cloudflare Vectorize or a
separate vector server should be considered only if measured scale or latency requires it.

## Out of scope

- Android `VoiceInteractionService`, hotword activation, and production transport deployment/scale work beyond the Mobile approval contract;
- streaming audio/VAD/noise suppression beyond the adapter boundary;
- external tools such as Google Maps;
- gamification;
- semantic embeddings before lexical-search measurements;
- multi-user tenancy until ownership is defined consistently for all planner data.
