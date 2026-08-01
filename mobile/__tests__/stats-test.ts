import { taskDateKey } from '@/src/utils/stats';
import type { ScheduleEntry, TaskRow } from '@/src/api/types';

describe('taskDateKey', () => {
  const makeTask = (partial: Partial<TaskRow>): TaskRow =>
    ({
      id: 'task-1',
      display_id: 1,
      title: 'Test',
      end_at: '2026-07-31T23:59:00Z',
      avg_minutes: 60,
      sigma_minutes: 5,
      depends: '[]',
      parallelizable: false,
      allows_parallel: false,
      abandonability: 0.5,
      status: 'pending',
      user_edited: false,
      fixed: false,
      quantity_done: 0,
      created_at: '2026-07-28T00:00:00Z',
      updated_at: '2026-07-28T00:00:00Z',
      ...partial,
    }) as TaskRow;

  it('uses end_at for non-skipped tasks', () => {
    const task = makeTask({ status: 'scheduled' });
    const scheduleMap = new Map<string, ScheduleEntry>();
    expect(taskDateKey(task, scheduleMap, 'UTC')).toBe('2026-07-31');
  });

  it("uses the task's start_at for a skipped task with a scheduled day", () => {
    const task = makeTask({
      status: 'skipped',
      start_at: '2026-07-25T10:00:00Z',
      end_at: '2026-07-31T23:59:00Z',
    });
    const scheduleMap = new Map<string, ScheduleEntry>();
    expect(taskDateKey(task, scheduleMap, 'UTC')).toBe('2026-07-25');
  });

  it('falls back to end_at for a skipped pending-style task with no start_at', () => {
    const task = makeTask({
      status: 'skipped',
      start_at: null,
      end_at: '2026-07-31T23:59:00Z',
    });
    const scheduleMap = new Map<string, ScheduleEntry>();
    expect(taskDateKey(task, scheduleMap, 'UTC')).toBe('2026-07-31');
  });

  it('skips empty start_at strings and falls back to end_at', () => {
    const task = makeTask({
      status: 'skipped',
      start_at: '',
      end_at: '2026-07-31T23:59:00Z',
    });
    const scheduleMap = new Map<string, ScheduleEntry>();
    expect(taskDateKey(task, scheduleMap, 'UTC')).toBe('2026-07-31');
  });

  it('prefers the schedule map start_at for a skipped task still in the schedule', () => {
    const task = makeTask({
      status: 'skipped',
      start_at: '2026-07-25T10:00:00Z',
      end_at: '2026-07-31T23:59:00Z',
    });
    const scheduleMap = new Map<string, ScheduleEntry>([
      [
        'task-1',
        {
          task_id: 'task-1',
          start_at: '2026-07-24T09:00:00Z',
          end_at: '2026-07-24T10:00:00Z',
        },
      ],
    ]);
    expect(taskDateKey(task, scheduleMap, 'UTC')).toBe('2026-07-24');
  });
});
