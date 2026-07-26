// CancelConfirmButton — two-tap cancel button to prevent accidental discards.
// First tap arms (red, trash icon), second tap fires onConfirm.
// Auto-disarms after 3s.

import { useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

export function CancelConfirmButton({ onConfirm }: { onConfirm: () => void }) {
  const colors = useColors();
  const [armed, setArmed] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  return (
    <Pressable
      style={[styles.button, armed && { backgroundColor: colors.red }]}
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
      <Ionicons
        name={armed ? 'trash' : 'close'}
        size={22}
        color={armed ? colors.white : colors.red}
      />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
});
