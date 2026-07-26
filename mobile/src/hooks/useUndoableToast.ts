import { useCallback } from 'react';
import { useTopToast } from '@/src/components/TopToast';
import { undoRedo } from '@/src/api/undoRedo';
import { showError } from '@/src/api/errors';

const UNDO_TOAST_DURATION = 5000;

export function useUndoableToast(): (message: string) => string {
  const { showTopToast, hideTopToast } = useTopToast();

  return useCallback(
    (message: string) => {
      let toastId = '';
      toastId = showTopToast(message, {
        type: 'success',
        duration: UNDO_TOAST_DURATION,
        swipeable: false,
        action: {
          label: '元に戻す',
          onPress: () => {
            hideTopToast(toastId);
            undoRedo
              .undo({ silent: true })
              .catch((e) => showError(e, '削除の取り消しに失敗'));
          },
        },
      });
      return toastId;
    },
    [showTopToast, hideTopToast],
  );
}
