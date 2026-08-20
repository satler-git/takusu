// Notification action categories — interactive notification buttons.

import * as Notifications from 'expo-notifications';

// Category for in-progress task notifications: DONE + CANCEL actions
export const CATEGORY_TASK_IN_PROGRESS = 'taskinprogress';

// Category for task start reminders with 行動/ズラす action groups (WI-4).
//
// TODO(per-event categories): SNOOZE ('10分後') is removed for now because
// the same category is currently used for task_start_time_reached and
// task_non_start_continued events (and other check-in types where a fixed
// 10-minute snooze is semantically wrong). Once each planner event kind has
// its own category, a delay capability can be minted for the appropriate
// events and the SNOOZE action can be reintroduced safely. For now the
// 'ズラす' group only exposes RESCHEDULE, which opens the rescheduling panel.
export const CATEGORY_TASK_START = 'taskstart';

// Action identifiers for the start-time check-in card
export const ACTION_DONE = 'action_done';
export const ACTION_CANCEL = 'action_cancel';
export const ACTION_START = 'action_start';
export const ACTION_RESCHEDULE = 'action_reschedule';

export async function setupNotificationCategories(): Promise<void> {
  // Immediate actions (着手) should not open the app. On Android (SDK 56+)
  // the registered background task runs for action taps when the app is not
  // in the foreground (#788). The reschedule action needs the UI, so it opens
  // the app to the foreground.
  const opensAppToForeground = false;

  await Notifications.setNotificationCategoryAsync(CATEGORY_TASK_IN_PROGRESS, [
    {
      identifier: ACTION_DONE,
      buttonTitle: '完了',
      options: { isDestructive: false, opensAppToForeground },
    },
    {
      identifier: ACTION_CANCEL,
      buttonTitle: 'キャンセル',
      options: { isDestructive: true, opensAppToForeground },
    },
  ]);
  await Notifications.setNotificationCategoryAsync(CATEGORY_TASK_START, [
    {
      identifier: ACTION_START,
      buttonTitle: '着手',
      options: { isDestructive: false, opensAppToForeground },
    },
    {
      identifier: ACTION_RESCHEDULE,
      buttonTitle: '組み直す',
      options: { isDestructive: false, opensAppToForeground: true },
    },
  ]);
}
