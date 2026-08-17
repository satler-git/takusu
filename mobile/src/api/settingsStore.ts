// Persistent settings store.
// Sensitive values (Workers URL, token) use expo-secure-store.
// Non-sensitive values (theme, notification settings) use AsyncStorage.

import * as SecureStore from 'expo-secure-store';
import AsyncStorage from '@react-native-async-storage/async-storage';
import { Appearance } from 'react-native';
import { type AppTheme, APP_THEMES } from '@/src/theme';
import {
  type NotificationSettings,
  DEFAULT_NOTIFICATION_SETTINGS,
  loadNotificationSettings,
  saveNotificationSettings,
} from '@/src/notifications/settings';
import { DEFAULT_MAX_HISTORY } from './undoRedo';

export const AGENT_SESSION_HISTORY_DEFAULT = 4;
export const AGENT_SESSION_HISTORY_MIN = 3;
export const AGENT_SESSION_HISTORY_MAX = 5;

const KEYS = {
  workersUrl: 'takusu.workersUrl',
  workersToken: 'takusu.workersToken',
  theme: 'takusu.theme',
  undoSteps: 'takusu.undoSteps',
  agentSessionHistoryCount: 'takusu.agent.sessionHistoryCount',
  llmProviders: 'takusu.agent.llmProviders',
  llmModels: 'takusu.agent.llmModels',
  activeLlmModel: 'takusu.agent.activeLlmModel',
  ttsProviders: 'takusu.agent.ttsProviders',
  activeTtsProvider: 'takusu.agent.activeTtsProvider',
  ttsMuted: 'takusu.agent.ttsMuted',
  welcomeShownAt: 'takusu.welcomeShownAt',
} as const;

export type PermissionsMap = Record<string, boolean>;

export interface LlmProvider {
  id: string;
  name: string;
  baseUrl: string;
}

export interface LlmModelSettings {
  id: string;
  name: string;
  providerId: string;
  selectedModel: string;
  cachedModels: string[];
  modelsFetchedAt?: string;
  cost?: string;
  permissions?: PermissionsMap;
}

// Legacy combined provider+model settings kept for migration.
interface LegacyLlmProviderSettings {
  id: string;
  name: string;
  baseUrl: string;
  selectedModel: string;
  cachedModels: string[];
  modelsFetchedAt?: string;
  cost?: string;
  permissions?: PermissionsMap;
}

export type TtsProvider = 'cartesia' | 'android' | 'fish';

const VALID_TTS_PROVIDERS: TtsProvider[] = ['cartesia', 'android', 'fish'];

export const TTS_PROVIDER_LABELS: Record<TtsProvider, string> = {
  cartesia: 'Cartesia',
  android: 'Android 標準 TTS',
  fish: 'Fish Audio',
};

export interface TtsProviderSettings {
  id: string;
  name: string;
  provider: TtsProvider;
  voiceId: string;
  model?: string;
  language: string;
  sampleRate: number;
  speed?: number;
}

export interface PersistedSettings {
  workersUrl: string;
  workersToken: string;
  theme: AppTheme;
  undoSteps: number;
  agentSessionHistoryCount: number;
  notifications: NotificationSettings;
  llmProviders: LlmProvider[];
  llmModels: LlmModelSettings[];
  activeLlmModel: string | null;
  ttsProviders: TtsProviderSettings[];
  activeTtsProvider: string | null;
}

function isValidTheme(value: string | null): value is AppTheme {
  return value !== null && APP_THEMES.includes(value as AppTheme);
}

const LEGACY_DARK_MODE_KEY = 'takusu.darkMode';
const LEGACY_ACTIVE_LLM_PROVIDER_KEY = 'takusu.agent.activeLlmProvider';

export function systemInitialTheme(): AppTheme {
  return Appearance.getColorScheme() === 'dark' ? 'dark' : 'light';
}

// Resolve the active app theme from persisted storage, falling back to the
// system color scheme. This is the source of truth used by both the UI and
// notification icon tinting.
export async function loadTheme(): Promise<AppTheme> {
  const themeStr = await AsyncStorage.getItem(KEYS.theme);
  if (isValidTheme(themeStr)) {
    return themeStr;
  }

  const darkModeStr = await AsyncStorage.getItem(LEGACY_DARK_MODE_KEY);
  if (darkModeStr != null) {
    // Migrate legacy darkMode boolean to theme string and remove the old key.
    const theme = darkModeStr === 'true' ? 'dark' : 'light';
    try {
      await Promise.all([
        saveTheme(theme),
        AsyncStorage.removeItem(LEGACY_DARK_MODE_KEY),
      ]);
    } catch {
      // ignore migration cleanup failures
    }
    return theme;
  }

  return systemInitialTheme();
}

export function newId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
}

function isLlmProvider(value: unknown): value is LlmProvider {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as Record<string, unknown>).id === 'string' &&
    typeof (value as Record<string, unknown>).name === 'string' &&
    typeof (value as Record<string, unknown>).baseUrl === 'string' &&
    !('selectedModel' in value)
  );
}

function isLegacyLlmProvider(
  value: unknown,
): value is LegacyLlmProviderSettings {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as Record<string, unknown>).id === 'string' &&
    typeof (value as Record<string, unknown>).name === 'string' &&
    typeof (value as Record<string, unknown>).baseUrl === 'string' &&
    typeof (value as Record<string, unknown>).selectedModel === 'string' &&
    Array.isArray((value as Record<string, unknown>).cachedModels)
  );
}

function migrateLegacyLlmProviders(legacy: LegacyLlmProviderSettings[]): {
  providers: LlmProvider[];
  models: LlmModelSettings[];
  providerToModel: Record<string, string>;
} {
  const providers: LlmProvider[] = [];
  const models: LlmModelSettings[] = [];
  const providerToModel: Record<string, string> = {};
  for (const old of legacy) {
    providers.push({
      id: old.id,
      name: old.name,
      baseUrl: old.baseUrl,
    });
    const modelId = newId('llm-model');
    providerToModel[old.id] = modelId;
    models.push({
      id: modelId,
      name: old.selectedModel ? `${old.name} (${old.selectedModel})` : old.name,
      providerId: old.id,
      selectedModel: old.selectedModel,
      cachedModels: old.cachedModels,
      modelsFetchedAt: old.modelsFetchedAt,
      cost: old.cost,
      permissions: old.permissions,
    });
  }
  return { providers, models, providerToModel };
}

export async function loadSettings(): Promise<PersistedSettings> {
  const [
    workersUrl,
    workersToken,
    theme,
    undoStepsStr,
    agentSessionHistoryCountStr,
    notifications,
    llmProvidersStr,
    llmModelsStr,
    activeLlmModel,
    ttsProvidersStr,
    activeTtsProvider,
    legacyActiveLlmProvider,
  ] = await Promise.all([
    SecureStore.getItemAsync(KEYS.workersUrl),
    SecureStore.getItemAsync(KEYS.workersToken),
    loadTheme(),
    AsyncStorage.getItem(KEYS.undoSteps),
    AsyncStorage.getItem(KEYS.agentSessionHistoryCount),
    loadNotificationSettings(),
    AsyncStorage.getItem(KEYS.llmProviders),
    AsyncStorage.getItem(KEYS.llmModels),
    AsyncStorage.getItem(KEYS.activeLlmModel),
    AsyncStorage.getItem(KEYS.ttsProviders),
    AsyncStorage.getItem(KEYS.activeTtsProvider),
    AsyncStorage.getItem(LEGACY_ACTIVE_LLM_PROVIDER_KEY),
  ]);
  const parsedUndoSteps = undoStepsStr ? parseInt(undoStepsStr, 10) : NaN;
  const parsedSessionCount = agentSessionHistoryCountStr
    ? parseInt(agentSessionHistoryCountStr, 10)
    : NaN;

  const parsedLlmModels = parseJsonArray<LlmModelSettings>(llmModelsStr);
  const rawLlmProviders = parseJsonArray<unknown>(llmProvidersStr);

  let llmProviders: LlmProvider[] = [];
  let llmModels: LlmModelSettings[] = [];
  let activeLlmModelId: string | null = activeLlmModel ?? null;

  if (parsedLlmModels.length > 0) {
    // New split format is already stored.
    llmModels = parsedLlmModels;
    llmProviders = rawLlmProviders.filter(isLlmProvider);
  } else if (
    rawLlmProviders.length > 0 &&
    rawLlmProviders.every(isLegacyLlmProvider)
  ) {
    // Legacy combined provider+model settings; migrate on first load.
    const migrated = migrateLegacyLlmProviders(rawLlmProviders);
    llmProviders = migrated.providers;
    llmModels = migrated.models;
    if (legacyActiveLlmProvider) {
      activeLlmModelId =
        migrated.providerToModel[legacyActiveLlmProvider] ?? null;
    }
  } else if (rawLlmProviders.every(isLlmProvider)) {
    llmProviders = rawLlmProviders;
  }

  return {
    workersUrl: workersUrl ?? '',
    workersToken: workersToken ?? '',
    theme,
    undoSteps:
      Number.isFinite(parsedUndoSteps) && parsedUndoSteps > 0
        ? parsedUndoSteps
        : DEFAULT_MAX_HISTORY,
    agentSessionHistoryCount:
      Number.isFinite(parsedSessionCount) &&
      parsedSessionCount >= AGENT_SESSION_HISTORY_MIN &&
      parsedSessionCount <= AGENT_SESSION_HISTORY_MAX
        ? parsedSessionCount
        : AGENT_SESSION_HISTORY_DEFAULT,
    notifications,
    llmProviders,
    llmModels,
    activeLlmModel: activeLlmModelId,
    ttsProviders: parseTtsProviders(ttsProvidersStr),
    activeTtsProvider: activeTtsProvider ?? null,
  };
}

function parseJsonArray<T>(value: string | null): T[] {
  if (!value) return [];
  try {
    const parsed: unknown = JSON.parse(value);
    return Array.isArray(parsed) ? (parsed as T[]) : [];
  } catch {
    return [];
  }
}

function isValidTtsProvider(value: unknown): value is TtsProvider {
  return (
    typeof value === 'string' &&
    VALID_TTS_PROVIDERS.includes(value as TtsProvider)
  );
}

// Internal helper exposed for testing. Normal callers should use loadSettings().
export function parseTtsProviders(value: string | null): TtsProviderSettings[] {
  const parsed = parseJsonArray<unknown>(value);
  const result: TtsProviderSettings[] = [];
  for (const item of parsed) {
    if (item == null || typeof item !== 'object') continue;
    const raw = item as Record<string, unknown>;
    if (typeof raw.id !== 'string' || !raw.id) continue;
    const provider = isValidTtsProvider(raw.provider)
      ? raw.provider
      : 'cartesia';
    const sampleRate =
      typeof raw.sampleRate === 'number' &&
      Number.isFinite(raw.sampleRate) &&
      raw.sampleRate > 0
        ? raw.sampleRate
        : 44100;
    const speed =
      typeof raw.speed === 'number' &&
      Number.isFinite(raw.speed) &&
      raw.speed > 0
        ? raw.speed
        : undefined;
    result.push({
      id: raw.id,
      name:
        typeof raw.name === 'string' ? raw.name : TTS_PROVIDER_LABELS[provider],
      provider,
      voiceId: typeof raw.voiceId === 'string' ? raw.voiceId : '',
      model: typeof raw.model === 'string' ? raw.model : undefined,
      language: typeof raw.language === 'string' ? raw.language : 'ja',
      sampleRate,
      speed,
    });
  }
  return result;
}

export async function saveAgentProviders(
  llmProviders: LlmProvider[],
  llmModels: LlmModelSettings[],
  activeLlmModel: string | null,
  ttsProviders: TtsProviderSettings[],
  activeTtsProvider: string | null,
): Promise<void> {
  await Promise.all([
    AsyncStorage.setItem(KEYS.llmProviders, JSON.stringify(llmProviders)),
    AsyncStorage.setItem(KEYS.llmModels, JSON.stringify(llmModels)),
    AsyncStorage.setItem(KEYS.activeLlmModel, activeLlmModel ?? ''),
    AsyncStorage.setItem(KEYS.ttsProviders, JSON.stringify(ttsProviders)),
    AsyncStorage.setItem(KEYS.activeTtsProvider, activeTtsProvider ?? ''),
  ]);
}

export async function loadAgentApiKey(
  kind: 'llm' | 'tts',
  providerId: string,
): Promise<string> {
  return (
    (await SecureStore.getItemAsync(
      `takusu.agent.${kind}.apiKey.${providerId}`,
    )) ?? ''
  );
}

export async function saveAgentApiKey(
  kind: 'llm' | 'tts',
  providerId: string,
  apiKey: string,
): Promise<void> {
  const key = `takusu.agent.${kind}.apiKey.${providerId}`;
  if (apiKey) await SecureStore.setItemAsync(key, apiKey);
  else await SecureStore.deleteItemAsync(key);
}

export async function deleteAgentApiKey(
  kind: 'llm' | 'tts',
  providerId: string,
): Promise<void> {
  await SecureStore.deleteItemAsync(
    `takusu.agent.${kind}.apiKey.${providerId}`,
  );
}

export async function saveWorkersUrl(url: string): Promise<void> {
  if (url) {
    await SecureStore.setItemAsync(KEYS.workersUrl, url);
  } else {
    await SecureStore.deleteItemAsync(KEYS.workersUrl);
  }
}

export async function saveWorkersToken(token: string): Promise<void> {
  if (token) {
    await SecureStore.setItemAsync(KEYS.workersToken, token);
  } else {
    await SecureStore.deleteItemAsync(KEYS.workersToken);
  }
}

export async function saveTheme(theme: AppTheme): Promise<void> {
  await AsyncStorage.setItem(KEYS.theme, theme);
}

export async function saveUndoSteps(steps: number): Promise<void> {
  await AsyncStorage.setItem(KEYS.undoSteps, String(steps));
}

export async function saveAgentSessionHistoryCount(
  count: number,
): Promise<void> {
  const clamped = Math.max(
    AGENT_SESSION_HISTORY_MIN,
    Math.min(AGENT_SESSION_HISTORY_MAX, count),
  );
  await AsyncStorage.setItem(KEYS.agentSessionHistoryCount, String(clamped));
}

export async function loadTtsMuted(): Promise<boolean> {
  const value = await AsyncStorage.getItem(KEYS.ttsMuted);
  return value === 'true';
}

export async function saveTtsMuted(muted: boolean): Promise<void> {
  await AsyncStorage.setItem(KEYS.ttsMuted, muted ? 'true' : 'false');
}

export async function loadWelcomeShownAt(): Promise<number | null> {
  const value = await AsyncStorage.getItem(KEYS.welcomeShownAt);
  if (!value) return null;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : null;
}

export async function saveWelcomeShownAt(at: number): Promise<void> {
  await AsyncStorage.setItem(KEYS.welcomeShownAt, String(at));
}

export { saveNotificationSettings, DEFAULT_NOTIFICATION_SETTINGS };
export type { NotificationSettings };
