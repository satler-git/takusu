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
}

export interface AgentTurnResult {
  text: string;
  changes: ChangeReceipt[];
  schedule_dirty: boolean;
  approval_request: ApprovalRequest | null;
  presentation?: Presentation | null;
}

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

export interface TaskCard {
  title: string;
  reference: string;
  start_at?: string;
  end_at?: string;
  work_state: WorkState;
  authority: TaskAuthority;
  next_task?: string;
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
      return hasStr(obj, 'title', 'reference')
        ? (raw as Presentation)
        : { type: 'text', text: fallback };
    case 'work_transition':
      return hasStr(obj, 'kind', 'reference', 'title')
        ? (raw as Presentation)
        : { type: 'text', text: fallback };
    case 'schedule_summary':
      // All fields are optional on this variant; only the object shape matters.
      return raw as Presentation;
    case 'progress_summary':
      return hasNum(obj, 'done', 'in_progress', 'scheduled')
        ? (raw as Presentation)
        : { type: 'text', text: fallback };
    case 'schedule_alert':
      return hasStr(obj, 'kind', 'message')
        ? (raw as Presentation)
        : { type: 'text', text: fallback };
    case 'check_in':
      return hasStr(obj, 'question') && isObject(obj.act) && isObject(obj.shift)
        ? (raw as Presentation)
        : { type: 'text', text: fallback };
    case 'change_proposal':
      return hasStr(obj, 'id', 'why') && Array.isArray(obj.changes)
        ? (raw as Presentation)
        : { type: 'text', text: fallback };
    case 'clarification':
      return hasStr(obj, 'message')
        ? (raw as Presentation)
        : { type: 'text', text: fallback };
    case 'text':
      return { type: 'text', text: fallback };
    default:
      return { type: 'text', text: fallback };
  }
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null;
}

function hasStr(o: Record<string, unknown>, ...keys: string[]): boolean {
  return keys.every((k) => typeof o[k] === 'string');
}

function hasNum(o: Record<string, unknown>, ...keys: string[]): boolean {
  return keys.every((k) => typeof o[k] === 'number');
}

export type TurnEvent =
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
