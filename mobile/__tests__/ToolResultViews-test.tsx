import { render } from '@testing-library/react-native';
import { COLORS, type ColorSet } from '@/src/theme';
import {
  TaskResultView,
  HabitResultView,
} from '@/src/components/ToolResultViews';

const colors = COLORS as ColorSet;

const baseTask = {
  reference: '#42',
  title: 'test task',
  status: 'in_progress',
  start_at: '2026-07-27T10:00:00+09:00',
  end_at: '2026-07-27T11:00:00+09:00',
  avg_minutes: 60,
  sigma_minutes: 10,
  quantity_total: 10,
  quantity_done: 3,
  quantity_unit: 'pages',
  depends: ['#1'],
  description: 'desc',
};

describe('TaskResultView', () => {
  it('renders a list of tasks', async () => {
    const { getByText } = await render(
      <TaskResultView data={[baseTask]} colors={colors} />,
    );
    expect(getByText('[#42] test task')).toBeTruthy();
    expect(getByText('進行中')).toBeTruthy();
    expect(getByText('1時間 (σ 10分)')).toBeTruthy();
    expect(getByText('3/10 pages')).toBeTruthy();
  });

  it('renders overdue status label', async () => {
    const { getByText } = await render(
      <TaskResultView
        data={[{ ...baseTask, status: 'overdue' }]}
        colors={colors}
      />,
    );
    expect(getByText('[#42] test task')).toBeTruthy();
    expect(getByText('遅延')).toBeTruthy();
  });

  it('renders get_task result with dependencies and missing dependencies', async () => {
    const { getByText } = await render(
      <TaskResultView
        data={{
          tasks: [baseTask],
          dependencies: [
            {
              reference: '#1',
              title: 'dep task',
              status: 'pending',
              end_at: '2026-07-27T12:00:00+09:00',
              avg_minutes: 30,
            },
          ],
          missing_dependencies: ['#99'],
        }}
        colors={colors}
      />,
    );
    expect(getByText('[#42] test task')).toBeTruthy();
    expect(getByText('依存タスク')).toBeTruthy();
    expect(getByText('[#1] dep task')).toBeTruthy();
    expect(getByText('未解決の依存')).toBeTruthy();
    expect(getByText('#99')).toBeTruthy();
  });

  it('renders empty state for an empty task list', async () => {
    const { getByText } = await render(
      <TaskResultView data={[]} colors={colors} />,
    );
    expect(getByText('該当するタスクが見つかりませんでした。')).toBeTruthy();
  });

  it('renders empty state for non-array non-record data', async () => {
    const { getByText } = await render(
      <TaskResultView data="error text" colors={colors} />,
    );
    expect(getByText('該当するタスクが見つかりませんでした。')).toBeTruthy();
  });
});

describe('HabitResultView', () => {
  it('renders a list of habits', async () => {
    const { getByText } = await render(
      <HabitResultView
        data={[
          {
            reference: 'h1',
            title: 'running',
            recurrence: 'FREQ=DAILY',
            active: true,
            fixed: false,
            steps: [
              {
                position: 1,
                title: 'warmup',
                start_time: '09:00',
                end_time: '09:10',
                avg_minutes: 10,
              },
            ],
          },
        ]}
        colors={colors}
      />,
    );
    expect(getByText('[h1] running')).toBeTruthy();
    expect(getByText('毎日')).toBeTruthy();
    expect(getByText('1. warmup')).toBeTruthy();
    expect(getByText('09:00 〜 09:10 / 10分')).toBeTruthy();
  });

  it('renders get_habit result with habits array', async () => {
    const { getByText } = await render(
      <HabitResultView
        data={{
          habits: [
            {
              reference: 'h2',
              title: 'reading',
              recurrence: 'FREQ=WEEKLY',
              active: true,
              fixed: true,
            },
          ],
        }}
        colors={colors}
      />,
    );
    expect(getByText('[h2] reading')).toBeTruthy();
    expect(getByText('毎週')).toBeTruthy();
  });

  it('renders empty state for an empty habit list', async () => {
    const { getByText } = await render(
      <HabitResultView data={[]} colors={colors} />,
    );
    expect(getByText('該当する習慣が見つかりませんでした。')).toBeTruthy();
  });
});
