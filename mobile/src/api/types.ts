// Types mirroring takusu-client/src/lib.rs and takusu-storage/src/model.rs

export type TaskStatus =
  | 'pending'
  | 'scheduled'
  | 'in_progress'
  | 'completed'
  | 'skipped';

export interface TaskRow {
  id: string;
  display_id: number;
  title: string;
  description?: string;
  start_at?: string;
  end_at: string;
  avg_minutes: number;
  sigma_minutes: number;
  depends: string; // JSON-encoded Vec<String>
  parallelizable: boolean;
  allows_parallel: boolean;
  abandonability: number;
  status: TaskStatus;
  habit_id?: string;
  ical_uid?: string;
  user_edited: boolean;
  fixed: boolean;
  habit_step_id?: string; // habit step that generated this task (#95)
  // WI-9: quantity fields
  quantity_total?: number;
  quantity_done: number;
  quantity_unit?: string;
  completed_at?: string;
  split_from_task_id?: string;
  original_quantity_total?: number;
  /// Total active work minutes from task_work_sessions (absent when no work has been done).
  actual_minutes?: number;
  created_at: string;
  updated_at: string;
}

export interface CreateTask {
  title: string;
  description?: string;
  start_at?: string;
  end_at: string;
  avg_minutes: number;
  sigma_minutes?: number;
  depends?: string[];
  parallelizable?: boolean;
  allows_parallel?: boolean;
  abandonability?: number;
  ical_uid?: string;
  habit_id?: string;
  fixed?: boolean;
  // WI-9: quantity fields
  quantity_total?: number;
  quantity_done?: number;
  quantity_unit?: string;
  original_quantity_total?: number;
}

export interface UpdateTask {
  title?: string;
  description?: string;
  start_at?: string;
  end_at?: string;
  avg_minutes?: number;
  sigma_minutes?: number;
  depends?: string[];
  parallelizable?: boolean;
  allows_parallel?: boolean;
  abandonability?: number;
  status?: TaskStatus;
  user_edited?: boolean;
  fixed?: boolean;
  // WI-9: quantity fields
  quantity_total?: number;
  quantity_done?: number;
  quantity_unit?: string;
  original_quantity_total?: number;
}

export interface TaskQuery {
  status?: TaskStatus | 'overdue';
  from?: string;
  until?: string;
  no_overdue?: boolean;
  habit_id?: string;
  ical_uid?: string;
  q?: string;
  limit?: number;
}

export interface Completion {
  value: string;
  label: string;
}

export interface HabitRow {
  id: string;
  display_id: number;
  title: string;
  description?: string;
  recurrence: string;
  start_time: string;
  end_time: string;
  avg_minutes: number;
  sigma_minutes: number;
  parallelizable: boolean;
  allows_parallel: boolean;
  abandonability: number;
  active: boolean;
  fixed: boolean;
  window_mode: string;
  created_at: string;
  updated_at: string;
}

export interface CreateHabit {
  title: string;
  description?: string;
  recurrence: string;
  start_time: string;
  end_time: string;
  avg_minutes: number;
  sigma_minutes?: number;
  parallelizable?: boolean;
  allows_parallel?: boolean;
  abandonability?: number;
  fixed?: boolean;
  window_mode?: string;
}

export interface UpdateHabit {
  title?: string;
  description?: string;
  recurrence?: string;
  start_time?: string;
  end_time?: string;
  avg_minutes?: number;
  sigma_minutes?: number;
  parallelizable?: boolean;
  allows_parallel?: boolean;
  abandonability?: number;
  active?: boolean;
  fixed?: boolean;
  window_mode?: string;
}

// ── Habit scheduled spans (#303 / #503) ──
// Effect depends on `habits.active`:
// - active habit: span dates suppress task generation (a pause).
// - disabled habit: span dates enable task generation (an activation window).
// start_date / end_date are inclusive 'YYYY-MM-DD' strings in the user's
// local timezone.

export interface HabitScheduledSpanRow {
  id: string;
  habit_id: string;
  start_date: string;
  end_date: string;
  reason?: string;
  created_at: string;
}

export interface CreateHabitScheduledSpan {
  start_date: string;
  end_date: string;
  reason?: string;
}

// ── Habit steps (#95) ──
// A step of a multi-step habit. Each step produces one task per
// occurrence with its own window / cost / flags. Steps form a DAG via
// depends_on (JSON array of step ids within the same habit).

export interface HabitStepRow {
  id: string;
  habit_id: string;
  position: number;
  title: string;
  description?: string;
  start_time: string;
  end_time: string;
  avg_minutes: number;
  sigma_minutes: number;
  parallelizable: boolean;
  allows_parallel: boolean;
  abandonability: number;
  fixed: boolean;
  depends_on: string; // JSON-encoded Vec<String>
  created_at: string;
}

export interface HabitStepInput {
  id?: string;
  position: number;
  title: string;
  description?: string;
  start_time: string;
  end_time: string;
  avg_minutes: number;
  sigma_minutes?: number;
  parallelizable?: boolean;
  allows_parallel?: boolean;
  abandonability?: number;
  fixed?: boolean;
  depends_on: string[];
}

// Habit detail response: habit fields + steps (#95). GET /api/habits/:id
// returns this shape (steps flattened alongside the habit fields).
export interface HabitDetail extends HabitRow {
  steps: HabitStepRow[];
}

// Habit estimate from completed task actuals (#919).
export interface HabitEstimateRequest {
  detect_outliers?: boolean;
  apply?: boolean;
}

export interface HabitEstimateSample {
  task_id: string;
  title: string;
  actual_minutes: number;
  excluded: boolean;
}

export interface HabitEstimateStep {
  step_id: string;
  title: string;
  avg_minutes: number;
  sigma_minutes: number;
  sample_count: number;
  excluded_count: number;
  applied: boolean;
}

export interface HabitEstimateResult {
  avg_minutes: number;
  sigma_minutes: number;
  sample_count: number;
  excluded_count: number;
  samples: HabitEstimateSample[];
  steps: HabitEstimateStep[];
  applied: boolean;
  habit?: HabitRow;
}

// ── Habit preview (#1033) ──

export interface HabitPreviewRequest {
  title: string;
  description?: string;
  recurrence: string;
  start_time: string;
  end_time: string;
  avg_minutes: number;
  sigma_minutes?: number;
  parallelizable?: boolean;
  allows_parallel?: boolean;
  abandonability?: number;
  fixed?: boolean;
  window_mode?: string;
  steps?: HabitStepInput[];
  from?: string;
  until?: string;
  max_occurrences?: number;
}

export interface HabitPreviewTask {
  title: string;
  start_at: string;
  end_at: string;
}

export interface ScheduleEntry {
  task_id: string;
  start_at: string;
  end_at: string;
}

export interface ScheduleRow {
  id: string;
  created_at: string;
  updated_at: string;
  schedule: string; // JSON-encoded ScheduleEntry[]
}

export interface GenerateSchedule {
  task_ids?: string[];
  sleep?: string;
}

export interface RescheduleRequest {
  mode: 'range' | 'tasks';
  from?: string;
  until?: string;
  task_ids?: string[];
  pinned?: string[];
  sleep?: string;
}

export interface MoveEntryRequest {
  start_at: string;
  force?: boolean;
}

export interface MoveEntryResponse {
  task_id: string;
  start_at: string;
  end_at: string;
  warnings: string[];
}

export interface SettingsRow {
  tz: string;
  sleep_start: string;
  sleep_end: string;
  /// #459: 1 日の快適な作業時間（分）。`null` または未設定の場合はデフォルト（8 時間）を使う。
  comfortable_minutes: number | null;
  /// #459: 1 日の最大作業時間（分）。`null` または未設定の場合はデフォルト（12 時間）を使う。
  maximum_minutes: number | null;
  /// #789: 使用する solver。`"sa"` / `"priority"` / `"auto"`。空または不明な場合は `"sa"`。
  solver: string;
  /// #789: 求解時間の上限（ミリ秒）。`null` または `0` の場合は制限なし。
  time_budget_ms: number | null;
  /// #789: 乱数シード。`null` の場合は決定的なデフォルト。
  seed: number | null;
  /// #789: 前回スケジュールから priority/ALNS の初期解を warm start する。
  warm_start: boolean;
}

export interface UpdateSettings {
  tz?: string;
  sleep_start?: string;
  sleep_end?: string;
  /// #459: 1 日の快適な作業時間（分）。`null` または未設定の場合はデフォルトを使う。
  comfortable_minutes?: number | null;
  /// #459: 1 日の最大作業時間（分）。`null` または未設定の場合はデフォルトを使う。
  maximum_minutes?: number | null;
  /// #789: 使用する solver。`"sa"` / `"priority"` / `"auto"`。
  solver?: string;
  /// #789: 求解時間の上限（ミリ秒）。`null` または `0` で制限なし。
  time_budget_ms?: number | null;
  /// #789: 乱数シード。`null` でデフォルト。
  seed?: number | null;
  /// #789: 前回スケジュールから priority/ALNS の初期解を warm start する。
  warm_start?: boolean;
}

export interface TokenRow {
  id: number;
  jti: string;
  scope: string;
  label?: string | null;
  created_by: string;
  created_at: string;
  revoked_at?: string | null;
  expires_at?: string | null;
}

export interface TokenCreateResponse {
  id: number;
  token: string;
  scope: string;
  label?: string | null;
  created_at: string;
  expires_at?: string | null;
}

// ── Google Calendar Sync ──

export interface GoogleCalSettings {
  enabled: boolean;
  calendar_id: string;
  client_id: string;
  has_client_secret: boolean;
  has_refresh_token: boolean;
}

export interface UpdateGoogleCalSettings {
  enabled?: boolean;
  calendar_id?: string;
  client_id?: string;
  client_secret?: string;
  refresh_token?: string;
}

export interface OAuthCallbackResponse {
  refresh_token_set: boolean;
}

// ── Skills (#WI-6) ──

export interface SkillRow {
  slug: string;
  name: string;
  description: string;
  body: string;
  built_in: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreateSkill {
  slug: string;
  name: string;
  description: string;
  body: string;
  built_in?: boolean;
}

export interface UpdateSkill {
  name?: string;
  description?: string;
  body?: string;
}

export interface SyncTriggerResponse {
  status: string;
}

export interface DeleteAllGcalFailure {
  task_id: string;
  error: string;
}

export interface DeleteAllGcalResponse {
  deleted: number;
  failed: DeleteAllGcalFailure[];
}

export interface GoogleCalEventMapping {
  task_id: string;
  google_event_id: string;
}

// ── iCal Import ──

export interface IcalImportResult {
  imported: number;
  task_ids: string[];
}

// ── Composite (redundant) dependency detection (#355) ──
// A redundant edge is a direct dependency that is already implied by a
// longer path (transitive reduction). `via` is the witness path including
// both endpoints (length >= 3).

export interface DependencyNode {
  id: string;
  title: string;
}

export interface RedundantDependency {
  from: string;
  from_title: string;
  to: string;
  to_title: string;
  via: DependencyNode[];
}

export interface DependencyAnalysisResponse {
  redundant: RedundantDependency[];
}

// ── Task progress (#757) ──

export interface RecordProgress {
  quantity_done: number;
  note?: string;
}

export interface ProgressEventRow {
  id: string;
  task_id: string;
  at: string;
  quantity_done?: number;
  delta_quantity?: number;
  active_minutes: number;
  note?: string;
}

export interface ProgressResult {
  task: TaskRow;
  event?: ProgressEventRow;
  suggests_completion: boolean;
}

export interface SplitTask {
  retained_quantity: number;
  set_dependency?: boolean;
  title?: string;
  description?: string;
  end_at?: string;
}

export interface SplitResult {
  original: TaskRow;
  remainder: TaskRow;
}

// Helper: parse depends JSON string
export function parseDepends(depends: string): string[] {
  try {
    const parsed = JSON.parse(depends);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

// Helper: parse depends_on JSON string (habit steps, #95)
export function parseDependsOn(dependsOn: string): string[] {
  try {
    const parsed = JSON.parse(dependsOn);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

// window_mode values for habits (#window_mode)
export const WINDOW_MODE_DAY = 'day';
export const WINDOW_MODE_PERIOD = 'period';
export type WindowMode = typeof WINDOW_MODE_DAY | typeof WINDOW_MODE_PERIOD;

// Helper: parse schedule JSON string
export function parseSchedule(schedule: string): ScheduleEntry[] {
  try {
    const parsed = JSON.parse(schedule);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}
