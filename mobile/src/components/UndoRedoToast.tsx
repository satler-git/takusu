// UndoRedoToast — listens to undoRedo callbacks and shows a standard top
// toast with the action description when undo/redo fires.
// Mounted once at the app root so it works across all views.

import { useEffect, useRef } from 'react';
import { useTopToast } from '@/src/components/TopToast';
import { undoRedo } from '@/src/api/undoRedo';
import { haptic } from '@/src/components/haptics';

const UNDO_REDO_TOAST_DURATION = 2000;

export function UndoRedoToast() {
  const { showTopToast, hideTopToast } = useTopToast();
  const toastIdRef = useRef<string | null>(null);

  useEffect(() => {
    function show(description: string, prefix: string) {
      haptic.success();
      if (toastIdRef.current) {
        hideTopToast(toastIdRef.current);
      }
      toastIdRef.current = showTopToast(`${prefix}: ${description}`, {
        type: 'info',
        duration: UNDO_REDO_TOAST_DURATION,
      });
    }

    function showUndo(description: string) {
      show(description, 'Undo');
    }

    function showRedo(description: string) {
      show(description, 'Redo');
    }

    undoRedo.setOnUndo(showUndo);
    undoRedo.setOnRedo(showRedo);

    return () => {
      undoRedo.setOnUndo(null);
      undoRedo.setOnRedo(null);
    };
  }, [showTopToast, hideTopToast]);

  return null;
}
