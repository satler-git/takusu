import type {
  ActionCapability,
  AgentHistoryMessage,
  AgentStreamEvent,
  AgentTurnResult,
  ApprovalRequest,
  ApprovalResult,
  DeliveryMode,
  DeliveryModeResponse,
  EventEvaluationResult,
  EventLedgerRow,
  CapabilityRequest,
  IntakeState,
  PlannerStateEvent,
  Presentation,
  ProposalDecision,
  StartTimeNotificationList,
  SurfaceAudioCallback,
  SurfaceCommand,
  SurfaceCommandResponse,
  SurfaceEvent,
  SurfaceSnapshot,
  TurnEvent,
  UserInputAnswer,
} from './agentTypes';
import {
  decodePresentation,
  decodeSurfaceCommandResponse,
  decodeSurfaceEvent,
  decodeSurfaceSnapshot,
} from './agentTypes';
import type { HabitPreviewRequest, HabitPreviewTask } from './types';
import type { PermissionsMap } from './settingsStore';

/// Extract the `data:` payload from a single SSE block. Returns `undefined`
/// when the block contains no `data:` lines.
function parseSseBlock(block: string): string | undefined {
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
    return undefined;
  }
  return dataLines.join('\n');
}

/// Buffers incoming SSE text, splits it into discrete event blocks, and keeps
/// the tail of an incomplete block for the next feed.
class SseBuffer {
  private buffer = '';
  private lastTotal = 0;

  feed(totalText: string) {
    if (totalText.length > this.lastTotal) {
      this.buffer += totalText.slice(this.lastTotal);
      this.lastTotal = totalText.length;
    }
  }

  takeBlocks(): string[] {
    const blocks: string[] = [];
    while (true) {
      const lf = this.buffer.indexOf('\n\n');
      const crlf = this.buffer.indexOf('\r\n\r\n');
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
      blocks.push(this.buffer.slice(0, idx));
      this.buffer = this.buffer.slice(idx + delimLen);
    }
    return blocks;
  }

  tail(): string {
    return this.buffer;
  }

  clear() {
    this.buffer = '';
    this.lastTotal = 0;
  }
}

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

type SseDecodeResult<T> = { type: 'event'; event: T } | { type: 'reconnect' };

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
    intakeState?: IntakeState | null;
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
        intake_state: options.intakeState,
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
    const result = await this.request<AgentTurnResult>(
      'POST',
      `/api/agent/v1/sessions/${encodeURIComponent(sessionId)}/turns`,
      { version: 1, text, idempotency_key: idempotencyKey },
    );
    if (result?.presentation) {
      result.presentation = decodePresentation(result.presentation);
    }
    return result;
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

      const sseBuffer = new SseBuffer();
      const state = { done: false };

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

      const handleBlocks = () => {
        for (const block of sseBuffer.takeBlocks()) {
          this.handleStreamBlock(block, onEvent, state, resolve, cleanupSignal);
        }
      };

      xhr.onprogress = () => {
        if (xhr.status >= 400) {
          return;
        }
        sseBuffer.feed(xhr.responseText ?? '');
        handleBlocks();
      };

      xhr.onload = () => {
        cleanupSignal();
        if (xhr.status >= 400) {
          reject(
            new AgentApiError(xhr.status, xhr.responseText || 'request failed'),
          );
          return;
        }
        sseBuffer.feed(xhr.responseText ?? '');
        handleBlocks();
        const tail = sseBuffer.tail().trim();
        if (tail.length > 0) {
          this.handleStreamBlock(tail, onEvent, state, resolve, cleanupSignal);
        }
        if (!state.done) {
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

  private handleStreamBlock(
    block: string,
    onEvent: (event: AgentStreamEvent) => void,
    state: { done: boolean },
    resolve: (value: AgentTurnResult) => void,
    cleanup: () => void,
  ) {
    const payload = parseSseBlock(block);
    if (payload === undefined) {
      return;
    }
    if (payload === '[DONE]') {
      return;
    }
    try {
      const parsed: unknown = JSON.parse(payload);
      if (
        typeof parsed !== 'object' ||
        parsed === null ||
        Array.isArray(parsed)
      ) {
        return;
      }
      const eventLike = parsed as Record<string, unknown>;
      const eventType =
        typeof eventLike.type === 'string' ? eventLike.type : undefined;
      if (eventType === undefined) {
        return;
      }
      if (state.done) {
        return;
      }
      if (eventType === 'TtsBlock') {
        onEvent(parsed as AgentStreamEvent);
      } else {
        const event = parsed as TurnEvent;
        if (event.type === 'Done') {
          const result = event.data;
          if (result && typeof result === 'object' && result.presentation) {
            result.presentation = decodePresentation(result.presentation);
          }
        }
        onEvent(event);
        if (event.type === 'Done') {
          state.done = true;
          cleanup();
          resolve(event.data);
        }
      }
    } catch {
      // Ignore malformed SSE data.
    }
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

  private subscribeSse<T>(
    path: string,
    decode: (raw: unknown) => SseDecodeResult<T> | undefined,
    onEvent: (event: T) => void,
    onConnect?: () => void,
  ): () => void {
    const url = `${this.baseUrl}${path}`;
    let unsubscribed = false;
    let permanentlyFailed = false;
    let currentXhr: XMLHttpRequest | null = null;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    const baseDelay = 1000;
    const maxDelay = 30000;
    const backoff = 1.5;
    const stableConnectionMs = 5000;
    let nextDelay = baseDelay;
    let connectionStartedAt = 0;

    const isAuthError = (status: number): boolean =>
      status === 401 || status === 403;

    const stop = () => {
      permanentlyFailed = true;
      if (retryTimer !== null) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
    };

    const scheduleReconnect = () => {
      if (unsubscribed || permanentlyFailed || retryTimer !== null) {
        return;
      }
      retryTimer = setTimeout(() => {
        retryTimer = null;
        nextDelay = Math.min(maxDelay, nextDelay * backoff);
        connect();
      }, nextDelay);
    };

    const maybeResetBackoff = () => {
      if (
        connectionStartedAt !== 0 &&
        Date.now() - connectionStartedAt >= stableConnectionMs
      ) {
        nextDelay = baseDelay;
      }
    };

    const handleBlock = (block: string) => {
      const payload = parseSseBlock(block);
      if (payload === undefined || payload === '[DONE]') {
        return;
      }
      try {
        const decision = decode(JSON.parse(payload) as unknown);
        if (decision?.type === 'reconnect') {
          currentXhr?.abort();
          scheduleReconnect();
        } else if (decision?.type === 'event') {
          onEvent(decision.event);
        }
      } catch {
        // Ignore malformed SSE data.
      }
    };

    const connect = () => {
      if (unsubscribed || permanentlyFailed) {
        return;
      }
      onConnect?.();
      const xhr = new XMLHttpRequest();
      currentXhr = xhr;
      connectionStartedAt = Date.now();
      const sseBuffer = new SseBuffer();
      xhr.open('GET', url);
      xhr.setRequestHeader('Authorization', `Bearer ${this.token}`);
      xhr.setRequestHeader('Accept', 'text/event-stream');

      xhr.onprogress = () => {
        if (unsubscribed || xhr.status >= 400) {
          return;
        }
        maybeResetBackoff();
        sseBuffer.feed(xhr.responseText ?? '');
        for (const block of sseBuffer.takeBlocks()) {
          handleBlock(block);
        }
      };

      xhr.onload = () => {
        if (unsubscribed) {
          return;
        }
        if (isAuthError(xhr.status)) {
          stop();
          return;
        }
        if (xhr.status >= 400) {
          scheduleReconnect();
          return;
        }
        maybeResetBackoff();
        sseBuffer.feed(xhr.responseText ?? '');
        for (const block of sseBuffer.takeBlocks()) {
          handleBlock(block);
        }
        const tail = sseBuffer.tail().trim();
        if (tail.length > 0) {
          handleBlock(tail);
        }
        scheduleReconnect();
      };

      xhr.onerror = () => {
        if (unsubscribed || permanentlyFailed) {
          return;
        }
        if (isAuthError(xhr.status)) {
          stop();
          return;
        }
        scheduleReconnect();
      };

      xhr.onabort = () => {
        // Clean unsubscribe or an intentional reconnect; neither needs a second timer.
      };

      xhr.send();
    };

    connect();

    return () => {
      unsubscribed = true;
      if (retryTimer !== null) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      currentXhr?.abort();
    };
  }

  async getSurfaceSnapshot(): Promise<SurfaceSnapshot> {
    const response = await this.request<SurfaceSnapshot & { version?: number }>(
      'GET',
      '/api/agent/v1/surface',
    );
    return decodeSurfaceSnapshot(response);
  }

  async sendSurfaceCommand(
    command: SurfaceCommand,
    operationId?: number,
  ): Promise<SurfaceCommandResponse> {
    const response = await this.request<
      SurfaceCommandResponse & { version?: number }
    >('POST', '/api/agent/v1/surface/commands', {
      version: 1,
      command,
      ...(operationId !== undefined && { operation_id: operationId }),
    });
    return decodeSurfaceCommandResponse(response);
  }

  async reportSurfaceAudio(
    callback: SurfaceAudioCallback,
    operationId?: number,
  ): Promise<SurfaceSnapshot> {
    const response = await this.request<SurfaceSnapshot & { version?: number }>(
      'POST',
      '/api/agent/v1/surface/audio',
      {
        version: 1,
        callback,
        ...(operationId !== undefined && { operation_id: operationId }),
      },
    );
    return decodeSurfaceSnapshot(response);
  }

  /// Subscribe to the device-scoped surface state. The server sends a snapshot
  /// first and does the same after every reconnect, so a surface does not need
  /// to coordinate a separate snapshot request with its stream.
  subscribeSurface(onEvent: (event: SurfaceEvent) => void): () => void {
    let lastRevision: number | undefined;
    return this.subscribeSse(
      '/api/agent/v1/surface/events',
      (raw): SseDecodeResult<SurfaceEvent> | undefined => {
        const event = decodeSurfaceEvent(raw);
        if (event === undefined) {
          return undefined;
        }
        if (event.type === 'snapshot') {
          if (lastRevision !== undefined && event.revision < lastRevision) {
            return undefined;
          }
          lastRevision = event.revision;
          return { type: 'event', event };
        }
        if (lastRevision !== undefined) {
          if (event.revision <= lastRevision) {
            return undefined;
          }
          if (event.revision > lastRevision + 1) {
            return { type: 'reconnect' };
          }
        }
        lastRevision = event.revision;
        return { type: 'event', event };
      },
      onEvent,
      () => {
        lastRevision = undefined;
      },
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

  async evaluatePlannerEvents(
    deviceId = 'mobile',
  ): Promise<EventEvaluationResult> {
    return this.request<EventEvaluationResult>('POST', '/api/events/evaluate', {
      device_id: deviceId,
    });
  }

  async listPlannerEvents(deviceId = 'mobile'): Promise<EventLedgerRow[]> {
    return this.request<EventLedgerRow[]>(
      'GET',
      `/api/events?device_id=${encodeURIComponent(deviceId)}`,
    );
  }

  async claimPlannerEvent(
    eventId: string,
    deviceId = 'mobile',
  ): Promise<boolean> {
    const result = await this.request<{ claimed: boolean }>(
      'POST',
      `/api/events/${encodeURIComponent(eventId)}/claim`,
      { device_id: deviceId },
    );
    return result.claimed;
  }

  async eventDeliveryMode(
    eventId: string,
    deviceId = 'mobile',
  ): Promise<DeliveryMode> {
    const response = await this.request<DeliveryModeResponse>(
      'GET',
      `/api/events/${encodeURIComponent(eventId)}/delivery?device_id=${encodeURIComponent(deviceId)}`,
    );
    return response.mode;
  }

  async suppressDevice(deviceId: string, minutes = 60): Promise<void> {
    await this.request<unknown>(
      'POST',
      `/api/devices/${encodeURIComponent(deviceId)}/suppress`,
      { minutes },
    );
  }

  async updatePlannerEventState(
    eventId: string,
    state: EventLedgerRow['delivery_state'],
  ): Promise<EventLedgerRow> {
    return this.request<EventLedgerRow>(
      'PUT',
      `/api/events/${encodeURIComponent(eventId)}/state`,
      state,
    );
  }

  /// Fetch the next start-time notifications with embedded action capabilities (WI-4).
  async getStartTimeNotifications(
    limit = 10,
    deviceId = 'mobile',
    tz?: string,
  ): Promise<StartTimeNotificationList['notifications']> {
    let url = `/api/agent/v1/notifications/start-time?limit=${limit}&device_id=${encodeURIComponent(deviceId)}`;
    if (tz) {
      url += `&tz=${encodeURIComponent(tz)}`;
    }
    const response = await this.request<
      StartTimeNotificationList & {
        version?: number;
      }
    >('GET', url);
    return response.notifications.map((n) => ({
      ...n,
      check_in: decodePresentation(n.check_in),
    }));
  }

  /// Subscribe to planner state changes (WI-3). Authentication failures stop
  /// the stream; transient failures use the shared reconnect implementation.
  subscribeEvents(onEvent: (event: PlannerStateEvent) => void): () => void {
    return this.subscribeSse(
      '/api/agent/v1/events',
      (raw): SseDecodeResult<PlannerStateEvent> | undefined => {
        if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
          return undefined;
        }
        const event = raw as Record<string, unknown>;
        if (
          event.type !== 'state_changed' ||
          typeof event.changed_at !== 'string' ||
          typeof event.source !== 'string' ||
          !Array.isArray(event.kinds) ||
          !event.kinds.every((kind) => typeof kind === 'string')
        ) {
          return undefined;
        }
        return {
          type: 'event',
          event: event as unknown as PlannerStateEvent,
        };
      },
      onEvent,
    );
  }
}
