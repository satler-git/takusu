# takusu Resident Planner Agent Implementation Plan

## Summary

Implement the resident planner agent defined in `../resident-agent.md`: an agent that is available
throughout the day on Android and Linux desktop, presents planner state instead of free-form chat,
reacts to planner events with σ-driven interventions, supports a hands-free voice loop, and finally
ambient listening behind on-device gates.

The implementation proceeds sequentially with one agent, split into four phases matching the
rollout section of the design document:

```text
Phase 1  planner execution loop UI (presentation types, current task card, state sync)
Phase 2  shared core + thin surfaces (state machine, resident button, tray daemon, events, arbitration)
Phase 3  voice loop (VAD endpointing, continuous session, three-layer voice approval, event speech)
Phase 4  ambient listening (VAD → KWS → ASR gates on desktop first, then Android foreground service)
```

Each work item (WI) should leave the repository working and tested before the next begins.
Contracts in this document may be refined when implementation reveals a problem, but the document
and all affected clients must be updated in the same change.

This plan builds on the implemented `plan-agent.md` stack: `AgentSession` / `TurnResult` /
`ApprovalRequest` in `takusu-agent`, the HTTP+SSE transport (`/api/agent/v1/*`), planner /
progress / memory / skills tools, the session-scoped `Permissions` map, sentence-streaming TTS
(`TtsQueue`), the Mobile `AgentView` + `ApprovalPanel`, the Android widget, and the server progress
endpoints (`/api/tasks/:id/work/*`, `/progress`, `/split`).

## Product invariants from `../resident-agent.md`

The implementation must preserve these behaviors across all work items:

1. **Typed presentation, not generated UI**: the LLM never generates arbitrary UI JSON. Clients
   render from typed presentation data built out of tool results, schedule state, and approval
   requests. Voice output is generated from the same presentation types with fixed templates.
2. **Approval invariants carry over**: all `plan-agent.md` invariants remain. Task, habit,
   schedule, and persistent-skill changes are never written before approval; ambiguous
   acknowledgements are never treated as approval.
3. **Three-layer voice confirmation**: immediate (start/pause — reversible, executed without
   confirmation as a default-granted permission class), voice-confirmed (progress, complete,
   single creates/edits with readable knock-on effects — readback + explicit affirmation +
   speaker verification), screen-required (delete, whole-schedule replacement, unreadable impact).
4. **Speaker verification is a soft gate**: a failed verification degrades to the screen fallback;
   it never hard-rejects. Reactive responses and the immediate layer do not require verification.
5. **σ-driven intervention**: deviation is measured in z-scores of `NormalDist(avg, sigma)`, not
   fixed minute thresholds. `<1σ` stays silent, `1σ–2σ` asks for progress, `>2σ` proposes
   rescheduling. All delay/overrun/event firing conditions defer to this scale.
6. **Proactive speech only on private channels**: reactive speech is always allowed. Proactive
   speech requires earphones on Android (or an ongoing voice conversation); otherwise it degrades
   to a notification. Desktop speaks by default, suppressed only by quiet hours. No sensor-based
   scene inference.
7. **One voice per event**: device arbitration is mechanical (priority list + heartbeat). Exactly
   one live device holds the voice role; all others degrade to notifications. No distributed
   consensus.
8. **Ambient is gated and opt-in**: ambient listening is off by default, always visibly indicated,
   stoppable from the notification and the surface. Only VAD and KWS run continuously; ASR and the
   LLM start only after the wake gate. Pre-gate raw audio is never persisted, logged, or uploaded.
   Voiceprint embeddings stay on device and are deletable.
9. **Session state lives in the shared Rust core**: surfaces (Android UI, tray) subscribe to one
   state machine; they never own recording, turn, TTS, or approval state. Platform shells own the
   microphone lifecycle; Expo/React Native component lifecycles never own continuous recording.

## Architecture

```text
crates/takusu-agent                    # existing; grows the shared resident core
├── src/presentation.rs        (new)   # typed presentation model + voice templates
├── src/surface.rs              (new)  # SurfaceState machine + surface protocol events
├── src/events.rs               (new)  # planner event detection (σ-driven), event → action policy
├── src/voice_session.rs        (new)  # continuous voice session loop (listen→act→speak)
├── src/approval_layers.rs      (new)  # three-layer classification on top of Permissions
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
├── src/popover.rs                     # compact popover (phase 2: tray menu; window later)
└── src/audio.rs                       # PipeWire/cpal capture ownership, ambient gates (phase 4)

takusu-local-lib / takusu-local        # existing; grows event + device infrastructure
├── migrations/0xx_devices.sql         # device registry, priority, heartbeat
├── /api/devices/*                     # register, heartbeat, voice-role resolution
└── /api/agent/v1/... events           # server-side planner event stream (SSE)

mobile/                                # existing; grows resident surfaces
├── src/components/ResidentAgentButton.tsx   # app-wide draggable button (replaces Home-only FAB)
├── src/components/AgentCompactPanel.tsx     # one-round-trip sheet/overlay
├── src/components/CurrentTaskCard.tsx       # Home current/next task + quick actions
└── modules/takusu-agent-service/            # phase 4: microphone foreground service (Kotlin)
```

Placement rules:

- Planner event detection needs storage access and must behave identically for both platforms, so
  it lives in the shared Rust application layer and is exposed through the agent transport, not
  reimplemented per surface.
- Device registry and heartbeat must be visible across devices, so they live in storage (SQLite
  and D1/Worker parity, like every other storage feature).
- The Linux daemon consumes `takusu-client` and the agent transport; it contains no planner logic.
- Ambient audio is owned by the platform shell (Android foreground service / Linux daemon) and
  processed by `takusu-audio`; JS/UI layers only subscribe to state.

## Shared contracts

### Presentation contract

`TurnResult` and the event stream gain a typed presentation payload. The initial closed set:

```rust
pub enum Presentation {
    CurrentTask(TaskCard),            // current + next task, quick actions
    WorkTransition(WorkTransition),   // start / pause / progress / complete / delay result
    ScheduleSummary(ScheduleSummary),
    ProgressSummary(ProgressSummary), // done count, in progress, active time, vs. estimate
    ScheduleAlert(ScheduleAlert),     // conflict / overdue (σ-annotated)
    ChangeProposal,                   // rendered from the existing ApprovalRequest
    Clarification(FocusedQuestion),
    Text(String),                     // fallback
}
```

Rules:

- Presentations are constructed by Rust code from tool results and schedule state; the LLM chooses
  which tool to call, never the rendering.
- Every presentation type has a deterministic voice template (fixed sentence structure, values
  interpolated) used for TTS and notification bodies. Template output is what the readback layer
  reads before a voice confirmation.
- Clients may render richer views but must not require fields outside the contract. Unknown
  presentation kinds must degrade to `Text` rendering so old clients survive additions.

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

### Planner event contract

Event kinds (from the design doc): task start time reached, estimated-end overrun (in σ), predicted
deadline violation, schedule gap, carried-over incomplete tasks, schedule not generated, sleep
impact.

```rust
pub struct PlannerEvent {
    pub id: String,
    pub kind: PlannerEventKind,
    pub task_ref: Option<TaskRef>,
    pub z_score: Option<f64>,        // σ-measured deviation where applicable
    pub presentation: Presentation,  // deterministic notification/action content
    pub urgency: Urgency,            // drives notify vs. speak vs. suppress
}
```

- Detection runs server-side on a timer plus on progress/schedule writes, dedupes per
  task+kind+threshold crossing, and respects quiet hours (derived from sleep settings).
- σ thresholds: `<1σ` no event; `1σ` progress inquiry; `2σ` reschedule proposal. Thresholds are
  constants in one module, not scattered.
- An event does not necessarily start an LLM turn. Deterministic notifications and quick actions
  (`[着手] [10分後] [組み直す]`) are generated by application code; the LLM is invoked only for
  ambiguous adjustment, explanation, or proposals.
- Deep links: notification actions either call the immediate-layer progress API directly
  (start/snooze) or open the compact panel / approval UI.

### Device arbitration contract

- `devices` table (both backends): id, name, platform, priority, last_heartbeat_at. Priority list
  is a user setting (default: desktop > Android).
- Each agent service heartbeats (e.g. every 30 s). The highest-priority device with a fresh
  heartbeat (< 2 intervals) holds the voice role; it is computed by readers, not elected.
- Non-voice devices show notifications only. Notifications go to all devices.
- Offline devices keep their last known role; simultaneous speech from two devices during a
  partition is accepted. No consensus protocol.
- Pending approvals stay session-owned (current design). Cross-device approval portability remains
  an open question and is out of scope for this plan.

### Voice approval layers contract

Built on the existing `Permissions` map rather than a parallel system:

- **Immediate layer** = a named permission class (`work:start`, `work:pause`) granted by default in
  every session. Executed without confirmation; the result is read back afterwards.
- **Voice-confirmed layer**: the proposal's presentation template is read aloud; an explicit
  affirmative within the confirmation window resolves the existing `ApprovalRequest`. The
  affirmative classifier is a closed-vocabulary on-device check, and the utterance must pass
  speaker verification. Ambiguity, silence, timeout, or failed verification falls back to the
  screen with `waiting_for_approval` lit.
- **Screen-required layer**: deletes, whole-schedule replacement, and any change whose impact
  cannot be fully read back always route to the screen approval UI (existing `ApprovalPanel`).
- Layer classification is a pure function of `ProposedChange` sets in `approval_layers.rs`, unit
  tested against the canonical scenario in the design doc.

## Phase 1: planner execution loop UI

Progress storage, APIs, and agent tools already exist; this phase closes the experience loop.

### WI-1: Presentation types and transport exposure

**Files**: `crates/takusu-agent/src/presentation.rs`, `src/lib.rs`, `src/transport.rs`,
`mobile/src/api/agentTypes.ts`, shared JSON fixtures.

Define the `Presentation` enum and its construction from existing tool outputs: progress tools
produce `WorkTransition`, schedule reads produce `ScheduleSummary`, progress reads produce
`ProgressSummary`, approval requests produce `ChangeProposal`, `user_input` questions produce
`Clarification`. Attach presentations to `TurnResult` and stream them over the existing SSE
transport with a version-tolerant encoding (unknown kinds → `Text`). Implement the deterministic
voice/notification templates alongside the types so later phases reuse them.

**Verify**: unit tests mapping each tool output to its presentation; fixture round-trip tests
shared with the Mobile client; template snapshot tests (e.g. progress summary always renders the
same sentence structure for the same data).

### WI-2: Current task card and quick actions on Home

**Files**: `mobile/src/components/CurrentTaskCard.tsx`, `mobile/src/views/HomeView.tsx`, focused
additions to `mobile/src/api/`.

Home shows a current-task card (task, time window, work state) with quick actions
`[着手] [完了] [延期] [相談]`. Start/pause call the progress API directly (immediate layer, no
agent turn). Complete and delay open the agent compact flow (phase 1: existing AgentView with the
intent prefilled; phase 2 swaps in the compact panel). The card renders from the same
`CurrentTask` presentation data used by the agent.

**Verify**: component tests for card states (no schedule, no current task, in_progress, overdue);
integration test that 着手 flips the task to in_progress without creating an approval; lint,
`npx tsc --noEmit`, `npm run fmt:check`.

### WI-3: State synchronization across Home, widget, and Agent UI

**Files**: `mobile/src/views/HomeView.tsx`, widget snapshot push, `mobile/src/api/` stores,
`crates/takusu-agent/src/transport.rs` if a change-notification hook is missing.

Agent-approved changes and quick-action writes must reflect in Home, the Android widget, and any
open Agent view without manual refresh: after `ApprovalResult` or a progress write, invalidate and
refetch task/schedule state and push a fresh widget snapshot. Add a lightweight “planner state
changed” signal on the transport if polling proves insufficient.

**Verify**: integration tests for approve → Home refresh, quick action → widget snapshot update;
manual on-device check of the full loop 「始める → in_progress 表示 → 進捗 → 完了 → 次の行動」.

## Phase 2: shared core and thin surfaces

Fix the shared layer first; both platform surfaces stay thin and land together.

### WI-4: Surface state machine and surface protocol

**Files**: `crates/takusu-agent/src/surface.rs`, `src/lib.rs`, `src/transport.rs`,
`mobile/src/api/agentClient.ts`.

Implement the surface state machine contract: derive states from `TurnEvent`s and audio callbacks,
expose an SSE state stream + snapshot, and accept surface commands (confirm-recording, open-panel,
stop-tts, open-approval, show-recovery). All surfaces of one device render this single state.

**Verify**: state-transition unit tests (every state × command), transport tests for stream
subscribe/reconnect/snapshot, and a scripted turn asserting the state sequence
idle→thinking→speaking→idle.

### WI-5: Android resident button and compact panel

**Files**: `mobile/src/components/ResidentAgentButton.tsx` (evolves `FloatingVoiceButton.tsx`),
`mobile/src/components/AgentCompactPanel.tsx`, `mobile/app/_layout.tsx`.

Make the button app-wide (all main screens, not just Home), draggable with persisted position, and
avoidance of keyboard, modals, approval sheets, and OS gesture areas. It renders the surface state
(color/icon/animation per state) and implements the tap mapping from the contract; long-press
starts Listen (push-to-talk for now). The compact panel is a sheet/overlay showing recognized
utterance, running action, result presentation, and inline approval; full `AgentView` remains for
history and long consultations. Keep the existing slide-up gesture for manual task creation.

**Verify**: gesture tests (tap vs. long-press vs. slide), position persistence, avoidance
behavior with keyboard/modal open, compact panel rendering for each presentation kind, approval
approve/deny from the panel; existing AgentView regression tests still pass.

### WI-6: Linux tray daemon

**Files**: new crate `crates/takusu-desktop` (`main.rs`, `tray.rs`, `notify.rs`, `popover.rs`),
a packaged systemd user unit, flake packaging.

A daemon that connects to a configured takusu server via `takusu-client` and the agent transport,
subscribes to surface state, and shows a StatusNotifierItem tray icon (`ksni`) whose icon reflects
the state machine. Desktop notifications (`zbus` org.freedesktop.Notifications) carry action
buttons wired to the same commands/deep-link actions as Android. The compact popover starts as a
tray menu (current task, quick actions, open approval); a real popover window is deferred until the
menu proves insufficient. No planner logic in the daemon.

Library choices (`ksni`, `zbus`) are the initial selection; if a target environment lacks SNI
support, revisit with a fallback before expanding scope.

**Verify**: `cargo nextest run -p takusu-desktop` for state→icon mapping and notification action
routing with a mock transport; manual smoke test on the user's desktop (tray states, notification
actions, systemd unit start/stop/restart).

### WI-7: Planner event detection and notifications

**Files**: `crates/takusu-agent/src/events.rs` (detection policy),
`takusu-local-lib` app-layer scheduler + `takusu-local` routes for the event stream,
`mobile/` notification handling, `takusu-desktop/src/notify.rs`.

Implement the planner event contract: σ-driven detection over schedule + work sessions + progress
events, deduped threshold crossings, quiet hours, and deterministic notification presentations
with quick actions. Events reach clients through an SSE stream (desktop daemon) and notifications
(Android local notifications from the in-process server; desktop via WI-6). Notification actions
execute immediate-layer operations directly or deep-link into the compact panel/approval UI.
z-score computation uses the task's `NormalDist(avg, sigma)` against active work time, not
wall-clock time.

**Verify**: unit tests for each event kind and threshold (0.9σ silent, 1σ inquiry, 2σ reschedule),
dedupe across repeated evaluation, quiet-hours suppression, carried-over and schedule-not-generated
detection; integration test event → notification with actions → immediate-layer execution.

### WI-8: Multi-device arbitration

**Files**: new migration in both backends (`devices` table), `takusu-storage` trait + models,
`takusu-local-lib` app/routes, `takusu-worker` parity, `takusu-client`, daemon and Android service
heartbeat loops, settings UI for the priority list.

Implement the device arbitration contract: device registration, heartbeat writes, priority-list
setting, and a `voice_role` resolution read used by every device before speaking proactively.
Desktop sleep drops its heartbeat; Android promotes automatically and silently. Notifications are
never gated by the voice role.

**Verify**: storage tests for registration/heartbeat/priority resolution and local/Worker parity;
simulated two-device tests (desktop alive → desktop speaks; heartbeat expiry → Android promotes;
recovery → silent demotion); offline behavior keeps the last known role.

## Phase 3: voice loop

### WI-9: VAD endpointing and voice-session minimum

**Files**: `crates/takusu-audio/src/vad.rs`, `crates/takusu-agent/src/voice_session.rs`,
`src/audio.rs`, mobile recording path (`modules/`, `VoiceContext`), `takusu-desktop/src/audio.rs`.

Integrate silero VAD (via sherpa-onnx) for utterance endpointing so recording stops itself a few
hundred ms after speech ends instead of requiring a tap. Implement the continuous voice session:
after explicit start, loop listening → transcribing → acting → speaking → listening until the user
ends it or an idle timeout fires. TTS remains sentence-streamed (existing `TtsQueue`); tap-to-stop
TTS is wired through the surface command from WI-4. Responses are modality-aware: only turns that
began with voice input auto-speak; text turns and background events follow the private-channel and
urgency rules.

**Verify**: VAD unit tests on recorded fixtures (endpoint latency, noise robustness with Hush),
voice-session state-machine tests (multi-turn continuation, user exit, timeout), modality tests
(text turn does not auto-speak), manual push-to-talk → continuous session smoke test.

### WI-10: Speaker verification

**Files**: `crates/takusu-audio/src/speaker.rs`, model download in `models.rs`, enrollment flow in
settings (mobile + a CLI/daemon command), on-device embedding storage.

Implement speaker embedding (sherpa-onnx WeSpeaker/3D-Speaker line) with an enrollment flow
(N utterances → stored embedding) and a verification call (utterance → similarity score).
Embeddings are stored on device only, never uploaded, and deletable from settings immediately.
Thresholds are configurable constants pending real-device tuning (open question in the design doc);
the API returns a score so the approval layer can treat it as a soft signal.

**Verify**: embedding determinism and similarity tests on fixture audio (same/different speaker),
enrollment/deletion round-trip, no embedding bytes in logs or server requests.

### WI-11: Three-layer voice approval

**Files**: `crates/takusu-agent/src/approval_layers.rs`, `src/lib.rs` (approval resolution path),
`src/voice_session.rs`, permissions defaults, compact panel + tray fallback UI.

Implement the voice approval layers contract. Classify each `ApprovalRequest` by its
`ProposedChange` set; immediate-layer operations ride the default-granted permission class and are
announced after execution; voice-confirmed proposals are read back from their presentation template
and resolved by an on-device closed-vocabulary affirmative that passes speaker verification;
everything else (and every fallback: ambiguity, silence, timeout, verification failure) lands on
the screen with `waiting_for_approval` lit on all surfaces. A spoken acknowledgement outside the
active confirmation window never resolves an approval.

**Verify**: classification unit tests against the canonical scenario (start → immediate; lunch
shift compound → voice; delete → screen); confirmation-window tests (affirmative, negative,
ambiguous, silence, timeout, failed verification → screen fallback, one-shot resolution); assert
no planner write occurs before the affirmative resolves the request.

### WI-12: Event-driven speech

**Files**: `crates/takusu-agent/src/events.rs` (speech policy), `src/voice_session.rs`, daemon and
Android service delivery paths.

Connect WI-7 events to speech: the voice-role device speaks proactively only on a private channel
(Android: earphone route detected, or continuation of a recent voice conversation; desktop: always,
minus quiet hours); otherwise the event degrades to its WI-7 notification. Implement the postpone
follow-up (“なにか詰まってる?” asked exactly once, silence not chased) and silent degradation on
no-response. Earphone detection uses the audio output route only; no sensors.

**Verify**: policy unit tests (earphones × voice role × quiet hours matrix), one-question postpone
flow with and without an answer, no-response → notification degradation, and a scripted end-to-end
of the canonical 11:10 scenario (1σ overrun → inquiry → compound change → voice confirm).

### WI-13: Conversation polish

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

### WI-14: Ambient gate pipeline (shared)

**Files**: `crates/takusu-audio/src/kws.rs`, `src/stream_asr.rs`, gate orchestration in
`takusu-agent` or `takusu-audio`.

Implement the gate chain: continuous VAD (WI-9) → sherpa-onnx KWS binary wake decision → speaker
gate where required → streaming ASR for the full utterance → LLM turn. A short rolling buffer is
memory-only; non-target speech is discarded. Nothing before the KWS gate is persisted, logged, or
transmitted. Cloud cost begins strictly at the LLM turn.

**Verify**: gate unit tests with fixture audio (wake phrase fires, near-misses do not, pre-gate
buffer is dropped), log audit test (no raw audio/transcript in logs), CPU usage measurement of the
always-on VAD+KWS pair.

### WI-15: Desktop opt-in ambient and wake word evaluation

**Files**: `crates/takusu-desktop/src/audio.rs`, tray state additions, config.

The daemon owns continuous capture (PipeWire via cpal) behind an explicit opt-in; the tray shows an
unremovable mic-active indication and offers immediate stop from both tray and notification.
Evaluate wake word feasibility on real hardware: Japanese phrase on pretrained sherpa-onnx KWS
models, versus custom training (openWakeWord line), versus desktop-only streaming-ASR + text match.
Record false-fire and miss rates; the result decides the Android wake word approach and may update
the design doc's open question.

**Verify**: opt-in default-off, indicator always present while capturing, immediate-stop paths,
multi-day false-fire log on the user's machine, documented wake word evaluation results.

### WI-16: Android microphone foreground service

**Files**: `mobile/modules/takusu-agent-service/` (Kotlin foreground service),
`crates/takusu-android` (PCM bridge into the gate pipeline), app config/permissions, boot receiver
behind a separate opt-in.

Port the validated pipeline: a microphone foreground service owns AudioRecord and feeds PCM into
the Rust gates; a persistent notification shows mic use and offers immediate stop; ambient works
with the screen off and locked. Boot auto-start is an independent opt-in. Define and test behavior
during calls, other apps recording, audio focus loss, and battery saver (suspend gates, show state,
resume cleanly). `VoiceInteractionService` is evaluated only if the plain service proves
insufficient.

**Verify**: service lifecycle tests (start/stop/kill/restart, boot opt-in on/off), persistent
notification and immediate stop, lock-screen operation, call/focus-loss suspension, battery and
thermal measurements over a full day, Robolectric unit tests via `nix run .#test-android-unit`.

### WI-17: Ambient hardening and privacy audit

**Files**: across the phase's surfaces; settings screens.

Close the privacy and safety boundary list from the design doc: enable-time explanation of mic
use/processing/upload conditions, voiceprint deletion, defined behavior for every degraded state
(LLM/TTS/network failure never leaves recording state ambiguous), and a final audit that no
planner mutation is confirmed directly from ambient input outside the three approval layers.

**Verify**: checklist walk of every boundary bullet in `../resident-agent.md` §Privacy and safety
boundaries with a test or manual evidence per item; full canonical-scenario run on real devices
(desktop + Android) matching the appendix script.

## Implementation order

```text
Phase 1: WI-1 presentations → WI-2 task card → WI-3 state sync
Phase 2: WI-4 state machine → WI-5 Android surface ∥ WI-6 tray daemon → WI-7 events → WI-8 arbitration
Phase 3: WI-9 VAD + voice session → WI-10 speaker verification → WI-11 voice approval
         → WI-12 event speech → WI-13 polish
Phase 4: WI-14 gates → WI-15 desktop ambient → WI-16 Android service → WI-17 hardening
```

WI-5 and WI-6 may proceed in either order after WI-4 but should land in the same phase so neither
platform's surface runs ahead of the other. Do not start a phase before the previous phase's
success criteria (below) hold in daily use.

Use one focused jj change per WI when practical. Before each push, rebase onto `main`, then run
`cargo fmt`, `cargo clippy --workspace`, `cargo nextest run --workspace`, and for mobile WIs
`npm run lint`, `npx tsc --noEmit`, `npm run fmt:check`. If a contract changes, update this
document, `../resident-agent.md` when the design itself shifts, and all affected tests in the same
change.

## Phase gate criteria (from Success criteria)

- **Phase 1 done**: start/progress/complete/delay work without opening the full Agent view; agent
  results reflect immediately on Home, schedule, and the widget.
- **Phase 2 done**: one shared session state is visible on the resident button and tray; planner
  events arrive as actionable notifications on both platforms; exactly one device would voice a
  given event.
- **Phase 3 done**: multiple turns continue within one explicit voice session; sub-1σ deviations
  stay silent while 1σ/2σ produce inquiry/replan; no speaker-side proactive speech on Android
  without earphones; voice-confirmed changes require the registered speaker.
- **Phase 4 done**: ambient start/run/stop always visible; non-target audio neither uploaded nor
  persisted; all planner mutations still pass the three approval layers.

## Out of scope

- Cross-device approval portability (pending approvals stay session-owned).
- Wake-word-less task-utterance classification (future upgrade during in_progress sessions only).
- Local TTS migration (stay on Cartesia; evaluate VOICEVOX / sherpa-onnx VITS / Kokoro when the
  quality/latency bar is met on CPU).
- Location profiles (“speak on home Wi-Fi”), sensor-based scene inference.
- Desktop full Agent view / full planner UI (web and CLI retopo are separate tracks).
- Distributed consensus for arbitration; multi-user tenancy.
- Any full-duplex conversation API.
