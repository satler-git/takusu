// UndoRedoToast — listens to undoRedo callbacks and shows a Snackbar
// with the action description when undo/redo fires.
// Mounted once at the app root so it works across all views.

import { useEffect, useState } from 'react';
import { Snackbar } from 'react-native-paper';
import { undoRedo } from '@/src/api/undoRedo';

import { useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

export function UndoRedoToast() {
  const colors = useColors();
  const [visible, setVisible] = useState(false);
  const [message, setMessage] = useState('');

  useEffect(() => {
    function showUndo(description: string) {
      haptic.success();
      setMessage(`Undo: ${description}`);
      setVisible(true);
    }
    function showRedo(description: string) {
      haptic.success();
      setMessage(`Redo: ${description}`);
      setVisible(true);
    }
    undoRedo.setOnUndo(showUndo);
    undoRedo.setOnRedo(showRedo);
    return () => {
      undoRedo.setOnUndo(null);
      undoRedo.setOnRedo(null);
    };
  }, []);

  return (
    <Snackbar
      visible={visible}
      onDismiss={() => setVisible(false)}
      duration={2000}
      style={{ backgroundColor: colors.brand }}
      theme={{ colors: { onSurface: colors.onBrand } }}
    >
      {message}
    </Snackbar>
  );
}
