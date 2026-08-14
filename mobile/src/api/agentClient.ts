import type {
  ActionCapability,
  AgentHistoryMessage,
  AgentStreamEvent,
  AgentTurnResult,
  ApprovalRequest,
  ApprovalResult,
  CapabilityRequest,
  Presentation,
  ProposalDecision,
  TurnEvent,
  UserInputAnswer,
} from './agentTypes';
import { decodePresentation } from './agentTypes';
import type { HabitPreviewRequest, HabitPreviewTask } from './types';
import type { PermissionsMap } from './settingsStore';

export type { AgentTurnResult };

export interface AgentCapabilities {
  audio_input: boolean;
  tts: boolean;
  approvals: boolean;
  user_input: boolean;
}

export class AgentApiError extends Error {
  constructor(
    public status: number,
    public body: string,
  ) {
    super(`Agent API error ${status}: ${body}`);
    this.name = 'AgentApiError';
  }
}

export class AbortError extends Error {
  constructor(message = 'Stream aborted') {
    super(message);
    this.name = 'AbortError';
  }
}

export interface AgentLlmSettings {
  base_url: string;
  model: string;
  api_key?: string;
  permissions?: PermissionsMap;
}

export interface AgentTtsSettings {
  backend: string;
  api_key?: string;
  voice_id: string;
  language: string;
  sample_rate: number;
  model?: string;
  speed?: number;
}

export interface AgentAudioSettings {
  tts: AgentTtsSettings;
}

export interface AgentUpdateSettings {
  llm?: AgentLlmSettings;
  audio?: AgentAudioSettings;
}

export interface ToolStat {
  count: number;
  error_count: number;
  last_used: string | null;
}

export interface ToolStatsSnapshot {
  tools: Record<string, ToolStat>;
}

export class AgentClient {
  private readonly baseUrl: string;
  private readonly token: string;

  constructor(baseUrl: string, token: string) {
    this.baseUrl = baseUrl.replace(/\/+$/, '');
    this.token = token;
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
  ): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${this.token}`,
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text().catch(() => '');
    if (!response.ok) throw new AgentApiError(response.status, text);
    return text ? (JSON.parse(text) as T) : (undefined as T);
  }

  async previewHabit(body: HabitPreviewRequest): Promise<HabitPreviewTask[]> {
    return this.request('POST', '/api/habits/preview', body);
  }

  async health(): Promise<void> {
    await this.request<{ ok: boolean }>('GET', '/api/agent/v1/health');
  }

  async capabilities(): Promise<AgentCapabilities> {
    return this.request<AgentCapabilities>('GET', '/api/agent/v1/capabilities');
  }

  async createSession(permissions?: PermissionsMap): Promise<string> {
    const response = await this.request<{ session_id: string }>(
      'POST',
      '/api/agent/v1/sessions',
      { version: 1, permissions },
    );
    return response.session_id;
  }

  async resumeSession(options: {
    sessionId?: string;
    permissions?: PermissionsMap;
    history?: AgentHistoryMessage[];
    pendingApproval?: ApprovalRequest;
    scheduleDirty?: boolean;
    compactionSummary?: string;
  }): Promise<string> {
    const response = await this.request<{ session_id: string }>(
      'POST',
      '/api/agent/v1/sessions/resume',
      {
        version: 1,
        session_id: options.sessionId,
        permissions: options.permissions,
        history: options.history,
        pending_approval: options.pendingApproval,
        schedule_dirty: options.scheduleDirty,
        compaction_summary: options.compactionSummary,
      },
    );
    return response.session_id;
  }

  async updateSessionSettings(
    sessionId: string,
    permissions?: PermissionsMap,
  ): Promise<void> {
    await this.request<{ ok: boolean }>(
      'PUT',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/settings`,
      { version: 1, permissions },
    );
  }

  async runTurn(
    sessionId: string,
    text: string,
    idempotencyKey: string,
  ): Promise<AgentTurnResult> {
    return this.request<AgentTurnResult>(
      'POST',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/turns`,
      { version: 1, text, idempotency_key: idempotencyKey },
    );
  }

  runTurnStream(
    sessionId: string,
    text: string,
    idempotencyKey: string,
    onEvent: (event: AgentStreamEvent) => void,
    signal?: AbortSignal,
  ): Promise<AgentTurnResult> {
    const url = `${this.baseUrl}/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/turns/stream`;
    return this.streamRequest(
      url,
      { version: 1, text, idempotency_key: idempotencyKey },
      onEvent,
      signal,
    );
  }

  editTurnStream(
    sessionId: string,
    turnIndex: number,
    text: string,
    idempotencyKey: string,
    onEvent: (event: AgentStreamEvent) => void,
    signal?: AbortSignal,
  ): Promise<AgentTurnResult> {
    const url = `${this.baseUrl}/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/turns/${turnIndex}/edit/stream`;
    return this.streamRequest(
      url,
      { version: 1, text, idempotency_key: idempotencyKey },
      onEvent,
      signal,
    );
  }

  async revertTurn(
    sessionId: string,
    turnIndex: number,
    afterUser: boolean,
  ): Promise<void> {
    await this.request<{ ok: boolean }>(
      'POST',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/turns/${turnIndex}/revert`,
      { version: 1, after_user: afterUser },
    );
  }

  private streamRequest(
    url: string,
    body: unknown,
    onEvent: (event: AgentStreamEvent) => void,
    signal?: AbortSignal,
  ): Promise<AgentTurnResult> {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open('POST', url);
      xhr.setRequestHeader('Authorization', `Bearer ${this.token}`);
      xhr.setRequestHeader('Content-Type', 'application/json');
      xhr.setRequestHeader('Accept', 'text/event-stream');

      let buffer = '';
      let lastResponseLength = 0;
      let done = false;

      const abort = () => {
        try {
          xhr.abort();
        } catch {
          // Ignore errors from a double-abort or a completed request.
        }
      };
      const cleanupSignal = () => {
        signal?.removeEventListener('abort', abort);
      };
      if (signal?.aborted) {
        reject(new AbortError('Stream aborted'));
        return;
      }
      signal?.addEventListener('abort', abort);

      const parseBlock = (block: string) => {
        const dataLines: string[] = [];
        for (const line of block.split('\n')) {
          const trimmed = line.trimStart();
          if (trimmed === '' || trimmed.startsWith(':')) {
            continue;
          }
          if (trimmed.startsWith('data:')) {
            const data = trimmed
              .slice('data:'.length)
              .trimStart()
              .replace(/\r$/u, '');
            dataLines.push(data);
          }
        }
        if (dataLines.length === 0) {
          return;
        }
        const payload = dataLines.join('\n');
        if (payload === '[DONE]') {
          return;
        }
        try {
          const parsed = JSON.parse(payload) as { type: string };
          if (done) {
            return;
          }
          if (parsed.type === 'TtsBlock') {
            onEvent(parsed as AgentStreamEvent);
          } else {
            const event = parsed as TurnEvent;
            onEvent(event);
            if (event.type === 'Done') {
              done = true;
              cleanupSignal();
              resolve(event.data);
            }
          }
        } catch {
          // Ignore malformed SSE data.
        }
      };

      const flush = () => {
        while (true) {
          const lf = buffer.indexOf('\n\n');
          const crlf = buffer.indexOf('\r\n\r\n');
          let idx: number;
          let delimLen: number;
          if (lf === -1 && crlf === -1) {
            break;
          } else if (crlf === -1 || (lf !== -1 && lf < crlf)) {
            idx = lf;
            delimLen = 2;
          } else {
            idx = crlf;
            delimLen = 4;
          }
          const block = buffer.slice(0, idx);
          buffer = buffer.slice(idx + delimLen);
          parseBlock(block);
        }
      };

      xhr.onprogress = () => {
        if (xhr.status >= 400) {
          return;
        }
        const response = xhr.responseText ?? '';
        if (response.length > lastResponseLength) {
          buffer += response.slice(lastResponseLength);
          lastResponseLength = response.length;
          flush();
        }
      };

      xhr.onload = () => {
        cleanupSignal();
        if (xhr.status >= 400) {
          reject(
            new AgentApiError(xhr.status, xhr.responseText || 'request failed'),
          );
          return;
        }
        flush();
        if (buffer.trim().length > 0) {
          parseBlock(buffer);
          buffer = '';
        }
        if (!done) {
          reject(new Error('Stream ended without a Done event'));
        }
      };

      xhr.onerror = () => {
        cleanupSignal();
        reject(
          new AgentApiError(xhr.status, xhr.responseText || 'network error'),
        );
      };

      xhr.onabort = () => {
        cleanupSignal();
        reject(new AbortError('Stream aborted'));
      };

      xhr.send(JSON.stringify(body));
    });
  }

  async getApproval(sessionId: string): Promise<ApprovalRequest | null> {
    const response = await this.request<ApprovalRequest | { version: number }>(
      'GET',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/approval`,
    );
    if (response && 'id' in response && typeof response.id === 'string') {
      return response as ApprovalRequest;
    }
    return null;
  }

  async resolveApproval(
    sessionId: string,
    approvalId: string,
    approve: boolean,
    idempotencyKey: string,
    proposals?: ProposalDecision[],
  ): Promise<ApprovalResult> {
    const body: Record<string, unknown> = {
      version: 1,
      approve,
      idempotency_key: idempotencyKey,
    };
    if (proposals !== undefined) {
      body.proposals = proposals;
    }
    return this.request<ApprovalResult>(
      'POST',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/approvals/${encodeURIComponent(approvalId)}`,
      body,
    );
  }

  async submitUserInput(
    sessionId: string,
    callId: string,
    answers: UserInputAnswer[],
  ): Promise<void> {
    await this.request<{ ok: boolean }>(
      'POST',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/tool-calls/${encodeURIComponent(callId)}/user-input`,
      { version: 1, answers },
    );
  }

  async deleteSession(sessionId: string): Promise<void> {
    await this.request<void>(
      'DELETE',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}`,
    );
  }

  async updateSettings(settings: AgentUpdateSettings): Promise<void> {
    await this.request<{ version: number; ok: boolean }>(
      'PUT',
      '/api/agent/v1/settings',
      { version: 1, ...settings },
    );
  }

  async getToolStats(): Promise<ToolStatsSnapshot> {
    return this.request<ToolStatsSnapshot>('GET', '/api/agent/v1/stats/tools');
  }

  async clearToolStats(): Promise<void> {
    await this.request<void>('DELETE', '/api/agent/v1/stats/tools');
  }

  async mintCapability(request: CapabilityRequest): Promise<ActionCapability> {
    return this.request<ActionCapability>(
      'POST',
      '/api/agent/v1/capabilities',
      { version: 1, ...request },
    );
  }

  async authorizeAction(capability: ActionCapability): Promise<Presentation> {
    const raw = await this.request<Presentation>(
      'POST',
      '/api/agent/v1/actions',
      {
        version: 1,
        ...capability,
      },
    );
    return decodePresentation(raw);
  }

  async quickAction(request: CapabilityRequest): Promise<Presentation> {
    const capability = await this.mintCapability(request);
    return this.authorizeAction(capability);
  }
}
