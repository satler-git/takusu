// CrossFadeIcon — Ionicons wrapper that cross-fades between icon states
// with opacity (0 -> 1) and scale (0.25 -> 1). Uses no blur because RN core
// does not expose a simple image blur filter.

import { useEffect, useRef, useState } from 'react';
import { View, StyleSheet, type ViewStyle } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  withSpring,
} from 'react-native-reanimated';

interface CrossFadeIconState {
  name: keyof typeof Ionicons.glyphMap;
  color: string;
}

interface CrossFadeIconProps {
  name: keyof typeof Ionicons.glyphMap;
  size: number;
  color: string;
  style?: ViewStyle | ViewStyle[];
}

const springConfig = {
  stiffness: 300,
  damping: 35,
} as const;

export function CrossFadeIcon({
  name,
  size,
  color,
  style,
}: CrossFadeIconProps) {
  const currentRef = useRef<CrossFadeIconState>({ name, color });
  const [current, setCurrent] = useState<CrossFadeIconState>({ name, color });
  const [previous, setPrevious] = useState<CrossFadeIconState | null>(null);
  const progress = useSharedValue(1);

  useEffect(() => {
    if (
      currentRef.current.name !== name ||
      currentRef.current.color !== color
    ) {
      const old = currentRef.current;
      currentRef.current = { name, color };
      setCurrent({ name, color });
      setPrevious(old);
      progress.value = 0;
      progress.value = withSpring(1, springConfig);
    }
  }, [name, color, progress]);

  const fromStyle = useAnimatedStyle(() => ({
    opacity: 1 - progress.value,
    transform: [{ scale: 1 - 0.75 * progress.value }],
  }));

  const toStyle = useAnimatedStyle(() => ({
    opacity: progress.value,
    transform: [{ scale: 0.25 + 0.75 * progress.value }],
  }));

  return (
    <View style={[styles.container, { width: size, height: size }, style]}>
      {previous && (
        <Reanimated.View
          style={[StyleSheet.absoluteFill, styles.center, fromStyle]}
          pointerEvents="none"
        >
          <Ionicons name={previous.name} size={size} color={previous.color} />
        </Reanimated.View>
      )}
      <Reanimated.View
        style={[StyleSheet.absoluteFill, styles.center, toStyle]}
        pointerEvents="none"
      >
        <Ionicons name={current.name} size={size} color={current.color} />
      </Reanimated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  center: {
    alignItems: 'center',
    justifyContent: 'center',
  },
});
