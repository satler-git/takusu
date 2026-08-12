import type {
  TaskRow,
  CreateTask,
  UpdateTask,
  TaskQuery,
  HabitRow,
  HabitDetail,
  HabitEstimateRequest,
  HabitEstimateResult,
  HabitPreviewRequest,
  HabitPreviewTask,
  CreateHabit,
  UpdateHabit,
  HabitScheduledSpanRow,
  CreateHabitScheduledSpan,
  HabitStepRow,
  HabitStepInput,
  ScheduleRow,
  GenerateSchedule,
  RescheduleRequest,
  MoveEntryRequest,
  MoveEntryResponse,
  SettingsRow,
  UpdateSettings,
  TokenRow,
  TokenCreateResponse,
  GoogleCalSettings,
  UpdateGoogleCalSettings,
  OAuthCallbackResponse,
  SyncTriggerResponse,
  DeleteAllGcalResponse,
  GoogleCalEventMapping,
  IcalImportResult,
  DependencyAnalysisResponse,
  SkillRow,
  CreateSkill,
  UpdateSkill,
  StartWorkSession,
  AttachWorkSession,
  ConvertWorkSession,
  RecordWorkSessionProgress,
  WorkSessionRow,
  WorkSessionProgressResult,
  SplitTask,
  SplitResult,
  Completion,
  CommentRow,
  CreateComment,
} from './types';

export class ApiError extends Error {
  constructor(
    public status: number,
    public body: string,
  ) {
    super(`API error ${status}: ${body}`);
    this.name = 'ApiError';
  }
}

export class TakusuClient {
  readonly baseUrl: string;
  private token: string;

  constructor(baseUrl: string, token: string) {
    this.baseUrl = baseUrl.replace(/\/+$/, '');
    this.token = token;
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    operationId?: string,
  ): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = {
      Authorization: `Bearer ${this.token}`,
    };
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
    }
    if (operationId !== undefined && operationId !== '') {
      headers['Idempotency-Key'] = operationId;
    }
    const resp = await fetch(url, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    const status = resp.status;
    if (status >= 400) {
      const text = await resp.text().catch(() => '');
      throw new ApiError(status, text);
    }
    const text = await resp.text();
    if (!text) return undefined as T;
    return JSON.parse(text) as T;
  }

  // ── Health ──
  async health(signal?: AbortSignal): Promise<string> {
    const resp = await fetch(`${this.baseUrl}/health`, { signal });
    if (resp.status >= 400) {
      const body = await resp.text().catch(() => '');
      throw new ApiError(resp.status, body);
    }
    return resp.text();
  }

  // ── Task ──
  async listTasks(query?: TaskQuery): Promise<TaskRow[]> {
    // Build the query string manually: Hermes does not provide a working
    // URLSearchParams (its methods throw or are missing at runtime).
    const params: string[] = [];
    if (query?.status)
      params.push(`status=${encodeURIComponent(query.status)}`);
    if (query?.from) params.push(`from=${encodeURIComponent(query.from)}`);
    if (query?.until) params.push(`until=${encodeURIComponent(query.until)}`);
    if (query?.no_overdue !== undefined && query.no_overdue !== null)
      params.push(`no_overdue=${query.no_overdue}`);
    if (query?.habit_id)
      params.push(`habit_id=${encodeURIComponent(query.habit_id)}`);
    if (query?.ical_uid)
      params.push(`ical_uid=${encodeURIComponent(query.ical_uid)}`);
    if (query?.q) params.push(`q=${encodeURIComponent(query.q)}`);
    if (query?.limit !== undefined && query.limit !== null)
      params.push(`limit=${encodeURIComponent(query.limit.toString())}`);
    const qs = params.join('&');
    return this.request('GET', `/api/tasks${qs ? `?${qs}` : ''}`);
  }

  async completeTaskQuery(q: string, limit?: number): Promise<Completion[]> {
    const limitParam =
      limit !== undefined
        ? `&limit=${encodeURIComponent(limit.toString())}`
        : '';
    return this.request(
      'GET',
      `/api/tasks/complete?q=${encodeURIComponent(q)}${limitParam}`,
    );
  }

  async getTask(id: string): Promise<TaskRow> {
    return this.request('GET', `/api/tasks/${encodeURIComponent(id)}`);
  }

  async createTask(body: CreateTask): Promise<TaskRow> {
    return this.request('POST', '/api/tasks', body);
  }

  async updateTask(id: string, body: UpdateTask): Promise<TaskRow> {
    return this.request('PATCH', `/api/tasks/${encodeURIComponent(id)}`, body);
  }

  async replaceTask(id: string, body: CreateTask): Promise<TaskRow> {
    return this.request('PUT', `/api/tasks/${encodeURIComponent(id)}`, body);
  }

  async deleteTask(id: string): Promise<void> {
    return this.request('DELETE', `/api/tasks/${encodeURIComponent(id)}`);
  }

  // ── Task comments (WI-1) ──
  async listComments(taskId: string): Promise<CommentRow[]> {
    return this.request(
      'GET',
      `/api/tasks/${encodeURIComponent(taskId)}/comments`,
    );
  }

  async createComment(
    taskId: string,
    body: CreateComment,
    operationId?: string,
  ): Promise<CommentRow> {
    return this.request(
      'POST',
      `/api/tasks/${encodeURIComponent(taskId)}/comments`,
      body,
      operationId,
    );
  }

  async createAgentComment(
    taskId: string,
    body: CreateComment,
    operationId?: string,
  ): Promise<CommentRow> {
    return this.request(
      'POST',
      `/api/tasks/${encodeURIComponent(taskId)}/comments/agent`,
      body,
      operationId,
    );
  }

  async deleteComment(id: string): Promise<void> {
    return this.request('DELETE', `/api/comments/${encodeURIComponent(id)}`);
  }

  // ── Work sessions (#1393) ──
  async createWorkSession(
    body: StartWorkSession,
    operationId?: string,
  ): Promise<WorkSessionRow> {
    return this.request('POST', '/api/work-sessions', body, operationId);
  }

  async listWorkSessions(taskId?: string): Promise<WorkSessionRow[]> {
    const qs = taskId ? `?task_id=${encodeURIComponent(taskId)}` : '';
    return this.request('GET', `/api/work-sessions${qs}`);
  }

  async getWorkSession(id: string): Promise<WorkSessionRow> {
    return this.request('GET', `/api/work-sessions/${encodeURIComponent(id)}`);
  }

  async pauseWorkSession(
    id: string,
    operationId?: string,
  ): Promise<WorkSessionRow> {
    return this.request(
      'POST',
      `/api/work-sessions/${encodeURIComponent(id)}/pause`,
      undefined,
      operationId,
    );
  }

  async completeWorkSession(
    id: string,
    operationId?: string,
  ): Promise<WorkSessionRow> {
    return this.request(
      'POST',
      `/api/work-sessions/${encodeURIComponent(id)}/complete`,
      undefined,
      operationId,
    );
  }

  async recordWorkSessionProgress(
    id: string,
    body: RecordWorkSessionProgress,
    operationId?: string,
  ): Promise<WorkSessionProgressResult> {
    return this.request(
      'POST',
      `/api/work-sessions/${encodeURIComponent(id)}/progress`,
      body,
      operationId,
    );
  }

  async attachWorkSession(
    id: string,
    body: AttachWorkSession,
    operationId?: string,
  ): Promise<WorkSessionRow> {
    return this.request(
      'POST',
      `/api/work-sessions/${encodeURIComponent(id)}/attach`,
      body,
      operationId,
    );
  }

  async convertWorkSession(
    id: string,
    body: ConvertWorkSession,
    operationId?: string,
  ): Promise<TaskRow> {
    return this.request(
      'POST',
      `/api/work-sessions/${encodeURIComponent(id)}/convert`,
      body,
      operationId,
    );
  }

  async splitTask(
    id: string,
    body: SplitTask,
    operationId?: string,
  ): Promise<SplitResult> {
    return this.request(
      'POST',
      `/api/tasks/${encodeURIComponent(id)}/split`,
      body,
      operationId,
    );
  }

  // ── Composite dependency analysis (#355) ──
  async analyzeTaskDependencies(): Promise<DependencyAnalysisResponse> {
    return this.request('GET', '/api/tasks/dependency-analysis');
  }

  async importIcal(icalText: string): Promise<IcalImportResult> {
    const url = `${this.baseUrl}/api/tasks/import/ical`;
    const resp = await fetch(url, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${this.token}`,
        'Content-Type': 'text/plain',
      },
      body: icalText,
    });
    const status = resp.status;
    if (status >= 400) {
      const text = await resp.text().catch(() => '');
      throw new ApiError(status, text);
    }
    const text = await resp.text();
    if (!text) return { imported: 0, task_ids: [] };
    return JSON.parse(text) as IcalImportResult;
  }

  // ── Habit ──
  async listHabits(): Promise<HabitRow[]> {
    return this.request('GET', '/api/habits');
  }

  async getHabit(id: string): Promise<HabitDetail> {
    return this.request('GET', `/api/habits/${encodeURIComponent(id)}`);
  }

  async estimateHabit(
    id: string,
    body: HabitEstimateRequest,
  ): Promise<HabitEstimateResult> {
    return this.request(
      'POST',
      `/api/habits/${encodeURIComponent(id)}/estimate`,
      body,
    );
  }

  async createHabit(body: CreateHabit): Promise<HabitRow> {
    return this.request('POST', '/api/habits', body);
  }

  async previewHabit(body: HabitPreviewRequest): Promise<HabitPreviewTask[]> {
    return this.request('POST', '/api/habits/preview', body);
  }

  async updateHabit(id: string, body: UpdateHabit): Promise<HabitRow> {
    return this.request('PATCH', `/api/habits/${encodeURIComponent(id)}`, body);
  }

  async replaceHabit(id: string, body: CreateHabit): Promise<HabitRow> {
    return this.request('PUT', `/api/habits/${encodeURIComponent(id)}`, body);
  }

  async deleteHabit(id: string): Promise<void> {
    return this.request('DELETE', `/api/habits/${encodeURIComponent(id)}`);
  }

  // ── Habit scheduled spans (#303 / #503) ──
  async listHabitScheduledSpans(id: string): Promise<HabitScheduledSpanRow[]> {
    return this.request(
      'GET',
      `/api/habits/${encodeURIComponent(id)}/scheduled-spans`,
    );
  }

  async listAllHabitScheduledSpans(): Promise<HabitScheduledSpanRow[]> {
    return this.request('GET', '/api/habits/scheduled-spans');
  }

  async createHabitScheduledSpan(
    id: string,
    body: CreateHabitScheduledSpan,
  ): Promise<HabitScheduledSpanRow> {
    return this.request(
      'POST',
      `/api/habits/${encodeURIComponent(id)}/scheduled-spans`,
      body,
    );
  }

  async deleteHabitScheduledSpan(id: string, spanId: string): Promise<void> {
    return this.request(
      'DELETE',
      `/api/habits/${encodeURIComponent(id)}/scheduled-spans/${encodeURIComponent(spanId)}`,
    );
  }

  // ── Habit steps (#95) ──
  async listHabitSteps(id: string): Promise<HabitStepRow[]> {
    return this.request('GET', `/api/habits/${encodeURIComponent(id)}/steps`);
  }

  async listAllHabitSteps(): Promise<HabitStepRow[]> {
    return this.request('GET', '/api/habits/steps');
  }

  async replaceHabitSteps(
    id: string,
    steps: HabitStepInput[],
  ): Promise<HabitStepRow[]> {
    return this.request(
      'PUT',
      `/api/habits/${encodeURIComponent(id)}/steps`,
      steps,
    );
  }

  async analyzeHabitStepDependencies(
    id: string,
  ): Promise<DependencyAnalysisResponse> {
    return this.request(
      'GET',
      `/api/habits/${encodeURIComponent(id)}/steps/dependency-analysis`,
    );
  }

  // ── Schedule ──
  async getSchedule(): Promise<ScheduleRow> {
    return this.request('GET', '/api/schedule');
  }

  async generateSchedule(body: GenerateSchedule): Promise<ScheduleRow> {
    return this.request('POST', '/api/schedule/generate', body);
  }

  async reschedule(body: RescheduleRequest): Promise<ScheduleRow> {
    return this.request('POST', '/api/schedule/reschedule', body);
  }

  async moveEntry(
    taskId: string,
    body: MoveEntryRequest,
  ): Promise<MoveEntryResponse> {
    const raw = await this.request<MoveEntryResponse>(
      'PATCH',
      `/api/schedule/entries/${encodeURIComponent(taskId)}`,
      body,
    );
    if (!raw.task_id || !raw.start_at || !raw.end_at) {
      throw new ApiError(0, 'invalid move entry response from server');
    }
    return {
      task_id: raw.task_id,
      start_at: raw.start_at,
      end_at: raw.end_at,
      warnings: raw.warnings ?? [],
    };
  }

  async clearSchedule(): Promise<void> {
    return this.request('DELETE', '/api/schedule');
  }

  // ── Settings ──
  async getSettings(): Promise<SettingsRow> {
    return this.request('GET', '/api/settings');
  }

  async updateSettings(body: UpdateSettings): Promise<SettingsRow> {
    return this.request('PUT', '/api/settings', body);
  }

  // ── Token ──
  async listTokens(): Promise<TokenRow[]> {
    return this.request('GET', '/api/tokens');
  }

  async createToken(label?: string): Promise<TokenCreateResponse> {
    return this.request('POST', '/api/tokens', { label });
  }

  async revokeToken(id: number): Promise<void> {
    if (!Number.isFinite(id) || !Number.isInteger(id) || id <= 0) {
      throw new ApiError(400, 'token id must be a positive integer');
    }
    return this.request('DELETE', `/api/tokens/${encodeURIComponent(id)}`);
  }

  // ── Sync / Google Calendar ──
  async getGcalSettings(): Promise<GoogleCalSettings> {
    return this.request('GET', '/api/sync/settings');
  }

  async oauthCallback(
    code: string,
    redirectUri?: string,
  ): Promise<OAuthCallbackResponse> {
    const body: { code: string; redirect_uri?: string } = { code };
    if (redirectUri !== undefined) {
      body.redirect_uri = redirectUri;
    }
    return this.request('POST', '/api/sync/oauth/callback', body);
  }

  async updateGcalSettings(
    body: UpdateGoogleCalSettings,
  ): Promise<GoogleCalSettings> {
    return this.request('PUT', '/api/sync/settings', body);
  }

  async triggerSync(): Promise<SyncTriggerResponse> {
    return this.request('POST', '/api/sync/trigger');
  }

  async deleteAllGcalEvents(): Promise<DeleteAllGcalResponse> {
    return this.request('POST', '/api/sync/delete-all');
  }

  async listGcalMappings(): Promise<GoogleCalEventMapping[]> {
    return this.request('GET', '/api/sync/mappings');
  }

  // ── Skills (#WI-6) ──
  async listSkills(): Promise<SkillRow[]> {
    return this.request('GET', '/api/skills');
  }

  async getSkill(slug: string): Promise<SkillRow> {
    return this.request('GET', `/api/skills/${encodeURIComponent(slug)}`);
  }

  async createSkill(body: CreateSkill): Promise<SkillRow> {
    return this.request('POST', '/api/skills', body);
  }

  async updateSkill(slug: string, body: UpdateSkill): Promise<SkillRow> {
    return this.request(
      'PATCH',
      `/api/skills/${encodeURIComponent(slug)}`,
      body,
    );
  }

  async deleteSkill(slug: string): Promise<void> {
    return this.request('DELETE', `/api/skills/${encodeURIComponent(slug)}`);
  }

  // ── Health ──
  async workerHealthCheck(): Promise<{ status: string }> {
    return this.request('GET', '/api/workers/health');
  }

  async updateWorkersConfig(body: {
    url: string;
    token: string;
  }): Promise<{ ok: boolean }> {
    return this.request('PUT', '/api/workers/config', body);
  }
}
