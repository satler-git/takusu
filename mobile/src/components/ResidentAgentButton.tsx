import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Keyboard,
  StyleSheet,
  View,
  useWindowDimensions,
  type KeyboardEvent,
} from 'react-native';
import AsyncStorage from '@react-native-async-storage/async-storage';
import { Ionicons } from '@expo/vector-icons';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  withSpring,
  runOnJS,
} from 'react-native-reanimated';
import { usePathname, useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useSurface } from '@/src/api/SurfaceContext';
import { useVoice } from '@/src/api/VoiceContext';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { startRecording, stopAndTranscribe } from '@/src/utils/voice';
import { AgentCompactPanel } from '@/src/components/AgentCompactPanel';
import type { SurfaceSnapshot } from '@/src/api/agentTypes';

const BUTTON_SIZE = 56;
const TASKADD_SLIDE_THRESHOLD = 60;
const LONG_PRESS_MS = 400;
const DRAG_THRESHOLD = 30;
const VELOCITY_THRESHOLD = 800;
const POSITION_KEY = 'takusu.residentAgentButton.position';

function iconForState(snapshot: SurfaceSnapshot | null) {
  if (!snapshot) return 'chatbubble' as const;
  switch (snapshot.state) {
    case 'listening':
      return 'mic' as const;
    case 'transcribing':
      return 'ellipsis-horizontal' as const;
    case 'thinking':
      return 'sparkles' as const;
    case 'waiting_for_user':
      return 'help-circle' as const;
    case 'waiting_for_approval':
      return 'checkmark-circle' as const;
    case 'speaking':
      return 'volume-high' as const;
    case 'error':
      return 'warning' as const;
    case 'idle':
    default:
      return 'chatbubble' as const;
  }
}

function colorForState(colors: ColorSet, snapshot: SurfaceSnapshot | null) {
  if (!snapshot) return colors.brand;
  switch (snapshot.state) {
    case 'listening':
    case 'transcribing':
      return colors.brand;
    case 'thinking':
      return colors.brandLight;
    case 'waiting_for_approval':
      return colors.warning;
    case 'speaking':
      return colors.success;
    case 'error':
      return colors.error;
    case 'idle':
    case 'waiting_for_user':
    default:
      return colors.brand;
  }
}

function stateLabel(snapshot: SurfaceSnapshot | null): string {
  if (!snapshot) return 'エージェント';
  switch (snapshot.state) {
    case 'idle':
      return 'エージェント';
    case 'listening':
      return '話してください';
    case 'transcribing':
      return '書き起こし中';
    case 'thinking':
      return '考え中';
    case 'waiting_for_user':
      return '確認待ち';
    case 'waiting_for_approval':
      return '承認待ち';
    case 'speaking':
      return '話しています';
    case 'error':
      return 'エラー';
  }
}

interface ButtonPosition {
  x: number;
  y: number;
}

interface ResidentAgentButtonProps {
  /** Default bottom-center position; restored from AsyncStorage if saved. */
  defaultPosition?: ButtonPosition;
}

/**
 * App-wide resident agent button.
 *
 * The gesture callbacks run on the Reanimated UI thread. All JS-side actions
 * (recording, routing, AsyncStorage, timers) are scheduled with `runOnJS`,
 * while per-gesture state and position live in `SharedValue`s so the worklet
 * can read and write them without crossing the JS/UI bridge.
 */
export function ResidentAgentButton({
  defaultPosition,
}: ResidentAgentButtonProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const pathname = usePathname();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const { top, left, right, bottom } = insets;
  const { width, height } = useWindowDimensions();
  const { snapshot, agentClient, sendCommand, reportAudio } = useSurface();
  const { setIsRecording, setPendingSessionId } = useVoice();

  const [keyboardHeight, setKeyboardHeight] = useState(0);
  const [panelVisible, setPanelVisible] = useState(false);
  const [surfaceTranscript, setSurfaceTranscript] = useState('');
  const [surfaceSessionId, setSurfaceSessionId] = useState<
    string | undefined
  >();

  // Boundaries and position are shared values so the gesture worklet can read
  // them directly. They update when the window, insets, or keyboard change.
  const minX = useSharedValue(0);
  const maxX = useSharedValue(0);
  const minY = useSharedValue(0);
  const maxY = useSharedValue(0);

  const screenWidth = width || 0;
  const screenHeight = height || 0;

  const startPos = useMemo<ButtonPosition>(() => {
    if (defaultPosition) return defaultPosition;
    return {
      x: (screenWidth - BUTTON_SIZE) / 2,
      y: screenHeight - BUTTON_SIZE - insets.bottom - 16,
    };
  }, [defaultPosition, screenWidth, screenHeight, insets.bottom]);

  // Start off-screen; the default-position effect will move the button to a
  // valid bottom-center (or saved) position once dimensions are known.
  const buttonX = useSharedValue(-BUTTON_SIZE);
  const buttonY = useSharedValue(-BUTTON_SIZE);
  const needsDefaultPosition = useRef(true);

  useEffect(() => {
    minX.value = left + 8;
    maxX.value = screenWidth - BUTTON_SIZE - right - 8;
    minY.value = top + 8;
    maxY.value = screenHeight - BUTTON_SIZE - bottom - 8 - keyboardHeight;
    // Clamp the current position whenever the available screen area changes
    // (rotation, keyboard show/hide, safe-area updates). Skip clamping while
    // the default-position effect has not yet placed the button, otherwise the
    // off-screen initial value (-BUTTON_SIZE) gets snapped to the top-left.
    if (screenWidth <= 0 || screenHeight <= 0) return;
    if (needsDefaultPosition.current) return;
    buttonX.value = Math.max(minX.value, Math.min(maxX.value, buttonX.value));
    buttonY.value = Math.max(minY.value, Math.min(maxY.value, buttonY.value));
  }, [
    top,
    left,
    right,
    bottom,
    screenWidth,
    screenHeight,
    keyboardHeight,
    buttonX,
    buttonY,
    minX,
    maxX,
    minY,
    maxY,
  ]);

  // Load saved position once on mount. If a saved position exists, restore it;
  // otherwise the default-position effect below will place the button.
  useEffect(() => {
    AsyncStorage.getItem(POSITION_KEY)
      .then((raw) => {
        if (!raw) return;
        try {
          const parsed = JSON.parse(raw) as unknown;
          if (
            parsed &&
            typeof parsed === 'object' &&
            'x' in parsed &&
            'y' in parsed &&
            typeof (parsed as ButtonPosition).x === 'number' &&
            typeof (parsed as ButtonPosition).y === 'number'
          ) {
            const pos = parsed as ButtonPosition;
            const x = Math.max(minX.value, Math.min(maxX.value, pos.x));
            const y = Math.max(minY.value, Math.min(maxY.value, pos.y));
            buttonX.value = withSpring(x);
            buttonY.value = withSpring(y);
            needsDefaultPosition.current = false;
          }
        } catch {
          // ignore malformed saved position
        }
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Default position: bottom-center once we have a valid screen size. This
  // also replaces any invalid -BUTTON_SIZE initial value from first render.
  useEffect(() => {
    if (!needsDefaultPosition.current) return;
    if (screenWidth <= 0 || screenHeight <= 0) return;
    const x = Math.max(minX.value, Math.min(maxX.value, startPos.x));
    const y = Math.max(minY.value, Math.min(maxY.value, startPos.y));
    buttonX.value = withSpring(x);
    buttonY.value = withSpring(y);
    needsDefaultPosition.current = false;
  }, [
    screenWidth,
    screenHeight,
    startPos,
    buttonX,
    buttonY,
    minX,
    maxX,
    minY,
    maxY,
  ]);

  // Keyboard avoidance: shift the button up when the keyboard appears.
  useEffect(() => {
    const show = (e: KeyboardEvent) =>
      setKeyboardHeight(e.endCoordinates.height);
    const hide = () => setKeyboardHeight(0);
    const showSub = Keyboard.addListener('keyboardDidShow', show);
    const hideSub = Keyboard.addListener('keyboardDidHide', hide);
    return () => {
      showSub.remove();
      hideSub.remove();
    };
  }, []);

  // Per-gesture state lives in shared values so the worklet can access it.
  const recording = useSharedValue(false);
  const didUpdate = useSharedValue(false);
  const didDrag = useSharedValue(false);
  const didSlide = useSharedValue(false);
  const routed = useSharedValue(false);
  const released = useSharedValue(false);
  const startTime = useSharedValue(0);
  const startX = useSharedValue(0);
  const startY = useSharedValue(0);

  const longPressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // The surface snapshot changes frequently; keep a ref so `handleTap` does not
  // recreate the gesture object on every SSE update.
  const snapshotRef = useRef(snapshot);
  snapshotRef.current = snapshot;

  const openAgent = useCallback(() => {
    if (pathname === '/agent') return;
    if (agentClient) {
      setPendingSessionId(`__new__${Date.now()}`);
    }
    router.push('/agent');
  }, [pathname, router, agentClient, setPendingSessionId]);

  const openTaskAdd = useCallback(() => {
    if (pathname === '/task/add') return;
    router.push('/task/add');
  }, [pathname, router]);

  const hapticLight = useCallback(() => {
    haptic.light();
  }, []);

  const clearLongPressTimer = useCallback(() => {
    if (longPressTimerRef.current) {
      clearTimeout(longPressTimerRef.current);
      longPressTimerRef.current = null;
    }
  }, []);

  const beginRecording = useCallback(async () => {
    if (recording.value || !agentClient) return;
    recording.value = true;
    setIsRecording(true);
    try {
      await startRecording();
      // The user may have released while we were waiting for permissions or
      // audio setup. If recording was cancelled, do not report listening.
      if (!recording.value) return;
      await reportAudio('listening');
    } catch {
      recording.value = false;
      setIsRecording(false);
    }
  }, [agentClient, reportAudio, setIsRecording, recording]);

  const finishRecording = useCallback(async () => {
    if (!recording.value) return;
    recording.value = false;
    setIsRecording(false);
    try {
      const transcript = await stopAndTranscribe();
      if (agentClient) {
        await reportAudio('transcribing');
      }
      if (transcript.trim()) {
        setSurfaceTranscript(transcript);
        setSurfaceSessionId(undefined);
        setPanelVisible(true);
      } else {
        setSurfaceTranscript('');
      }
    } catch {
      // ignore
    }
  }, [agentClient, reportAudio, setIsRecording, recording]);

  const startLongPressTimer = useCallback(() => {
    clearLongPressTimer();
    longPressTimerRef.current = setTimeout(() => {
      beginRecording();
    }, LONG_PRESS_MS);
  }, [clearLongPressTimer, beginRecording]);

  const setStartTime = useCallback(() => {
    startTime.value = Date.now();
  }, [startTime]);

  const savePosition = useCallback(
    (x: number, y: number) => {
      const pos = {
        x: Math.max(minX.value, Math.min(maxX.value, x)),
        y: Math.max(minY.value, Math.min(maxY.value, y)),
      };
      AsyncStorage.setItem(POSITION_KEY, JSON.stringify(pos)).catch(() => {});
    },
    [minX, maxX, minY, maxY],
  );

  const reset = useCallback(() => {
    clearLongPressTimer();
    didUpdate.value = false;
    didDrag.value = false;
    didSlide.value = false;
    routed.value = false;
    released.value = false;
    recording.value = false;
    startTime.value = 0;
  }, [
    clearLongPressTimer,
    didUpdate,
    didDrag,
    didSlide,
    routed,
    released,
    recording,
    startTime,
  ]);

  const handleTap = useCallback(async () => {
    const current = snapshotRef.current;
    if (!current) {
      openAgent();
      return;
    }
    switch (current.state) {
      case 'listening':
        // release() already handles finishRecording when recording is active,
        // so this is only reached when the user is not recording.
        await sendCommand('confirm-recording', current.operation_id);
        break;
      case 'speaking':
        await sendCommand('stop-tts', current.operation_id);
        break;
      case 'thinking':
        await sendCommand('open-panel', current.operation_id);
        openAgent();
        break;
      case 'waiting_for_approval':
        await sendCommand('open-approval', current.operation_id);
        openAgent();
        break;
      case 'error':
        await sendCommand('show-recovery', current.operation_id);
        openAgent();
        break;
      case 'waiting_for_user':
      case 'idle':
      case 'transcribing':
      default:
        openAgent();
        break;
    }
  }, [openAgent, sendCommand]);

  // Release runs on the JS thread so it can use Date.now() and React state.
  const release = useCallback(async () => {
    if (released.value) return;
    released.value = true;
    clearLongPressTimer();
    if (routed.value) {
      reset();
      return;
    }
    if (recording.value) {
      finishRecording();
      return;
    }
    if (didSlide.value) {
      reset();
      return;
    }
    if (didDrag.value) {
      savePosition(buttonX.value, buttonY.value);
      reset();
      return;
    }
    if (didUpdate.value) {
      // A small slide was detected; do not open the agent.
      reset();
      return;
    }
    const elapsed = Date.now() - startTime.value;
    if (elapsed < LONG_PRESS_MS) {
      try {
        await handleTap();
      } catch (e) {
        console.error(e);
      }
    }
    reset();
  }, [
    clearLongPressTimer,
    reset,
    finishRecording,
    handleTap,
    savePosition,
    released,
    routed,
    recording,
    didSlide,
    didDrag,
    didUpdate,
    startTime,
    buttonX,
    buttonY,
  ]);

  const closePanel = useCallback(() => {
    setPanelVisible(false);
    setSurfaceTranscript('');
    setSurfaceSessionId(undefined);
  }, []);

  const onPanelComplete = useCallback(() => {
    // The surface turn has finished; keep the panel open so the user can read
    // the result, but clear the transcript so a new turn can start.
    setSurfaceTranscript('');
  }, []);

  const surfaceSpec = useMemo(
    () => ({
      transcript: surfaceTranscript,
      sessionId: surfaceSessionId,
      onComplete: onPanelComplete,
    }),
    [surfaceTranscript, surfaceSessionId, onPanelComplete],
  );

  const panGesture = useMemo(
    () =>
      Gesture.Pan()
        .activeOffsetY([-8, 8])
        .failOffsetX([-24, 24])
        .withTestId('resident-agent-button-pan')
        .onBegin(() => {
          'worklet';
          startX.value = buttonX.value;
          startY.value = buttonY.value;
          didUpdate.value = false;
          didDrag.value = false;
          didSlide.value = false;
          routed.value = false;
          released.value = false;
          startTime.value = 0;
          runOnJS(setStartTime)();
          runOnJS(startLongPressTimer)();
        })
        .onUpdate((e) => {
          'worklet';
          const absX = Math.abs(e.translationX);
          const absY = Math.abs(e.translationY);

          if (recording.value) {
            // While recording the button stays in place; the user can release
            // to finish.
            return;
          }

          // Quick upward fling → add task. Velocity distinguishes a flick from
          // a slow drag.
          if (
            !didSlide.value &&
            !didDrag.value &&
            absY > absX &&
            e.translationY < -TASKADD_SLIDE_THRESHOLD &&
            absX < 20 &&
            e.velocityY < -VELOCITY_THRESHOLD
          ) {
            didSlide.value = true;
            routed.value = true;
            runOnJS(hapticLight)();
            runOnJS(openTaskAdd)();
            runOnJS(clearLongPressTimer)();
            return;
          }

          if (!didUpdate.value) {
            didUpdate.value = true;
          }

          const moved = absX > DRAG_THRESHOLD || absY > DRAG_THRESHOLD;
          if (moved && !didDrag.value) {
            didDrag.value = true;
            runOnJS(clearLongPressTimer)();
          }

          if (didDrag.value) {
            buttonX.value = Math.max(
              minX.value,
              Math.min(maxX.value, startX.value + e.translationX),
            );
            buttonY.value = Math.max(
              minY.value,
              Math.min(maxY.value, startY.value + e.translationY),
            );
          }
        })
        .onEnd(() => {
          'worklet';
          runOnJS(release)();
        })
        .onFinalize((_e, success) => {
          'worklet';
          if (!success) {
            runOnJS(release)();
          }
        }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      hapticLight,
      openTaskAdd,
      clearLongPressTimer,
      startLongPressTimer,
      setStartTime,
      release,
      buttonX,
      buttonY,
      minX,
      maxX,
      minY,
      maxY,
      startX,
      startY,
      startTime,
      didUpdate,
      didDrag,
      didSlide,
      routed,
      released,
      recording,
    ],
  );

  const buttonStyle = useAnimatedStyle(
    () => ({
      transform: [{ translateX: buttonX.value }, { translateY: buttonY.value }],
    }),
    [],
  );

  const icon = useMemo(() => iconForState(snapshot), [snapshot]);
  const backgroundColor = useMemo(
    () => colorForState(colors, snapshot),
    [colors, snapshot],
  );

  // Avoid the OS gesture areas and keyboard; the clamp already respects safe
  // insets, so the absolute top/left position is sufficient.
  if (pathname === '/agent' || pathname === '/task/add') {
    return null;
  }

  return (
    <View
      style={[
        styles.container,
        {
          bottom: 0,
          left: 0,
          right: 0,
          top: 0,
        },
      ]}
      pointerEvents="box-none"
    >
      <GestureDetector gesture={panGesture}>
        <Reanimated.View
          style={[styles.button, buttonStyle, { backgroundColor }]}
          accessibilityRole="button"
          accessibilityLabel={stateLabel(snapshot)}
        >
          <Ionicons name={icon} size={28} color={colors.white} />
        </Reanimated.View>
      </GestureDetector>
      <AgentCompactPanel
        visible={panelVisible}
        onClose={closePanel}
        agentClient={agentClient}
        snapshot={snapshot}
        surface={surfaceSpec}
      />
    </View>
  );
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      position: 'absolute',
      zIndex: 100,
    },
    button: {
      position: 'absolute',
      width: BUTTON_SIZE,
      height: BUTTON_SIZE,
      borderRadius: BUTTON_SIZE / 2,
      alignItems: 'center',
      justifyContent: 'center',
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 2 },
      shadowOpacity: 0.3,
      shadowRadius: 4,
      elevation: 4,
    },
  });
