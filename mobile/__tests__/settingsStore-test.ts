jest.mock('@react-native-async-storage/async-storage', () => ({
  __esModule: true,
  default: {
    getItem: jest.fn(),
    setItem: jest.fn(),
    removeItem: jest.fn(),
  },
}));

jest.mock('expo-secure-store', () => ({
  getItemAsync: jest.fn(),
  setItemAsync: jest.fn(),
  deleteItemAsync: jest.fn(),
}));

jest.mock('@/src/notifications/settings', () => ({
  DEFAULT_NOTIFICATION_SETTINGS: {},
  loadNotificationSettings: jest.fn(),
  saveNotificationSettings: jest.fn(),
}));

import AsyncStorage from '@react-native-async-storage/async-storage';
import * as SecureStore from 'expo-secure-store';
import { Appearance } from 'react-native';
import { loadNotificationSettings } from '@/src/notifications/settings';
import {
  loadSettings,
  loadTheme,
  loadWelcomeShownAt,
  parseTtsProviders,
  saveWelcomeShownAt,
} from '@/src/api/settingsStore';

const asyncStorageGetItem = AsyncStorage.getItem as jest.Mock;
const asyncStorageSetItem = AsyncStorage.setItem as jest.Mock;
const asyncStorageRemoveItem = AsyncStorage.removeItem as jest.Mock;
const secureStoreGetItemAsync = SecureStore.getItemAsync as jest.Mock;
const loadNotificationSettingsMock = loadNotificationSettings as jest.Mock;

beforeEach(() => {
  asyncStorageGetItem.mockReset();
  asyncStorageSetItem.mockReset();
  asyncStorageRemoveItem.mockReset();
  secureStoreGetItemAsync.mockReset();
  loadNotificationSettingsMock.mockReset();
  loadNotificationSettingsMock.mockResolvedValue({});
});

function mockStorage(values: Record<string, string | null>) {
  asyncStorageGetItem.mockImplementation((key: string) =>
    Promise.resolve(values[key] ?? null),
  );
  secureStoreGetItemAsync.mockImplementation((key: string) =>
    Promise.resolve(values[key] ?? null),
  );
}

describe('loadSettings migration', () => {
  it('migrates legacy combined llm providers to split providers and models', async () => {
    const legacyProviders = [
      {
        id: 'legacy-1',
        name: 'OpenRouter',
        baseUrl: 'https://openrouter.ai/api/v1',
        selectedModel: 'openai/gpt-4o-mini',
        cachedModels: ['openai/gpt-4o-mini', 'openai/gpt-4o'],
        modelsFetchedAt: '2026-07-20T00:00:00.000Z',
        cost: 'in $0.15, out $0.6 / 1M tokens',
        permissions: { todos: true },
      },
    ];
    mockStorage({
      'takusu.agent.llmProviders': JSON.stringify(legacyProviders),
      'takusu.agent.activeLlmProvider': 'legacy-1',
    });

    const settings = await loadSettings();

    expect(settings.llmProviders).toHaveLength(1);
    expect(settings.llmProviders[0]).toEqual({
      id: 'legacy-1',
      name: 'OpenRouter',
      baseUrl: 'https://openrouter.ai/api/v1',
    });
    expect(settings.llmModels).toHaveLength(1);
    expect(settings.llmModels[0]).toMatchObject({
      name: 'OpenRouter (openai/gpt-4o-mini)',
      providerId: 'legacy-1',
      selectedModel: 'openai/gpt-4o-mini',
      cachedModels: ['openai/gpt-4o-mini', 'openai/gpt-4o'],
      cost: 'in $0.15, out $0.6 / 1M tokens',
      permissions: { todos: true },
    });
    expect(settings.activeLlmModel).toBe(settings.llmModels[0]?.id);
  });

  it('maps legacy active provider id to new active model id', async () => {
    const legacyProviders = [
      {
        id: 'legacy-a',
        name: 'A',
        baseUrl: 'https://a.example.com/v1',
        selectedModel: 'model-a',
        cachedModels: ['model-a'],
      },
      {
        id: 'legacy-b',
        name: 'B',
        baseUrl: 'https://b.example.com/v1',
        selectedModel: 'model-b',
        cachedModels: ['model-b'],
      },
    ];
    mockStorage({
      'takusu.agent.llmProviders': JSON.stringify(legacyProviders),
      'takusu.agent.activeLlmProvider': 'legacy-b',
    });

    const settings = await loadSettings();

    const activeModel = settings.llmModels.find(
      (m) => m.id === settings.activeLlmModel,
    );
    expect(activeModel?.providerId).toBe('legacy-b');
  });

  it('uses new split format when llmModels is already stored', async () => {
    const providers = [
      { id: 'p1', name: 'Provider', baseUrl: 'https://p1.example.com/v1' },
    ];
    const models = [
      {
        id: 'm1',
        name: 'Model',
        providerId: 'p1',
        selectedModel: 'model-1',
        cachedModels: ['model-1'],
      },
    ];
    mockStorage({
      'takusu.agent.llmProviders': JSON.stringify(providers),
      'takusu.agent.llmModels': JSON.stringify(models),
      'takusu.agent.activeLlmModel': 'm1',
    });

    const settings = await loadSettings();

    expect(settings.llmProviders).toEqual(providers);
    expect(settings.llmModels).toEqual(models);
    expect(settings.activeLlmModel).toBe('m1');
  });

  it('does not misinterpret legacy providers as new providers', async () => {
    const legacyProviders = [
      {
        id: 'legacy-1',
        name: 'Legacy',
        baseUrl: 'https://legacy.example.com/v1',
        selectedModel: 'model',
        cachedModels: ['model'],
      },
    ];
    mockStorage({
      'takusu.agent.llmProviders': JSON.stringify(legacyProviders),
    });

    const settings = await loadSettings();

    expect(settings.llmProviders).toHaveLength(1);
    expect(settings.llmModels).toHaveLength(1);
    expect(settings.llmProviders[0]?.baseUrl).toBe(
      'https://legacy.example.com/v1',
    );
    expect(settings.llmModels[0]?.selectedModel).toBe('model');
  });
});

describe('loadTheme', () => {
  afterEach(() => {
    jest.restoreAllMocks();
  });

  it('returns the stored theme', async () => {
    mockStorage({ 'takusu.theme': 'dark' });
    expect(await loadTheme()).toBe('dark');
  });

  it('migrates legacy darkMode true to dark and removes the old key', async () => {
    mockStorage({ 'takusu.darkMode': 'true' });
    expect(await loadTheme()).toBe('dark');
    expect(asyncStorageSetItem).toHaveBeenCalledWith('takusu.theme', 'dark');
    expect(asyncStorageRemoveItem).toHaveBeenCalledWith('takusu.darkMode');
  });

  it('migrates legacy darkMode false to light and removes the old key', async () => {
    mockStorage({ 'takusu.darkMode': 'false' });
    expect(await loadTheme()).toBe('light');
    expect(asyncStorageSetItem).toHaveBeenCalledWith('takusu.theme', 'light');
    expect(asyncStorageRemoveItem).toHaveBeenCalledWith('takusu.darkMode');
  });

  it('falls back to the system dark color scheme', async () => {
    mockStorage({});
    jest.spyOn(Appearance, 'getColorScheme').mockReturnValue('dark');
    expect(await loadTheme()).toBe('dark');
  });

  it('falls back to light when the system is light', async () => {
    mockStorage({});
    jest.spyOn(Appearance, 'getColorScheme').mockReturnValue('light');
    expect(await loadTheme()).toBe('light');
  });
});

describe('parseTtsProviders', () => {
  it('returns an empty array for null', () => {
    expect(parseTtsProviders(null)).toEqual([]);
  });

  it('returns an empty array for invalid JSON', () => {
    expect(parseTtsProviders('not json')).toEqual([]);
  });

  it('keeps valid cartesia providers', () => {
    const input = JSON.stringify([
      {
        id: 'p1',
        name: 'Cartesia',
        provider: 'cartesia',
        voiceId: 'voice-1',
        model: 'sonic-3.5',
        language: 'ja',
        sampleRate: 44100,
        speed: 1.2,
      },
    ]);
    expect(parseTtsProviders(input)).toEqual([
      {
        id: 'p1',
        name: 'Cartesia',
        provider: 'cartesia',
        voiceId: 'voice-1',
        model: 'sonic-3.5',
        language: 'ja',
        sampleRate: 44100,
        speed: 1.2,
      },
    ]);
  });

  it('keeps valid fish providers', () => {
    const input = JSON.stringify([
      {
        id: 'p-fish',
        name: 'Fish',
        provider: 'fish',
        voiceId: '',
        model: 's2.1-pro-free',
        language: 'ja',
        sampleRate: 44100,
      },
    ]);
    const result = parseTtsProviders(input);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({
      id: 'p-fish',
      name: 'Fish',
      provider: 'fish',
      voiceId: '',
      model: 's2.1-pro-free',
      language: 'ja',
      sampleRate: 44100,
    });
  });

  it('keeps valid android providers', () => {
    const input = JSON.stringify([
      {
        id: 'p2',
        name: 'Android',
        provider: 'android',
        voiceId: '',
        language: 'ja',
        sampleRate: 44100,
      },
    ]);
    const result = parseTtsProviders(input);
    expect(result).toHaveLength(1);
    expect(result[0]?.provider).toBe('android');
  });

  it('falls back invalid provider names to cartesia', () => {
    const input = JSON.stringify([
      {
        id: 'p3',
        name: 'Bad',
        provider: 'unknown',
        voiceId: '',
        language: 'ja',
        sampleRate: 44100,
      },
    ]);
    const result = parseTtsProviders(input);
    expect(result).toHaveLength(1);
    expect(result[0]?.provider).toBe('cartesia');
  });

  it('fixes out-of-range speed values', () => {
    const input = JSON.stringify([
      {
        id: 'p4',
        name: 'Fast',
        provider: 'cartesia',
        voiceId: 'v',
        language: 'ja',
        sampleRate: 44100,
        speed: 0,
      },
      {
        id: 'p5',
        name: 'Slow',
        provider: 'cartesia',
        voiceId: 'v',
        language: 'ja',
        sampleRate: 44100,
        speed: -1,
      },
    ]);
    const result = parseTtsProviders(input);
    expect(result[0]?.speed).toBeUndefined();
    expect(result[1]?.speed).toBeUndefined();
  });

  it('fixes invalid sampleRate', () => {
    const input = JSON.stringify([
      {
        id: 'p6',
        name: 'Cartesia',
        provider: 'cartesia',
        voiceId: 'v',
        language: 'ja',
        sampleRate: -1,
      },
    ]);
    const result = parseTtsProviders(input);
    expect(result[0]?.sampleRate).toBe(44100);
  });

  it('skips entries without a valid id', () => {
    const input = JSON.stringify([
      { provider: 'android', voiceId: '', language: 'ja', sampleRate: 44100 },
    ]);
    expect(parseTtsProviders(input)).toEqual([]);
  });
});

describe('loadWelcomeShownAt', () => {
  it('returns null when no value is stored', async () => {
    asyncStorageGetItem.mockResolvedValue(null);
    expect(await loadWelcomeShownAt()).toBeNull();
  });

  it('returns the stored timestamp', async () => {
    asyncStorageGetItem.mockResolvedValue('1234567890');
    expect(await loadWelcomeShownAt()).toBe(1234567890);
  });

  it('returns null for malformed numeric strings', async () => {
    asyncStorageGetItem.mockResolvedValue('123abc');
    expect(await loadWelcomeShownAt()).toBeNull();
  });

  it('returns null for non-finite values', async () => {
    asyncStorageGetItem.mockResolvedValue('Infinity');
    expect(await loadWelcomeShownAt()).toBeNull();
  });
});

describe('saveWelcomeShownAt', () => {
  it('saves the timestamp as a string', async () => {
    await saveWelcomeShownAt(1234567890);
    expect(asyncStorageSetItem).toHaveBeenCalledWith(
      'takusu.welcomeShownAt',
      '1234567890',
    );
  });
});
