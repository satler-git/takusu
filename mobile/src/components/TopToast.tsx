// Top toast — auto-dismissing banner from the top of the screen.
// Multiple toasts stack downward from a fixed top corner: new toasts append
// to the bottom of the stack and slide up into their slot, while earlier
// toasts keep their positions. Swiping any toast sideways dismisses it.
// Implemented with react-native-reanimated and react-native-gesture-handler
// so all animation and pan handling runs on the native thread.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import {
  ActivityIndicator,
  StyleSheet,
  Text,
  View,
  useWindowDimensions,
  type LayoutChangeEvent,
} from 'react-native';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Reanimated, {
  cancelAnimation,
  Easing,
  runOnJS,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
} from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useColors } from '@/src/theme';

const DEFAULT_DURATION = 3000;
const OFFSCREEN_MARGIN = 50;
const SWIPE_DISMISS_THRESHOLD = 50;
const SWIPE_VELOCITY_THRESHOLD = 300;
const ESTIMATED_HEIGHT = 64;
const GAP = 8;

export type ToastType = 'info' | 'success' | 'error' | 'loading';

export interface ToastOptions {
  type?: ToastType;
  duration?: number;
}

interface Toast {
  id: string;
  message: string;
  type: ToastType;
  duration: number;
}

export interface TopToastContextValue {
  showTopToast: (message: string, options?: number | ToastOptions) => string;
  hideTopToast: (id: string) => void;
}

const TopToastContext = createContext<TopToastContextValue | null>(null);

export function TopToastProvider({ children }: { children: ReactNode }) {
  const colors = useColors();
  const insets = useSafeAreaInsets();
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [heights, setHeights] = useState<Record<string, number>>({});
  const dismissRegistry = useRef(new Map<string, () => void>());

  const handleLayout = useCallback((id: string, height: number) => {
    setHeights((prev) =>
      prev[id] === height ? prev : { ...prev, [id]: height },
    );
  }, []);

  const handleDismiss = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
    setHeights((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, []);

  const registerDismiss = useCallback((id: string, fn: () => void) => {
    dismissRegistry.current.set(id, fn);
  }, []);

  const unregisterDismiss = useCallback((id: string) => {
    dismissRegistry.current.delete(id);
  }, []);

  const hideTopToast = useCallback((id: string) => {
    dismissRegistry.current.get(id)?.();
  }, []);

  const showTopToast = useMemo(
    () =>
      (message: string, options?: number | ToastOptions): string => {
        const opts =
          typeof options === 'number' ? { duration: options } : (options ?? {});
        const id = `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
        const next: Toast = {
          id,
          message,
          type: opts.type ?? 'info',
          duration: opts.duration ?? DEFAULT_DURATION,
        };
        setToasts((prev) => [...prev, next]);
        return id;
      },
    [],
  );

  const offsets = useMemo(() => {
    let accumulated = 0;
    return toasts.map((toast) => {
      const offset = accumulated;
      accumulated += (heights[toast.id] ?? ESTIMATED_HEIGHT) + GAP;
      return offset;
    });
  }, [toasts, heights]);

  const value = useMemo(
    () => ({ showTopToast, hideTopToast }),
    [showTopToast, hideTopToast],
  );

  return (
    <TopToastContext.Provider value={value}>
      {children}
      {toasts.map((toast, index) => (
        <ToastItem
          key={toast.id}
          id={toast.id}
          message={toast.message}
          type={toast.type}
          duration={toast.duration}
          offset={offsets[index] ?? 0}
          height={heights[toast.id] ?? ESTIMATED_HEIGHT}
          insetsTop={insets.top + 8}
          zIndex={toasts.length - index}
          colors={colors}
          onLayout={handleLayout}
          onDismiss={handleDismiss}
          onRegisterDismiss={registerDismiss}
          onUnregisterDismiss={unregisterDismiss}
        />
      ))}
    </TopToastContext.Provider>
  );
}

interface ToastItemProps {
  id: string;
  message: string;
  type: ToastType;
  duration: number;
  offset: number;
  height: number;
  insetsTop: number;
  zIndex: number;
  colors: ReturnType<typeof useColors>;
  onLayout: (id: string, height: number) => void;
  onDismiss: (id: string) => void;
  onRegisterDismiss: (id: string, fn: () => void) => void;
  onUnregisterDismiss: (id: string) => void;
}

function ToastItem({
  id,
  message,
  type,
  duration,
  offset,
  height,
  insetsTop,
  zIndex,
  colors,
  onLayout,
  onDismiss,
  onRegisterDismiss,
  onUnregisterDismiss,
}: ToastItemProps) {
  const { width: screenWidth } = useWindowDimensions();
  const offsetY = useSharedValue(offset + ESTIMATED_HEIGHT);
  const offsetYTarget = useSharedValue(offset);
  const panY = useSharedValue(0);
  const panX = useSharedValue(0);
  const dismissing = useSharedValue(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const offsetRef = useRef(offset);
  const heightRef = useRef(height);
  offsetRef.current = offset;
  heightRef.current = height;

  const accentColor = useMemo(() => {
    switch (type) {
      case 'success':
        return '#2E7D32';
      case 'error':
        return '#C62828';
      case 'loading':
        return colors.gray;
      case 'info':
      default:
        return colors.brand;
    }
  }, [colors.brand, colors.gray, type]);

  const clearDismissTimer = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const dismiss = useCallback(() => {
    if (dismissing.value) return;
    dismissing.value = true;
    clearDismissTimer();
    cancelAnimation(panX);
    panX.value = 0;
    const target =
      -offsetRef.current - heightRef.current - insetsTop - OFFSCREEN_MARGIN;
    panY.value = withTiming(
      target,
      { duration: 200, easing: Easing.out(Easing.ease) },
      (finished) => {
        'worklet';
        if (finished) runOnJS(onDismiss)(id);
      },
    );
  }, [clearDismissTimer, dismissing, id, insetsTop, onDismiss, panX, panY]);

  const dismissHorizontal = useCallback(
    (direction: 'left' | 'right') => {
      if (dismissing.value) return;
      dismissing.value = true;
      clearDismissTimer();
      const target = direction === 'right' ? screenWidth : -screenWidth;
      panX.value = withTiming(
        target,
        { duration: 250, easing: Easing.out(Easing.ease) },
        (finished) => {
          'worklet';
          if (finished) runOnJS(onDismiss)(id);
        },
      );
    },
    [clearDismissTimer, dismissing, id, onDismiss, panX, screenWidth],
  );

  const startDismissTimer = useCallback(() => {
    clearDismissTimer();
    if (Number.isFinite(duration) && duration > 0) {
      timerRef.current = setTimeout(() => dismiss(), duration);
    }
  }, [clearDismissTimer, dismiss, duration]);

  const resetPanAndRestartTimer = useCallback(() => {
    panX.value = withTiming(0, {
      duration: 200,
      easing: Easing.out(Easing.ease),
    });
    panY.value = withTiming(0, {
      duration: 200,
      easing: Easing.out(Easing.ease),
    });
    offsetY.value = withTiming(offsetYTarget.value, {
      duration: 250,
      easing: Easing.out(Easing.ease),
    });
    startDismissTimer();
  }, [panX, panY, offsetY, offsetYTarget, startDismissTimer]);

  // Keep the target offset in sync with the parent stack at all times.
  // Animate to it only when the toast is not already dismissing.
  useEffect(() => {
    offsetYTarget.value = offset;
    if (dismissing.value) return;
    offsetY.value = withTiming(offset, {
      duration: 250,
      easing: Easing.out(Easing.ease),
    });
  }, [dismissing, offset, offsetY, offsetYTarget]);

  // Start the auto-dismiss timer on mount; clear it on unmount.
  useEffect(() => {
    startDismissTimer();
    return clearDismissTimer;
  }, [clearDismissTimer, duration, startDismissTimer]);

  // Register the dismiss callback so hideTopToast can trigger the exit animation.
  useEffect(() => {
    onRegisterDismiss(id, dismiss);
    return () => onUnregisterDismiss(id);
  }, [id, dismiss, onRegisterDismiss, onUnregisterDismiss]);

  const handleLayout = useCallback(
    (event: LayoutChangeEvent) => {
      const h = event.nativeEvent.layout.height;
      if (h > 0) onLayout(id, h);
    },
    [id, onLayout],
  );

  const gesture = useMemo(
    () =>
      Gesture.Pan()
        .activeOffsetX([-10, 10])
        .failOffsetY([-10, 10])
        .onBegin(() => {
          cancelAnimation(panX);
          cancelAnimation(panY);
          panX.value = 0;
          panY.value = 0;
          dismissing.value = false;
          runOnJS(clearDismissTimer)();
        })
        .onUpdate((e) => {
          panX.value = e.translationX;
        })
        .onEnd((e, success) => {
          if (!success) return;
          if (
            e.translationX > SWIPE_DISMISS_THRESHOLD ||
            e.velocityX > SWIPE_VELOCITY_THRESHOLD
          ) {
            runOnJS(dismissHorizontal)('right');
          } else if (
            e.translationX < -SWIPE_DISMISS_THRESHOLD ||
            e.velocityX < -SWIPE_VELOCITY_THRESHOLD
          ) {
            runOnJS(dismissHorizontal)('left');
          } else {
            runOnJS(resetPanAndRestartTimer)();
          }
        })
        .onFinalize((_e, success) => {
          if (success || dismissing.value) return;
          runOnJS(resetPanAndRestartTimer)();
        }),
    [
      clearDismissTimer,
      dismissing,
      dismissHorizontal,
      panX,
      panY,
      resetPanAndRestartTimer,
    ],
  );

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [
      { translateY: offsetY.value + panY.value },
      { translateX: panX.value },
    ],
  }));

  return (
    <GestureDetector gesture={gesture}>
      <Reanimated.View
        pointerEvents="auto"
        style={[styles.item, { top: insetsTop, zIndex }, animatedStyle]}
        onLayout={handleLayout}
      >
        <View
          style={[
            styles.toast,
            {
              backgroundColor: colors.surfaceTint,
              borderTopColor: accentColor,
              shadowColor: '#000000',
            },
          ]}
        >
          <View style={styles.content}>
            {type === 'loading' && (
              <ActivityIndicator size="small" color={colors.black} />
            )}
            <Text style={[styles.text, { color: colors.black }]}>
              {message}
            </Text>
          </View>
        </View>
      </Reanimated.View>
    </GestureDetector>
  );
}

export function useTopToast(): TopToastContextValue {
  const ctx = useContext(TopToastContext);
  if (!ctx) {
    throw new Error('useTopToast must be used within a TopToastProvider');
  }
  return ctx;
}

const styles = StyleSheet.create({
  item: {
    position: 'absolute',
    left: 16,
    right: 16,
    zIndex: 1000,
    elevation: 10,
  },
  toast: {
    borderTopWidth: 4,
    borderRadius: 12,
    paddingHorizontal: 16,
    paddingVertical: 12,
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.15,
    shadowRadius: 8,
  },
  content: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  text: {
    fontSize: 14,
    lineHeight: 20,
    flexShrink: 1,
  },
});
