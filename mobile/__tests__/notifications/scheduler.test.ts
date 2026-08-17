process.env.TZ = 'UTC';

jest.mock('@react-native-async-storage/async-storage', () => ({
  setItem: jest.fn(),
  getItem: jest.fn(),
  removeItem: jest.fn(),
}));

import AsyncStorage from '@react-native-async-storage/async-storage';
import { Appearance } from 'react-native';
import { rescheduleNotifications } from '@/src/notifications/scheduler';
import type { ScheduleData } from '@/src/notifications/scheduler';
import type { TaskRow, ScheduleEntry } from '@/src/api/types';
import type { NotificationSettings } from '@/src/notifications/settings';
import { COLORS, DARK_COLORS } from '@/src/theme';

const scheduled: Array<Record<string, unknown>> = [];
const asyncStorageGetItem = AsyncStorage.getItem as jest.Mock;

jest.mock('expo-notifications', () => ({
  cancelAllScheduledNotificationsAsync: jest.fn().mockResolvedValue(undefined),
  scheduleNotificationAsync: jest.fn(async (request) => {
    scheduled.push(request as Record<string, unknown>);
    return 'notification-id';
  }),
  SchedulableTriggerInputTypes: {
    DATE: 'date',
  },
}));

describe('rescheduleNotifications', () => {
  beforeEach(() => {
    jest.useFakeTimers({ now: new Date('2026-07-23T10:00:00Z').getTime() });
    scheduled.length = 0;
    asyncStorageGetItem.mockReset();
    asyncStorageGetItem.mockResolvedValue(null);
    jest.spyOn(Appearance, 'getColorScheme').mockReturnValue(null);
  });

  afterEach(() => {
    jest.useRealTimers();
    jest.restoreAllMocks();
  });

  const baseTask: Omit<
    TaskRow,
    | 'id'
    | 'title'
    | 'status'
    | 'start_at'
    | 'end_at'
    | 'created_at'
    | 'updated_at'
  > = {
    display_id: 1,
    description: '',
    avg_minutes: 60,
    sigma_minutes: 0,
    depends: '[]',
    parallelizable: false,
    allows_parallel: false,
    abandonability: 0.5,
    user_edited: false,
    fixed: false,
    quantity_done: 0,
    completed_at: undefined,
  };

  const settings: NotificationSettings = {
    enabled: true,
    morningBriefing: false,
    morningBriefingTime: 8 * 60,
    preStartReminder: true,
    preStartReminderMinutes: 5,
    startOverdue: true,
    unscheduledIdle: false,
    unscheduledIdleHours: 24,
    inProgress: true,
    endTime: false,
  };

  it('does not schedule pre-start/start notifications for pending tasks', async () => {
    const startAt = '2026-07-24T09:00:00Z';
    const endAt = '2026-07-24T10:00:00Z';
    const tasks: TaskRow[] = [
      {
        ...baseTask,
        id: 'scheduled-1',
        title: 'Scheduled Task',
        status: 'scheduled',
        start_at: startAt,
        end_at: endAt,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
      {
        ...baseTask,
        id: 'pending-1',
        title: 'Pending Task',
        status: 'pending',
        start_at: startAt,
        end_at: endAt,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
    ];
    const schedule: ScheduleEntry[] = [
      { task_id: 'scheduled-1', start_at: startAt, end_at: endAt },
      { task_id: 'pending-1', start_at: startAt, end_at: endAt },
    ];

    const data: ScheduleData = { tasks, schedule, settings };
    await rescheduleNotifications(data);

    // Only the scheduled task should generate notifications (pre-start + start-overdue = 2).
    // Pending tasks are not in the planner output and should not get these reminders.
    const taskIds = scheduled
      .map((r) => (r.content as { data?: { taskId?: string } }).data?.taskId)
      .filter(Boolean);
    expect(taskIds).toEqual(['scheduled-1', 'scheduled-1']);
  });

  it('morning briefing counts incomplete scheduled tasks for the notification date', async () => {
    // Use a local noon time so the schedule entries fall on the same local day
    // regardless of the test runner's timezone.
    const now = new Date(2026, 6, 23, 10, 0, 0);
    jest.setSystemTime(now.getTime());

    const morningSettings: NotificationSettings = {
      ...settings,
      morningBriefing: true,
      morningBriefingTime: 10 * 60 + 1,
      preStartReminder: false,
      startOverdue: false,
    };
    const todayNoon = new Date(2026, 6, 23, 12, 0, 0).toISOString();
    const todayEnd = new Date(2026, 6, 23, 13, 0, 0).toISOString();
    const tomorrowNoon = new Date(2026, 6, 24, 12, 0, 0).toISOString();
    const tomorrowEnd = new Date(2026, 6, 24, 13, 0, 0).toISOString();

    const tasks: TaskRow[] = [
      {
        ...baseTask,
        id: 'today-done',
        title: 'Today done',
        status: 'completed',
        start_at: todayNoon,
        end_at: todayEnd,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
      {
        ...baseTask,
        id: 'today-todo',
        title: 'Today todo',
        status: 'scheduled',
        start_at: todayNoon,
        end_at: todayEnd,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
      {
        ...baseTask,
        id: 'tomorrow-todo',
        title: 'Tomorrow todo',
        status: 'scheduled',
        start_at: tomorrowNoon,
        end_at: tomorrowEnd,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
      {
        ...baseTask,
        id: 'pending-today',
        title: 'Pending today',
        status: 'pending',
        start_at: todayNoon,
        end_at: todayEnd,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
    ];

    // Include a stale schedule entry for the pending task; it should be ignored.
    const schedule: ScheduleEntry[] = [
      { task_id: 'today-done', start_at: todayNoon, end_at: todayEnd },
      { task_id: 'today-todo', start_at: todayNoon, end_at: todayEnd },
      {
        task_id: 'tomorrow-todo',
        start_at: tomorrowNoon,
        end_at: tomorrowEnd,
      },
      { task_id: 'pending-today', start_at: todayNoon, end_at: todayEnd },
    ];

    const data: ScheduleData = { tasks, schedule, settings: morningSettings };
    await rescheduleNotifications(data);

    const summary = scheduled.find((r) =>
      (r.content as { title?: string }).title?.includes('未完了タスク'),
    );
    expect(summary).toBeDefined();
    expect((summary!.content as { title: string }).title).toBe(
      '今日は1個の未完了タスクがあります',
    );

    // The briefing is set one minute after the current time, so it should be
    // scheduled for today.
    const target = (summary!.trigger as { date: Date }).date;
    expect(target.getFullYear()).toBe(2026);
    expect(target.getMonth()).toBe(6);
    expect(target.getDate()).toBe(23);
    expect(target.getHours()).toBe(10);
    expect(target.getMinutes()).toBe(1);
  });

  it('morning briefing counts tomorrow tasks when the briefing time has passed', async () => {
    const now = new Date(2026, 6, 23, 10, 0, 0);
    jest.setSystemTime(now.getTime());

    const morningSettings: NotificationSettings = {
      ...settings,
      morningBriefing: true,
      morningBriefingTime: 8 * 60,
      preStartReminder: false,
      startOverdue: false,
    };
    const todayNoon = new Date(2026, 6, 23, 12, 0, 0).toISOString();
    const todayEnd = new Date(2026, 6, 23, 13, 0, 0).toISOString();
    const tomorrowNoon = new Date(2026, 6, 24, 12, 0, 0).toISOString();
    const tomorrowEnd = new Date(2026, 6, 24, 13, 0, 0).toISOString();

    const tasks: TaskRow[] = [
      {
        ...baseTask,
        id: 'today-todo',
        title: 'Today todo',
        status: 'scheduled',
        start_at: todayNoon,
        end_at: todayEnd,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
      {
        ...baseTask,
        id: 'tomorrow-todo',
        title: 'Tomorrow todo',
        status: 'scheduled',
        start_at: tomorrowNoon,
        end_at: tomorrowEnd,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
    ];

    const schedule: ScheduleEntry[] = [
      { task_id: 'today-todo', start_at: todayNoon, end_at: todayEnd },
      { task_id: 'tomorrow-todo', start_at: tomorrowNoon, end_at: tomorrowEnd },
    ];

    const data: ScheduleData = { tasks, schedule, settings: morningSettings };
    await rescheduleNotifications(data);

    const summary = scheduled.find((r) =>
      (r.content as { title?: string }).title?.includes('未完了タスク'),
    );
    expect(summary).toBeDefined();
    expect((summary!.content as { title: string }).title).toBe(
      '今日は1個の未完了タスクがあります',
    );

    // 08:00 has already passed, so the briefing should be scheduled for
    // tomorrow and count tomorrow's tasks.
    const target = (summary!.trigger as { date: Date }).date;
    expect(target.getFullYear()).toBe(2026);
    expect(target.getMonth()).toBe(6);
    expect(target.getDate()).toBe(24);
    expect(target.getHours()).toBe(8);
    expect(target.getMinutes()).toBe(0);
  });

  it('uses server timezone for morning briefing time and count', async () => {
    // 20:00 UTC = 05:00 JST (next day). Morning briefing at 08:00 JST
    // should be scheduled for 23:00 UTC today and count tasks on the JST day.
    const now = new Date('2026-07-24T20:00:00Z');
    jest.setSystemTime(now.getTime());

    const morningSettings: NotificationSettings = {
      ...settings,
      morningBriefing: true,
      morningBriefingTime: 8 * 60,
      preStartReminder: false,
      startOverdue: false,
    };
    const jstMorningStart = '2026-07-24T23:00:00Z';
    const jstMorningEnd = '2026-07-25T00:00:00Z';

    const tasks: TaskRow[] = [
      {
        ...baseTask,
        id: 'jst-task',
        title: 'JST Task',
        status: 'scheduled',
        start_at: jstMorningStart,
        end_at: jstMorningEnd,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
    ];
    const schedule: ScheduleEntry[] = [
      { task_id: 'jst-task', start_at: jstMorningStart, end_at: jstMorningEnd },
    ];

    const data: ScheduleData = {
      tasks,
      schedule,
      settings: morningSettings,
      tz: 'Asia/Tokyo',
    };
    await rescheduleNotifications(data);

    const summary = scheduled.find((r) =>
      (r.content as { title?: string }).title?.includes('未完了タスク'),
    );
    expect(summary).toBeDefined();
    expect((summary!.content as { title: string }).title).toBe(
      '今日は1個の未完了タスクがあります',
    );
    const target = (summary!.trigger as { date: Date }).date;
    expect(target.toISOString()).toBe('2026-07-24T23:00:00.000Z');
  });

  it('uses server timezone for start reminder times', async () => {
    // 20:00 UTC = 05:00 JST. A task at 01:00 UTC is 10:00 JST today.
    const now = new Date('2026-07-24T20:00:00Z');
    jest.setSystemTime(now.getTime());

    const reminderSettings: NotificationSettings = {
      ...settings,
      morningBriefing: false,
      preStartReminder: true,
      preStartReminderMinutes: 5,
      startOverdue: true,
    };
    const startAt = '2026-07-25T01:00:00Z';
    const endAt = '2026-07-25T02:00:00Z';

    const tasks: TaskRow[] = [
      {
        ...baseTask,
        id: 'jst-reminder-task',
        title: 'JST Reminder Task',
        status: 'scheduled',
        start_at: startAt,
        end_at: endAt,
        created_at: '2026-07-22T00:00:00Z',
        updated_at: '2026-07-22T00:00:00Z',
      },
    ];
    const schedule: ScheduleEntry[] = [
      { task_id: 'jst-reminder-task', start_at: startAt, end_at: endAt },
    ];

    const data: ScheduleData = {
      tasks,
      schedule,
      settings: reminderSettings,
      tz: 'Asia/Tokyo',
    };
    await rescheduleNotifications(data);

    const preStart = scheduled.find(
      (r) => (r.content as { title?: string }).title === 'タスク開始直前',
    );
    const start = scheduled.find(
      (r) => (r.content as { title?: string }).title === 'タスク開始時間',
    );
    expect(preStart).toBeDefined();
    expect(start).toBeDefined();

    expect((preStart!.content as { body: string }).body).toContain('10:00');
    expect((preStart!.trigger as { date: Date }).date.toISOString()).toBe(
      '2026-07-25T00:55:00.000Z',
    );
    expect((start!.trigger as { date: Date }).date.toISOString()).toBe(
      '2026-07-25T01:00:00.000Z',
    );
  });

  it('rolls morning briefing to the next day when the time has passed in server tz', async () => {
    // 15:00 UTC = 10:00 EST (America/New_York). 08:00 EST has passed,
    // so the briefing should be scheduled for 08:00 EST the next day.
    const now = new Date('2026-01-15T15:00:00Z');
    jest.setSystemTime(now.getTime());

    const morningSettings: NotificationSettings = {
      ...settings,
      morningBriefing: true,
      morningBriefingTime: 8 * 60,
      preStartReminder: false,
      startOverdue: false,
    };
    const estNextDayStart = '2026-01-16T13:00:00Z'; // 08:00 EST Jan 16
    const estNextDayEnd = '2026-01-16T14:00:00Z';

    const tasks: TaskRow[] = [
      {
        ...baseTask,
        id: 'est-task',
        title: 'EST Task',
        status: 'scheduled',
        start_at: estNextDayStart,
        end_at: estNextDayEnd,
        created_at: '2026-01-14T00:00:00Z',
        updated_at: '2026-01-14T00:00:00Z',
      },
    ];
    const schedule: ScheduleEntry[] = [
      {
        task_id: 'est-task',
        start_at: estNextDayStart,
        end_at: estNextDayEnd,
      },
    ];

    const data: ScheduleData = {
      tasks,
      schedule,
      settings: morningSettings,
      tz: 'America/New_York',
    };
    await rescheduleNotifications(data);

    const summary = scheduled.find((r) =>
      (r.content as { title?: string }).title?.includes('未完了タスク'),
    );
    expect(summary).toBeDefined();
    expect((summary!.content as { title: string }).title).toBe(
      '今日は1個の未完了タスクがあります',
    );
    const target = (summary!.trigger as { date: Date }).date;
    expect(target.toISOString()).toBe('2026-01-16T13:00:00.000Z');
  });

  it('handles a DST spring-forward gap when scheduling the next occurrence', async () => {
    // 2026-03-08 07:00 UTC is the spring-forward instant in the US:
    // clocks jump from 02:00 EST to 03:00 EDT. 02:30 does not exist on Mar 8,
    // so the next 02:30 wall-clock time is Mar 9 02:30 EDT = 06:30 UTC.
    const now = new Date('2026-03-08T07:00:00Z');
    jest.setSystemTime(now.getTime());

    const morningSettings: NotificationSettings = {
      ...settings,
      morningBriefing: true,
      morningBriefingTime: 2 * 60 + 30,
      preStartReminder: false,
      startOverdue: false,
    };

    const data: ScheduleData = {
      tasks: [],
      schedule: [],
      settings: morningSettings,
      tz: 'America/New_York',
    };
    await rescheduleNotifications(data);

    const summary = scheduled.find((r) =>
      (r.content as { title?: string }).title?.includes('おはようございます'),
    );
    expect(summary).toBeDefined();
    const target = (summary!.trigger as { date: Date }).date;
    expect(target.toISOString()).toBe('2026-03-09T06:30:00.000Z');
  });

  it('tints notification icons with the active theme color', async () => {
    asyncStorageGetItem.mockImplementation((key: string) =>
      Promise.resolve(key === 'takusu.theme' ? 'dark' : null),
    );
    jest.spyOn(Appearance, 'getColorScheme').mockReturnValue(null);

    const data: ScheduleData = {
      tasks: [],
      schedule: [],
      settings: { ...settings, morningBriefing: true },
    };
    await rescheduleNotifications(data);

    const summary = scheduled.find((r) =>
      (r.content as { title?: string }).title?.includes('おはようございます'),
    );
    expect(summary).toBeDefined();
    expect((summary!.content as { color: string }).color).toBe(
      DARK_COLORS.brandLight,
    );
  });

  it('falls back to the system color scheme when no theme is stored', async () => {
    asyncStorageGetItem.mockResolvedValue(null);
    jest.spyOn(Appearance, 'getColorScheme').mockReturnValue('light');

    const data: ScheduleData = {
      tasks: [],
      schedule: [],
      settings: { ...settings, morningBriefing: true },
    };
    await rescheduleNotifications(data);

    const summary = scheduled.find((r) =>
      (r.content as { title?: string }).title?.includes('おはようございます'),
    );
    expect(summary).toBeDefined();
    expect((summary!.content as { color: string }).color).toBe(COLORS.brand);
  });
});
