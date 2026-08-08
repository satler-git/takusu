import { act, fireEvent, render } from '@testing-library/react-native';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import { ThemeProvider } from '@/src/theme';
import { TaskProgressSheet } from '@/src/components/TaskProgressSheet';
import type { WorkSessionRow } from '@/src/api/types';

const safeAreaMetrics = {
  insets: { top: 0, left: 0, right: 0, bottom: 0 },
  frame: { x: 0, y: 0, width: 400, height: 800 },
};

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

function TestWrapper({ children }: { children: React.ReactNode }) {
  return (
    <SafeAreaProvider initialMetrics={safeAreaMetrics}>
      <ThemeProvider theme="light">{children}</ThemeProvider>
    </SafeAreaProvider>
  );
}

async function renderSheet(
  props: Partial<React.ComponentProps<typeof TaskProgressSheet>> = {},
) {
  const session = makeSession();
  const onConfirm = jest.fn();
  const onRecord = jest.fn();
  const onCancel = jest.fn();
  const rendered = await render(
    <TaskProgressSheet
      visible
      session={session}
      mode="pause"
      onConfirm={onConfirm}
      onRecord={onRecord}
      onCancel={onCancel}
      {...props}
    />,
    { wrapper: TestWrapper },
  );
  return { ...rendered, onConfirm, onRecord, onCancel, session };
}

describe('TaskProgressSheet', () => {
  beforeEach(() => {
    jest.useRealTimers();
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  it('calls onConfirm for a short press', async () => {
    const { getByLabelText, onConfirm, onRecord } = await renderSheet();

    const button = getByLabelText('停止');
    await fireEvent.press(button);

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onRecord).not.toHaveBeenCalled();
  });

  it('calls onRecord for a long press', async () => {
    jest.useFakeTimers();
    const { getByLabelText, onConfirm, onRecord } = await renderSheet();

    const button = getByLabelText('停止');

    await fireEvent(button, 'onPressIn');
    await act(() => {
      jest.advanceTimersByTime(600);
    });
    await fireEvent(button, 'onPressOut');
    await fireEvent(button, 'onPress');

    expect(onRecord).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('resets to confirm after cancelling a long press so the next tap calls onConfirm', async () => {
    jest.useFakeTimers();
    const { getByLabelText, onConfirm, onRecord } = await renderSheet();

    const button = getByLabelText('停止');

    // Long-press, but release outside (no onPress)
    await fireEvent(button, 'onPressIn');
    await act(() => {
      jest.advanceTimersByTime(600);
    });
    await fireEvent(button, 'onPressOut');

    expect(onConfirm).not.toHaveBeenCalled();
    expect(onRecord).not.toHaveBeenCalled();

    // Next short tap should be a confirm, not a record
    await fireEvent(button, 'onPressIn');
    await fireEvent(button, 'onPressOut');
    await fireEvent(button, 'onPress');

    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(onRecord).not.toHaveBeenCalled();
  });

  it('calls onRecord again after a completed long press and a second long press', async () => {
    jest.useFakeTimers();
    const { getByLabelText, onRecord } = await renderSheet();

    const button = getByLabelText('停止');

    for (let i = 0; i < 2; i += 1) {
      await fireEvent(button, 'onPressIn');
      await act(() => {
        jest.advanceTimersByTime(600);
      });
      await fireEvent(button, 'onPressOut');
      await fireEvent(button, 'onPress');
    }

    expect(onRecord).toHaveBeenCalledTimes(2);
  });
});
