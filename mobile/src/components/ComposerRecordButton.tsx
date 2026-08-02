import { useCallback, useEffect, useRef, useMemo } from 'react';
import { I18nManager, StyleSheet } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { CrossFadeIcon } from '@/src/components/CrossFadeIcon';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Reanimated, {
  useAnimatedStyle,
  useSharedValue,
  withSpring,
  runOnJS,
} from 'react-native-reanimated';
import { startRecording, stopAndTranscribe } from '@/src/utils/voice';
import { useVoice } from '@/src/api/VoiceContext';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

const LONG_PRESS_MS = 350;
const AXIS_LOCK_THRESHOLD = 15;
const LOCK_THRESHOLD = 80;
const CANCEL_THRESHOLD = 80;

interface ComposerRecordButtonProps {
  audioReady: boolean;
  historyReady: boolean;
  busy: boolean;
  onAppend: (transcript: string) => void;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    button: {
      width: 44,
      height: 44,
      borderRadius: 22,
      borderWidth: 1,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: 'transparent',
    },
    hint: {
      position: 'absolute',
      width: 32,
      height: 32,
      borderRadius: 16,
      backgroundColor: colors.surfaceTranslucent,
      alignItems: 'center',
      justifyContent: 'center',
      opacity: 0,
    },
    lockHint: {
      top: -44,
      borderColor: colors.brand,
      borderWidth: 1,
    },
    cancelHint: {
      start: -44,
      borderColor: colors.destructive,
      borderWidth: 1,
    },
  });

export function ComposerRecordButton({
  audioReady,
  historyReady,
  busy,
  onAppend,
}: ComposerRecordButtonProps) {
  const colors = useColors();
  const { brand, destructive, white } = colors;
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const { isRecording } = useVoice();

  const recordingRef = useRef(false);
  const longPressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const translateX = useSharedValue(0);
  const translateY = useSharedValue(0);
  const willCancelShared = useSharedValue(false);
  const willLockShared = useSharedValue(false);
  const isLockedShared = useSharedValue(false);
  const longPressFired = useSharedValue(false);
  const gestureAxis = useSharedValue<'none' | 'vertical' | 'horizontal'>(
    'none',
  );

  const isRTL = I18nManager.isRTL;
  const startSign = isRTL ? 1 : -1;

  useEffect(() => {
    recordingRef.current = isRecording;
  }, [isRecording]);

  const clearLongPressTimer = useCallback(() => {
    if (longPressTimerRef.current) {
      clearTimeout(longPressTimerRef.current);
      longPressTimerRef.current = null;
    }
  }, []);

  const reset = useCallback(() => {
    clearLongPressTimer();
    recordingRef.current = false;
    isLockedShared.value = false;
    willCancelShared.value = false;
    willLockShared.value = false;
    longPressFired.value = false;
    gestureAxis.value = 'none';
    translateX.value = withSpring(0);
    translateY.value = withSpring(0);
  }, [
    clearLongPressTimer,
    isLockedShared,
    longPressFired,
    gestureAxis,
    translateX,
    translateY,
    willCancelShared,
    willLockShared,
  ]);

  const stopAndAppend = useCallback(async () => {
    if (!recordingRef.current) return;
    recordingRef.current = false;
    try {
      const transcript = await stopAndTranscribe();
      if (transcript) onAppend(transcript);
    } catch {
      // ignore
    } finally {
      reset();
    }
  }, [onAppend, reset]);

  const discardCurrent = useCallback(async () => {
    if (!recordingRef.current) return;
    recordingRef.current = false;
    try {
      await stopAndTranscribe();
    } catch {
      // ignore
    } finally {
      reset();
    }
  }, [reset]);

  const beginRecording = useCallback(() => {
    if (busy || !historyReady || !audioReady) return;
    if (recordingRef.current) return;
    recordingRef.current = true;
    startRecording().catch(() => {
      recordingRef.current = false;
    });
  }, [audioReady, busy, historyReady]);

  const handleBegin = useCallback(async () => {
    clearLongPressTimer();
    willCancelShared.value = false;
    willLockShared.value = false;
    gestureAxis.value = 'none';
    longPressFired.value = false;

    if (recordingRef.current) {
      // Already recording: only a locked tap stops and appends.
      if (isLockedShared.value) {
        await stopAndAppend();
      }
      return;
    }

    longPressTimerRef.current = setTimeout(() => {
      longPressFired.value = true;
      beginRecording();
    }, LONG_PRESS_MS);
  }, [
    beginRecording,
    clearLongPressTimer,
    isLockedShared,
    longPressFired,
    gestureAxis,
    stopAndAppend,
    willCancelShared,
    willLockShared,
  ]);

  const handleLock = useCallback(() => {
    if (
      willLockShared.value ||
      willCancelShared.value ||
      isLockedShared.value
    ) {
      return;
    }
    willLockShared.value = true;
    isLockedShared.value = true;
    haptic.light();
  }, [isLockedShared, willLockShared, willCancelShared]);

  const handleCancel = useCallback(() => {
    if (willCancelShared.value) return;
    willCancelShared.value = true;
    if (isLockedShared.value) {
      isLockedShared.value = false;
    }
    haptic.light();
  }, [isLockedShared, willCancelShared]);

  const handleEnd = useCallback(async () => {
    clearLongPressTimer();
    if (isLockedShared.value) {
      // Locked: keep recording until the user taps the mic again.
      return;
    }
    if (!longPressFired.value) {
      // Short tap without starting recording.
      reset();
      return;
    }
    if (willCancelShared.value) {
      await discardCurrent();
      return;
    }
    if (willLockShared.value) {
      // Lock confirmed: keep recording, hide lock-hint bounce and reset position.
      willLockShared.value = false;
      translateX.value = withSpring(0);
      translateY.value = withSpring(0);
      return;
    }
    if (recordingRef.current) {
      await stopAndAppend();
      return;
    }
    // Nothing started (permission denied or very quick tap); just reset.
    reset();
  }, [
    clearLongPressTimer,
    discardCurrent,
    isLockedShared,
    longPressFired,
    reset,
    stopAndAppend,
    translateX,
    translateY,
    willCancelShared,
    willLockShared,
  ]);

  const panGesture = Gesture.Pan()
    .activeOffsetY([-AXIS_LOCK_THRESHOLD, AXIS_LOCK_THRESHOLD])
    .activeOffsetX([-AXIS_LOCK_THRESHOLD, AXIS_LOCK_THRESHOLD])
    .onBegin(() => {
      runOnJS(handleBegin)();
    })
    .onUpdate((e) => {
      if (isLockedShared.value) {
        return;
      }

      const absX = Math.abs(e.translationX);
      const absY = Math.abs(e.translationY);
      const moved = absX > AXIS_LOCK_THRESHOLD || absY > AXIS_LOCK_THRESHOLD;

      if (!longPressFired.value && moved) {
        runOnJS(clearLongPressTimer)();
        longPressFired.value = true;
        runOnJS(beginRecording)();
      }

      if (gestureAxis.value === 'none' && moved) {
        // Lock to one axis (x xor y) like Discord.
        if (absY >= AXIS_LOCK_THRESHOLD && absX < AXIS_LOCK_THRESHOLD) {
          gestureAxis.value = 'vertical';
        } else if (absX >= AXIS_LOCK_THRESHOLD && absY < AXIS_LOCK_THRESHOLD) {
          gestureAxis.value = 'horizontal';
        } else if (absX >= AXIS_LOCK_THRESHOLD && absY >= AXIS_LOCK_THRESHOLD) {
          gestureAxis.value = absY > absX ? 'vertical' : 'horizontal';
        }
      }

      if (gestureAxis.value === 'vertical') {
        translateY.value = Math.min(0, e.translationY);
        translateX.value = withSpring(0);
        if (
          e.translationY < -LOCK_THRESHOLD &&
          !isLockedShared.value &&
          !willCancelShared.value &&
          !willLockShared.value
        ) {
          runOnJS(handleLock)();
        }
      } else if (gestureAxis.value === 'horizontal') {
        // Clamp to the start side (left in LTR, right in RTL).
        translateX.value = startSign * Math.max(0, startSign * e.translationX);
        translateY.value = withSpring(0);
        if (
          startSign * e.translationX > CANCEL_THRESHOLD &&
          !willCancelShared.value
        ) {
          runOnJS(handleCancel)();
        }
      } else {
        // Axis not locked yet: follow the finger freely for visual feedback.
        translateX.value = e.translationX;
        translateY.value = e.translationY;
      }
    })
    .onEnd(() => {
      runOnJS(handleEnd)();
    })
    .onFinalize((_e, success) => {
      if (!success) {
        runOnJS(handleEnd)();
      }
    });

  const animatedStyle = useAnimatedStyle(
    () => ({
      transform: [
        { translateX: translateX.value },
        { translateY: translateY.value },
      ],
      backgroundColor: isRecording ? destructive : 'transparent',
      borderColor: willCancelShared.value
        ? destructive
        : isRecording
          ? destructive
          : brand,
    }),
    [isRecording, brand, destructive],
  );

  const lockHintStyle = useAnimatedStyle(() => ({
    opacity: isLockedShared.value || willLockShared.value ? 1 : 0,
    transform: [
      {
        translateY: isLockedShared.value || willLockShared.value ? -8 : 0,
      },
    ],
  }));

  const cancelHintStyle = useAnimatedStyle(() => ({
    opacity: willCancelShared.value ? 1 : 0,
    transform: [{ translateX: willCancelShared.value ? startSign * 8 : 0 }],
  }));

  return (
    <GestureDetector gesture={panGesture}>
      <Reanimated.View
        style={[styles.button, animatedStyle]}
        pointerEvents="auto"
      >
        <CrossFadeIcon
          name={isRecording ? 'close' : 'mic'}
          size={24}
          color={isRecording ? white : brand}
        />
        <Reanimated.View style={[styles.hint, styles.lockHint, lockHintStyle]}>
          <Ionicons name="lock-closed" size={16} color={brand} />
        </Reanimated.View>
        <Reanimated.View
          style={[styles.hint, styles.cancelHint, cancelHintStyle]}
        >
          <Ionicons name="trash" size={16} color={destructive} />
        </Reanimated.View>
      </Reanimated.View>
    </GestureDetector>
  );
}
