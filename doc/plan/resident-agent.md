# takusu Resident Planner Agent Implementation Plan

## Summary

Implement the resident planner agent defined in `../resident-agent.md`: an agent whose job is to
keep the plan and reality in sync by running three loops — **capture** (reality → plan),
**sync** (deviation detection → adjustment), and the **execution loop** (start → progress →
complete) — on Android and Linux desktop. The agent presents planner state instead of free-form
chat, contacts the user through one-round-trip check-ins that always offer both 「行動」 and
「ズラす」, drives in-progress interventions from duration-distribution posteriors, supports a
hands-free voice loop, and finally ambient listening behind on-device gates.

The implementation proceeds sequentially with one agent, split into four phases matching the
rollout section of the design document:

```text
Phase 1  execution loop UI + minimal sync (presentation types, current task card, start-time notifications)
Phase 2  shared core + thin surfaces (state machine, surfaces, intervention math, event ledger, coverage, arbitration)
Phase 3  voice loop + capture (VAD endpointing, voice approval layers, one-utterance capture, intake, 精算)
Phase 4  ambient listening (VAD → KWS → speaker → ASR gates on desktop first, then Android foreground service)
```

Each work item (WI) should leave the repository working and tested before the next begins. Each WI
is opened as its own pull request against the accumulating `resident-agent` branch (one WI per PR,
parented on `main`), so review and rollout stay incremental; a later WI that depends on an earlier
one builds on the merged PR. **At each phase boundary (`resident-agent` passes the phase gate
below), it is merged into `main` and released as a normal release**; `resident-agent` is then
re-based/re-cut from the new `main` tip so the next phase builds on the released code rather than
carrying unreleased changes into the next phase.
Contracts in this document may be refined when implementation reveals a problem, but the document
and all affected clients must be updated in the same change.

This plan builds on the implemented `plan-agent.md` stack: `AgentSession` / `TurnResult` /
`ApprovalRequest` in `takusu-agent`, the HTTP+SSE transport (`/api/agent/v1/*`), planner /
progress / memory / skills / comments tools, the session-scoped `Permissions` map,
sentence-streaming TTS (`TtsQueue`), the Mobile `AgentView` + `ApprovalPanel`, the Android widget,
and the server progress endpoints (`/api/tasks/:id/work/*`, `/progress`, `/split`). It also builds
on `task-comments-memory-redesign.md`: postpone reasons and settlement notes are stored as task
comments, and the sync-loop reconciliation comment hook deferred there lands here in Phase 3.

## Resolved architecture decisions

These decisions are fixed for this plan and are not implementation-time open questions:

- **Per-device local server**: each Android or Linux surface keeps its own `takusu-local` process
  or embedded equivalent. It remains an API proxy and application host; it is not replaced by one
  shared server endpoint. Multi-device state is shared through one Worker/D1 or other common
  backend. Independent per-device SQLite databases remain unsupported.
- **Resident authority**: the resident device is the single owner of planner-event evaluation and
  event-ledger commits. The role is retained when its microphone service is stopped, so planner
  synchronization and notification fallback continue.
- **Speech capability**: microphone service availability and private output routing are separate
  device capabilities. `SpeechCapability = false` changes proactive delivery to a notification;
  it does not transfer event authority or cause another device to evaluate the same event.
- **Android evaluator lifecycle**: an exact-alarm receiver invokes a bounded Rust
  `evaluate_and_commit_events` entry point directly, evaluates the shared snapshot, commits the
  ledger, posts or queues delivery, and reserves the next alarm. It does not start a full local
  HTTP server for every alarm. The microphone foreground service owns only continuous audio,
  voice sessions, and its audio heartbeat.
- **Heartbeat and lease scopes**: evaluator authority and audio-service liveness are separate.
  Desktop refreshes an evaluator heartbeat while its host is running. Android does not run a
  high-frequency heartbeat solely for residency; each exact alarm reserves an evaluator lease valid
  through the next scheduled evaluation plus a grace period, and the receiver renews or reacquires
  it before committing. Proactive speech additionally requires local `SpeechCapability`. A stopped
  microphone service therefore produces notification delivery, not a second resident evaluator.
- **Estimator model**: the estimator uses a positive-support truncated-normal distribution and an
  exact truncated-normal posterior update in minute units. Planner slot conversion happens only at
  the `takusu-core` planner boundary.
- **Phase 1 surface**: the compact panel is available in Phase 1. Start, progress, complete, and
  delay do not require opening the full Agent view.
- **Quick-action authorization**: screen and notification actions use a server-issued, one-shot
  capability bound to the event, device, action, and expiry. The capability supplies the trusted
  input path to the common authorization layer; clients cannot self-assert an `InputPath`.

## Product invariants from `../resident-agent.md`

The implementation must preserve these behaviors across all work items:

1. **Sync is the product**: the agent's success on any day is that the plan reflects reality by
   the end of it, not that the plan was executed. Capture and sync failures happen before the
   execution loop; features must not assume a task is registered or started.
2. **Check-in is the contact atom**: every proactive contact is a one-round-trip check-in whose
   answer feeds capture, sync, or the execution loop. A check-in response completes in one
   utterance; the agent never turns classification into an interrogation.
3. **Every deviation contact offers 「行動」 and 「ズラす」** as equal-cost options. Honesty is
   always cheap; the agent records, never lectures or grades. Ignoring is always free: no response
   degrades to a notification and is not chased.
4. **Typed presentation, not generated UI**: the LLM never generates arbitrary UI JSON. Clients
   render from typed presentation data built out of tool results, schedule state, and approval
   requests. Voice output is generated from the same presentation types with fixed templates.
5. **Approval invariants carry over**: all `plan-agent.md` invariants remain. Persistent task,
   habit, schedule, and persistent-skill changes are never written before approval; reversible
   work-session start/pause and short snooze actions are the explicit capability-authorized
   exception. Ambiguous acknowledgements are never treated as approval.
6. **Input-path-aware approval layers**: operations are classified by misattribution damage into
   immediate (a valid screen/notification capability or explicit-session start/pause/short snooze),
   ambient-immediate (wake-word start/pause; speaker verification required), voice-confirmed
   (readback + explicit affirmative + speaker verification), and screen-required. Layer
   classification runs **before** any permission grant is consulted, so a default-granted
   operation arriving over an unverified path never bypasses its layer. A client cannot supply its
   own `InputPath`; the server-issued one-shot capability or trusted session host supplies it.
   Undoing a wrong start/pause also cancels the estimator observation it produced as a compensating
   observation, never a revision rollback.
7. **Speaker verification is a soft gate**: a failed verification degrades to the screen fallback;
   it never hard-rejects. Reactive responses and screen/notification immediate actions do not
   require verification.
8. **Distribution-driven intervention**: in-progress interventions are derived from the task's
   duration distribution, using the observation that is actually available (right-censored overrun
   or progress-based pace), normalized into three bands (通常 / 注意 / 再計画). Start delays are
   sync events, not distribution observations. `fixed = true` tasks are excluded from deviation
   judgement. Non-fixed tasks with `sigma = 0` select a task-kind prior or the wide fallback rather
   than being silently excluded.
9. **Proactive speech only on private channels**: reactive speech is always allowed. Proactive
   speech requires earphones on Android (or an ongoing voice conversation); otherwise it degrades
   to a notification. Desktop speaks by default, suppressed only by quiet hours. No sensor-based
   scene inference.
10. **One resident authority**: arbitration is mechanical (priority list + evaluator heartbeat or lease).
    Exactly one live device holds planner-event evaluation authority and commits events to the
    shared ledger. Speech capability is separate: a resident device without a microphone service
    continues evaluation and notification delivery but does not speak proactively. No distributed
    consensus; the event ledger absorbs partition duplicates.
11. **Events are ledgered and idempotent**: planner events are recorded with deterministic IDs,
    snapshot revisions, immutable presentation payloads, delivery state, and stable action
    operation IDs. The same event never fires twice under normal connectivity and never
    re-executes the same mutation after retries or partition merges.
12. **Coverage gates authority**: current task is presented as a candidate in bootstrap, as
    「今やること」 only from today-covered, and settlement is offered before current task when
    stale. Trust states are computed from structured coverage confirmations and time intervals,
    external calendar health, and other observable conditions, never from task counts.
13. **Ambient is gated and opt-in**: ambient listening is off by default, always visibly
    indicated, stoppable from the notification and the surface. Only VAD and KWS run continuously;
    ASR and the LLM start only after the wake gate. Pre-gate raw audio is never persisted, logged,
    or uploaded. Voiceprint embeddings stay on device and are deletable.
14. **Session state lives in the shared Rust core**: surfaces (Android UI, tray) subscribe to one
    state machine; they never own recording, turn, TTS, or approval state. Platform shells own the
    microphone lifecycle; Expo/React Native component lifecycles never own continuous recording.

## Architecture

```text
crates/takusu-core                     # existing; grows the estimator math
└── src/estimator.rs            (new)  # duration-distribution posteriors + intervention bands (pure)

crates/takusu-agent                    # existing; grows the shared resident core
├── src/presentation.rs         (new)  # typed presentation model + voice templates
├── src/surface.rs              (new)  # SurfaceState machine + surface protocol events
├── src/events.rs               (new)  # planner event evaluation (pure policy) + contact policy
├── src/coverage.rs             (new)  # coverage trust-state computation
├── src/voice_session.rs        (new)  # continuous voice session loop (listen→act→speak)
├── src/approval_layers.rs      (new)  # four-layer classification on top of Permissions
└── src/transport.rs                   # extended: surface state SSE, presentation payloads

crates/takusu-audio                    # existing; grows the on-device speech stack
├── src/vad.rs                  (new)  # silero VAD via sherpa-onnx (endpointing + ambient gate)
├── src/stream_asr.rs           (new)  # sherpa-onnx streaming ASR (zipformer transducer)
├── src/kws.rs                  (new)  # sherpa-onnx keyword spotting (wake word)
└── src/speaker.rs              (new)  # speaker embedding + verification (WeSpeaker/3D-Speaker)

crates/takusu-desktop           (new)  # Linux resident daemon (systemd user service)
├── src/main.rs                        # daemon: agent client, heartbeat, event subscription
├── src/tray.rs                        # StatusNotifierItem tray icon (ksni), state → icon
├── src/notify.rs                      # desktop notifications with action buttons (zbus)
├── src/popover.rs                     # compact popover window (GPUI), replacing a tray menu
└── src/audio.rs                       # PipeWire/cpal capture ownership, ambient gates (phase 4)

takusu-local-lib / takusu-local        # existing; per-device app host and event evaluator
├── migrations/0xx_devices.sql         # device registry, evaluator lease/audio status, priority
├── migrations/0xx_event_ledger.sql    # event ledger, payload, delivery state, claims
├── migrations/0xx_schedule_revision.sql # monotonic active-schedule revision
├── migrations/0xx_estimator_state.sql # task revisions, observation lineage, task-kind priors
├── migrations/0xx_coverage.sql        # confirmations and unsettled time intervals
├── /api/devices/*                     # register, heartbeat, resident-role resolution
├── /api/events/*                      # evaluate, claim, acknowledge, replay
└── /api/agent/v1/... events           # per-device surface and event stream (SSE)

mobile/                                # existing; grows resident surfaces
├── src/components/ResidentAgentButton.tsx   # app-wide draggable button (replaces Home-only FAB)
├── src/components/AgentCompactPanel.tsx     # one-round-trip sheet/overlay
├── src/components/CurrentTaskCard.tsx       # Home current/next task + quick actions
└── modules/takusu-agent-service/            # phase 4: microphone foreground service (Kotlin)
```

Placement rules:

- Duration-distribution math is pure and lives in `takusu-core`. Its public estimator API uses
  minutes because work sessions and task rows use minutes; conversion to the planner's 5-minute
  slots happens only at the planner boundary. The planner, evaluator, and tests share one
  implementation.
- Planner event evaluation must behave identically on both platforms and both alarm models, so it
  is a pure function in the shared Rust application layer:
  `(consistent snapshot, now, ledger view) → (due events, next_eval_at)`. The snapshot carries
  schedule and distribution revisions. The resident device invokes it from its per-device
  `takusu-local` application host; Android's exact-alarm receiver calls the bounded Rust entry
  point directly rather than starting a full HTTP server. Progress and schedule writes publish a
  state-changed signal after committing their new revisions; only the resident authority invokes
  the evaluator.
- A resident evaluator may commit an event only while holding the evaluator heartbeat or lease. The
  ledger commit verifies the snapshot revisions and atomically claims the deterministic event ID.
  Quiet hours defer delivery rather than consuming an event.
- The event ledger, device registry, estimator state, coverage records, and evaluator lease must
  be visible across devices, so they live in the common backend (SQLite and D1/Worker parity, like
  every other storage feature). Independent per-device SQLite databases remain unsupported.
- Each device may keep a local `takusu-local` API host. The Linux daemon consumes its local host,
  `takusu-client`, and the agent transport; it contains no planner logic. The Android embedded
  host exposes the same application-layer evaluator to its alarm receiver.
- Ambient audio is owned by the platform shell (Android foreground service / Linux daemon) and
  processed by `takusu-audio`; JS/UI layers only subscribe to state. The microphone service owns
  continuous audio, voice sessions, and audio heartbeat, not planner-event authority.
- The Android event evaluator and the microphone foreground service have independent lifecycles;
  evaluation and notification continue while the microphone is stopped, with proactive speech
  degrading to notification when `SpeechCapability` is false.

## Shared contracts

### Presentation contract

`TurnResult` and the event stream gain a typed presentation payload. The initial closed set:

```rust
pub enum Presentation {
    CurrentTask(TaskCard),            // current + next task, quick actions; carries authority level
    WorkTransition(WorkTransition),   // start / pause / progress / complete / delay result
    ScheduleSummary(ScheduleSummary),
    ProgressSummary(ProgressSummary), // done count, in progress, active time, vs. estimate
    ScheduleAlert(ScheduleAlert),     // conflict / overdue / generation failure
    CheckIn(CheckInCard),             // one question + [行動] [ズラす] (+相談) actions
    ChangeProposal(ApprovalRequest),  // rendered from the existing ApprovalRequest
    Clarification(FocusedQuestion),
    Text { text: String },            // fallback (struct variant: serde cannot internally-tag a primitive newtype)
}
```

Rules:

- Presentations are constructed by Rust code from tool results and schedule state; the LLM chooses
  which tool to call, never the rendering.
- Every presentation type has a deterministic voice template (fixed sentence structure, values
  interpolated) used for TTS and notification bodies. Template output is what the readback layer
  reads before a voice confirmation.
- `CheckInCard` stores non-empty `ActionGroup`s for both 「行動」 and 「ズラす」. The non-empty
  wrapper, not a runtime convention, prevents a card without either group. Every immediately
  executable action receives a server-issued capability when its delivery becomes eligible; a
  deferred quiet-hours event does not hold an already-expiring capability. A long postponement
  carries an `ApprovalRequest` instead.
- `TaskCard` carries the coverage authority (candidate vs. 「今やること」); clients render the
  distinction. The same authority and coverage state are included in the widget snapshot.
- Clients may render richer views but must not require fields outside the contract. The wire
  decoder explicitly maps unknown presentation tags to `Text` using the accompanying fallback
  text; merely adding a `Text` variant is not sufficient for forward compatibility.

### Input path and quick-action capability contract

```rust
enum InputPath {
    ScreenCapability,
    NotificationCapability,
    ExplicitVoiceSession,
    AmbientWakeWord,
    PlainText,
}

struct ActionCapability {
    id: String,
    event_id: Option<String>,
    device_id: String,
    action: String,
    input_path: InputPath,
    expires_at: Timestamp,
    one_shot: bool,
}
```

The server creates an opaque, one-shot capability for screen and notification actions. The client
returns the capability unchanged; it cannot create or change `input_path`, target, operation, or
expiry. The common authorization layer verifies the capability, device binding, expiry, and replay
state before classifying the operation and consulting `Permissions`. The action's operation ID is
derived from the capability ID, so retries return the previous result instead of applying a second
mutation. Plain text turns never receive an immediate capability.

### Surface state machine contract

```text
idle | listening | transcribing | thinking | waiting_for_user
     | waiting_for_approval | speaking | error
```

- The state machine lives in `takusu-agent` next to `AgentSession`; the transport exposes it as an
  SSE stream plus a snapshot endpoint. Existing `TurnEvent`s feed it; audio components report
  listening/transcribing/speaking.
- Surface taps map to fixed commands: confirm-recording (listening), open-panel (thinking), stop
  TTS (speaking), open approval UI (waiting_for_approval), show recovery (error). Commands are
  transport messages; surfaces contain no transition logic.
- One session's state is shared by all surfaces of that device. Switching surfaces never moves
  ownership of recording, turn, TTS, or approvals.

### Intervention contract (duration distributions)

There is no single "σ deviation" formula. Each in-progress intervention is defined by a random
variable, an observation, a distribution, and a threshold as one set:

- **Overrun without progress**: random variable is the task's total duration `T`; the observation
  is right-censored (`T > active_elapsed`, task still incomplete). Intervention strength comes from
  the conditional survival probability, not from a raw `(active_elapsed - avg) / sigma`.
- **Pace deviation with progress**: random variable is the total duration predicted from progress;
  the observation is active work time plus quantity completion rate. Intervention strength comes
  from how far the updated total-duration distribution has shifted slow of the prior.
- **Start delay**: not a duration observation. Handled by start-time events and sync check-ins;
  never expressed in σ.

Estimator model (v1) is fixed so that independent implementations agree:

- **Unit**: all estimator inputs and outputs use minutes as `f64` or integer minutes. Conversion to
  `takusu-core`'s 5-minute planner slots occurs only at the planner boundary; event and work-session
  calculations never compare slots directly with minutes.
- **Support**: `T ~ TruncNormal(μ, σ², 0, ∞)` from the task duration distribution. The `fixed`
  task flag, not `sigma = 0`, excludes a task from deviation judgement.
- **Revisions are monotonic and persistent**: the engine holds one current distribution `(μ_r, σ_r)`
  per task, tagged with a monotonically increasing distribution revision `r`. The current revision,
  distribution parameters, observation lineage, and compensating-observation links are stored in the
  common backend. Every posterior-producing observation issues a new revision; revisions are never
  rolled back, so event IDs keyed on them stay unique across restarts and devices.
- **Censored band check**: intervention strength is the survival probability
  `S_r(e) = P(T > e)` under the current revision at active time `e`. 通常 while `S_r(e) > 0.15`;
  注意 when `0.03 < S_r(e) ≤ 0.15`; 再計画 when `S_r(e) ≤ 0.03` (approximately crossing
  `μ + 1.04σ` and `μ + 1.88σ` for an untruncated normal). Replan proposals use the conditional
  expected remaining time `E[T − e | T > e]`. A censored band check does not create a new
  distribution revision by itself.
- **Exact progress update**: on each quantity report `q ∈ (0, 1]` at active time `e`, the naive
  projection is `y = e / q`, observed with noise `Y | T ~ Normal(T, τ²)` where
  `τ² = c · σ² · (1 − q) / q`. Let the prior be the truncated normal above. For `σ > 0` and
  `τ > 0`, first compute the untruncated normal product
  `v = (1/σ² + 1/τ²)⁻¹` and `m = v · (μ/σ² + y/τ²)`, then use `TruncNormal(m, v, 0, ∞)` as the exact normalized posterior. Let `μ_post` and `σ_post` be its normalized mean and standard deviation. The `τ = 0` case is a degenerate observation at `max(y, 0)`. The report is also band-checked by the standardized prior shift `z = (μ_post − μ_prior) / σ_prior`: 注意 at `z ≥ 1`, 再計画 at `z ≥ 2`.
- **`next_crossing_time`**: `S_r` is monotone decreasing in active time, so the next band-boundary
  crossing is deterministic: `now + (S_r⁻¹(p_band) − e)` counted in active time. If a new revision
  has already crossed a boundary at the current `e`, that band fires immediately; otherwise the next
  crossing is scheduled only while a work session runs, cancelled on pause, recomputed on resume,
  and recomputed on every revision bump. Unstarted tasks have no censored observation and therefore
  no crossing time; start delay belongs to sync events.
- The tunable constants (`0.15`, `0.03`, the `z` thresholds, likelihood noise `c`, and the task-kind
  fallback prior) live in one module (`takusu-core/estimator.rs`). The model shape and units are
  not per-event-kind.

Rules:

- One firing per (task, distribution revision, observation kind, band); a new revision re-arms all
  bands, but a contact policy suppresses duplicate contacts caused by the same user-visible
  deviation.
- `fixed = true` appointments are excluded from deviation judgement; only start-time and end-time
  rules apply. A non-fixed task with `sigma = 0` selects a task-kind prior or, if none exists, the
  wide low-confidence fallback `N(μ_default, (0.5·μ_default)²)` before participating normally.
- Task-kind priors have an explicit storage source and revision; if no history or kind prior exists,
  `μ_default` is a configured minute value and is never inferred from planner slots.
- Active work time comes from work-session start/pause, never wall-clock time. Across multiple work
  sessions, `e` is the task's accumulated active minutes.

Estimator state is persisted in a backend table or equivalent task revision columns. A progress,
completion, start/pause compensation, or direct quick-action write updates the task distribution and
revision in the same transaction as its work-session or progress mutation. All clients use this
storage path; the Agent tool is not allowed to maintain a private revision counter.

### Planner event contract

Event kinds (from the design doc): task start time reached, continued non-start past start time
(sync check-in), continued unclassified gap (capture/sync check-in), duration-distribution overrun
or pace deviation, predicted deadline violation, carried-over incomplete tasks (settlement entry
point), schedule not generated / generation failure, sleep impact.

```rust
pub struct PlannerEvent {
    pub id: String,                  // deterministic canonical event key
    pub kind: PlannerEventKind,
    pub task_ref: Option<TaskRef>,
    pub band: Option<InterventionBand>, // where distribution-driven
    pub presentation: Presentation,  // immutable notification/action content
    pub urgency: Urgency,            // drives notify vs. speak vs. suppress
    pub schedule_revision: i64,
    pub distribution_revision: Option<i64>,
}
```

- **Consistent snapshot**: the evaluator reads one snapshot containing planner state, progress,
  coverage, schedule revision, per-task distribution revisions, and a ledger view. A schedule or
  distribution write that races evaluation invalidates the snapshot; the evaluator retries with the
  new revision rather than committing a stale event.
- **Canonical event ID**: the ID includes event kind, task or gap identity, canonical scheduled
  boundary, schedule revision, distribution revision when applicable, and observation kind. Gap
  events use the canonical gap interval rather than an arbitrary evaluator wake time.
- **Ledger**: the ledger stores the event ID, immutable presentation payload, snapshot revisions,
  delivery state, action capabilities, and mutation operation IDs. Its transaction atomically
  claims a deterministic event ID after validating the evaluator heartbeat lease and snapshot
  revisions. A unique constraint alone is not considered sufficient for idempotency.
- **Delivery state**: `pending_delivery`, `delivered`, `deferred_quiet_hours`, `acknowledged`,
  `ignored`, and `resolved` are distinct states. Quiet hours defer delivery without consuming the
  event. Device-specific delivery claims prevent duplicate notifications on one device, while the
  event ID and action capability prevent duplicate planner mutations across devices and retries.
- **Evaluation is timer-independent**: the evaluator is a pure function that returns the next time
  its result can change. The resident device invokes it after progress/schedule writes and from
  Android's exact-alarm receiver. The receiver invokes the bounded Rust entry point directly,
  commits due events, posts or queues delivery, and reserves the next alarm. It never starts a full
  HTTP server for one evaluation and never depends on the microphone service.
- **Gap taxonomy**: schedule blanks are classified as 自由時間 / buffer / routine / 未分類 gap /
  生成失敗. Only unclassified gaps are check-in targets; routine gets at most a start-time cue;
  generation failure renders a planner-error `ScheduleAlert` with a replan action, never a
  check-in.
- **Gap taxonomy derivation (v1, app layer — no planner rewrite)**: routine comes from habit
  projections; 生成失敗 from the planner's existing unplaced-task tracking (exposed through the
  schedule read — a minimal core-output addition if not already surfaced); 自由時間 from
  configured free-time windows; everything else is 未分類. The buffer category stays **empty**
  until takusu-core grows an explicit buffer concept; the taxonomy type includes it from the
  start so its later arrival is not a contract change.
- Quiet hours (derived from sleep settings) suppress both voice and notifications; the 「緊急」
  exception stays an open question and defaults to nothing passing.
- An event does not necessarily start an LLM turn. Deterministic notifications and quick actions
  (`[着手] [10分後] [組み直す]`) are generated by application code; the LLM is invoked only for
  ambiguous adjustment, explanation, or proposals.
- Deep links: notification actions carry a server-issued capability. They call the common
  capability authorization endpoint, which invokes the immediate-layer progress or `Snooze`
  mutation with a stable operation ID, or they open the compact panel / approval UI.

### Contact policy contract (check-in behavior)

- Contact requires a concrete basis: continued non-start or a continued unclassified gap. A vague
  "seems off" is not a trigger (v1 uses no other signals).
- A check-in is one question and one user answer. The answer must route immediately to a structured
  capture, sync, progress, or action/shift proposal. Choosing an approval action afterward is an
  authorization step, not a second classification interview. The agent never asks a second
  classification question to determine one-off versus recurring activity; the presentation offers
  explicit one-off, recurring, free-time, and routine outcomes when the answer leaves that choice.
- No response → degrade to a notification, never re-ask about the same deviation; the next contact
  waits for the next event. Time bands with repeated no-response lower check-in frequency for the
  rest of the day.
- A per-day cap applies to proactive check-ins that ask about unknown activity; start-time and
  deadline notifications are not counted against it.
- 「ほっといて」 applies a timed contact suppression immediately.
- Postponing to another time band may ask 「なにか詰まってる?」 exactly once as a post-action
  reason hook, never as a repeated deviation check-in; silence is not chased and the answer is
  stored as a task comment. Snoozes of tens of minutes never ask a reason.

### Coverage contract

```text
bootstrap | today-covered | trusted | stale
```

- Computed in `coverage.rs` from structured storage records: coverage confirmations, confirmed
  period and schedule revision, unclassified gap intervals, unsettled-time intervals, and external
  calendar health. Never inferred from task counts.
- `coverage_confirmations` records the user or intake flow, local date range, timezone, source,
  confirmed schedule revision, and confirmation time. `unsettled_intervals` records a start, end,
  timezone, classification state, source, and settlement operation ID. Comments explain a settlement
  but do not replace these machine-readable records.
- State precedence is explicit: `bootstrap` applies before any confirmation; `stale` applies when
  unsettled intervals, stale calendar health, or an expired confirmation remain; `today-covered`
  applies when today's required period is confirmed and has no unresolved stale condition; `trusted`
  applies only after the target period's confirmation procedure has passed.
- bootstrap: `TaskCard` authority is "candidate"; intake is prompted. today-covered: today's
  current task is authoritative. stale: current-task authority drops and settlement is presented
  first. trusted: the confirmation procedure has been passed for the target period, not a
  completeness proof.
- Coverage is user-scoped state in the common backend and is shared by all devices. Home, widget,
  tray, and compact panel render the same authority and coverage state.

### Device arbitration contract

- **Deployment assumption**: multi-device features presuppose that all devices talk to **one
  shared takusu server/storage backend** (the normal deployment). Each device may still run its own
  `takusu-local` API host or embedded equivalent. Two devices holding independent SQLite databases
  are unsupported; no cross-database replication is planned. "Partition" means a device
  temporarily unable to reach the shared backend, not divergent databases.
- `devices` table (both backends): id, name, platform, priority, evaluator heartbeat or lease
  expiry, next scheduled evaluation, and audio-service status. Priority list is a user setting
  (default: desktop > Android).
- Desktop refreshes its evaluator heartbeat while the host runs. Android reserves an evaluator lease
  through its next scheduled exact alarm plus a grace period; the receiver renews or reacquires the
  lease before committing. The highest-priority device with a valid evaluator heartbeat or lease
  holds the **resident authority**; the role is computed by readers, not elected. Android does not
  schedule a high-frequency alarm solely to maintain the role.
- Audio heartbeat and private output route determine local `SpeechCapability`; they do not determine
  resident authority. A resident device without a microphone service evaluates events and delivers
  notifications. Proactive speech is permitted only when the local speech capability and private
  channel policy both pass.
- Only the resident authority evaluates planner events and commits them to the shared ledger.
  Non-resident devices show notifications for confirmed events, render surfaces, and serve
  user-initiated reactive turns and capability-authorized quick actions. Notifications go to all
  devices according to delivery claims.
- Offline devices keep their last known role, but an offline evaluator cannot claim new shared
  events until it reconnects and validates the snapshot and heartbeat lease. Duplicate detection
  during a partition is accepted; after reconnection, the ledger and stable operation IDs converge
  mutations without a second application. No consensus protocol.
- State ownership: **user-scoped** (planner state, coverage, planner events, proposals),
  **session-scoped** (turn, history, pending approval — owned by the initiating device),
  **device-scoped** (recording, TTS, audio route, surface state, speech capability), **ephemeral
  coordination** (resident authority, evaluator/audio heartbeat, delivery claim). Pending approvals
  stay session-owned; cross-device approval portability remains an open question and is out of
  scope.

### Voice approval layers contract

Built on the existing `Permissions` map rather than a parallel system. Classification depends on
both the operation and the trusted input path:

| Layer | Operations | Resolution | Speaker verification |
|---|---|---|---|
| Immediate | start/pause/short `Snooze` from a valid screen or notification capability, or explicit continuous session | execute, present result | session identity established at start |
| Ambient immediate | start/pause initiated by wake word | execute, read back result | required; screen fallback on failure |
| Voice-confirmed | progress, complete, single creates/edits with readable knock-on effects | readback + explicit affirmative | required |
| Screen-required | delete, whole-schedule replacement, unreadable impact | change-specific response or screen approval | n/a |

- Layer classification runs **before** the `Permissions` lookup: the authorization decision takes
  `(ProposedChange set, trusted InputPath)` and consults default grants only when the classified
  layer is Immediate. A start/pause/snooze arriving over ambient input or a plain text turn is
  classified into its own layer and can never ride the default grant. The existing `Permissions`
  map keys on target and operation only; `InputPath` is threaded through the server-side tool and
  surface command path, not bolted onto the permission key.
- The immediate layer is a named capability (`work:start`, `work:pause`, `schedule:snooze`) granted
  by default only after the server validates a one-shot screen/notification capability or an
  explicit voice session whose identity was established at start. This is not a client-side
  approval bypass. `Snooze` is a first-class reversible operation with a bounded duration; moving
  to another time band uses ordinary approval-gated `Move` or schedule changes. The design doc's
  layer table carries the same addition.
- Direct progress APIs are not an alternate authorization path. A screen or notification action
  calls the common capability endpoint, which validates and consumes the capability before invoking
  the ordinary progress or schedule mutation with its stable operation ID.
- Undoing a wrong start/pause reverts the work session **and** cancels the estimator observation
  via a compensating observation that issues a **new** distribution revision; revisions are never
  rolled back (a reused revision would collide with ledger event IDs and suppress later firings).
- The affirmative classifier is a closed-vocabulary on-device check; an acknowledgement outside
  the active confirmation window never resolves an approval. Ambiguity, silence, timeout, or
  failed verification falls back to the screen with `waiting_for_approval` lit on all surfaces.
- Layer classification is a pure function of `ProposedChange` sets plus trusted input path in
  `approval_layers.rs`, unit tested against the canonical scenarios in the design doc.

## Phase 1: execution loop UI and minimal sync

Progress storage, APIs, and agent tools already exist; this phase closes the experience loop and
puts the first 「行動」+「ズラす」 contact in front of the user.

### WI-1: Presentation types and transport exposure

**Files**: `crates/takusu-agent/src/presentation.rs`, `src/lib.rs`, `src/transport.rs`,
`mobile/src/api/agentTypes.ts`, shared JSON fixtures.

Define the `Presentation` enum and its construction from existing tool outputs: progress tools
produce `WorkTransition`, schedule reads produce `ScheduleSummary`, progress reads produce
`ProgressSummary`, approval requests produce `ChangeProposal`, `user_input` questions produce
`Clarification`. Include `CheckInCard` with its 行動+ズラす type shape even though nothing fires it
until WI-4. Attach presentations to `TurnResult` and stream them over the existing SSE transport
with a version-tolerant encoding (unknown kinds → `Text`). Implement the deterministic
voice/notification templates alongside the types so later phases reuse them.

**Verify**: unit tests mapping each tool output to its presentation; fixture round-trip tests
shared with the Mobile client; template snapshot tests (e.g. progress summary always renders the
same sentence structure for the same data); a `CheckInCard` cannot be constructed without both
action groups.

### WI-2: Current task card, compact actions, and quick-action capabilities

**Files**: `mobile/src/components/CurrentTaskCard.tsx`, `mobile/src/components/AgentCompactPanel.tsx`,
`mobile/src/views/HomeView.tsx`, focused additions to `mobile/src/api/`, capability handling in
`crates/takusu-agent/src/transport.rs`.

Home shows a current-task card (task, time window, work state) with quick actions
`[着手] [進捗] [完了] [延期] [相談]`. The card obtains a server-issued one-shot capability for a screen
action and sends that capability through the common authorization endpoint; it never calls a
progress mutation directly with a self-declared input path. Start and pause are immediate
capability-authorized work-session operations. Complete and delay use the Phase 1 compact panel,
which renders the result, inline approval, and a bounded `Snooze` action without opening the full
Agent view. The card renders from the same `CurrentTask` presentation data used by the agent,
including authority and coverage state. Until WI-10 lands, authority defaults to **candidate**,
the safe side of the coverage invariant; WI-10 promotes it from observed conditions and extends
the same field to the widget.

**Verify**: component tests for card states (no schedule, no current task, in_progress, overdue);
integration tests that a valid capability starts work without an approval, an expired or replayed
capability is rejected, and complete/delay finish in the compact panel without opening full Agent;
capability retry reuses the same operation ID; lint, `npx tsc --noEmit`, `npm run fmt:check`.

### WI-3: State synchronization across Home, widget, and Agent UI

**Files**: `mobile/src/views/HomeView.tsx`, widget snapshot push, `mobile/src/api/` stores,
`crates/takusu-agent/src/transport.rs` if a change-notification hook is missing.

Agent-approved changes and capability-authorized quick-action writes must reflect in Home, the
Android widget, and any open Agent view without manual refresh: after `ApprovalResult` or a progress
write, invalidate and refetch task, schedule, coverage, and capability state, then push a fresh
widget snapshot. The widget snapshot includes coverage state, task authority, settlement-first
presentation, and action capabilities. Add a lightweight “planner state changed” signal on the
transport if polling proves insufficient.

**Verify**: integration tests for approve → Home refresh, quick action → widget snapshot update,
expired capability → refreshed capability, bootstrap/stale rendering on Home and widget; manual
on-device check of the full loop 「始める → in_progress 表示 → 進捗 → 完了 → 次の行動」.

### WI-4: Start-time notifications with 行動 and ズラす (minimal sync)

**Files**: `takusu-local-lib` app layer (start-time evaluation), Android local notification
delivery from the in-process server, `mobile/` notification action handling.

The minimal form of sync, before the full event engine exists: a local notification at each task's
start time carrying both option groups (`「レポートの開始時刻です」 [着手] [10分後] [組み直す]`).
Each action carries a server-issued one-shot capability. 着手 and short `Snooze` invoke the common
capability authorization path, which then calls the ordinary progress or schedule API with a stable
operation ID; 組み直す deep-links into the compact panel. Short snoozes never ask a reason, while a
move to another time band opens the ordinary approval flow. Rendering goes through the WI-1
`CheckInCard` template. This scheduling path is deliberately simple (next start time only) and is
superseded by the WI-9 evaluator, keeping its capability and delivery wiring.

**Verify**: unit test that the notification presentation always contains both action groups;
integration tests for valid capability → 着手 → in_progress, replay/expiry rejection, and short
snooze → start time moved with no reason prompt; manual on-device check with the app closed.

## Phase 2: shared core and thin surfaces

Fix the shared layer first; both platform surfaces stay thin and land together.

### WI-5: Surface state machine and surface protocol

**Files**: `crates/takusu-agent/src/surface.rs`, `src/lib.rs`, `src/transport.rs`,
`mobile/src/api/agentClient.ts`.

Implement the surface state machine contract: derive states from `TurnEvent`s and audio callbacks,
expose an SSE state stream + snapshot, and accept surface commands (confirm-recording, open-panel,
stop-tts, open-approval, show-recovery). All surfaces of one device render this single state.
Codify the state-ownership scoping (user / session / device / ephemeral) in the transport types so
later WIs cannot accidentally share device-scoped state across devices.

**Verify**: state-transition unit tests (every state × command), transport tests for stream
subscribe/reconnect/snapshot, and a scripted turn asserting the state sequence
idle→thinking→speaking→idle.

### WI-6: Android resident button and shared surface integration

**Files**: `mobile/src/components/ResidentAgentButton.tsx` (evolves `FloatingVoiceButton.tsx`),
`mobile/src/components/AgentCompactPanel.tsx` (Phase 1 panel, Phase 2 state integration),
`mobile/app/_layout.tsx`.

Make the button app-wide (all main screens, not just Home), draggable with persisted position, and
avoidance of keyboard, modals, approval sheets, and OS gesture areas. It renders the shared surface
state (color/icon/animation per state) and implements the tap mapping from the contract; long-press
starts Listen (push-to-talk for now). Extend the Phase 1 compact panel to consume the surface SSE,
show recognized utterance, running action, result presentation, inline approval, capability expiry,
and recovery state; full `AgentView` remains for history and long consultations. Keep the existing
slide-up gesture for manual task creation.

**Verify**: gesture tests (tap vs. long-press vs. slide), position persistence, avoidance
behavior with keyboard/modal open, compact panel rendering for each presentation kind, approval
approve/deny from the panel; existing AgentView regression tests still pass.

### WI-7: Linux tray daemon

**Files**: new crate `crates/takusu-desktop` (`main.rs`, `tray.rs`, `notify.rs`, `popover.rs`),
`crates/takusu-local/Cargo.toml` and local Agent/event routes, a packaged systemd user unit, flake
packaging.

A daemon that owns or connects to the device's local `takusu-local` application host via
`takusu-client` and the Agent transport, subscribes to surface and replayable event state, and shows
a StatusNotifierItem tray icon (`ksni`) whose icon reflects the state machine. The local host uses
the common backend and invokes the shared evaluator when this device holds resident authority;
the tray daemon contains no planner logic. Desktop notifications (`zbus`
`org.freedesktop.Notifications`) carry action buttons wired to server-issued capabilities and the
same compact-panel commands as Android. The compact popover is a real window rendered with
**GPUI** (from `zed-industries/gpui`), showing current task, quick actions, and the open approval
panel; the StatusNotifierItem icon toggles this window. A StatusNotifierItem *menu* is only a
fallback if the window cannot be brought up.

Library choices are the initial selection: `ksni` for the StatusNotifierItem icon, `zbus` for
desktop notifications, and **GPUI** (`zed-industries/gpui`) for the compact popover window. If a
target environment lacks SNI support, revisit with a fallback before expanding scope.

**Verify**: `cargo nextest run -p takusu-desktop` for state→icon mapping and notification action
routing with a mock transport; manual smoke test on the user's desktop (tray states, notification
actions, systemd unit start/stop/restart).

### WI-8: Duration-distribution intervention math and persistent revisions

**Files**: `crates/takusu-core/src/estimator.rs`, `src/lib.rs` exports,
`crates/takusu-agent/src/tools/progress.rs`, storage contracts and methods, estimator-state
migrations in both backends, and every direct work-session progress path.

Implement the estimator model (v1) fixed in the intervention contract as pure functions in minute
units: the truncated-normal survival check `S_r(e)` against the band constants; the exact
truncated-normal progress posterior with noise `τ² = c·σ²(1−q)/q` and the prior-shift `z` check;
monotonically increasing distribution revisions with compensating observations for undo;
`next_crossing_time` via `S_r⁻¹`; fixed-task exclusion; task-kind priors; and the wide low-confidence
fallback prior. Add persistent current distribution, revision, observation lineage, and
compensating-observation records. Every storage backend and client path uses the same transaction;
no revision is maintained only inside the Agent tool. The API takes a consistent snapshot and
returns `(band, revision, next_crossing_time)` so WI-9 can schedule re-evaluation without
re-deriving the math.

**Verify**: property/unit tests for exact posterior normalization, both observation types (posterior
moves the right way, censor time monotonicity, progress report tightening the posterior), minute and
slot boundary conversions, band boundary tests around the constants, revision bump semantics (same
revision never re-fires, new revision re-fires), fixed versus non-fixed `sigma = 0`, fallback
selection, restart persistence, concurrent progress writes, compensating undo, and the canonical
script (10:20 progress report stays 通常, 11:10 censored observation reaches 注意).

### WI-9: Planner event engine, ledger, and notifications

**Files**: `crates/takusu-agent/src/events.rs` (pure evaluator + policy), event-ledger and
monotonic schedule-revision migrations in both backends, `takusu-local-lib` app-layer evaluator
and capability endpoints, `takusu-local` routes, `takusu-worker` parity, `mobile/` notification
handling + exact-alarm receiver, and `takusu-desktop/src/notify.rs`.

Implement the planner event contract: the pure evaluator over one consistent snapshot containing
planner, progress, coverage, schedule and distribution revisions, current time, and ledger view;
return due events and `next_eval_at`. Only the resident authority may commit the evaluator result.
The Android exact-alarm receiver invokes a bounded Rust entry point directly, commits with snapshot
revision and evaluator-lease checks, posts or queues delivery, and reserves the next alarm. It does
not start a full local HTTP server for one evaluation. If exact-alarm permission is unavailable, the
app reports degraded timing, uses the least-cost permitted fallback, and replays missed evaluation
at the next wake; it never treats a missed alarm as a delivered event. A progress or schedule write
publishes a state-changed signal; only the resident authority invokes the evaluator and commits the
result after the write transaction.

Persist deterministic event IDs, immutable presentation payloads, schedule/distribution revisions,
observation kind, delivery state, device claims, capability templates, and stable mutation operation
IDs. Mint short-lived action capabilities only when a delivery becomes eligible. Model
`pending_delivery`, `delivered`, `deferred_quiet_hours`, `acknowledged`, `ignored`, and
`resolved` separately. Quiet hours defer delivery without consuming the event. Use the gap taxonomy
(自由時間 / buffer / routine / 未分類 gap / 生成失敗), with check-ins only for unclassified gaps;
buffer is empty in v1 and 生成失敗 comes from the planner's unplaced-task tracking. Notifications
and actions reach each device through the per-device local host, SSE replay, or local notification
path; actions always pass the common capability authorization endpoint. This WI retires the WI-4
minimal scheduler while keeping its capability and delivery wiring.

**Verify**: unit tests per event kind; snapshot race and evaluator-lease tests; ledger idempotency
and delivery-state tests (repeated evaluation fires once, quiet-hours delivery is deferred,
simulated partition merge does not re-execute mutation, new schedule/distribution revision fires
anew); gap-taxonomy tests; `next_eval_at` recomputation on writes; capability expiry/replay tests;
integration test event → notification → capability authorization → mutation; alarm-receiver
round-trip on a device with the microphone service stopped; exact-alarm permission denied →
degraded scheduling, missed-event replay, and no false delivery claim.

### WI-10: Coverage trust states and settlement-first presentation

**Files**: `crates/takusu-agent/src/coverage.rs`, migrations for `coverage_confirmations` and
`unsettled_intervals` in both backends, `takusu-contracts` models and storage methods,
`presentation.rs` authority wiring, Home/widget/tray/compact-panel rendering.

Implement the coverage contract with explicit state precedence: bootstrap, stale,
today-covered, then trusted. Record the confirmed local period, timezone, source, schedule revision,
calendar health, unresolved gap intervals, and settlement operation IDs. Wire authority into
`TaskCard` (candidate in bootstrap, 「今やること」 from today-covered) and make stale render a
settlement prompt ahead of the current task on Home, widget, tray, and compact panel. Bootstrap
renders an intake prompt (the intake flow itself is WI-16; until then the prompt deep-links to
manual task creation).

**Verify**: unit tests for each state's conditions and precedence (no confirmation → bootstrap;
confirmation today → today-covered; unresolved interval, expired confirmation, or stale calendar
sync → stale; target-period procedure → trusted); settlement interval idempotency; presentation
tests that bootstrap demotes authority and stale leads with settlement; storage parity tests.

### WI-11: Multi-device arbitration

**Files**: new migration in both backends (`devices` table), `takusu-contracts` trait + models,
`takusu-local-lib` app/routes, `takusu-worker` parity, `takusu-client`, daemon and Android
application-host evaluator heartbeat/lease handling, settings UI for the priority list.

Implement the device arbitration contract with separate evaluator authority and audio status.
Device registration plus desktop heartbeat or Android evaluator lease determines the single
`resident_authority`; audio status and private output route determine local `SpeechCapability` only.
Desktop sleep drops its evaluator heartbeat; Android exact-alarm evaluation renews or reacquires its
lease at the next scheduled evaluation and promotes when needed.
Stopping the microphone service does not demote the resident authority: event evaluation and
notification delivery continue, while proactive speech degrades to notification. Only the resident
authority may commit events. Confirmed events are delivered to all devices via ledger replay;
device delivery claims prevent duplicate notifications on one device.

**Verify**: storage tests for registration, evaluator heartbeat/lease, audio status, priority
resolution, lease expiry, and local/Worker parity; simulated two-device tests (desktop alive → desktop is resident;
heartbeat expiry → Android's next exact alarm reacquires and promotes; recovery → silent demotion);
microphone service stopped while
Android remains evaluator resident; offline evaluation cannot claim without a fresh lease; ledger
prevents duplicate mutations when both devices evaluate during a partition.

## Phase 3: voice loop and capture

### WI-12: VAD endpointing and voice-session minimum

**Files**: `crates/takusu-audio/src/vad.rs`, `crates/takusu-agent/src/voice_session.rs`,
`src/audio.rs`, mobile recording path (`modules/`, `VoiceContext`), `takusu-desktop/src/audio.rs`.

Integrate silero VAD (via sherpa-onnx) for utterance endpointing so recording stops itself a few
hundred ms after speech ends instead of requiring a tap. Implement the continuous voice session:
after explicit start, loop listening → transcribing → acting → speaking → listening until the user
ends it or an idle timeout fires. TTS remains sentence-streamed (existing `TtsQueue`); tap-to-stop
TTS is wired through the surface command from WI-5. Responses are modality-aware: only turns that
began with voice input auto-speak; text turns and background events follow the private-channel and
urgency rules. No full-duplex conversation API.

**Verify**: VAD unit tests on recorded fixtures (endpoint latency, noise robustness with Hush),
voice-session state-machine tests (multi-turn continuation, user exit, timeout), modality tests
(text turn does not auto-speak), manual push-to-talk → continuous session smoke test.

### WI-13: Speaker verification

**Files**: `crates/takusu-audio/src/speaker.rs`, model download in `models.rs`, enrollment flow in
settings (mobile + a CLI/daemon command), on-device embedding storage.

Implement speaker embedding (sherpa-onnx WeSpeaker/3D-Speaker line) with an enrollment flow
(N utterances → stored embedding) and a verification call (utterance → similarity score).
Embeddings are stored on device only, never uploaded, and deletable from settings immediately.
Thresholds are configurable constants pending real-device tuning (open question in the design doc);
the API returns a score so the approval layer can treat it as a soft signal.

**Verify**: embedding determinism and similarity tests on fixture audio (same/different speaker),
enrollment/deletion round-trip, no embedding bytes in logs or server requests.

### WI-14: Voice approval layers and common capability authorization

**Files**: `crates/takusu-agent/src/approval_layers.rs`, `src/lib.rs` (approval resolution path),
`src/voice_session.rs`, `src/transport.rs`, `src/tool.rs` (`InputPath` and `Snooze`), permissions
defaults, capability storage and replay checks, progress undo path (estimator revert), compact
panel + tray fallback UI.

Implement the voice approval layers contract. Classify each proposed operation by its
`ProposedChange` set **and trusted input path**, applied in the authorization path **before** any
`Permissions` default grant is consulted. Screen and notification actions must consume a
server-issued one-shot capability bound to event, device, action, and expiry; direct progress APIs
cannot bypass this authorization path. Immediate-layer operations ride the validated capability
and are announced after execution; `Snooze` is a first-class bounded reversible operation;
voice-confirmed proposals are read back from their presentation template and resolved by an
on-device closed-vocabulary affirmative that passes speaker verification. Everything else, and
every fallback for ambiguity, silence, timeout, or verification failure, lands on the screen with
`waiting_for_approval` lit on all surfaces. A spoken acknowledgement outside the active
confirmation window never resolves an approval.

Implement start/pause undo that reverts the work session and cancels the estimator observation via
a compensating observation issuing a new distribution revision. The compensation and the original
mutation use stable operation IDs and remain in the revision lineage; this is required before the
ambient-immediate layer exists in Phase 4.

**Verify**: classification unit tests against the canonical scenario (start from session →
immediate; lunch-shift compound → voice; delete → screen; wake-word start classified
ambient-immediate for Phase 4); valid capability → immediate action; expired, replayed, mismatched,
and client-forged capability rejection; a start/pause/snooze issued from a plain text turn or an
unverified ambient path never rides the default grant; confirmation-window tests (affirmative,
negative, ambiguous, silence, timeout, failed verification → screen fallback, one-shot resolution);
undo test asserting the estimator observation is compensated under a new revision and previously
fired ledger events stay intact; assert no planner write occurs before the affirmative resolves the
request.

### WI-15: One-utterance capture

**Files**: `crates/takusu-agent/src/lib.rs` / prompts, capture-oriented use of `similar_tasks` and
memory injection, `presentation.rs` (capture readback), mobile/desktop entry points.

Make 「たくす、演習30題追加。金曜まで」 register a complete task in one round trip: the agent
fills estimate, quantity, and start window from memory and similar tasks with stated rationale,
producing one approval (readback on voice per WI-14, approval sheet on screen). At most one
focused clarification is allowed when data is genuinely missing; multi-question interrogation is a
test failure, not a tuning knob. Knock-on schedule effects within readable range ride the same
proposal (canonical 12:40 歯医者 scenario).

**Verify**: scripted capture tests (utterance → single proposal with estimate + quantity filled;
missing-deadline case → exactly one clarification); readback template contains the knock-on
change; voice-confirmed resolution end-to-end on device.

### WI-16: Intake interview

**Files**: agent prompt/skill for intake, session resumability on top of existing `AgentSession`
persistence, `coverage.rs` integration, mobile onboarding entry point.

Implement intake as an agent-led interview: the agent asks in a fixed order (deadlines first, then
recurring commitments, then calendar import confirmation), the user answers in free speech/text,
and the agent structures, estimates, and batches everything into one approval set. Sessions are
interruptible at any point and resumable later; completion is never required. The batch must use an
atomic server-side intake operation or an explicit staged state: an existing multi-operation
approval may partially fail, so coverage must not advance until every intended item and the
confirmation record commits. Finishing today's fixed appointments and imminent deadlines moves
coverage to today-covered (WI-10). Task creation follows the WI-15 capture path per item.

**Verify**: scripted interview tests (order of prompts, batch approval contents, estimates filled
per item); interruption + resume keeps state; atomic batch and partial-failure recovery tests;
coverage transition test (confirming today → today-covered); the intake scenario appendix replays
without contradiction.

### WI-17: Check-in policy and event-driven speech

**Files**: `crates/takusu-agent/src/events.rs` (contact + speech policy), `src/voice_session.rs`,
daemon and Android service delivery paths, comments tool wiring for postpone reasons.

Connect WI-9 events to contact behavior: the resident authority speaks proactively only when its
local `SpeechCapability` and a private channel pass (Android: earphone route detected, or
continuation of a recent voice conversation; desktop: always, minus quiet hours). A resident device
without microphone service or private output still evaluates and commits events, then degrades to
its WI-9 notification. Non-resident devices never re-evaluate the event; they replay the confirmed
ledger event and apply their device-specific delivery claim.

Implement the contact policy contract: a check-in is one question and one answer routed immediately
to a structured outcome; no-response degrades without re-asking; per-day check-in cap excludes
start-time/deadline notifications; frequency decays in unresponsive time bands; 「ほっといて」
provides timed suppression; and the postpone reason is a one-time post-action hook (「なにか詰まっ
てる?」) rather than a second classification interview. Short snoozes never ask a reason; answers
are stored as task comments. Earphone detection uses the audio output route only; no sensors.

**Verify**: policy unit tests (SpeechCapability × private channel × resident authority × quiet
hours matrix), degradation and cap tests (no repeat contact for one deviation, cap excludes
start-time notifications, decay after ignored band), suppression window test, one-question
post-action reason flow with and without an answer (reason lands in comments), and a scripted
end-to-end of the canonical 11:10 scenario (注意 band → inquiry → compound change → voice confirm).

### WI-18: Settlement (精算)

**Files**: agent prompt/flow + a settlement proposal builder in `takusu-agent`, structured
`unsettled_intervals` and coverage confirmation storage from WI-10, `presentation.rs` (settlement
readback), comments hook (reconciliation notes from `task-comments-memory-redesign.md`).

One confession (「ごめん今までゲームしてた」) produces one proposal that records a structured
elapsed-time interval, attaches the user-provided classification, and replans the remainder of the
day; planner changes require WI-14 approval and the interval uses a stable settlement operation ID.
The agent also initiates settlement at the next check-in or end of day for unsettled time
(「さっきの2時間はどうしておく?」), and as a single batched settlement after multi-day absence
(carried-over event from WI-9). The interval and planner revision are committed atomically; the
task comment explains the decision but never replaces the interval. Elapsed use is estimator/planner
input and is never graded. Settlement resolves the stale coverage state only after the structured
interval and coverage confirmation commit.

**Verify**: scripted tests for the bad-day scenario (confession → one proposal covering interval,
comment, and replan; end-of-day prompt for unsettled time; multi-day return → one batched settlement);
operation replay and partial-failure tests; coverage returns to today-covered only after settlement;
recorded time appears as comments/observations with no evaluative language in templates.

### WI-19: Conversation polish

**Files**: `crates/takusu-agent/src/voice_session.rs`, `takusu-audio` (AEC evaluation), platform
audio paths.

After the loop is in daily use: barge-in (keep the microphone open during TTS, AEC to remove self
speech, VAD on the residual; fall back to tap-to-stop where AEC is ineffective), latency budget
measurements (endpoint → first TTS audio), and interruption/timeout/error recovery so a failed
turn never leaves the session in a stuck state. This WI is intentionally last in the phase and may
be trimmed based on real usage.

**Verify**: AEC effectiveness measurements per test device, barge-in behavior tests, recorded
latency numbers in the PR description, chaos tests for LLM/TTS/network failure mid-session
(recording state never ambiguous).

## Phase 4: ambient listening

Desktop is the proving ground; Android receives only what desktop has validated.

### WI-20: Ambient gate pipeline (shared)

**Files**: `crates/takusu-audio/src/kws.rs`, `src/stream_asr.rs`, gate orchestration in
`takusu-agent` or `takusu-audio`.

Implement the gate chain: continuous VAD (WI-12) → sherpa-onnx KWS binary wake decision → speaker
gate where the approval layer requires it → streaming ASR for the full utterance → LLM turn. Wire
the ambient-immediate approval layer: wake-word start/pause executes only after speaker
verification and reads back the result; failure degrades to the screen. A short rolling buffer is
memory-only; non-target speech is discarded. Nothing before the KWS gate is persisted, logged, or
transmitted. Cloud cost begins strictly at the LLM turn.

**Verify**: gate unit tests with fixture audio (wake phrase fires, near-misses do not, pre-gate
buffer is dropped), ambient start requires verification and reads back, log audit test (no raw
audio/transcript in logs), CPU usage measurement of the always-on VAD+KWS pair.

### WI-21: Desktop opt-in ambient and wake word evaluation

**Files**: `crates/takusu-desktop/src/audio.rs`, tray state additions, config.

The daemon owns continuous capture (PipeWire via cpal) behind an explicit opt-in; the tray shows an
unremovable mic-active indication and offers immediate stop from both tray and notification.
Evaluate wake word feasibility on real hardware: Japanese phrase on pretrained sherpa-onnx KWS
models, versus custom training (openWakeWord line), versus desktop-only streaming-ASR + text match.
Record false-fire and miss rates; the result decides the Android wake word approach and may update
the design doc's open question.

**Verify**: opt-in default-off, indicator always present while capturing, immediate-stop paths,
multi-day false-fire log on the user's machine, documented wake word evaluation results.

### WI-22: Unclassified-gap check-in (「今なにしてる?」)

**Files**: `crates/takusu-agent/src/events.rs` (gap check-in firing), capture flow reuse from
WI-15, contact policy from WI-17.

Enable the proactive unknown-activity check-in: when an unclassified gap persists (threshold
constant, open question), the resident authority fires a 「今なにしてる?」 check-in — voice when
ambient is active and the channel is private, notification otherwise. The answer routes through
capture in one question and one answer: the `CheckInCard` offers one-off, recurring, free-time, and
routine outcomes when the answer does not determine the classification. A selected planner change
then follows the capability or approval layer; no second classification question is asked. All
WI-17 caps, decay, and suppression apply; free time, buffer, and routine never trigger it.

**Verify**: firing tests over the gap taxonomy (only unclassified gaps, only past the threshold);
one-round-trip capture from the answer (recurring registration test from the intake scenario); cap
and suppression integration with WI-17; scripted bad-day 16:40 replay.

### WI-23: Android microphone foreground service and re-arm

**Files**: `mobile/modules/takusu-agent-service/` (Kotlin foreground service),
`crates/takusu-android` (PCM bridge and bounded evaluator entry point),
`takusu-local-lib` evaluator FFI, app config/permissions, exact-alarm receiver, boot receiver,
re-arm notification.

Port the validated pipeline: a microphone foreground service, started only from a visible activity,
notification, or widget under Android 14 while-in-use rules, owns AudioRecord and feeds PCM into the
Rust gates. The service owns continuous audio, voice sessions, and local audio status; a persistent notification
shows mic use and offers immediate stop. Ambient works with the screen off and locked. The
exact-alarm receiver invokes the bounded Rust evaluator directly, renews or reacquires the evaluator
lease through the next scheduled evaluation, and continues planner event evaluation and notification
delivery while the microphone service is stopped. It does not start a full local HTTP server for each
alarm or schedule a high-frequency alarm solely for heartbeat.

The boot receiver never starts recording: it restores event-evaluation alarms and posts a 「Listen を
再開」 notification whose action `PendingIntent` starts the microphone service in one tap. The state
evaluator detects "ambient enabled but microphone service absent" and maintains the re-arm
notification; recovery never depends on OS service resurrection. Define and test behavior during
calls, other apps recording, audio focus loss, and battery saver (suspend gates, show state, resume
cleanly). No default-assistant role, no `VoiceInteractionService`; Ok Google coexists.

**Verify**: service lifecycle tests (start/stop/kill/restart; boot → alarms restored + re-arm
notification, no recording; re-arm action starts capture), receiver evaluator round-trip with the
microphone service stopped, evaluator heartbeat versus audio heartbeat, persistent notification
and immediate stop, lock-screen operation, call/focus-loss suspension, battery and thermal
measurements over a full day, Robolectric unit tests via `nix run .#test-android-unit`.

### WI-24: Ambient hardening and privacy audit

**Files**: across the phase's surfaces; settings screens.

Close the privacy and safety boundary list from the design doc: enable-time explanation of mic
use/processing/upload conditions, voiceprint deletion, defined behavior for every degraded state
(LLM/TTS/network failure never leaves recording state ambiguous), and a final audit that no
planner mutation is confirmed directly from ambient input outside the approval layers.

**Verify**: checklist walk of every boundary bullet in `../resident-agent.md` §Privacy and safety
boundaries with a test or manual evidence per item; full canonical-scenario and bad-day-scenario
runs on real devices (desktop + Android) matching the appendix scripts.

## Implementation order

```text
Phase 1: WI-1 presentations → WI-2 compact task actions → WI-3 state sync → WI-4 start-time notifications
Phase 2: WI-5 state machine → WI-6 Android surface ∥ WI-7 tray daemon
         → WI-8 estimator math → WI-11 arbitration → WI-9 event engine + ledger → WI-10 coverage
Phase 3: WI-12 VAD + voice session → WI-13 speaker verification → WI-14 approval layers
         → WI-15 capture → WI-16 intake ∥ WI-17 check-in policy → WI-18 settlement → WI-19 polish
Phase 4: WI-20 gates → WI-21 desktop ambient → WI-22 gap check-in → WI-23 Android service → WI-24 audit
```

WI-6 and WI-7 may proceed in either order after WI-5 but should land in the same phase so neither
platform's surface runs ahead of the other. WI-11 must provide the evaluator lease before WI-9 can
enable event commits; WI-9 may develop its pure evaluator earlier but must remain disabled until
arbitration is available. WI-16 and WI-17 are independent after WI-15. `task-comments-memory-redesign.md`
is implemented; WI-17 and WI-18 use comments directly. Do not start a phase before the previous
phase's gate criteria (below) hold in daily use. **After each phase's gate is met, merge
`resident-agent` → `main` and ship a release**, then re-cut `resident-agent` from the new `main` tip
before starting the next phase so the next phase is released incrementally rather than delayed by
the full plan.

Use one focused jj change per WI when practical. Before each push, rebase onto `main` (or the
current `resident-agent` tip), then run
`cargo fmt`, `cargo clippy --workspace`, `cargo nextest run --workspace`, and for mobile WIs
`npm run lint`, `npx tsc --noEmit`, `npm run fmt:check`. If a contract changes, update this
document, `../resident-agent.md` when the design itself shifts, and all affected tests in the same
change.

## Phase gate criteria (from Success criteria)

- **Phase 1 done**: start/progress/complete/delay work without opening the full Agent view; compact
  actions use server-issued one-shot capabilities; agent results reflect immediately on Home,
  schedule, widget, and coverage state; start-time notifications offer both 「行動」 and 「ズラす」,
  each completing in one tap.
- **Phase 2 done**: one shared session state is visible on the resident button and tray; planner
  events arrive as actionable notifications on both platforms with snapshot-checked,
  ledger-backed once-only firing; bootstrap demotes current-task authority and stale leads with
  settlement; exactly one device holds resident authority, converging after partitions; a resident
  device without microphone service continues evaluation and notification fallback; no event-derived
  mutation is ever applied twice.
- **Phase 3 done**: multiple turns continue within one explicit voice session; a new task
  registers from one utterance with estimate filled; intake is interruptible and coverage grows
  through sync; 通常-band deviations stay silent while 注意/再計画 produce inquiry/replan exactly
  once per distribution revision; each check-in is one question and one user answer, with any
  approval as a separate authorization step and no classification interrogation; a bad day ends
  back at today-covered via structured settlement; no speaker-side proactive speech on Android
  without earphones; voice-confirmed changes require the registered speaker.
- **Phase 4 done**: ambient start/run/stop/re-arm-waiting always visible; recording resumes in one
  notification tap after reboot; non-target audio neither uploaded nor persisted; no unknown-
  activity check-in during free time/buffer/routine; all planner mutations still pass the approval
  layers.

## Out of scope

- Cross-device approval portability (pending approvals stay session-owned; open question).
- Wake-word-less task-utterance classification (future upgrade during in_progress sessions only).
- Local TTS migration (stay on Cartesia; evaluate VOICEVOX / sherpa-onnx VITS / Kokoro when the
  quality/latency bar is met on CPU).
- Location profiles (“speak on home Wi-Fi”), sensor-based scene inference, pocket detection.
- Desktop full Agent view / full planner UI (web and CLI retopo are separate tracks).
- Distributed consensus for arbitration; multi-user tenancy.
- Any full-duplex conversation API.
- Census-style bulk-entry forms for initial registration.
- Grading, scoring, or visualizing past time use to pressure behavior change.
