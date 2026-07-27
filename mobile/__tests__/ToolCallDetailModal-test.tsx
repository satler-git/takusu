import { render } from '@testing-library/react-native';
import { COLORS, type ColorSet } from '@/src/theme';
import { ResultContent } from '@/src/components/ToolCallDetailModal';

const colors = COLORS as ColorSet;

describe('ResultContent', () => {
  it('renders list_tasks as a task result view for a JSON array', async () => {
    const { getByText, queryByText } = await render(
      <ResultContent
        name="list_tasks"
        result={JSON.stringify([
          {
            reference: '#42',
            title: 'test task',
            status: 'in_progress',
            start_at: '2026-07-27T10:00:00+09:00',
            end_at: '2026-07-27T11:00:00+09:00',
            avg_minutes: 60,
          },
        ])}
        isRejected={false}
        colors={colors}
      />,
    );
    expect(getByText('[#42] test task')).toBeTruthy();
    expect(queryByText('該当するタスクが見つかりませんでした。')).toBeNull();
  });

  it('renders list_tasks string result as a raw value fallback', async () => {
    const { getByText, queryByText } = await render(
      <ResultContent
        name="list_tasks"
        result="not a task result"
        isRejected={false}
        colors={colors}
      />,
    );
    expect(getByText('not a task result')).toBeTruthy();
    expect(queryByText('該当するタスクが見つかりませんでした。')).toBeNull();
  });

  it('renders list_habits string result as a raw value fallback', async () => {
    const { getByText, queryByText } = await render(
      <ResultContent
        name="list_habits"
        result="not a habit result"
        isRejected={false}
        colors={colors}
      />,
    );
    expect(getByText('not a habit result')).toBeTruthy();
    expect(queryByText('該当する習慣が見つかりませんでした。')).toBeNull();
  });
});
