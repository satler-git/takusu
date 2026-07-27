type TaskStatus = 'pending' | 'scheduled' | 'in_progress' | 'completed' | 'skipped';
interface TaskRow {
    id: string;
    display_id: number;
    title: string;
    description?: string;
    start_at?: string;
    end_at: string;
    avg_minutes: number;
    sigma_minutes: number;
    depends: string;
    parallelizable: boolean;
    allows_parallel: boolean;
    abandonability: number;
    status: TaskStatus;
    habit_id?: string;
    ical_uid?: string;
    user_edited: boolean;
    fixed: boolean;
    habit_step_id?: string;
    quantity_total?: number;
    quantity_done: number;
    quantity_unit?: string;
    completed_at?: string;
    split_from_task_id?: string;
    original_quantity_total?: number;
    actual_minutes?: number;
    created_at: string;
    updated_at: string;
}
interface CreateTask {
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
    quantity_total?: number;
    quantity_done?: number;
    quantity_unit?: string;
    original_quantity_total?: number;
}
interface UpdateTask {
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
    quantity_total?: number;
    quantity_done?: number;
    quantity_unit?: string;
    original_quantity_total?: number;
}
interface TaskQuery {
    status?: TaskStatus | 'overdue';
    from?: string;
    until?: string;
    no_overdue?: boolean;
    habit_id?: string;
    ical_uid?: string;
    q?: string;
    limit?: number;
}
interface Completion {
    value: string;
    label: string;
}
interface HabitRow {
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
interface CreateHabit {
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
interface UpdateHabit {
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
interface HabitScheduledSpanRow {
    id: string;
    habit_id: string;
    start_date: string;
    end_date: string;
    reason?: string;
    created_at: string;
}
interface CreateHabitScheduledSpan {
    start_date: string;
    end_date: string;
    reason?: string;
}
interface HabitStepRow {
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
    depends_on: string;
    created_at: string;
}
interface HabitStepInput {
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
interface HabitDetail extends HabitRow {
    steps: HabitStepRow[];
}
interface HabitEstimateRequest {
    detect_outliers?: boolean;
    apply?: boolean;
}
interface HabitEstimateSample {
    task_id: string;
    title: string;
    actual_minutes: number;
    excluded: boolean;
}
interface HabitEstimateStep {
    step_id: string;
    title: string;
    avg_minutes: number;
    sigma_minutes: number;
    sample_count: number;
    excluded_count: number;
    applied: boolean;
}
interface HabitEstimateResult {
    avg_minutes: number;
    sigma_minutes: number;
    sample_count: number;
    excluded_count: number;
    samples: HabitEstimateSample[];
    steps: HabitEstimateStep[];
    applied: boolean;
    habit?: HabitRow;
}
interface HabitPreviewRequest {
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
interface HabitPreviewTask {
    title: string;
    start_at: string;
    end_at: string;
}
interface ScheduleEntry {
    task_id: string;
    start_at: string;
    end_at: string;
}
interface ScheduleRow {
    id: string;
    created_at: string;
    updated_at: string;
    schedule: string;
}
interface GenerateSchedule {
    task_ids?: string[];
    sleep?: string;
}
interface RescheduleRequest {
    mode: 'range' | 'tasks';
    from?: string;
    until?: string;
    task_ids?: string[];
    pinned?: string[];
    sleep?: string;
}
interface MoveEntryRequest {
    start_at: string;
    force?: boolean;
}
interface MoveEntryResponse {
    task_id: string;
    start_at: string;
    end_at: string;
    warnings: string[];
}
interface SettingsRow {
    tz: string;
    sleep_start: string;
    sleep_end: string;
    comfortable_minutes: number | null;
    maximum_minutes: number | null;
    solver: string;
    time_budget_ms: number | null;
    seed: number | null;
    warm_start: boolean;
}
interface UpdateSettings {
    tz?: string;
    sleep_start?: string;
    sleep_end?: string;
    comfortable_minutes?: number | null;
    maximum_minutes?: number | null;
    solver?: string;
    time_budget_ms?: number | null;
    seed?: number | null;
    warm_start?: boolean;
}
interface TokenRow {
    id: number;
    jti: string;
    scope: string;
    label?: string | null;
    created_by: string;
    created_at: string;
    revoked_at?: string | null;
    expires_at?: string | null;
}
interface TokenCreateResponse {
    id: number;
    token: string;
    scope: string;
    label?: string | null;
    created_at: string;
    expires_at?: string | null;
}
interface GoogleCalSettings {
    enabled: boolean;
    calendar_id: string;
    client_id: string;
    has_client_secret: boolean;
    has_refresh_token: boolean;
}
interface UpdateGoogleCalSettings {
    enabled?: boolean;
    calendar_id?: string;
    client_id?: string;
    client_secret?: string;
    refresh_token?: string;
}
interface OAuthCallbackResponse {
    refresh_token_set: boolean;
}
interface SkillRow {
    slug: string;
    name: string;
    description: string;
    body: string;
    built_in: boolean;
    created_at: string;
    updated_at: string;
}
interface CreateSkill {
    slug: string;
    name: string;
    description: string;
    body: string;
    built_in?: boolean;
}
interface UpdateSkill {
    name?: string;
    description?: string;
    body?: string;
}
interface SyncTriggerResponse {
    status: string;
}
interface DeleteAllGcalFailure {
    task_id: string;
    error: string;
}
interface DeleteAllGcalResponse {
    deleted: number;
    failed: DeleteAllGcalFailure[];
}
interface GoogleCalEventMapping {
    task_id: string;
    google_event_id: string;
}
interface IcalImportResult {
    imported: number;
    task_ids: string[];
}
interface DependencyNode {
    id: string;
    title: string;
}
interface RedundantDependency {
    from: string;
    from_title: string;
    to: string;
    to_title: string;
    via: DependencyNode[];
}
interface DependencyAnalysisResponse {
    redundant: RedundantDependency[];
}
interface RecordProgress {
    quantity_done: number;
    note?: string;
}
interface ProgressEventRow {
    id: string;
    task_id: string;
    at: string;
    quantity_done?: number;
    delta_quantity?: number;
    active_minutes: number;
    note?: string;
}
interface ProgressResult {
    task: TaskRow;
    event?: ProgressEventRow;
    suggests_completion: boolean;
}
interface SplitTask {
    retained_quantity: number;
    set_dependency?: boolean;
    title?: string;
    description?: string;
    end_at?: string;
}
interface SplitResult {
    original: TaskRow;
    remainder: TaskRow;
}
declare function parseDepends(depends: string): string[];
declare function parseDependsOn(dependsOn: string): string[];
declare const WINDOW_MODE_DAY = "day";
declare const WINDOW_MODE_PERIOD = "period";
type WindowMode = typeof WINDOW_MODE_DAY | typeof WINDOW_MODE_PERIOD;
declare function parseSchedule(schedule: string): ScheduleEntry[];

declare class ApiError extends Error {
    status: number;
    body: string;
    constructor(status: number, body: string);
}
declare class TakusuClient {
    readonly baseUrl: string;
    private token;
    constructor(baseUrl: string, token: string);
    private request;
    health(signal?: AbortSignal): Promise<string>;
    listTasks(query?: TaskQuery): Promise<TaskRow[]>;
    completeTaskQuery(q: string, limit?: number): Promise<Completion[]>;
    getTask(id: string): Promise<TaskRow>;
    createTask(body: CreateTask): Promise<TaskRow>;
    updateTask(id: string, body: UpdateTask): Promise<TaskRow>;
    replaceTask(id: string, body: CreateTask): Promise<TaskRow>;
    deleteTask(id: string): Promise<void>;
    startTaskWork(id: string, operationId?: string): Promise<TaskRow>;
    pauseTaskWork(id: string, operationId?: string): Promise<TaskRow>;
    recordProgress(id: string, body: RecordProgress, operationId?: string): Promise<ProgressResult>;
    completeTaskWork(id: string, operationId?: string): Promise<TaskRow>;
    splitTask(id: string, body: SplitTask, operationId?: string): Promise<SplitResult>;
    analyzeTaskDependencies(): Promise<DependencyAnalysisResponse>;
    importIcal(icalText: string): Promise<IcalImportResult>;
    listHabits(): Promise<HabitRow[]>;
    getHabit(id: string): Promise<HabitDetail>;
    estimateHabit(id: string, body: HabitEstimateRequest): Promise<HabitEstimateResult>;
    createHabit(body: CreateHabit): Promise<HabitRow>;
    previewHabit(body: HabitPreviewRequest): Promise<HabitPreviewTask[]>;
    updateHabit(id: string, body: UpdateHabit): Promise<HabitRow>;
    replaceHabit(id: string, body: CreateHabit): Promise<HabitRow>;
    deleteHabit(id: string): Promise<void>;
    listHabitScheduledSpans(id: string): Promise<HabitScheduledSpanRow[]>;
    listAllHabitScheduledSpans(): Promise<HabitScheduledSpanRow[]>;
    createHabitScheduledSpan(id: string, body: CreateHabitScheduledSpan): Promise<HabitScheduledSpanRow>;
    deleteHabitScheduledSpan(id: string, spanId: string): Promise<void>;
    listHabitSteps(id: string): Promise<HabitStepRow[]>;
    listAllHabitSteps(): Promise<HabitStepRow[]>;
    replaceHabitSteps(id: string, steps: HabitStepInput[]): Promise<HabitStepRow[]>;
    analyzeHabitStepDependencies(id: string): Promise<DependencyAnalysisResponse>;
    getSchedule(): Promise<ScheduleRow>;
    generateSchedule(body: GenerateSchedule): Promise<ScheduleRow>;
    reschedule(body: RescheduleRequest): Promise<ScheduleRow>;
    moveEntry(taskId: string, body: MoveEntryRequest): Promise<MoveEntryResponse>;
    clearSchedule(): Promise<void>;
    getSettings(): Promise<SettingsRow>;
    updateSettings(body: UpdateSettings): Promise<SettingsRow>;
    listTokens(): Promise<TokenRow[]>;
    createToken(label?: string): Promise<TokenCreateResponse>;
    revokeToken(id: number): Promise<void>;
    getGcalSettings(): Promise<GoogleCalSettings>;
    oauthCallback(code: string, redirectUri?: string): Promise<OAuthCallbackResponse>;
    updateGcalSettings(body: UpdateGoogleCalSettings): Promise<GoogleCalSettings>;
    triggerSync(): Promise<SyncTriggerResponse>;
    deleteAllGcalEvents(): Promise<DeleteAllGcalResponse>;
    listGcalMappings(): Promise<GoogleCalEventMapping[]>;
    listSkills(): Promise<SkillRow[]>;
    getSkill(slug: string): Promise<SkillRow>;
    createSkill(body: CreateSkill): Promise<SkillRow>;
    updateSkill(slug: string, body: UpdateSkill): Promise<SkillRow>;
    deleteSkill(slug: string): Promise<void>;
    workerHealthCheck(): Promise<{
        status: string;
    }>;
    updateWorkersConfig(body: {
        url: string;
        token: string;
    }): Promise<{
        ok: boolean;
    }>;
}

export { ApiError, type Completion, type CreateHabit, type CreateHabitScheduledSpan, type CreateSkill, type CreateTask, type DeleteAllGcalFailure, type DeleteAllGcalResponse, type DependencyAnalysisResponse, type DependencyNode, type GenerateSchedule, type GoogleCalEventMapping, type GoogleCalSettings, type HabitDetail, type HabitEstimateRequest, type HabitEstimateResult, type HabitEstimateSample, type HabitEstimateStep, type HabitPreviewRequest, type HabitPreviewTask, type HabitRow, type HabitScheduledSpanRow, type HabitStepInput, type HabitStepRow, type IcalImportResult, type MoveEntryRequest, type MoveEntryResponse, type OAuthCallbackResponse, type ProgressEventRow, type ProgressResult, type RecordProgress, type RedundantDependency, type RescheduleRequest, type ScheduleEntry, type ScheduleRow, type SettingsRow, type SkillRow, type SplitResult, type SplitTask, type SyncTriggerResponse, TakusuClient, type TaskQuery, type TaskRow, type TaskStatus, type TokenCreateResponse, type TokenRow, type UpdateGoogleCalSettings, type UpdateHabit, type UpdateSettings, type UpdateSkill, type UpdateTask, WINDOW_MODE_DAY, WINDOW_MODE_PERIOD, type WindowMode, parseDepends, parseDependsOn, parseSchedule };
