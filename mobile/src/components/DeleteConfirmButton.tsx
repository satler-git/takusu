// DeleteConfirmButton — two-tap delete button to prevent accidental deletes.
// First tap arms (red background, trash icon), second tap fires onConfirm.
// Auto-disarms after 3s.

import { useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

interface DeleteConfirmButtonProps {
  onConfirm: () => void;
  size?: number;
  iconSize?: number;
  hitSlop?: number;
  disabled?: boolean;
}

export function DeleteConfirmButton({
  onConfirm,
  size = 40,
  iconSize = 22,
  hitSlop,
  disabled,
}: DeleteConfirmButtonProps) {
  const colors = useColors();
  const [armed, setArmed] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (disabled && armed) {
      if (timerRef.current) clearTimeout(timerRef.current);
      setArmed(false);
    }
  }, [disabled, armed]);

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  const iconName = disabled
    ? 'trash-outline'
    : armed
      ? 'trash'
      : 'trash-outline';
  const iconColor = disabled ? colors.gray : armed ? colors.white : colors.red;
  const backgroundStyle = disabled
    ? { opacity: 0.4 }
    : armed
      ? { backgroundColor: colors.red }
      : undefined;

  return (
    <Pressable
      style={[
        styles.button,
        {
          width: size,
          height: size,
          borderRadius: size / 2,
        },
        backgroundStyle,
      ]}
      hitSlop={hitSlop}
      disabled={disabled}
      onPress={() => {
        if (armed) {
          if (timerRef.current) clearTimeout(timerRef.current);
          onConfirm();
          setArmed(false);
        } else {
          haptic.medium();
          setArmed(true);
          timerRef.current = setTimeout(() => setArmed(false), 3000);
        }
      }}
    >
      <Ionicons name={iconName} size={iconSize} color={iconColor} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: {
    alignItems: 'center',
    justifyContent: 'center',
  },
});
