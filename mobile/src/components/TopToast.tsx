// Top toast — auto-dismissing banner from the top of the screen.
// Multiple toasts stack downward from a fixed top corner: new toasts slide
// in from the right edge into their slot, while earlier toasts keep their
// positions. Swiping any toast sideways dismisses it.
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
  Pressable,
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
import { setTopToastRef } from '@/src/api/topToastRef';

const DEFAULT_DURATION = 3000;
const OFFSCREEN_MARGIN = 50;
const SWIPE_DISMISS_THRESHOLD = 50;
const SWIPE_VELOCITY_THRESHOLD = 300;
const ESTIMATED_HEIGHT = 64;
const GAP = 8;

export type ToastType = 'info' | 'success' | 'error' | 'loading';

export interface ToastAction {
  label: string;
  onPress: () => void | Promise<void>;
}

export interface ToastOptions {
  type?: ToastType;
  duration?: number;
  action?: ToastAction;
  swipeable?: boolean;
  onDismiss?: () => void;
}

interface Toast {
  id: string;
  message: string;
  type: ToastType;
  duration: number;
  action?: ToastAction;
  swipeable: boolean;
  onDismiss?: () => void;
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
          action: opts.action,
          swipeable: opts.swipeable ?? true,
          onDismiss: opts.onDismiss,
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

  useEffect(() => {
    setTopToastRef(value);
    return () => {
      setTopToastRef(null);
    };
  }, [value]);

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
          action={toast.action}
          swipeable={toast.swipeable}
          onDismissed={toast.onDismiss}
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
  action?: ToastAction;
  swipeable: boolean;
  onDismissed?: () => void;
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
  action,
  swipeable,
  onDismissed,
  onLayout,
  onDismiss,
  onRegisterDismiss,
  onUnregisterDismiss,
}: ToastItemProps) {
  const { width: screenWidth } = useWindowDimensions();
  const offsetY = useSharedValue(offset);
  const offsetYTarget = useSharedValue(offset);
  const panY = useSharedValue(0);
  const panX = useSharedValue(0);
  // Enter from the right edge so the toast slides in horizontally instead of
  // appearing from the middle of the screen (#1176).
  const enterX = useSharedValue(screenWidth);
  const dismissing = useSharedValue(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const offsetRef = useRef(offset);
  const heightRef = useRef(height);
  offsetRef.current = offset;
  heightRef.current = height;

  const accentColor = useMemo(() => {
    switch (type) {
      case 'success':
        return colors.success;
      case 'error':
        return colors.error;
      case 'loading':
        return colors.gray;
      case 'info':
      default:
        return colors.brand;
    }
  }, [colors.brand, colors.error, colors.gray, colors.success, type]);

  const clearDismissTimer = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const notifyDismissed = useCallback(() => {
    onDismiss(id);
    onDismissed?.();
  }, [id, onDismiss, onDismissed]);

  const dismiss = useCallback(() => {
    if (dismissing.value) return;
    dismissing.value = true;
    clearDismissTimer();
    cancelAnimation(panX);
    cancelAnimation(enterX);
    enterX.value = 0;
    panX.value = 0;
    const target =
      -offsetRef.current - heightRef.current - insetsTop - OFFSCREEN_MARGIN;
    panY.value = withTiming(
      target,
      { duration: 200, easing: Easing.out(Easing.ease) },
      (finished) => {
        'worklet';
        if (finished) runOnJS(notifyDismissed)();
      },
    );
  }, [
    clearDismissTimer,
    dismissing,
    enterX,
    insetsTop,
    notifyDismissed,
    panX,
    panY,
  ]);

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
          if (finished) runOnJS(notifyDismissed)();
        },
      );
    },
    [clearDismissTimer, dismissing, notifyDismissed, panX, screenWidth],
  );

  const startDismissTimer = useCallback(() => {
    clearDismissTimer();
    if (Number.isFinite(duration) && duration > 0) {
      timerRef.current = setTimeout(() => dismiss(), duration);
    }
  }, [clearDismissTimer, dismiss, duration]);

  const resetPanAndRestartTimer = useCallback(() => {
    if (dismissing.value) return;
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
  }, [dismissing, panX, panY, offsetY, offsetYTarget, startDismissTimer]);

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

  // Slide in from the right edge on mount (#1176).
  useEffect(() => {
    enterX.value = withTiming(0, {
      duration: 250,
      easing: Easing.out(Easing.ease),
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
        .enabled(swipeable)
        .activeOffsetX([-10, 10])
        .failOffsetY([-15, 15])
        .onBegin(() => {
          cancelAnimation(panX);
          cancelAnimation(panY);
          cancelAnimation(offsetY);
          cancelAnimation(enterX);
          enterX.value = 0;
          panX.value = 0;
          panY.value = 0;
          dismissing.value = false;
          runOnJS(clearDismissTimer)();
        })
        .onUpdate((e) => {
          panX.value = e.translationX;
        })
        .onEnd((e, success) => {
          if (!success || dismissing.value) return;
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
      enterX,
      offsetY,
      panX,
      panY,
      resetPanAndRestartTimer,
      swipeable,
    ],
  );

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [
      { translateY: offsetY.value + panY.value },
      { translateX: panX.value + enterX.value },
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
              shadowColor: colors.shadow,
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
          {action && (
            <Pressable
              onPress={action.onPress}
              style={styles.action}
              hitSlop={8}
              accessibilityRole="button"
              accessibilityLabel={action.label}
            >
              <Text style={[styles.actionText, { color: colors.brand }]}>
                {action.label}
              </Text>
            </Pressable>
          )}
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
    flexDirection: 'row',
    alignItems: 'center',
    shadowOffset: { width: 0, height: 4 },
    shadowOpacity: 0.15,
    shadowRadius: 8,
  },
  content: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  text: {
    fontSize: 14,
    lineHeight: 20,
    flexShrink: 1,
  },
  action: {
    paddingHorizontal: 4,
    paddingVertical: 4,
    marginStart: 12,
  },
  actionText: {
    fontSize: 14,
    lineHeight: 20,
    fontWeight: '600',
  },
});
