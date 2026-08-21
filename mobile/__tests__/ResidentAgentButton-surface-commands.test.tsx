jest.mock('react-native-reanimated', () => {
  const RN = require('react-native');
  const NOOP = () => {};
  const ID = (x: any) => x;
  const useSharedValue = (init: any) => {
    const value = { value: init };
    return new Proxy(value, {
      get(target, prop) {
        if (prop === 'value') return target.value;
        if (prop === 'get') return () => target.value;
        if (prop === 'set')
          return (newValue: any) => {
            if (typeof newValue === 'function') {
              target.value = newValue(target.value);
            } else {
              target.value = newValue;
            }
          };
        return undefined;
      },
      set(target, prop: string, newValue) {
        if (prop === 'value') {
          target.value = newValue;
          return true;
        }
        return false;
      },
    });
  };
  return {
    __esModule: true,
    default: {
      View: RN.View,
      Text: RN.Text,
      Image: RN.Image,
      ScrollView: RN.Animated?.ScrollView ?? RN.View,
      FlatList: RN.Animated?.FlatList ?? RN.View,
      createAnimatedComponent: ID,
    },
    runOnJS: ID,
    runOnUI: ID,
    useSharedValue,
    useDerivedValue: (processor: () => any) => ({
      value: processor(),
      get: () => processor(),
    }),
    useEvent: () => NOOP,
    useAnimatedProps: (cb: any) => cb(),
    useAnimatedStyle: (cb: any, _deps?: any) => cb(),
    setGestureState: NOOP,
    withSpring: (value: any) => value,
    withTiming: (value: any) => value,
  };
});

jest.mock('@/src/api/ServerProvider', () => ({
  useServer: jest.fn(),
  DEFAULT_PORT: 8080,
}));

jest.mock('@react-native-async-storage/async-storage', () => ({
  __esModule: true,
  default: {
    getItem: jest.fn(() => Promise.resolve(null)),
    setItem: jest.fn(() => Promise.resolve()),
  },
}));

jest.mock('@/src/utils/voice', () => ({
  startRecording: jest.fn(),
  stopAndTranscribe: jest.fn(),
  setRecordingChangeListener: jest.fn(),
  isRecordingActive: jest.fn(),
  ensureAudioConfigured: jest.fn(),
}));

jest.mock('@/src/components/haptics', () => ({
  haptic: {
    light: jest.fn(),
    medium: jest.fn(),
    select: jest.fn(),
    heavy: jest.fn(),
    success: jest.fn(),
    warning: jest.fn(),
    error: jest.fn(),
  },
}));

jest.mock('expo-router', () => ({
  useRouter: jest.fn(),
  usePathname: jest.fn(),
}));

jest.mock('@/src/api/SurfaceContext', () => ({
  useSurface: jest.fn(),
  SurfaceProvider: ({ children }: { children: React.ReactNode }) => children,
}));

import { act, render } from '@testing-library/react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import { getByGestureTestId } from 'react-native-gesture-handler/lib/commonjs/jestUtils';
import { useRouter, usePathname } from 'expo-router';
import { useServer } from '@/src/api/ServerProvider';
import { useSurface } from '@/src/api/SurfaceContext';
import { ThemeProvider } from '@/src/theme';
import { VoiceProvider } from '@/src/api/VoiceContext';
import { ResidentAgentButton } from '@/src/components/ResidentAgentButton';

const safeAreaMetrics = {
  insets: { top: 0, left: 0, right: 0, bottom: 0 },
  frame: { x: 0, y: 0, width: 400, height: 800 },
};

function TestWrapper({ children }: { children: React.ReactNode }) {
  return (
    <SafeAreaProvider initialMetrics={safeAreaMetrics}>
      <ThemeProvider theme="light">
        <VoiceProvider>{children}</VoiceProvider>
      </ThemeProvider>
    </SafeAreaProvider>
  );
}

describe('ResidentAgentButton surface commands', () => {
  const pushMock = jest.fn();
  const sendCommandMock = jest.fn().mockResolvedValue({
    accepted: true,
    snapshot: { state: 'idle', operation_id: 0 },
  });

  beforeEach(() => {
    pushMock.mockClear();
    sendCommandMock.mockClear();
    (useRouter as jest.Mock).mockReturnValue({ push: pushMock });
    (usePathname as jest.Mock).mockReturnValue('/');
    (useServer as jest.Mock).mockReturnValue({ workersToken: '' });
  });

  function setSurfaceState(state: string) {
    (useSurface as jest.Mock).mockReturnValue({
      snapshot: { state, operation_id: 123 },
      agentClient: null,
      sendCommand: sendCommandMock,
      reportAudio: jest.fn(),
      connected: true,
      error: null,
    });
  }

  async function shortTap() {
    await render(<ResidentAgentButton />, { wrapper: TestWrapper });
    const panGesture = getByGestureTestId('resident-agent-button-pan') as any;
    await act(async () => {
      panGesture.handlers.onBegin({
        x: 200,
        y: 700,
        numberOfPointers: 1,
      });
    });
    await act(async () => {
      panGesture.handlers.onEnd({
        x: 200,
        y: 700,
        numberOfPointers: 1,
      });
    });
  }

  it('sends open-panel and opens the agent when the surface is thinking', async () => {
    setSurfaceState('thinking');
    await shortTap();
    expect(sendCommandMock).toHaveBeenCalledWith('open-panel', 123);
    expect(pushMock).toHaveBeenCalledWith('/agent');
  });

  it('sends open-approval when the surface is waiting_for_approval', async () => {
    setSurfaceState('waiting_for_approval');
    await shortTap();
    expect(sendCommandMock).toHaveBeenCalledWith('open-approval', 123);
    expect(pushMock).toHaveBeenCalledWith('/agent');
  });

  it('sends show-recovery when the surface is in error', async () => {
    setSurfaceState('error');
    await shortTap();
    expect(sendCommandMock).toHaveBeenCalledWith('show-recovery', 123);
    expect(pushMock).toHaveBeenCalledWith('/agent');
  });

  it('sends confirm-recording when the surface is listening', async () => {
    setSurfaceState('listening');
    await shortTap();
    expect(sendCommandMock).toHaveBeenCalledWith('confirm-recording', 123);
    expect(pushMock).not.toHaveBeenCalled();
  });

  it('sends stop-tts when the surface is speaking', async () => {
    setSurfaceState('speaking');
    await shortTap();
    expect(sendCommandMock).toHaveBeenCalledWith('stop-tts', 123);
    expect(pushMock).not.toHaveBeenCalled();
  });

  it('opens the agent directly when the surface is idle', async () => {
    setSurfaceState('idle');
    await shortTap();
    expect(sendCommandMock).not.toHaveBeenCalled();
    expect(pushMock).toHaveBeenCalledWith('/agent');
  });
});
