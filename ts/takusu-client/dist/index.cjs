"use strict";
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __export = (target, all) => {
  for (var name in all)
    __defProp(target, name, { get: all[name], enumerable: true });
};
var __copyProps = (to, from, except, desc) => {
  if (from && typeof from === "object" || typeof from === "function") {
    for (let key of __getOwnPropNames(from))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
  }
  return to;
};
var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

// src/index.ts
var index_exports = {};
__export(index_exports, {
  ApiError: () => ApiError,
  TakusuClient: () => TakusuClient,
  WINDOW_MODE_DAY: () => WINDOW_MODE_DAY,
  WINDOW_MODE_PERIOD: () => WINDOW_MODE_PERIOD,
  parseDepends: () => parseDepends,
  parseDependsOn: () => parseDependsOn,
  parseSchedule: () => parseSchedule
});
module.exports = __toCommonJS(index_exports);

// src/types.ts
function parseDepends(depends) {
  try {
    const parsed = JSON.parse(depends);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}
function parseDependsOn(dependsOn) {
  try {
    const parsed = JSON.parse(dependsOn);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}
var WINDOW_MODE_DAY = "day";
var WINDOW_MODE_PERIOD = "period";
function parseSchedule(schedule) {
  try {
    const parsed = JSON.parse(schedule);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

// src/client.ts
var ApiError = class extends Error {
  constructor(status, body) {
    super(`API error ${status}: ${body}`);
    this.status = status;
    this.body = body;
    this.name = "ApiError";
  }
  status;
  body;
};
var TakusuClient = class {
  baseUrl;
  token;
  constructor(baseUrl, token) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.token = token;
  }
  async request(method, path, body, operationId) {
    const url = `${this.baseUrl}${path}`;
    const headers = {
      Authorization: `Bearer ${this.token}`
    };
    if (body !== void 0) {
      headers["Content-Type"] = "application/json";
    }
    if (operationId !== void 0 && operationId !== "") {
      headers["Idempotency-Key"] = operationId;
    }
    const resp = await fetch(url, {
      method,
      headers,
      body: body !== void 0 ? JSON.stringify(body) : void 0
    });
    const status = resp.status;
    if (status >= 400) {
      const text2 = await resp.text().catch(() => "");
      throw new ApiError(status, text2);
    }
    const text = await resp.text();
    if (!text) return void 0;
    return JSON.parse(text);
  }
  // ── Health ──
  async health() {
    const resp = await fetch(`${this.baseUrl}/health`);
    return resp.text();
  }
  // ── Task ──
  async listTasks(query) {
    const params = [];
    if (query?.status)
      params.push(`status=${encodeURIComponent(query.status)}`);
    if (query?.from) params.push(`from=${encodeURIComponent(query.from)}`);
    if (query?.until) params.push(`until=${encodeURIComponent(query.until)}`);
    if (query?.no_overdue !== void 0)
      params.push(`no_overdue=${query.no_overdue}`);
    if (query?.habit_id)
      params.push(`habit_id=${encodeURIComponent(query.habit_id)}`);
    if (query?.ical_uid)
      params.push(`ical_uid=${encodeURIComponent(query.ical_uid)}`);
    if (query?.q) params.push(`q=${encodeURIComponent(query.q)}`);
    if (query?.limit !== void 0)
      params.push(`limit=${encodeURIComponent(query.limit.toString())}`);
    const qs = params.join("&");
    return this.request("GET", `/api/tasks${qs ? `?${qs}` : ""}`);
  }
  async completeTaskQuery(q, limit) {
    const limitParam = limit !== void 0 ? `&limit=${encodeURIComponent(limit.toString())}` : "";
    return this.request(
      "GET",
      `/api/tasks/complete?q=${encodeURIComponent(q)}${limitParam}`
    );
  }
  async getTask(id) {
    return this.request("GET", `/api/tasks/${encodeURIComponent(id)}`);
  }
  async createTask(body) {
    return this.request("POST", "/api/tasks", body);
  }
  async updateTask(id, body) {
    return this.request("PATCH", `/api/tasks/${encodeURIComponent(id)}`, body);
  }
  async replaceTask(id, body) {
    return this.request("PUT", `/api/tasks/${encodeURIComponent(id)}`, body);
  }
  async deleteTask(id) {
    return this.request("DELETE", `/api/tasks/${encodeURIComponent(id)}`);
  }
  // ── Task progress (#757) ──
  async startTaskWork(id, operationId) {
    return this.request(
      "POST",
      `/api/tasks/${encodeURIComponent(id)}/work/start`,
      void 0,
      operationId
    );
  }
  async pauseTaskWork(id, operationId) {
    return this.request(
      "POST",
      `/api/tasks/${encodeURIComponent(id)}/work/pause`,
      void 0,
      operationId
    );
  }
  async recordProgress(id, body, operationId) {
    return this.request(
      "POST",
      `/api/tasks/${encodeURIComponent(id)}/progress`,
      body,
      operationId
    );
  }
  async completeTaskWork(id, operationId) {
    return this.request(
      "POST",
      `/api/tasks/${encodeURIComponent(id)}/work/complete`,
      void 0,
      operationId
    );
  }
  async splitTask(id, body, operationId) {
    return this.request(
      "POST",
      `/api/tasks/${encodeURIComponent(id)}/split`,
      body,
      operationId
    );
  }
  // ── Composite dependency analysis (#355) ──
  async analyzeTaskDependencies() {
    return this.request("GET", "/api/tasks/dependency-analysis");
  }
  async importIcal(icalText) {
    const url = `${this.baseUrl}/api/tasks/import/ical`;
    const resp = await fetch(url, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${this.token}`,
        "Content-Type": "text/plain"
      },
      body: icalText
    });
    const status = resp.status;
    if (status >= 400) {
      const text2 = await resp.text().catch(() => "");
      throw new ApiError(status, text2);
    }
    const text = await resp.text();
    if (!text) return { imported: 0, task_ids: [] };
    return JSON.parse(text);
  }
  // ── Habit ──
  async listHabits() {
    return this.request("GET", "/api/habits");
  }
  async getHabit(id) {
    return this.request("GET", `/api/habits/${encodeURIComponent(id)}`);
  }
  async estimateHabit(id, body) {
    return this.request(
      "POST",
      `/api/habits/${encodeURIComponent(id)}/estimate`,
      body
    );
  }
  async createHabit(body) {
    return this.request("POST", "/api/habits", body);
  }
  async previewHabit(body) {
    return this.request("POST", "/api/habits/preview", body);
  }
  async updateHabit(id, body) {
    return this.request("PATCH", `/api/habits/${encodeURIComponent(id)}`, body);
  }
  async replaceHabit(id, body) {
    return this.request("PUT", `/api/habits/${encodeURIComponent(id)}`, body);
  }
  async deleteHabit(id) {
    return this.request("DELETE", `/api/habits/${encodeURIComponent(id)}`);
  }
  // ── Habit scheduled spans (#303 / #503) ──
  async listHabitScheduledSpans(id) {
    return this.request(
      "GET",
      `/api/habits/${encodeURIComponent(id)}/scheduled-spans`
    );
  }
  async listAllHabitScheduledSpans() {
    return this.request("GET", "/api/habits/scheduled-spans");
  }
  async createHabitScheduledSpan(id, body) {
    return this.request(
      "POST",
      `/api/habits/${encodeURIComponent(id)}/scheduled-spans`,
      body
    );
  }
  async deleteHabitScheduledSpan(id, spanId) {
    return this.request(
      "DELETE",
      `/api/habits/${encodeURIComponent(id)}/scheduled-spans/${encodeURIComponent(spanId)}`
    );
  }
  // ── Habit steps (#95) ──
  async listHabitSteps(id) {
    return this.request("GET", `/api/habits/${encodeURIComponent(id)}/steps`);
  }
  async listAllHabitSteps() {
    return this.request("GET", "/api/habits/steps");
  }
  async replaceHabitSteps(id, steps) {
    return this.request(
      "PUT",
      `/api/habits/${encodeURIComponent(id)}/steps`,
      steps
    );
  }
  async analyzeHabitStepDependencies(id) {
    return this.request(
      "GET",
      `/api/habits/${encodeURIComponent(id)}/steps/dependency-analysis`
    );
  }
  // ── Schedule ──
  async getSchedule() {
    return this.request("GET", "/api/schedule");
  }
  async generateSchedule(body) {
    return this.request("POST", "/api/schedule/generate", body);
  }
  async reschedule(body) {
    return this.request("POST", "/api/schedule/reschedule", body);
  }
  async moveEntry(taskId, body) {
    const raw = await this.request(
      "PATCH",
      `/api/schedule/entries/${encodeURIComponent(taskId)}`,
      body
    );
    if (!raw.task_id || !raw.start_at || !raw.end_at) {
      throw new ApiError(0, "invalid move entry response from server");
    }
    return {
      task_id: raw.task_id,
      start_at: raw.start_at,
      end_at: raw.end_at,
      warnings: raw.warnings ?? []
    };
  }
  async clearSchedule() {
    return this.request("DELETE", "/api/schedule");
  }
  // ── Settings ──
  async getSettings() {
    return this.request("GET", "/api/settings");
  }
  async updateSettings(body) {
    return this.request("PUT", "/api/settings", body);
  }
  // ── Token ──
  async listTokens() {
    return this.request("GET", "/api/tokens");
  }
  async createToken(label) {
    return this.request("POST", "/api/tokens", { label });
  }
  async revokeToken(id) {
    if (!Number.isFinite(id) || !Number.isInteger(id) || id <= 0) {
      throw new ApiError(400, "token id must be a positive integer");
    }
    return this.request("DELETE", `/api/tokens/${encodeURIComponent(id)}`);
  }
  // ── Sync / Google Calendar ──
  async getGcalSettings() {
    return this.request("GET", "/api/sync/settings");
  }
  async oauthCallback(code, redirectUri) {
    const body = { code };
    if (redirectUri !== void 0) {
      body.redirect_uri = redirectUri;
    }
    return this.request("POST", "/api/sync/oauth/callback", body);
  }
  async updateGcalSettings(body) {
    return this.request("PUT", "/api/sync/settings", body);
  }
  async triggerSync() {
    return this.request("POST", "/api/sync/trigger");
  }
  async deleteAllGcalEvents() {
    return this.request("POST", "/api/sync/delete-all");
  }
  async listGcalMappings() {
    return this.request("GET", "/api/sync/mappings");
  }
  // ── Skills (#WI-6) ──
  async listSkills() {
    return this.request("GET", "/api/skills");
  }
  async getSkill(slug) {
    return this.request("GET", `/api/skills/${encodeURIComponent(slug)}`);
  }
  async createSkill(body) {
    return this.request("POST", "/api/skills", body);
  }
  async updateSkill(slug, body) {
    return this.request(
      "PATCH",
      `/api/skills/${encodeURIComponent(slug)}`,
      body
    );
  }
  async deleteSkill(slug) {
    return this.request("DELETE", `/api/skills/${encodeURIComponent(slug)}`);
  }
  // ── Health ──
  async workerHealthCheck() {
    return this.request("GET", "/api/workers/health");
  }
  async updateWorkersConfig(body) {
    return this.request("PUT", "/api/workers/config", body);
  }
};
// Annotate the CommonJS export names for ESM import in node:
0 && (module.exports = {
  ApiError,
  TakusuClient,
  WINDOW_MODE_DAY,
  WINDOW_MODE_PERIOD,
  parseDepends,
  parseDependsOn,
  parseSchedule
});
//# sourceMappingURL=index.cjs.map