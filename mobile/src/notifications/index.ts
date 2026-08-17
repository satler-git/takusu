// Notification module — re-exports and setup helpers.

export {
  type NotificationSettings,
  DEFAULT_NOTIFICATION_SETTINGS,
} from './settings';
export {
  loadNotificationSettings,
  saveNotificationSettings,
  minutesToTime,
  timeToMinutes,
  formatTime,
} from './settings';
export { CHANNELS } from './channels';
export { setupNotificationChannels } from './channels';
export {
  CATEGORY_TASK_IN_PROGRESS,
  CATEGORY_TASK_START,
  ACTION_DONE,
  ACTION_CANCEL,
  ACTION_START,
  setupNotificationCategories,
} from './categories';
export { ensureNotificationPermissions } from './permissions';
export {
  rescheduleNotifications,
  rescheduleFromRaw,
  postInProgressNotification,
  postResultNotification,
  dismissInProgressNotification,
  dismissTaskNotifications,
  cancelScheduledTaskNotifications,
  cancelScheduledStartNotifications,
} from './scheduler';
export { notificationColorForTheme } from './theme';
