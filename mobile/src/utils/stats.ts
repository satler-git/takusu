import type { ScheduleEntry, TaskRow } from '@/src/api/types';
import { dateKey } from '@/src/utils/dateKey';

function firstNonEmptyDate(
  ...values: Array<string | null | undefined>
): string | null {
  for (const v of values) {
    if (v && v.trim()) {
      return v;
    }
  }
  return null;
}

export function taskDateKey(
  task: TaskRow,
  scheduleMap: Map<string, ScheduleEntry>,
  tz?: string,
): string | null {
  if (task.status === 'skipped') {
    // Count skips on the scheduled day (start_at) rather than the deadline.
    // Skipped tasks that are not in the schedule fall back to the task's own
    // start_at / end_at.
    const scheduled = scheduleMap.get(task.id);
    const date = firstNonEmptyDate(
      scheduled?.start_at,
      task.start_at,
      // Defensive: ScheduleEntry.start_at is non-null in the type, but a
      // malformed or stale entry with an empty start_at could reach this.
      scheduled?.end_at,
      task.end_at,
    );
    if (!date) return null;
    return dateKey(date, tz);
  }
  const endAt = firstNonEmptyDate(
    scheduleMap.get(task.id)?.end_at,
    task.end_at,
  );
  if (!endAt) return null;
  return dateKey(endAt, tz);
}
