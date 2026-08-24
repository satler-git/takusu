export interface ProposedChange {
  proposal_id: string;
  operation: string;
  target_label: string;
  description: string;
  before?: unknown;
  after?: unknown;
}

export interface InferredField {
  field: string;
  value: unknown;
  reason: string;
}

export interface ProposalDecision {
  proposal_id: string;
  approve: boolean;
}

export interface ApprovalRequest {
  id: string;
  why: string;
  changes: ProposedChange[];
  inferred_fields: InferredField[];
  warnings: string[];
  expires_at: string;
}

export interface ChangeReceipt {
  operation: string;
  target_type: string;
  target_id: string;
  before?: unknown;
  after?: unknown;
}

export interface ApprovalResult {
  id: string;
  approved: boolean;
  changes: ChangeReceipt[];
  schedule_dirty: boolean;
  /** Typed presentation derived from the executed changes, when available. */
  presentation?: Presentation;
}

export type IntakeStage =
  | 'not_started'
  | 'deadlines'
  | 'recurring'
  | 'calendar_import'
  | 'complete';

export interface IntakeState {
  stage: IntakeStage;
  proposal_id: string | null;
  coverage_pending: boolean;
  collected_ids: string[];
}

export interface AgentTurnResult {
  text: string;
  changes: ChangeReceipt[];
  schedule_dirty: boolean;
  approval_request: ApprovalRequest | null;
  presentation?: Presentation | null;
  intake_state?: IntakeState | null;
}

// ── Shared surface state (WI-5) ──────────────────────────────────────────

export type StateScope = 'user' | 'session' | 'device' | 'ephemeral';
export type SurfaceState =
  | 'idle'
  | 'listening'
  | 'transcribing'
  | 'thinking'
  | 'waiting_for_user'
  | 'waiting_for_approval'
  | 'speaking'
  | 'error';

export interface SurfaceSnapshot {
  /** Surface state is owned by the local device, not by a user session. */
  scope: StateScope;
  state: SurfaceState;
  revision: number;
  operation_id?: number;
  error?: string | null;
}

export type SurfaceEvent =
  | ({ type: 'snapshot' } & SurfaceSnapshot)
  | ({ type: 'state_changed' } & SurfaceSnapshot);

export type SurfaceCommand =
  | 'confirm-recording'
  | 'open-panel'
  | 'stop-tts'
  | 'open-approval'
  | 'show-recovery';

export interface SurfaceCommandResponse {
  command: SurfaceCommand;
  accepted: boolean;
  reason?: string | null;
  snapshot: SurfaceSnapshot;
}

export type SurfaceAudioCallback =
  | 'listening'
  | 'transcribing'
  | 'speaking'
  | 'playback_finished';

export interface UserInputQuestion {
  text: string;
  for: string;
}

export interface UserInputAnswer {
  text: string;
}

// ── Presentation types (WI-1) ───────────────────────────────────────────
// Typed presentation payloads built by Rust code from tool results; the LLM
// never generates arbitrary UI JSON. Wire form is internally tagged by `type`.

export type TaskAuthority = 'candidate' | 'today_covered';
export type WorkState = 'not_started' | 'in_progress' | 'overdue';

export type CoverageState = 'bootstrap' | 'today_covered' | 'trusted' | 'stale';

export interface CoverageEvaluation {
  state: CoverageState;
  confirmations: CoverageConfirmation[];
  unsettled_intervals: UnsettledInterval[];
  schedule_revision: number;
}

export interface CoverageConfirmation {
  id: string;
  start_at: string;
  end_at: string;
  timezone: string;
  source: string;
  schedule_revision: number;
  calendar_health: string;
  created_at: string;
  settled_at?: string | null;
  operation_id?: string | null;
}

export interface UnsettledInterval {
  id: string;
  start_at: string;
  end_at: string;
  classification: string;
  source: string;
  created_at: string;
  settled_at?: string | null;
  operation_id?: string | null;
}

export interface SettlementPrompt {
  question: string;
  act: ActionGroup;
  shift: ActionGroup;
}

export interface TaskCard {
  title: string;
  reference: string;
  start_at?: string;
  end_at?: string;
  work_state: WorkState;
  authority: TaskAuthority;
  next_task?: string;
  settlement?: SettlementPrompt;
}

export type WorkTransitionKind =
  | 'start'
  | 'pause'
  | 'progress'
  | 'complete'
  | 'delay'
  | 'split';

export interface WorkTransition {
  kind: WorkTransitionKind;
  reference: string;
  title: string;
  detail?: string;
}

export interface ScheduleEntry {
  reference: string;
  title: string;
  start_at?: string;
  end_at?: string;
}

export interface ScheduleSummary {
  next?: ScheduleEntry;
  entries?: ScheduleEntry[];
}

export interface ProgressSummary {
  done: number;
  in_progress: number;
  scheduled: number;
}

export type ScheduleAlertKind = 'conflict' | 'overdue' | 'generation_failure';

export interface ScheduleAlert {
  kind: ScheduleAlertKind;
  message: string;
}

export type ActionKind = 'immediate' | 'approval' | 'panel';

export interface Action {
  id: string;
  label: string;
  kind: ActionKind;
  /** The server-issued one-shot capability, present for immediate notification actions. */
  capability?: ActionCapability;
}

export interface ActionGroup {
  title: string;
  actions: Action[];
}

export interface CheckInCard {
  question: string;
  act: ActionGroup;
  shift: ActionGroup;
}

export interface FocusedQuestion {
  message: string;
  choices?: string[];
}

// The discriminated union serialized by the agent transport. `Text` is a
// struct variant (`{ type: 'text', text: '...' }`) because internally-tagged
// serde cannot tag a primitive newtype.
export type Presentation =
  | ({ type: 'current_task' } & TaskCard)
  | ({ type: 'work_transition' } & WorkTransition)
  | ({ type: 'schedule_summary' } & ScheduleSummary)
  | ({ type: 'progress_summary' } & ProgressSummary)
  | ({ type: 'schedule_alert' } & ScheduleAlert)
  | ({ type: 'check_in' } & CheckInCard)
  | ({ type: 'change_proposal' } & ApprovalRequest)
  | ({ type: 'clarification' } & FocusedQuestion)
  | { type: 'text'; text: string };

/**
 * Version-tolerant decoding for a raw presentation payload (e.g. parsed from a
 * fixture or a streamed event). Unknown variants degrade to a `text`
 * presentation using the accompanying `text` fallback when present, matching
 * the Rust deserializer. A **known** tag whose payload is malformed also
 * degrades to `text` (mirroring `Presentation::deserialize`, which falls back
 * when the inner `from_value` fails), so a forward extension or a parse
 * surprise never returns an object with undefined fields.
 */
export function decodePresentation(raw: unknown): Presentation {
  if (!isObject(raw)) {
    return { type: 'text', text: '' };
  }
  const obj = raw as Record<string, unknown>;
  const type = typeof obj.type === 'string' ? obj.type : 'text';
  const fallback = typeof obj.text === 'string' ? obj.text : '';

  switch (type) {
    case 'current_task':
      return decodeCurrentTask(obj) ?? { type: 'text', text: fallback };
    case 'work_transition':
      return decodeWorkTransition(obj) ?? { type: 'text', text: fallback };
    case 'schedule_summary':
      return decodeScheduleSummary(obj) ?? { type: 'text', text: fallback };
    case 'progress_summary':
      return decodeProgressSummary(obj) ?? { type: 'text', text: fallback };
    case 'schedule_alert':
      return decodeScheduleAlert(obj) ?? { type: 'text', text: fallback };
    case 'check_in':
      return decodeCheckIn(obj) ?? { type: 'text', text: fallback };
    case 'change_proposal':
      return decodeChangeProposal(obj) ?? { type: 'text', text: fallback };
    case 'clarification':
      return decodeClarification(obj) ?? { type: 'text', text: fallback };
    case 'text':
      return { type: 'text', text: fallback };
    default:
      return { type: 'text', text: fallback };
  }
}

const SURFACE_STATES: readonly SurfaceState[] = [
  'idle',
  'listening',
  'transcribing',
  'thinking',
  'waiting_for_user',
  'waiting_for_approval',
  'speaking',
  'error',
];

const STATE_SCOPES: readonly StateScope[] = [
  'user',
  'session',
  'device',
  'ephemeral',
];

const SURFACE_COMMANDS: readonly SurfaceCommand[] = [
  'confirm-recording',
  'open-panel',
  'stop-tts',
  'open-approval',
  'show-recovery',
];

function isSurfaceState(value: unknown): value is SurfaceState {
  return (
    typeof value === 'string' && SURFACE_STATES.includes(value as SurfaceState)
  );
}

function isStateScope(value: unknown): value is StateScope {
  return (
    typeof value === 'string' && STATE_SCOPES.includes(value as StateScope)
  );
}

function isSurfaceCommand(value: unknown): value is SurfaceCommand {
  return (
    typeof value === 'string' &&
    SURFACE_COMMANDS.includes(value as SurfaceCommand)
  );
}

function decodeSurfaceSnapshotValue(raw: unknown): SurfaceSnapshot | undefined {
  if (!isObject(raw)) {
    return undefined;
  }
  const scope = raw.scope;
  const state = raw.state;
  const revision = raw.revision;
  if (
    !isStateScope(scope) ||
    !isSurfaceState(state) ||
    typeof revision !== 'number' ||
    !Number.isSafeInteger(revision) ||
    revision < 0
  ) {
    return undefined;
  }
  const operationId = raw.operation_id;
  if (
    operationId !== undefined &&
    operationId !== null &&
    (typeof operationId !== 'number' ||
      !Number.isSafeInteger(operationId) ||
      operationId < 1)
  ) {
    return undefined;
  }
  const error = raw.error;
  if (error !== undefined && error !== null && typeof error !== 'string') {
    return undefined;
  }
  return {
    scope,
    state,
    revision,
    ...(operationId !== undefined &&
      operationId !== null && { operation_id: operationId }),
    ...(error !== undefined && error !== null && { error }),
  };
}

export function decodeSurfaceSnapshot(raw: unknown): SurfaceSnapshot {
  const snapshot = decodeSurfaceSnapshotValue(raw);
  if (snapshot === undefined) {
    throw new Error('Invalid surface snapshot');
  }
  return snapshot;
}

export function decodeSurfaceEvent(raw: unknown): SurfaceEvent | undefined {
  if (!isObject(raw)) {
    return undefined;
  }
  const type = raw.type;
  if (type !== 'snapshot' && type !== 'state_changed') {
    return undefined;
  }
  const snapshot = decodeSurfaceSnapshotValue(raw);
  return snapshot === undefined ? undefined : { type, ...snapshot };
}

export function decodeSurfaceCommandResponse(
  raw: unknown,
): SurfaceCommandResponse {
  if (!isObject(raw)) {
    throw new Error('Invalid surface command response');
  }
  const command = raw.command;
  const accepted = raw.accepted;
  const reason = raw.reason;
  if (
    !isSurfaceCommand(command) ||
    typeof accepted !== 'boolean' ||
    (reason !== undefined && reason !== null && typeof reason !== 'string')
  ) {
    throw new Error('Invalid surface command response');
  }
  return {
    command,
    accepted,
    ...(reason !== undefined && { reason }),
    snapshot: decodeSurfaceSnapshot(raw.snapshot),
  };
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null;
}

function strField(o: Record<string, unknown>, key: string): string | undefined {
  const v = o[key];
  return typeof v === 'string' ? v : undefined;
}

function numField(o: Record<string, unknown>, key: string): number | undefined {
  const v = o[key];
  return typeof v === 'number' ? v : undefined;
}

function isValidWorkState(v: unknown): v is WorkState {
  return v === 'not_started' || v === 'in_progress' || v === 'overdue';
}

function isValidTaskAuthority(v: unknown): v is TaskAuthority {
  return v === 'candidate' || v === 'today_covered';
}

function isValidWorkTransitionKind(v: unknown): v is WorkTransitionKind {
  return (
    v === 'start' ||
    v === 'pause' ||
    v === 'progress' ||
    v === 'complete' ||
    v === 'delay' ||
    v === 'split'
  );
}

function isValidScheduleAlertKind(v: unknown): v is ScheduleAlertKind {
  return v === 'conflict' || v === 'overdue' || v === 'generation_failure';
}

function isValidActionKind(v: unknown): v is ActionKind {
  return v === 'immediate' || v === 'approval' || v === 'panel';
}

function isValidInputPath(v: unknown): v is InputPath {
  return (
    v === 'screen_capability' ||
    v === 'notification_capability' ||
    v === 'explicit_voice_session' ||
    v === 'ambient_wake_word' ||
    v === 'plain_text'
  );
}

function isValidQuickAction(v: unknown): v is QuickAction {
  return (
    v === 'start' ||
    v === 'pause' ||
    v === 'progress' ||
    v === 'complete' ||
    v === 'delay'
  );
}

function decodeCapabilityRequest(v: unknown): CapabilityRequest | undefined {
  if (!isObject(v)) {
    return undefined;
  }
  const o = v as Record<string, unknown>;
  const taskId = strField(o, 'task_id');
  const action = isValidQuickAction(o.action) ? o.action : undefined;
  const deviceId = strField(o, 'device_id');
  if (taskId === undefined || action === undefined || deviceId === undefined) {
    return undefined;
  }
  const inputPath = isValidInputPath(o.input_path) ? o.input_path : undefined;
  const eventId = strField(o, 'event_id');
  const snoozeMinutes = numField(o, 'snooze_minutes');
  const snoozeTarget = strField(o, 'snooze_target');
  const quantityDone = numField(o, 'quantity_done');
  const quantityTotal = numField(o, 'quantity_total');
  const note = strField(o, 'note');
  const scheduledAt = strField(o, 'scheduled_at');
  return {
    task_id: taskId,
    action,
    device_id: deviceId,
    ...(inputPath !== undefined && { input_path: inputPath }),
    ...(eventId !== undefined && { event_id: eventId }),
    ...(snoozeMinutes !== undefined && { snooze_minutes: snoozeMinutes }),
    ...(snoozeTarget !== undefined && { snooze_target: snoozeTarget }),
    ...(quantityDone !== undefined && { quantity_done: quantityDone }),
    ...(quantityTotal !== undefined && { quantity_total: quantityTotal }),
    ...(note !== undefined && { note }),
    ...(scheduledAt !== undefined && { scheduled_at: scheduledAt }),
  };
}

function decodeCurrentTask(
  obj: Record<string, unknown>,
): Presentation | undefined {
  const title = strField(obj, 'title');
  const reference = strField(obj, 'reference');
  const workState = obj.work_state;
  const authority = obj.authority;
  if (
    title === undefined ||
    reference === undefined ||
    !isValidWorkState(workState) ||
    !isValidTaskAuthority(authority)
  ) {
    return undefined;
  }
  const startAt = strField(obj, 'start_at');
  const endAt = strField(obj, 'end_at');
  const nextTask = strField(obj, 'next_task');
  const settlement = isObject(obj.settlement)
    ? decodeSettlement(obj.settlement)
    : undefined;
  return {
    type: 'current_task',
    title,
    reference,
    work_state: workState,
    authority,
    ...(startAt !== undefined && { start_at: startAt }),
    ...(endAt !== undefined && { end_at: endAt }),
    ...(nextTask !== undefined && { next_task: nextTask }),
    ...(settlement !== undefined && { settlement }),
  } as Presentation;
}

function decodeSettlement(v: unknown): SettlementPrompt | undefined {
  if (!isObject(v)) {
    return undefined;
  }
  const o = v as Record<string, unknown>;
  const question = strField(o, 'question');
  const act = isObject(o.act) ? decodeActionGroup(o.act) : undefined;
  const shift = isObject(o.shift) ? decodeActionGroup(o.shift) : undefined;
  if (question === undefined || act === undefined || shift === undefined) {
    return undefined;
  }
  return { question, act, shift };
}

function decodeWorkTransition(
  obj: Record<string, unknown>,
): Presentation | undefined {
  const kind = obj.kind;
  const reference = strField(obj, 'reference');
  const title = strField(obj, 'title');
  if (
    !isValidWorkTransitionKind(kind) ||
    reference === undefined ||
    title === undefined
  ) {
    return undefined;
  }
  const detail = strField(obj, 'detail');
  return {
    type: 'work_transition',
    kind,
    reference,
    title,
    ...(detail !== undefined && { detail }),
  } as Presentation;
}

function decodeScheduleAlert(
  obj: Record<string, unknown>,
): Presentation | undefined {
  const kind = obj.kind;
  const message = strField(obj, 'message');
  if (!isValidScheduleAlertKind(kind) || message === undefined) {
    return undefined;
  }
  return { type: 'schedule_alert', kind, message } as Presentation;
}

function decodeProgressSummary(
  obj: Record<string, unknown>,
): Presentation | undefined {
  const done = numField(obj, 'done');
  const inProgress = numField(obj, 'in_progress');
  const scheduled = numField(obj, 'scheduled');
  if (
    done === undefined ||
    inProgress === undefined ||
    scheduled === undefined
  ) {
    return undefined;
  }
  return {
    type: 'progress_summary',
    done,
    in_progress: inProgress,
    scheduled,
  } as Presentation;
}

function decodeScheduleEntry(v: unknown): ScheduleEntry | undefined {
  if (!isObject(v)) {
    return undefined;
  }
  const o = v as Record<string, unknown>;
  const reference = strField(o, 'reference');
  const title = strField(o, 'title');
  if (reference === undefined || title === undefined) {
    return undefined;
  }
  const startAt = strField(o, 'start_at');
  const endAt = strField(o, 'end_at');
  return {
    reference,
    title,
    ...(startAt !== undefined && { start_at: startAt }),
    ...(endAt !== undefined && { end_at: endAt }),
  };
}

function decodeScheduleSummary(
  obj: Record<string, unknown>,
): Presentation | undefined {
  let next: ScheduleEntry | undefined;
  if (obj.next !== undefined) {
    const n = decodeScheduleEntry(obj.next);
    if (n === undefined) {
      return undefined;
    }
    next = n;
  }
  let entries: ScheduleEntry[] | undefined;
  if (obj.entries !== undefined) {
    if (!Array.isArray(obj.entries)) {
      return undefined;
    }
    const es: ScheduleEntry[] = [];
    for (const e of obj.entries) {
      const de = decodeScheduleEntry(e);
      if (de === undefined) {
        return undefined;
      }
      es.push(de);
    }
    entries = es;
  }
  return {
    type: 'schedule_summary',
    ...(next !== undefined && { next }),
    ...(entries !== undefined && { entries }),
  } as Presentation;
}

function decodeActionCapability(v: unknown): ActionCapability | undefined {
  if (!isObject(v)) {
    return undefined;
  }
  const o = v as Record<string, unknown>;
  const id = strField(o, 'id');
  const action = isValidQuickAction(o.action) ? o.action : undefined;
  const deviceId = strField(o, 'device_id');
  const taskId = strField(o, 'task_id');
  const expiresAt = strField(o, 'expires_at');
  const inputPath = isValidInputPath(o.input_path) ? o.input_path : undefined;
  const oneShot = o.one_shot;
  if (
    id === undefined ||
    action === undefined ||
    deviceId === undefined ||
    taskId === undefined ||
    expiresAt === undefined ||
    inputPath === undefined ||
    typeof oneShot !== 'boolean'
  ) {
    return undefined;
  }
  const eventId = strField(o, 'event_id');
  const snoozeMinutes = numField(o, 'snooze_minutes');
  const snoozeTarget = strField(o, 'snooze_target');
  const quantityDone = numField(o, 'quantity_done');
  const quantityTotal = numField(o, 'quantity_total');
  const note = strField(o, 'note');
  const scheduledAt = strField(o, 'scheduled_at');
  const request = isObject(o.request)
    ? decodeCapabilityRequest(o.request)
    : undefined;
  return {
    id,
    action,
    device_id: deviceId,
    task_id: taskId,
    expires_at: expiresAt,
    input_path: inputPath,
    one_shot: oneShot,
    ...(eventId !== undefined && { event_id: eventId }),
    ...(snoozeMinutes !== undefined && { snooze_minutes: snoozeMinutes }),
    ...(snoozeTarget !== undefined && { snooze_target: snoozeTarget }),
    ...(quantityDone !== undefined && { quantity_done: quantityDone }),
    ...(quantityTotal !== undefined && { quantity_total: quantityTotal }),
    ...(note !== undefined && { note }),
    ...(scheduledAt !== undefined && { scheduled_at: scheduledAt }),
    ...(request !== undefined && { request }),
  };
}

function decodeAction(v: unknown): Action | undefined {
  if (!isObject(v)) {
    return undefined;
  }
  const o = v as Record<string, unknown>;
  const id = strField(o, 'id');
  const label = strField(o, 'label');
  const kind = isValidActionKind(o.kind) ? o.kind : undefined;
  if (id === undefined || label === undefined || kind === undefined) {
    return undefined;
  }
  if (kind === 'immediate') {
    const capability = decodeActionCapability(o.capability);
    if (capability === undefined) {
      return undefined;
    }
    return { id, label, kind, capability };
  }
  return { id, label, kind };
}

function decodeActionGroup(v: unknown): ActionGroup | undefined {
  if (!isObject(v)) {
    return undefined;
  }
  const o = v as Record<string, unknown>;
  const title = strField(o, 'title');
  const actions = o.actions;
  if (title === undefined || !Array.isArray(actions) || actions.length === 0) {
    return undefined;
  }
  const decoded: Action[] = [];
  for (const a of actions) {
    const d = decodeAction(a);
    if (d === undefined) {
      return undefined;
    }
    decoded.push(d);
  }
  return { title, actions: decoded };
}

function decodeCheckIn(obj: Record<string, unknown>): Presentation | undefined {
  const question = strField(obj, 'question');
  const act = isObject(obj.act) ? decodeActionGroup(obj.act) : undefined;
  const shift = isObject(obj.shift) ? decodeActionGroup(obj.shift) : undefined;
  if (question === undefined || act === undefined || shift === undefined) {
    return undefined;
  }
  return { type: 'check_in', question, act, shift } as Presentation;
}

function decodeChangeProposal(
  obj: Record<string, unknown>,
): Presentation | undefined {
  const id = strField(obj, 'id');
  const why = strField(obj, 'why');
  const expiresAt = strField(obj, 'expires_at');
  const changes = Array.isArray(obj.changes) ? obj.changes : [];
  if (id === undefined || why === undefined || expiresAt === undefined) {
    return undefined;
  }
  return {
    type: 'change_proposal',
    id,
    why,
    expires_at: expiresAt,
    changes: changes as ProposedChange[],
    inferred_fields: (Array.isArray(obj.inferred_fields)
      ? obj.inferred_fields
      : []) as InferredField[],
    warnings: (Array.isArray(obj.warnings) ? obj.warnings : []) as string[],
  } as Presentation;
}

function decodeClarification(
  obj: Record<string, unknown>,
): Presentation | undefined {
  const message = strField(obj, 'message');
  if (message === undefined) {
    return undefined;
  }
  let choices: string[] | undefined;
  if (obj.choices !== undefined) {
    if (
      !Array.isArray(obj.choices) ||
      !obj.choices.every((c) => typeof c === 'string')
    ) {
      return undefined;
    }
    choices = obj.choices as string[];
  }
  return {
    type: 'clarification',
    message,
    ...(choices !== undefined && { choices }),
  } as Presentation;
}

export type TurnEvent =
  | { type: 'AsrText'; data: string }
  | { type: 'Thinking'; data: string }
  | { type: 'Text'; data: string }
  | {
      type: 'ToolCall';
      data: { name: string; call_id: string; arguments: unknown };
    }
  | {
      type: 'ToolResult';
      data: {
        name: string;
        call_id: string;
        content: string;
        is_error: boolean;
      };
    }
  | { type: 'Error'; data: string }
  | { type: 'Done'; data: AgentTurnResult };

export type TtsBlockEvent = { type: 'TtsBlock'; data: string };

export interface AgentHistoryToolCall {
  id: string;
  name: string;
  arguments?: unknown;
}

export type AgentHistoryMessage =
  | { role: 'system'; content: string }
  | { role: 'user'; content: string }
  | { role: 'assistant'; content?: string; tool_calls?: AgentHistoryToolCall[] }
  | {
      role: 'tool';
      tool_call_id: string;
      content: string;
      is_error?: boolean;
    };

export type AgentStreamEvent = TurnEvent | TtsBlockEvent;

// ── Planner state sync (WI-3) ───────────────────────────────────────────

export interface PlannerStateEvent {
  type: 'state_changed';
  changed_at: string;
  source: string;
  kinds: string[];
}

// ── Quick-action capability types (WI-2) ────────────────────────────────

export type InputPath =
  | 'screen_capability'
  | 'notification_capability'
  | 'explicit_voice_session'
  | 'ambient_wake_word'
  | 'plain_text';

export type QuickAction = 'start' | 'pause' | 'progress' | 'complete' | 'delay';

export interface ActionCapability {
  id: string;
  event_id?: string;
  device_id: string;
  action: QuickAction;
  input_path: InputPath;
  expires_at: string;
  one_shot: boolean;
  /** The task this capability is authorized to act on. */
  task_id: string;
  /** Snooze duration in minutes, present for `delay` capabilities. */
  snooze_minutes?: number;
  /** ISO 8601 target `start_at` for `delay` capabilities, computed client-side or on first tap. */
  snooze_target?: string;
  /** Quantity completed, present for `progress` capabilities. */
  quantity_done?: number;
  /** Total quantity for the task/session, present for `progress` capabilities. */
  quantity_total?: number;
  /** Note to attach with progress, present for `progress` capabilities. */
  note?: string;
  /** ISO 8601 scheduled delivery time for notification capabilities. */
  scheduled_at?: string;
  /** The original request, included for client round-trip only; server uses top-level fields. */
  request?: CapabilityRequest;
}

export interface CapabilityRequest {
  task_id: string;
  action: QuickAction;
  device_id: string;
  /** Trusted input path the client wants the capability issued for. The server chooses the actual path. */
  input_path?: InputPath;
  event_id?: string;
  snooze_minutes?: number;
  /** ISO 8601 target `start_at` for `delay` capabilities, computed client-side or on first tap. */
  snooze_target?: string;
  quantity_done?: number;
  quantity_total?: number;
  note?: string;
  /** ISO 8601 timestamp for which the notification capability should remain valid. */
  scheduled_at?: string;
}

/** A start-time notification returned by the agent, ready to be scheduled locally. */
export interface StartTimeNotification {
  task_id: string;
  title: string;
  body: string;
  /** ISO 8601 delivery time. */
  scheduled_at: string;
  check_in: Presentation;
}

/** Immutable event-ledger row replayed to a device (WI-9). */
export interface EventLedgerRow {
  id: string;
  kind: string;
  task_id?: string | null;
  /** JSON-encoded typed Presentation kept immutable by the server. */
  presentation: string;
  urgency: string;
  schedule_revision: number;
  distribution_revision?: number | null;
  observation_kind: string;
  delivery_state:
    | 'pending_delivery'
    | 'delivered'
    | 'deferred_quiet_hours'
    | 'acknowledged'
    | 'ignored'
    | 'resolved';
  created_at: string;
  delivered_at?: string | null;
}

export interface EventEvaluationResult {
  due_events: unknown[];
  next_eval_at?: string | null;
}

export type DeliveryMode =
  | 'speak'
  | 'notify'
  | 'suppress'
  | 'defer_quiet_hours';

export interface DeliveryModeResponse {
  mode: DeliveryMode;
}

/** Response body for the start-time notification endpoint (Versioned flattens it). */
export interface StartTimeNotificationList {
  notifications: StartTimeNotification[];
}
