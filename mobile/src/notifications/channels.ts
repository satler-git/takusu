// Android notification channels.

import * as Notifications from 'expo-notifications';
import { BRAND_COLOR } from '@/src/theme';

export const CHANNELS = {
  taskReminders: 'task-reminders',
  taskSummary: 'task-summary',
  taskInProgress: 'task-in-progress',
  taskIdle: 'task-idle',
  agentModelDownloads: 'agent-model-downloads',
} as const;

export async function setupNotificationChannels(): Promise<void> {
  await Promise.all([
    Notifications.setNotificationChannelAsync(CHANNELS.taskReminders, {
      name: 'タスクリマインダー',
      description: 'タスクの開始前・開始時間の通知',
      importance: Notifications.AndroidImportance.HIGH,
      vibrationPattern: [0, 250, 250, 250],
      lightColor: BRAND_COLOR,
    }),
    Notifications.setNotificationChannelAsync(CHANNELS.taskSummary, {
      name: 'サマリー',
      description: '朝のブリーフィング',
      importance: Notifications.AndroidImportance.DEFAULT,
    }),
    Notifications.setNotificationChannelAsync(CHANNELS.taskInProgress, {
      name: '実行中タスク',
      description: 'タスク実行中の常駐通知',
      importance: Notifications.AndroidImportance.LOW,
    }),
    Notifications.setNotificationChannelAsync(CHANNELS.taskIdle, {
      name: '未スケジュール通知',
      description: '長時間放置された未スケジュールタスクの通知',
      importance: Notifications.AndroidImportance.DEFAULT,
    }),
    Notifications.setNotificationChannelAsync(CHANNELS.agentModelDownloads, {
      name: '音声モデルの準備',
      description: '音声エージェントのモデルダウンロード進捗',
      importance: Notifications.AndroidImportance.LOW,
    }),
    // Clean up the orphaned habit-reminder channel from previous versions
    // (#360). deleteNotificationChannelAsync is a no-op if the channel
    // doesn't exist.
    Notifications.deleteNotificationChannelAsync('habit-reminder'),
  ]);
}
