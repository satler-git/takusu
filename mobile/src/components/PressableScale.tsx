// PressableScale — Pressable wrapper that adds a subtle scale(0.96) on press.
// Use this for icon and text buttons where the design calls for tactile feedback.

import { useMemo, useState, forwardRef, type ReactNode } from 'react';
import {
  Pressable,
  type PressableProps,
  type PressableStateCallbackType,
  type ViewStyle,
  View,
  StyleSheet,
} from 'react-native';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  withSpring,
} from 'react-native-reanimated';

interface PressableScaleProps extends PressableProps {
  activeScale?: number;
}

const LAYOUT_KEYS: ReadonlyArray<keyof ViewStyle> = [
  'flexDirection',
  'alignItems',
  'justifyContent',
  'gap',
];

function isValidStyle(
  style: unknown,
): style is
  | ViewStyle
  | ViewStyle[]
  | ((state: PressableStateCallbackType) => ViewStyle | ViewStyle[]) {
  return (
    style != null &&
    typeof style !== 'boolean' &&
    typeof style !== 'string' &&
    typeof style !== 'number'
  );
}

function resolveStyle(style: unknown, pressed: boolean): ViewStyle {
  if (!isValidStyle(style)) return {};
  const resolved = typeof style === 'function' ? style({ pressed }) : style;
  return StyleSheet.flatten(resolved as ViewStyle | ViewStyle[] | undefined);
}

function isPositiveNumber(value: unknown): boolean {
  return typeof value === 'number' && value > 0;
}

function isPositiveDimension(value: ViewStyle['width']): boolean {
  if (value == null || value === 'auto') return false;
  if (typeof value === 'number') return value > 0;
  if (typeof value === 'string') return true;
  // Animated/SharedValue nodes are not a statically known size; treat as false.
  return false;
}

function hasExplicitMainSize(style: ViewStyle): boolean {
  return (
    isPositiveDimension(style.width) ||
    isPositiveDimension(style.height) ||
    isPositiveNumber(style.flex) ||
    isPositiveNumber(style.flexGrow) ||
    isPositiveNumber(style.flexBasis)
  );
}

function childLayoutStyle(style: ViewStyle): ViewStyle {
  const child: ViewStyle = hasExplicitMainSize(style) ? { flex: 1 } : {};
  for (const key of LAYOUT_KEYS) {
    const value = style[key];
    if (value !== undefined) {
      (child as Record<string, unknown>)[key as string] = value;
    }
  }
  return child;
}

const springConfig = {
  stiffness: 300,
  damping: 35,
} as const;

export const PressableScale = forwardRef<View, PressableScaleProps>(
  function PressableScale(
    {
      children,
      style,
      activeScale = 0.96,
      onPressIn,
      onPressOut,
      ...props
    }: PressableScaleProps,
    ref,
  ) {
    const [pressed, setPressed] = useState(false);
    const scale = useSharedValue(1);
    const animatedStyle = useAnimatedStyle(() => ({
      transform: [{ scale: scale.value }],
    }));

    const resolvedStyle = useMemo(
      () => resolveStyle(style, pressed),
      [style, pressed],
    );
    const contentLayoutStyle = useMemo(
      () => childLayoutStyle(resolvedStyle),
      [resolvedStyle],
    );

    const childContent: ReactNode =
      typeof children === 'function'
        ? (children as (state: PressableStateCallbackType) => ReactNode)({
            pressed,
          })
        : children;

    return (
      <Pressable
        ref={ref}
        {...props}
        style={style}
        onPressIn={(e) => {
          if (!props.disabled) {
            scale.value = withSpring(activeScale, springConfig);
          }
          setPressed(true);
          onPressIn?.(e);
        }}
        onPressOut={(e) => {
          if (!props.disabled) {
            scale.value = withSpring(1, springConfig);
          }
          setPressed(false);
          onPressOut?.(e);
        }}
      >
        <Reanimated.View style={[contentLayoutStyle, animatedStyle]}>
          {childContent}
        </Reanimated.View>
      </Pressable>
    );
  },
);
