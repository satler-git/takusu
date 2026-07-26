import { useCallback, useEffect, useRef, useMemo } from 'react';
import { StyleSheet, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { GestureDetector, Gesture } from 'react-native-gesture-handler';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  withSpring,
  runOnJS,
} from 'react-native-reanimated';
import { usePathname, useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useServer } from '@/src/api/ServerProvider';
import { useVoice } from '@/src/api/VoiceContext';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

const TASKADD_SLIDE_THRESHOLD = 60;

type ButtonState = 'idle' | 'pending' | 'gesture';

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      position: 'absolute',
      left: '50%',
      width: 56,
      zIndex: 100,
      transform: [{ translateX: -28 }],
      alignItems: 'center',
      justifyContent: 'center',
    },
    button: {
      width: 56,
      height: 56,
      borderRadius: 28,
      backgroundColor: colors.brand,
      alignItems: 'center',
      justifyContent: 'center',
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 2 },
      shadowOpacity: 0.3,
      shadowRadius: 4,
      elevation: 4,
    },
    hint: {
      position: 'absolute',
      bottom: 76,
      alignItems: 'center',
      justifyContent: 'center',
      padding: 12,
      borderRadius: 32,
      backgroundColor: colors.surfaceTranslucent,
      opacity: 0,
    },
  });

export function FloatingVoiceButton() {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const pathname = usePathname();
  const router = useRouter();
  const insets = useSafeAreaInsets();
  const { workersToken } = useServer();
  const { setPendingSessionId } = useVoice();

  const stateRef = useRef<ButtonState>('idle');
  const isSlideRef = useRef(false);
  const transitionedRef = useRef(false);
  const slideResetTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const buttonY = useSharedValue(0);

  const isHome = pathname === '/' || pathname === '' || pathname === '/index';

  useEffect(() => {
    return () => {
      if (slideResetTimerRef.current) clearTimeout(slideResetTimerRef.current);
    };
  }, []);

  // FloatingVoiceButton is mounted at the root (_layout.tsx), so it stays
  // alive while navigating to /agent or /task/add. Reset only the button UI
  // when leaving the home screen; pendingSessionId must stay intact so
  // AgentView can consume the queued session.
  useEffect(() => {
    if (isHome) return;
    stateRef.current = 'idle';
    isSlideRef.current = false;
    transitionedRef.current = false;
    buttonY.value = 0;
    if (slideResetTimerRef.current) {
      clearTimeout(slideResetTimerRef.current);
      slideResetTimerRef.current = null;
    }
  }, [isHome, buttonY]);

  const reset = useCallback(() => {
    if (slideResetTimerRef.current) {
      clearTimeout(slideResetTimerRef.current);
      slideResetTimerRef.current = null;
    }
    stateRef.current = 'idle';
    isSlideRef.current = false;
    transitionedRef.current = false;
    buttonY.value = withSpring(0);
    setPendingSessionId(null);
  }, [buttonY, setPendingSessionId]);

  const pushAgent = useCallback(() => {
    if (transitionedRef.current || pathname === '/agent') return;

    // Mark the transition immediately so a second press cannot queue another
    // new-session request while the first one is being handled.
    transitionedRef.current = true;

    // Queue a new session request. AgentView will create the actual session
    // only if there is no existing empty session.
    if (workersToken) {
      setPendingSessionId(`__new__${Date.now()}`);
    }

    router.push('/agent');
  }, [pathname, router, setPendingSessionId, workersToken]);

  const pushTaskAdd = useCallback(() => {
    if (transitionedRef.current) return;
    transitionedRef.current = true;
    router.push('/task/add');
  }, [router]);

  const handleSlide = useCallback(() => {
    if (isSlideRef.current || transitionedRef.current) return;
    isSlideRef.current = true;
    stateRef.current = 'gesture';
    pushTaskAdd();
    haptic.light();
    slideResetTimerRef.current = setTimeout(reset, 100);
  }, [pushTaskAdd, reset]);

  const handlePressIn = useCallback(() => {
    if (stateRef.current !== 'idle' || transitionedRef.current) return;

    stateRef.current = 'pending';
    isSlideRef.current = false;
    transitionedRef.current = false;
    buttonY.value = 0;
  }, [buttonY]);

  const handleRelease = useCallback(() => {
    if (isSlideRef.current) {
      reset();
      return;
    }
    if (transitionedRef.current) {
      // Already transitioning; do not reset the queued request.
      return;
    }
    if (stateRef.current === 'pending') {
      pushAgent();
      return;
    }
    reset();
  }, [pushAgent, reset]);

  const panGesture = Gesture.Pan()
    .activeOffsetY([-10, 10])
    .failOffsetX([-20, 20])
    .onBegin(() => {
      runOnJS(handlePressIn)();
    })
    .onUpdate((e) => {
      buttonY.value = Math.min(0, e.translationY);
      if (e.translationY < -TASKADD_SLIDE_THRESHOLD) {
        runOnJS(handleSlide)();
      }
    })
    .onEnd(() => {
      runOnJS(handleRelease)();
    })
    .onFinalize((_e, success) => {
      if (!success) {
        runOnJS(handleRelease)();
      }
    });

  const buttonStyle = useAnimatedStyle(
    () => ({
      transform: [{ translateY: buttonY.value }],
    }),
    [],
  );

  const hintStyle = useAnimatedStyle(() => {
    const progress = Math.min(
      1,
      Math.max(0, -buttonY.value / (TASKADD_SLIDE_THRESHOLD * 0.7)),
    );
    return {
      opacity: progress,
      transform: [{ scale: 0.8 + progress * 0.2 }],
    };
  });

  if (!isHome) {
    return null;
  }

  return (
    <View
      style={[styles.container, { bottom: 16 + insets.bottom }]}
      pointerEvents="box-none"
    >
      <Reanimated.View style={[styles.hint, hintStyle]}>
        <Ionicons name="arrow-up" size={28} color={colors.brand} />
      </Reanimated.View>
      <GestureDetector gesture={panGesture}>
        <Reanimated.View style={[styles.button, buttonStyle]}>
          <Ionicons name="add" size={28} color={colors.white} />
        </Reanimated.View>
      </GestureDetector>
    </View>
  );
}
