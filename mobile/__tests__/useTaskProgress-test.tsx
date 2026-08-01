import { act, renderHook, waitFor } from '@testing-library/react-native';
import { useTaskProgress } from '@/src/hooks/useTaskProgress';
import type { WorkSessionRow } from '@/src/api/types';

function makeSession(overrides?: Partial<WorkSessionRow>): WorkSessionRow {
  return {
    id: 'session-1',
    task_id: 'task-1',
    title: '作業',
    quantity_done: 12,
    quantity_total: 20,
    quantity_unit: 'ページ',
    started_at: '2026-06-01T10:00:00Z',
    created_at: '2026-06-01T10:00:00Z',
    ...overrides,
  };
}

function makeInitialProps(
  overrides: {
    session?: WorkSessionRow;
    mode?: 'record' | 'pause' | 'complete';
    allowToggle?: boolean;
  } = {},
) {
  return {
    session: overrides.session ?? makeSession(),
    mode: overrides.mode ?? 'record',
    allowToggle: overrides.allowToggle ?? true,
  };
}

async function setup(
  overrides: {
    session?: WorkSessionRow;
    mode?: 'record' | 'pause' | 'complete';
    allowToggle?: boolean;
  } = {},
) {
  return renderHook(
    (props: ReturnType<typeof makeInitialProps>) => useTaskProgress(props),
    { initialProps: makeInitialProps(overrides) },
  );
}

describe('useTaskProgress', () => {
  it('builds a delta payload', async () => {
    const { result } = await setup();

    await act(() => {
      result.current.handleQtyChange('3');
      result.current.handleNoteChange('done');
      result.current.handleTotalChange('25');
    });

    await waitFor(() => {
      expect(result.current.qty).toBe('3');
    });

    expect(result.current.buildPayload()).toEqual({
      quantityDone: 15,
      note: 'done',
      quantityTotal: 25,
    });
  });

  it('builds a cumulative payload', async () => {
    const { result } = await setup();

    await act(() => {
      result.current.switchInputMode('cumulative');
      result.current.handleQtyChange('18');
    });

    expect(result.current.buildPayload()).toEqual({
      quantityDone: 18,
      note: undefined,
      quantityTotal: 20,
    });
  });

  it('builds a payload with quantityTotal when it matches current', async () => {
    const { result } = await setup();

    await act(() => {
      result.current.handleQtyChange('5');
      result.current.handleTotalChange('20');
    });

    expect(result.current.buildPayload()).toEqual({
      quantityDone: 17,
      note: undefined,
      quantityTotal: 20,
    });
  });

  it('adjusts the quantity with +/-', async () => {
    const { result } = await setup();

    await act(() => {
      result.current.adjustQty(1);
      result.current.adjustQty(1);
    });

    expect(result.current.qty).toBe('2');
    expect(result.current.afterDone).toBe(14);
  });

  it('switches between delta and cumulative while preserving the real quantity', async () => {
    const { result } = await setup();

    await act(() => {
      result.current.handleQtyChange('3');
    });
    expect(result.current.qty).toBe('3');
    expect(result.current.afterDone).toBe(15);

    await act(() => {
      result.current.switchInputMode('cumulative');
    });
    expect(result.current.qty).toBe('15');
    expect(result.current.inputMode).toBe('cumulative');

    await act(() => {
      result.current.switchInputMode('delta');
    });
    expect(result.current.qty).toBe('3');
    expect(result.current.inputMode).toBe('delta');
  });

  it('starts with a seeded total from the session', async () => {
    const { result } = await setup();
    expect(result.current.total).toBe('20');
    expect(result.current.afterTotal).toBe(20);
  });

  it('clears inputs when reset', async () => {
    const { result } = await setup();

    await act(() => {
      result.current.handleQtyChange('5');
      result.current.handleNoteChange('note');
      result.current.handleTotalChange('30');
      result.current.switchInputMode('cumulative');
      result.current.toggleAction();
    });

    await act(() => {
      result.current.reset();
    });

    expect(result.current.qty).toBe('');
    expect(result.current.note).toBe('');
    expect(result.current.total).toBe('20');
    expect(result.current.inputMode).toBe('delta');
    expect(result.current.action).toBe('confirm');
  });

  it('resets to the new session total', async () => {
    const { result } = await setup({
      session: makeSession({ quantity_total: 10 }),
    });

    await act(() => {
      result.current.handleQtyChange('5');
      result.current.handleTotalChange('30');
    });
    expect(result.current.qty).toBe('5');
    expect(result.current.total).toBe('30');

    await act(() => {
      result.current.reset();
    });

    expect(result.current.qty).toBe('');
    expect(result.current.total).toBe('10');
    expect(result.current.afterDone).toBe(12);
  });

  it('toggles action and updates labels', async () => {
    const { result } = await setup({ mode: 'pause' });

    expect(result.current.primaryLabel).toBe('停止');
    expect(result.current.hintLabel).toBe('長押し: 記録');

    await act(() => {
      result.current.toggleAction();
    });

    expect(result.current.action).toBe('record');
    expect(result.current.primaryLabel).toBe('記録');
    expect(result.current.hintLabel).toBe('長押し: 停止');
  });

  it('computes preview percentage', async () => {
    const { result } = await setup({
      session: makeSession({ quantity_done: 5 }),
    });

    await act(() => {
      result.current.handleQtyChange('5');
    });

    expect(result.current.afterDone).toBe(10);
    expect(result.current.afterTotal).toBe(20);
    expect(result.current.previewPct).toBe(50);
  });
});
