// Notification scheduler — cancels all scheduled notifications and reschedules
// based on current tasks, schedule, habits, and notification settings.
//
// Called from HomeView after refresh() and from _layout on app launch.
// In-progress notifications (#5) are posted immediately when a task is started,
// not scheduled here.

import * as Notifications from 'expo-notifications';
import type { TaskRow, ScheduleEntry } from '@/src/api/types';
import { parseSchedule } from '@/src/api/types';
import type { AgentClient } from '@/src/api/agentClient';
import type { StartTimeNotification } from '@/src/api/agentTypes';
import { type NotificationSettings, minutesToTime } from './settings';
import { CHANNELS } from './channels';
import { CATEGORY_TASK_IN_PROGRESS, CATEGORY_TASK_START } from './categories';
import { dateKey, todayDateKey } from '@/src/utils/dateKey';
import { getNotificationIconColor } from './theme';

// Android has a ~64 notification limit for scheduled notifications.
// We limit per-task notification batches (pre-start, start-overdue, end-time)
// so the total stays under the platform limit.
const MAX_SCHEDULED_PER_TYPE = 15;

export interface ScheduleData {
  tasks: TaskRow[];
  schedule: ScheduleEntry[];
  settings: NotificationSettings;
  tz?: string;
  /** Agent client for start-time check-in notifications (WI-4). */
  agentClient?: AgentClient;
}

const wallClockFormatterCache = new Map<string, Intl.DateTimeFormat | null>();

function getWallClockFormatter(tz?: string): Intl.DateTimeFormat | null {
  const key = tz ?? '';
  let fmt = wallClockFormatterCache.get(key);
  if (fmt === undefined) {
    try {
      fmt = new Intl.DateTimeFormat('en-CA', {
        timeZone: tz || undefined,
        hour: 'numeric',
        minute: 'numeric',
        hour12: false,
        hourCycle: 'h23',
      });
    } catch {
      fmt = null;
    }
    wallClockFormatterCache.set(key, fmt);
  }
  return fmt;
}

function getWallClockTime(
  date: Date,
  tz?: string,
): { hour: number; minute: number } | null {
  const fmt = getWallClockFormatter(tz);
  if (!fmt) return null;
  let hour = NaN;
  let minute = NaN;
  for (const part of fmt.formatToParts(date)) {
    if (part.type === 'hour') hour = parseInt(part.value, 10);
    if (part.type === 'minute') minute = parseInt(part.value, 10);
  }
  if (Number.isNaN(hour) || Number.isNaN(minute)) return null;
  return { hour, minute };
}

function formatTimeInZone(iso: string, tz?: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '--:--';
  const t = getWallClockTime(date, tz);
  if (!t) {
    return `${date.getHours().toString().padStart(2, '0')}:${date
      .getMinutes()
      .toString()
      .padStart(2, '0')}`;
  }
  return `${t.hour.toString().padStart(2, '0')}:${t.minute
    .toString()
    .padStart(2, '0')}`;
}

function addDaysToDateKey(key: string, days: number): string {
  const [y, m, d] = key.split('-').map(Number);
  const date = new Date(Date.UTC(y, m - 1, d + days));
  const yy = date.getUTCFullYear();
  const mm = (date.getUTCMonth() + 1).toString().padStart(2, '0');
  const dd = date.getUTCDate().toString().padStart(2, '0');
  return `${yy}-${mm}-${dd}`;
}

function isToday(iso: string, tz?: string): boolean {
  return dateKey(iso, tz) === todayDateKey(tz);
}

function isTomorrow(iso: string, tz?: string): boolean {
  return dateKey(iso, tz) === addDaysToDateKey(todayDateKey(tz), 1);
}

function isTodayOrTomorrow(iso: string, tz?: string): boolean {
  return isToday(iso, tz) || isTomorrow(iso, tz);
}

function isFuture(value: Date | string): boolean {
  const ts =
    typeof value === 'string' ? new Date(value).getTime() : value.getTime();
  return ts > Date.now();
}

// Count incomplete tasks scheduled for a specific date.
// A task counts if it appears in the schedule for that date, is not done
// (completed/skipped), and is not pending (pending tasks may carry stale
// schedule entries). in_progress tasks are included because their schedule
// entry is preserved and they are not yet done.
function countIncompleteTasksForDate(
  tasks: TaskRow[],
  schedule: ScheduleEntry[],
  targetDate: Date,
  tz?: string,
): number {
  const taskMap = new Map(tasks.map((t) => [t.id, t]));
  const targetKey = dateKey(targetDate.toISOString(), tz);

  const counted = new Set<string>();
  for (const e of schedule) {
    const t = taskMap.get(e.task_id);
    if (
      !t ||
      t.status === 'pending' ||
      t.status === 'completed' ||
      t.status === 'skipped'
    ) {
      continue;
    }
    if (dateKey(e.start_at, tz) === targetKey) {
      counted.add(e.task_id);
    }
  }
  return counted.size;
}

// Count pending tasks idle for more than threshold hours
function countIdlePendingTasks(
  tasks: TaskRow[],
  thresholdHours: number,
): number {
  const threshold = Date.now() - thresholdHours * 60 * 60 * 1000;
  return tasks.filter((t) => {
    if (t.status !== 'pending') return false;
    return new Date(t.created_at).getTime() < threshold;
  }).length;
}

function nextOccurrenceDate(hour: number, minute: number, tz?: string): Date {
  const now = new Date();
  if (getWallClockTime(now, tz) == null) {
    const candidate = new Date(now);
    candidate.setHours(hour, minute, 0, 0);
    if (candidate.getTime() <= now.getTime()) {
      candidate.setDate(candidate.getDate() + 1);
    }
    return candidate;
  }

  // Start at the next whole minute after now so an exact current wall-clock
  // time is treated as already passed (matching the old <= behavior).
  const startMinute = Math.trunc(now.getTime() / 60000) + 1;
  let candidate = new Date(startMinute * 60000);
  const maxMinutes = 60 * 48;
  for (let i = 0; i < maxMinutes; i++) {
    const t = getWallClockTime(candidate, tz);
    if (t && t.hour === hour && t.minute === minute) {
      return candidate;
    }
    candidate = new Date(candidate.getTime() + 60000);
  }

  // Fallback for pathological timezones. Normal IANA timezones should always
  // hit the target wall-clock time within 48 hours.
  return candidate;
}

// Schedule a one-time notification for the next occurrence of a daily time.
// If the time has already passed today, schedule for tomorrow.
// This avoids stale content from DAILY triggers — the app reschedules with
// fresh data each time it's opened.
async function scheduleNextOccurrence(
  channelId: string,
  hour: number,
  minute: number,
  title: string,
  body: string,
  data: Record<string, unknown>,
  color: string,
  tz?: string,
): Promise<void> {
  const target = nextOccurrenceDate(hour, minute, tz);
  await scheduleAt(channelId, target, title, body, data, color);
}

// Schedule a one-time notification at a specific date
async function scheduleCheckInNotification(
  date: Date,
  notification: StartTimeNotification,
  title: string,
  color: string,
): Promise<void> {
  await Notifications.scheduleNotificationAsync({
    content: {
      title,
      body: notification.body,
      data: {
        taskId: notification.task_id,
        scheduledAt: notification.scheduled_at,
        check_in: notification.check_in,
      },
      color,
      categoryIdentifier: CATEGORY_TASK_START,
    },
    trigger: {
      type: Notifications.SchedulableTriggerInputTypes.DATE,
      date,
      channelId: CHANNELS.taskReminders,
    },
  });
}

async function scheduleAt(
  channelId: string,
  date: Date,
  title: string,
  body: string,
  data: Record<string, unknown>,
  color: string,
  categoryIdentifier?: string,
): Promise<void> {
  await Notifications.scheduleNotificationAsync({
    content: {
      title,
      body,
      data,
      color,
      categoryIdentifier,
    },
    trigger: {
      type: Notifications.SchedulableTriggerInputTypes.DATE,
      date,
      channelId,
    },
  });
}

export async function rescheduleNotifications(
  data: ScheduleData,
  color?: string,
): Promise<void> {
  const { tasks, schedule, settings, tz } = data;

  if (!settings.enabled) {
    await Notifications.cancelAllScheduledNotificationsAsync();
    return;
  }

  // Cancel all previously scheduled notifications, then reschedule
  await Notifications.cancelAllScheduledNotificationsAsync();
  const iconColor = color ?? (await getNotificationIconColor());

  const scheduleMap = new Map<string, ScheduleEntry>();
  for (const e of schedule) scheduleMap.set(e.task_id, e);

  // ── 1. Morning briefing (next occurrence only) ──
  if (settings.morningBriefing) {
    const { hour, minute } = minutesToTime(settings.morningBriefingTime);
    const target = nextOccurrenceDate(hour, minute, tz);
    const count = countIncompleteTasksForDate(tasks, schedule, target, tz);
    const title =
      count === 0
        ? 'おはようございます'
        : `今日は${count}個の未完了タスクがあります`;
    const body = count === 0 ? 'タスクを追加しましょう' : 'タップして確認';
    await scheduleAt(
      CHANNELS.taskSummary,
      target,
      title,
      body,
      { url: '/' },
      iconColor,
    );
  }

  // ── 2. Pre-start reminder + 3. Start overdue (per-task, today/tomorrow only) ──
  // Only tasks that have actually been scheduled by the planner should get
  // start-time reminders. Pending tasks are not yet scheduled and may have
  // stale/outdated schedule entries, so they must be excluded.
  const upcomingTasks = tasks
    .filter((t) => t.status === 'scheduled' && scheduleMap.has(t.id))
    .map((t) => ({
      task: t,
      entry: scheduleMap.get(t.id)!,
      startDate: new Date(scheduleMap.get(t.id)!.start_at),
    }))
    .filter(
      ({ entry, startDate }) =>
        isTodayOrTomorrow(entry.start_at, tz) && isFuture(startDate),
    )
    .sort((a, b) => a.entry.start_at.localeCompare(b.entry.start_at))
    .slice(0, MAX_SCHEDULED_PER_TYPE);

  if (
    data.agentClient &&
    (settings.preStartReminder || settings.startOverdue)
  ) {
    // WI-4: use agent-issued start-time check-in cards with capabilities.
    const checkIns = await data.agentClient.getStartTimeNotifications(
      MAX_SCHEDULED_PER_TYPE,
      'mobile',
      data.tz,
    );
    for (const n of checkIns) {
      if (!isTodayOrTomorrow(n.scheduled_at, data.tz)) {
        continue;
      }
      const startDate = new Date(n.scheduled_at);
      if (settings.preStartReminder) {
        const reminderDate = new Date(
          startDate.getTime() - settings.preStartReminderMinutes * 60 * 1000,
        );
        if (isFuture(reminderDate)) {
          await scheduleCheckInNotification(
            reminderDate,
            n,
            'タスク開始直前',
            iconColor,
          );
        }
      }
      if (settings.startOverdue && isFuture(startDate)) {
        await scheduleCheckInNotification(startDate, n, n.title, iconColor);
      }
    }
  } else {
    // Fallback (legacy): schedule plain start-time reminders without capabilities.
    for (const { task, entry, startDate } of upcomingTasks) {
      const startTime = formatTimeInZone(entry.start_at, tz);

      if (settings.preStartReminder) {
        const reminderDate = new Date(
          startDate.getTime() - settings.preStartReminderMinutes * 60 * 1000,
        );
        if (isFuture(reminderDate)) {
          await scheduleAt(
            CHANNELS.taskReminders,
            reminderDate,
            'タスク開始直前',
            `「${task.title}」が${settings.preStartReminderMinutes}分後の${startTime}に開始します`,
            { url: `/task/${task.id}`, taskId: task.id },
            iconColor,
            CATEGORY_TASK_START,
          );
        }
      }

      if (settings.startOverdue) {
        await scheduleAt(
          CHANNELS.taskReminders,
          startDate,
          'タスク開始時間',
          `「${task.title}」の開始時間です (${startTime})`,
          { url: `/task/${task.id}`, taskId: task.id },
          iconColor,
          CATEGORY_TASK_START,
        );
      }
    }
  }

  // ── 3.5 End time notification (#417, #725) — only for in-progress tasks ──
  if (settings.endTime) {
    const endingTasks = tasks
      .filter((t) => t.status === 'in_progress' && scheduleMap.has(t.id))
      .map((t) => ({
        task: t,
        entry: scheduleMap.get(t.id)!,
        endDate: new Date(scheduleMap.get(t.id)!.end_at),
      }))
      .filter(
        ({ entry, endDate }) =>
          isTodayOrTomorrow(entry.end_at, tz) && isFuture(endDate),
      )
      .sort((a, b) => a.entry.end_at.localeCompare(b.entry.end_at))
      .slice(0, MAX_SCHEDULED_PER_TYPE);

    for (const { task, entry, endDate } of endingTasks) {
      const endTime = formatTimeInZone(entry.end_at, tz);
      await scheduleAt(
        CHANNELS.taskReminders,
        endDate,
        'タスク終了時間',
        `「${task.title}」の終了時間です (${endTime})`,
        { url: `/task/${task.id}`, taskId: task.id },
        iconColor,
      );
    }
  }

  // ── 4. Unscheduled idle (next occurrence at noon) ──
  if (settings.unscheduledIdle) {
    const idleCount = countIdlePendingTasks(
      tasks,
      settings.unscheduledIdleHours,
    );
    if (idleCount > 0) {
      await scheduleNextOccurrence(
        CHANNELS.taskIdle,
        12,
        0,
        '未スケジュールのタスクがあります',
        `${idleCount}個のタスクが${settings.unscheduledIdleHours}時間以上放置されています`,
        { url: '/' },
        iconColor,
        tz,
      );
    }
  }
}

// ── In-progress notification (#5) — posted immediately, not scheduled ──

export async function postInProgressNotification(
  task: TaskRow,
  color?: string,
): Promise<void> {
  const iconColor = color ?? (await getNotificationIconColor());
  await Notifications.scheduleNotificationAsync({
    content: {
      title: `実行中: ${task.title}`,
      body: 'タップして詳細を表示',
      data: { url: `/task/${task.id}`, taskId: task.id },
      color: iconColor,
      categoryIdentifier: CATEGORY_TASK_IN_PROGRESS,
      // Keep the in-progress notification visible on tap and prevent swipe dismissal
      // so the user can use the DONE/CANCEL actions while the task is running (#416).
      autoDismiss: false,
      sticky: true,
    },
    // Use channel-aware trigger for immediate delivery on the in-progress channel
    trigger: { channelId: CHANNELS.taskInProgress },
  });
}

export async function dismissInProgressNotification(
  taskId: string,
): Promise<void> {
  // Dismiss all presented notifications for this task
  // expo-notifications doesn't support dismissing by data, so we dismiss all
  // from the in-progress channel. This is acceptable since only one task
  // should be in_progress at a time.
  const presented = await Notifications.getPresentedNotificationsAsync();
  for (const n of presented) {
    if (n.request.content.data?.taskId === taskId) {
      await Notifications.dismissNotificationAsync(n.request.identifier);
    }
  }
}

// ── Task result notification (#418) — posted after a notification action completes a task.
export async function postResultNotification(
  taskId: string,
  taskTitle: string,
  status: 'completed' | 'skipped',
  color?: string,
): Promise<void> {
  const label = status === 'completed' ? '完了' : 'スキップ';
  const iconColor = color ?? (await getNotificationIconColor());
  await Notifications.scheduleNotificationAsync({
    content: {
      title: `タスクを${label}しました`,
      body: `「${taskTitle}」を${label}しました`,
      data: { url: `/task/${taskId}`, taskId },
      color: iconColor,
    },
    trigger: { channelId: CHANNELS.taskInProgress },
  });
}

// Dismiss all delivered notifications for a task (#257).
// When a task is completed, skipped, or started, any already-delivered
// reminder notifications (pre-start, start-overdue) sitting in the
// notification tray should be removed — cancelAllScheduledNotificationsAsync
// only cancels pending (not-yet-fired) notifications, not delivered ones.
export async function dismissTaskNotifications(taskId: string): Promise<void> {
  const presented = await Notifications.getPresentedNotificationsAsync();
  for (const n of presented) {
    if (n.request.content.data?.taskId === taskId) {
      await Notifications.dismissNotificationAsync(n.request.identifier);
    }
  }
}

// Cancel all pending scheduled notifications for a task.
// When a task is completed or skipped, any scheduled reminders
// (pre-start, start-overdue, end-time) should not fire.
export async function cancelScheduledTaskNotifications(
  taskId: string,
): Promise<void> {
  const scheduled = await Notifications.getAllScheduledNotificationsAsync();
  for (const n of scheduled) {
    if (n.content.data?.taskId === taskId) {
      await Notifications.cancelScheduledNotificationAsync(n.identifier);
    }
  }
}

// Cancel pending start-time notifications for one or more tasks.
// Pre-start reminders and start-overdue reminders are tagged with
// CATEGORY_TASK_START; when a task becomes in_progress, these should not fire.
export async function cancelScheduledStartNotifications(
  taskId: string | string[],
): Promise<void> {
  const ids = new Set(Array.isArray(taskId) ? taskId : [taskId]);
  const scheduled = await Notifications.getAllScheduledNotificationsAsync();
  for (const n of scheduled) {
    const id = n.content.data?.taskId;
    if (
      typeof id === 'string' &&
      ids.has(id) &&
      n.content.categoryIdentifier === CATEGORY_TASK_START
    ) {
      await Notifications.cancelScheduledNotificationAsync(n.identifier);
    }
  }
}

// Wrapper that accepts raw schedule JSON (convenience for HomeView)
export async function rescheduleFromRaw(
  tasks: TaskRow[],
  scheduleJson: string | null,
  settings: NotificationSettings,
  tz?: string,
  agentClient?: AgentClient,
  color?: string,
): Promise<void> {
  const schedule = scheduleJson ? parseSchedule(scheduleJson) : [];
  await rescheduleNotifications(
    { tasks, schedule, settings, tz, agentClient },
    color,
  );
}
