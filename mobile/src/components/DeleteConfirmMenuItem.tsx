// DeleteConfirmMenuItem — two-tap Menu.Item for delete actions.
// First tap arms the item (red label, filled icon), second tap fires onConfirm.
// Auto-disarms after 3s and resets when the parent menu closes.

import { useEffect, useRef, useState } from 'react';
import { Menu } from 'react-native-paper';
import { useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

interface DeleteConfirmMenuItemProps {
  onConfirm: () => void;
  visible?: boolean;
}

export function DeleteConfirmMenuItem({
  onConfirm,
  visible,
}: DeleteConfirmMenuItemProps) {
  const colors = useColors();
  const [armed, setArmed] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!visible) {
      setArmed(false);
      if (timerRef.current) clearTimeout(timerRef.current);
    }
  }, [visible]);

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  return (
    <Menu.Item
      onPress={() => {
        if (armed) {
          if (timerRef.current) clearTimeout(timerRef.current);
          onConfirm();
          // Parent menu will close on confirm; `visible` effect resets armed.
          return;
        } else {
          haptic.medium();
          setArmed(true);
          timerRef.current = setTimeout(() => setArmed(false), 3000);
        }
      }}
      title={armed ? 'もう一度タップして削除' : '削除'}
      leadingIcon={armed ? 'trash-can' : 'trash-can-outline'}
      titleStyle={armed ? { color: colors.red } : undefined}
      style={armed ? { backgroundColor: colors.destructiveBg } : undefined}
    />
  );
}
