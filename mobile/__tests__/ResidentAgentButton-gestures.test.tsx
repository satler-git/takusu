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
  };
});

jest.mock('@/src/api/ServerProvider', () => ({
  useServer: jest.fn(),
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

import { act, render } from '@testing-library/react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import {
  fireGestureHandler,
  getByGestureTestId,
} from 'react-native-gesture-handler/lib/commonjs/jestUtils';
import { usePathname, useRouter } from 'expo-router';
import { useServer } from '@/src/api/ServerProvider';
import { SurfaceProvider } from '@/src/api/SurfaceContext';
import { VoiceProvider } from '@/src/api/VoiceContext';
import { ThemeProvider } from '@/src/theme';
import { ResidentAgentButton } from '@/src/components/ResidentAgentButton';

const safeAreaMetrics = {
  insets: { top: 0, left: 0, right: 0, bottom: 0 },
  frame: { x: 0, y: 0, width: 400, height: 800 },
};

function TestWrapper({ children }: { children: React.ReactNode }) {
  return (
    <SafeAreaProvider initialMetrics={safeAreaMetrics}>
      <SurfaceProvider>
        <ThemeProvider theme="light">
          <VoiceProvider>{children}</VoiceProvider>
        </ThemeProvider>
      </SurfaceProvider>
    </SafeAreaProvider>
  );
}

describe('ResidentAgentButton gestures', () => {
  const pushMock = jest.fn();

  beforeEach(() => {
    pushMock.mockClear();
    (useRouter as jest.Mock).mockReturnValue({ push: pushMock });
    (usePathname as jest.Mock).mockReturnValue('/');
    (useServer as jest.Mock).mockReturnValue({ workersToken: '' });
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it('pushes /agent on a short tap', async () => {
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

    expect(pushMock).toHaveBeenCalledTimes(1);
    expect(pushMock).toHaveBeenCalledWith('/agent');
  });

  it('pushes /task/add once during a quick upward slide', async () => {
    await render(<ResidentAgentButton />, { wrapper: TestWrapper });

    const panGesture = getByGestureTestId('resident-agent-button-pan') as any;

    await act(async () => {
      fireGestureHandler(panGesture, [
        { x: 200, y: 700, numberOfPointers: 1 },
        {
          x: 200,
          y: 680,
          numberOfPointers: 1,
          translationX: 0,
          translationY: -20,
          velocityY: 0,
        },
        {
          x: 200,
          y: 620,
          numberOfPointers: 1,
          translationX: 0,
          translationY: -80,
          velocityY: -1000,
        },
        {
          x: 200,
          y: 600,
          numberOfPointers: 1,
          translationX: 0,
          translationY: -100,
          velocityY: 0,
        },
        {
          x: 200,
          y: 580,
          numberOfPointers: 1,
          translationX: 0,
          translationY: -120,
          velocityY: 0,
        },
      ]);
    });

    expect(pushMock).toHaveBeenCalledTimes(1);
    expect(pushMock).toHaveBeenCalledWith('/task/add');
  });

  it('pushes /task/add only once during a long slide', async () => {
    jest.useFakeTimers();
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
      panGesture.handlers.onUpdate({
        x: 200,
        y: 680,
        numberOfPointers: 1,
        translationX: 0,
        translationY: -20,
      });
    });
    expect(pushMock).not.toHaveBeenCalled();

    await act(async () => {
      panGesture.handlers.onUpdate({
        x: 200,
        y: 620,
        numberOfPointers: 1,
        translationX: 0,
        translationY: -80,
        velocityY: -1000,
      });
    });
    expect(pushMock).toHaveBeenCalledTimes(1);
    expect(pushMock).toHaveBeenCalledWith('/task/add');

    jest.advanceTimersByTime(150);

    await act(async () => {
      panGesture.handlers.onUpdate({
        x: 200,
        y: 600,
        numberOfPointers: 1,
        translationX: 0,
        translationY: -100,
      });
    });
    expect(pushMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      panGesture.handlers.onEnd({
        x: 200,
        y: 580,
        numberOfPointers: 1,
        translationX: 0,
        translationY: -120,
      });
    });
    expect(pushMock).toHaveBeenCalledTimes(1);
  });

  it('does not push any route when the slide is too short', async () => {
    await render(<ResidentAgentButton />, { wrapper: TestWrapper });

    const panGesture = getByGestureTestId('resident-agent-button-pan') as any;

    await act(async () => {
      fireGestureHandler(panGesture, [
        { x: 200, y: 700, numberOfPointers: 1 },
        {
          x: 200,
          y: 690,
          numberOfPointers: 1,
          translationX: 0,
          translationY: -10,
        },
        {
          x: 200,
          y: 670,
          numberOfPointers: 1,
          translationX: 0,
          translationY: -30,
        },
      ]);
    });

    expect(pushMock).not.toHaveBeenCalled();
  });
});
