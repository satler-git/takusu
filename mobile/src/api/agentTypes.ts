export interface ProposedChange {
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
}

export interface UserInputQuestion {
  text: string;
  for: string;
}

export interface UserInputAnswer {
  text: string;
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

export type AgentStreamEvent = TurnEvent | TtsBlockEvent;
