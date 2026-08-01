// useTaskProgress — shared logic for the TaskProgressSheet bottom sheet.
// Kept in a hook so state transitions and payload construction can be unit
// tested independently of React Native rendering.

import { useMemo, useState } from 'react';
import type { WorkSessionRow } from '@/src/api/types';
import { type ProgressPayload } from '@/src/utils/progress';

export type TaskProgressInputMode = 'delta' | 'cumulative';
export type TaskProgressAction = 'confirm' | 'record';

export interface UseTaskProgressArgs {
  session: WorkSessionRow;
  mode: 'record' | 'pause' | 'complete';
  allowToggle: boolean;
}

export interface UseTaskProgressReturn {
  inputMode: TaskProgressInputMode;
  qty: string;
  total: string;
  note: string;
  action: TaskProgressAction;
  afterDone: number;
  afterTotal: number;
  previewPct: number;
  primaryLabel: string;
  hintLabel: string;
  handleNoteChange: (text: string) => void;
  handleQtyChange: (text: string) => void;
  handleTotalChange: (text: string) => void;
  switchInputMode: (next: TaskProgressInputMode) => void;
  adjustQty: (delta: number) => void;
  toggleAction: () => void;
  buildPayload: () => ProgressPayload;
  reset: () => void;
}

export function digitsOnly(text: string): string {
  return text.replace(/[^0-9]/g, '');
}

export function parseQty(text: string): number {
  const n = parseInt(digitsOnly(text), 10);
  return Number.isNaN(n) ? 0 : n;
}

export function computeAfterDone(
  inputMode: TaskProgressInputMode,
  qty: string,
  currentDone: number,
): number {
  if (qty.trim() === '') {
    return currentDone;
  }
  const q = parseQty(qty);
  if (q === 0) {
    return currentDone;
  }
  if (inputMode === 'delta') {
    return currentDone + q;
  }
  return q;
}

export function computeTotal(total: string): number | undefined {
  if (total.trim() === '') {
    return undefined;
  }
  const t = parseQty(total);
  if (t <= 0) {
    return undefined;
  }
  return t;
}

export function buildProgressPayload(
  inputMode: TaskProgressInputMode,
  qty: string,
  total: string,
  note: string,
  currentDone: number,
): ProgressPayload {
  return {
    quantityDone: computeAfterDone(inputMode, qty, currentDone),
    note: note.trim() || undefined,
    quantityTotal: computeTotal(total),
  };
}

export function switchQtyForMode(
  inputMode: TaskProgressInputMode,
  nextMode: TaskProgressInputMode,
  qty: string,
  currentDone: number,
): string {
  const v = parseQty(qty);
  if (qty.trim() === '') {
    return nextMode === 'delta' ? '' : String(currentDone);
  }
  if (nextMode === 'delta') {
    return String(Math.max(0, v - currentDone));
  }
  return String(currentDone + v);
}

export function adjustQtyValue(
  inputMode: TaskProgressInputMode,
  qty: string,
  delta: number,
  currentDone: number,
): string {
  const base =
    qty.trim() === '' && inputMode === 'cumulative'
      ? currentDone
      : parseQty(qty);
  return String(Math.max(0, base + delta));
}

export function getPrimaryActionLabel(
  mode: 'record' | 'pause' | 'complete',
  action: TaskProgressAction,
): string {
  if (action === 'record') {
    return '記録';
  }
  switch (mode) {
    case 'pause':
      return '停止';
    case 'complete':
      return '完了';
    case 'record':
    default:
      return '記録';
  }
}

export function getHintLabel(
  mode: 'record' | 'pause' | 'complete',
  action: TaskProgressAction,
  allowToggle: boolean,
): string {
  if (!allowToggle) {
    return '';
  }
  const other = action === 'confirm' ? 'record' : 'confirm';
  if (other === 'record') {
    return '長押し: 記録';
  }
  switch (mode) {
    case 'pause':
      return '長押し: 停止';
    case 'record':
      return '長押し: 停止';
    default:
      return '';
  }
}

export function useTaskProgress({
  session,
  mode,
  allowToggle,
}: UseTaskProgressArgs): UseTaskProgressReturn {
  const currentDone = useMemo(
    () => session.quantity_done ?? 0,
    [session.quantity_done],
  );
  const currentTotal = useMemo(
    () => session.quantity_total ?? 0,
    [session.quantity_total],
  );

  const [inputMode, setInputMode] = useState<TaskProgressInputMode>('delta');
  const [qty, setQty] = useState('');
  const [total, setTotal] = useState(() =>
    currentTotal > 0 ? String(currentTotal) : '',
  );
  const [note, setNote] = useState('');
  const [action, setAction] = useState<TaskProgressAction>('confirm');

  const reset = () => {
    setInputMode('delta');
    setQty('');
    setTotal(currentTotal > 0 ? String(currentTotal) : '');
    setNote('');
    setAction('confirm');
  };

  const afterDone = useMemo(
    () => computeAfterDone(inputMode, qty, currentDone),
    [inputMode, qty, currentDone],
  );

  const afterTotal = useMemo(
    () => computeTotal(total) ?? currentTotal,
    [total, currentTotal],
  );

  const previewPct = useMemo(
    () =>
      afterTotal > 0
        ? Math.min(100, Math.max(0, (afterDone / afterTotal) * 100))
        : 0,
    [afterDone, afterTotal],
  );

  const primaryLabel = useMemo(
    () => getPrimaryActionLabel(mode, action),
    [mode, action],
  );

  const hintLabel = useMemo(
    () => getHintLabel(mode, action, allowToggle),
    [mode, action, allowToggle],
  );

  const handleNoteChange = (text: string) => setNote(text);
  const handleQtyChange = (text: string) => setQty(digitsOnly(text));
  const handleTotalChange = (text: string) => setTotal(digitsOnly(text));

  const switchInputMode = (next: TaskProgressInputMode) => {
    setInputMode(next);
    setQty((prev) => switchQtyForMode(inputMode, next, prev, currentDone));
  };

  const adjustQty = (delta: number) => {
    setQty((prev) => adjustQtyValue(inputMode, prev, delta, currentDone));
  };

  const toggleAction = () => {
    setAction((prev) => (prev === 'confirm' ? 'record' : 'confirm'));
  };

  const buildPayload = () =>
    buildProgressPayload(inputMode, qty, total, note, currentDone);

  return {
    inputMode,
    qty,
    total,
    note,
    action,
    afterDone,
    afterTotal,
    previewPct,
    primaryLabel,
    hintLabel,
    handleNoteChange,
    handleQtyChange,
    handleTotalChange,
    switchInputMode,
    adjustQty,
    toggleAction,
    buildPayload,
    reset,
  };
}
