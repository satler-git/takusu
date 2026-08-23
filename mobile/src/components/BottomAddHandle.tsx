// BottomAddHandle — bottom-centre pill that opens the task add screen.
// Tappable and draggable upward for the same action. Not a round button.

import { useMemo, useCallback } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  withSpring,
  withTiming,
  runOnJS,
} from 'react-native-reanimated';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

interface BottomAddHandleProps {
  onAdd: () => void;
}

const SLIDE_UP_THRESHOLD = 60;
const SPRING_CONFIG = { stiffness: 300, damping: 25 } as const;

export function BottomAddHandle({ onAdd }: BottomAddHandleProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();

  const translateY = useSharedValue(0);
  const pressed = useSharedValue(0);

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [
      { translateY: translateY.value },
      { scale: 1 - 0.04 * pressed.value },
    ],
  }));

  const handleAdd = useCallback(() => {
    haptic.light();
    onAdd();
  }, [onAdd]);

  const panGesture = useMemo(
    () =>
      Gesture.Pan()
        .enabled(true)
        .activeOffsetY([-10, 10])
        .failOffsetX([-20, 20])
        .onBegin(() => {
          'worklet';
          pressed.value = withTiming(1, { duration: 80 });
        })
        .onUpdate((e) => {
          'worklet';
          translateY.value = Math.min(0, e.translationY);
        })
        .onEnd((e) => {
          'worklet';
          translateY.value = withSpring(0, SPRING_CONFIG);
          if (e.translationY < -SLIDE_UP_THRESHOLD) {
            runOnJS(handleAdd)();
          }
        })
        .onFinalize((_e, success) => {
          'worklet';
          pressed.value = withTiming(0, { duration: 120 });
          if (!success) {
            translateY.value = withSpring(0, SPRING_CONFIG);
          }
        }),
    [handleAdd, pressed, translateY],
  );

  const tapGesture = useMemo(
    () =>
      Gesture.Tap()
        .onBegin(() => {
          'worklet';
          pressed.value = withTiming(1, { duration: 80 });
        })
        .onEnd(() => {
          'worklet';
          runOnJS(handleAdd)();
        })
        .onFinalize(() => {
          'worklet';
          pressed.value = withTiming(0, { duration: 120 });
        }),
    [handleAdd, pressed],
  );

  const gesture = useMemo(
    () => Gesture.Exclusive(panGesture, tapGesture),
    [panGesture, tapGesture],
  );

  return (
    <View style={[styles.container, { bottom: 16 + insets.bottom }]}>
      <GestureDetector gesture={gesture}>
        <Reanimated.View
          style={[styles.handle, animatedStyle]}
          accessible
          accessibilityRole="button"
          accessibilityLabel="タスクを追加"
          accessibilityHint="タップまたは上にスライドして新しいタスクを追加"
        >
          <View style={styles.bar} />
          <Text style={[styles.label, { color: colors.black }]}>
            タスクを追加
          </Text>
        </Reanimated.View>
      </GestureDetector>
    </View>
  );
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      position: 'absolute',
      start: 0,
      end: 0,
      alignItems: 'center',
    },
    handle: {
      alignItems: 'center',
      gap: 6,
      paddingHorizontal: 20,
      paddingVertical: 8,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.separator,
      backgroundColor: colors.surface,
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 2 },
      shadowOpacity: 0.15,
      shadowRadius: 3,
      elevation: 3,
    },
    bar: {
      width: 36,
      height: 4,
      borderRadius: 2,
      backgroundColor: colors.separator,
    },
    label: {
      fontSize: 12,
      fontWeight: '600',
    },
  });
