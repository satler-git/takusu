// Re-export all generated schema types.
export type components = import('./types.gen').components;
type S = components['schemas'];

// Direct re-exports (same name in Rust and TS).
export type TaskStatus = S['TaskStatus'];
export type TaskRow = S['TaskRow'];
export type CreateTask = S['CreateTask'];
export type UpdateTask = S['UpdateTask'];
export type Completion = S['Completion'];
export type HabitRow = S['HabitRow'];
export type CreateHabit = S['CreateHabit'];
export type UpdateHabit = S['UpdateHabit'];
export type HabitScheduledSpanRow = S['HabitScheduledSpanRow'];
export type CreateHabitScheduledSpan = S['CreateHabitScheduledSpan'];
export type HabitStepRow = S['HabitStepRow'];
export type HabitStepInput = S['HabitStepInput'];
export type HabitDetail = S['HabitDetail'];
export type HabitEstimateRequest = S['HabitEstimateRequest'];
export type HabitEstimateSample = S['HabitEstimateSample'];
export type HabitEstimateStep = S['HabitEstimateStep'];
export type HabitEstimateResult = S['HabitEstimateResult'];
export type HabitPreviewRequest = S['HabitPreviewRequest'];
export type HabitPreviewTask = S['HabitPreviewTask'];
export type ScheduleEntry = S['ScheduleEntry'];
export type ScheduleRow = S['ScheduleRow'];
export type GenerateSchedule = S['GenerateSchedule'];
export type SettingsRow = S['SettingsRow'];
export type UpdateSettings = S['UpdateSettings'];
export type TokenRow = S['TokenRow'];
export type TokenCreateResponse = S['TokenCreateResponse'];
export type SkillRow = S['SkillRow'];
export type CreateSkill = S['CreateSkill'];
export type UpdateSkill = S['UpdateSkill'];
export type DeleteAllGcalFailure = S['DeleteAllGcalFailure'];
export type IcalImportResult = S['IcalImportResult'];
export type DependencyNode = S['DependencyNode'];
export type RedundantDependency = S['RedundantDependency'];
export type DependencyAnalysisResponse = S['DependencyAnalysisResponse'];
export type StartWorkSession = S['StartWorkSession'];
export type AttachWorkSession = S['AttachWorkSession'];
export type ConvertWorkSession = S['ConvertWorkSession'];
export type RecordWorkSessionProgress = S['RecordWorkSessionProgress'];
export type WorkSessionRow = S['WorkSessionRow'];
export type WorkSessionProgressResult = S['WorkSessionProgressResult'];
export type ProgressEventRow = S['ProgressEventRow'];
export type SplitTask = S['SplitTask'];
export type SplitResult = S['SplitResult'];
export type SyncTriggerResponse = S['SyncTriggerResponse'];
export type OAuthCallbackResponse = S['OAuthCallbackResponse'];
export type WindowMode = S['WindowMode'];

// Aliases: old TS name → generated schema name.
export type TaskQuery = S['TaskQueryParams'];
export type RescheduleRequest = S['Reschedule'];
export type MoveEntryRequest = S['MoveEntry'];
export type MoveEntryResponse = S['MoveEntryResponse'];
export type GoogleCalSettings = S['GoogleCalSettingsOutput'];
export type DeleteAllGcalResponse = S['DeleteAllGcalResult'];
export type GoogleCalEventMapping = S['GoogleCalEventRow'];

// Re-export UpdateGoogleCalSettings (same name).
export type UpdateGoogleCalSettings = S['UpdateGoogleCalSettings'];

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
export const WINDOW_MODE_DAY = 'day' as const;
export const WINDOW_MODE_PERIOD = 'period' as const;

// Helper: parse schedule JSON string
export function parseSchedule(schedule: string): ScheduleEntry[] {
  try {
    const parsed = JSON.parse(schedule);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}
