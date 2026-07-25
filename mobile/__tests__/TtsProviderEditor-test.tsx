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

import { useState } from 'react';
import { Platform } from 'react-native';
import { fireEvent, render, waitFor } from '@testing-library/react-native';

import { ThemeProvider } from '@/src/theme';
import type { TtsProviderSettings } from '@/src/api/settingsStore';
import { TtsProviderEditor } from '@/src/components/settings/TtsProviderEditor';

jest.mock('@/modules/takusu-server/src/TakusuAudioModule', () => ({
  __esModule: true,
  default: {
    getAvailableVoices: jest.fn(),
  },
}));

import TakusuAudioModule from '@/modules/takusu-server/src/TakusuAudioModule';

const getAvailableVoicesMock = (
  TakusuAudioModule as unknown as { getAvailableVoices: jest.Mock }
).getAvailableVoices;

const baseProvider: TtsProviderSettings = {
  id: 'tts-1',
  name: 'Test',
  provider: 'android',
  voiceId: '',
  language: 'ja',
  sampleRate: 44100,
};

const mockVoices = [
  {
    name: 'ja-jp-x-abc',
    locale: 'ja-JP',
    quality: 400,
    latency: 200,
    requiresNetworkConnection: false,
    features: [],
  },
  {
    name: 'en-us-x-def',
    locale: 'en-US',
    quality: 400,
    latency: 200,
    requiresNetworkConnection: false,
    features: [],
  },
];

function TestWrapper({
  initialProvider,
  onChangeProvider,
}: {
  initialProvider: TtsProviderSettings;
  onChangeProvider?: (provider: TtsProviderSettings) => void;
}) {
  const [provider, setProvider] = useState(initialProvider);
  return (
    <ThemeProvider>
      <TtsProviderEditor
        provider={provider}
        apiKey=""
        onChangeProvider={(next) => {
          setProvider(next);
          onChangeProvider?.(next);
        }}
        onChangeApiKey={jest.fn()}
        onSave={jest.fn()}
        onCancel={jest.fn()}
      />
    </ThemeProvider>
  );
}

async function setup(overrides: Partial<TtsProviderSettings> = {}) {
  const onChangeProvider = jest.fn();
  const provider = { ...baseProvider, ...overrides };
  const utils = await render(
    <TestWrapper
      initialProvider={provider}
      onChangeProvider={onChangeProvider}
    />,
  );
  return { ...utils, onChangeProvider };
}

beforeEach(() => {
  getAvailableVoicesMock.mockReset();
  getAvailableVoicesMock.mockResolvedValue(mockVoices);
  Object.defineProperty(Platform, 'OS', {
    value: 'android',
    configurable: true,
  });
});

afterEach(() => {
  Object.defineProperty(Platform, 'OS', {
    value: 'android',
    configurable: true,
  });
});

describe('TtsProviderEditor', () => {
  it('loads and displays Android voices in a dropdown', async () => {
    const { getByText } = await setup();
    await waitFor(() => expect(getAvailableVoicesMock).toHaveBeenCalled());
    expect(getByText('自動（最初の声）')).toBeTruthy();
    fireEvent.press(getByText('自動（最初の声）'));
    await waitFor(() => {
      expect(getByText('ja-jp-x-abc (ja-JP)')).toBeTruthy();
      expect(getByText('en-us-x-def (en-US)')).toBeTruthy();
    });
  });

  it('selects a voice from the dropdown', async () => {
    const { getByText, onChangeProvider } = await setup();
    await waitFor(() => expect(getAvailableVoicesMock).toHaveBeenCalled());
    fireEvent.press(getByText('自動（最初の声）'));
    await waitFor(() => expect(getByText('ja-jp-x-abc (ja-JP)')).toBeTruthy());
    fireEvent.press(getByText('ja-jp-x-abc (ja-JP)'));
    await waitFor(() =>
      expect(onChangeProvider).toHaveBeenCalledWith(
        expect.objectContaining({ voiceId: 'ja-jp-x-abc' }),
      ),
    );
  });

  it('falls back to the first voice when the automatic option is selected', async () => {
    const { getByText, onChangeProvider } = await setup({
      voiceId: 'ja-jp-x-abc',
    });
    await waitFor(() => expect(getAvailableVoicesMock).toHaveBeenCalled());
    fireEvent.press(getByText('ja-jp-x-abc'));
    await waitFor(() => expect(getByText('自動（最初の声）')).toBeTruthy());
    fireEvent.press(getByText('自動（最初の声）'));
    await waitFor(() =>
      expect(onChangeProvider).toHaveBeenCalledWith(
        expect.objectContaining({ voiceId: '' }),
      ),
    );
  });

  it('uses a text input for the cartesia provider', async () => {
    const { getByPlaceholderText, queryByText } = await setup({
      provider: 'cartesia',
    });
    expect(getByPlaceholderText('Voice ID')).toBeTruthy();
    expect(queryByText('自動（最初の声）')).toBeNull();
  });
});
