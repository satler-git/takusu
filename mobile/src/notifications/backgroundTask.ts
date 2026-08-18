// Background notification task for Android action buttons.
//
// When a notification action button (START / DONE / CANCEL) is tapped while the
// app is not in the foreground, expo-notifications (SDK 56+) can run a JS task
// in the background. This task starts the local takusu server if needed and
// performs the same action that the foreground UI would.

import * as TaskManager from 'expo-task-manager';
import * as Notifications from 'expo-notifications';
import * as Sentry from '@sentry/react-native';
import { loadSettings } from '@/src/api/settingsStore';
import { ensureLocalServer, waitForLocalServerReady } from '@/src/api/server';
import { TakusuClient } from '@/src/api/client';
import { AgentClient } from '@/src/api/agentClient';
import { handleActionButtonResponse, NOOP_HAPTIC } from './actionHandler';
import { notificationColorForTheme } from './theme';
import {
  ACTION_DONE,
  ACTION_CANCEL,
  ACTION_START,
  ACTION_SNOOZE,
  ACTION_RESCHEDULE,
} from './categories';

export const BACKGROUND_NOTIFICATION_TASK = 'takusu-notification-action-task';

export function isActionResponse(
  data: Notifications.NotificationTaskPayload,
): data is Notifications.NotificationResponse {
  return (
    typeof data === 'object' &&
    data !== null &&
    'actionIdentifier' in data &&
    typeof (data as Notifications.NotificationResponse).actionIdentifier ===
      'string'
  );
}

function isKnownAction(actionId: string): boolean {
  return (
    actionId === ACTION_DONE ||
    actionId === ACTION_CANCEL ||
    actionId === ACTION_START ||
    actionId === ACTION_SNOOZE ||
    actionId === ACTION_RESCHEDULE
  );
}

TaskManager.defineTask<Notifications.NotificationTaskPayload>(
  BACKGROUND_NOTIFICATION_TASK,
  async ({ data }) => {
    if (!isActionResponse(data)) return;
    const actionId = data.actionIdentifier;
    if (!isKnownAction(actionId)) return;

    const settings = await loadSettings();
    if (!settings.workersUrl || !settings.workersToken) return;

    try {
      const client = await (async (): Promise<TakusuClient> => {
        const maxAttempts = 10;
        const delayMs = 200;
        let lastError: unknown = null;
        for (let attempt = 0; attempt < maxAttempts; attempt++) {
          try {
            const c = ensureLocalServer({
              workersUrl: settings.workersUrl,
              rootToken: settings.workersToken,
            });
            await waitForLocalServerReady(c);
            return c;
          } catch (e) {
            lastError = e;
            if (attempt < maxAttempts - 1) {
              await new Promise((resolve) => setTimeout(resolve, delayMs));
            }
          }
        }
        throw lastError ?? new Error('Local server did not start');
      })();
      const agentBaseUrl = client.baseUrl ?? settings.workersUrl;
      const agentClient = new AgentClient(agentBaseUrl, settings.workersToken);
      await handleActionButtonResponse(data, {
        client,
        agentClient,
        inProgressNotifications: settings.notifications.inProgress,
        haptic: NOOP_HAPTIC,
        color: notificationColorForTheme(settings.theme),
      });
    } catch (err) {
      Sentry.withScope((scope) => {
        scope.setTag('action', actionId);
        scope.setExtra('context', 'background-notification-task');
        Sentry.captureException(err);
      });
      console.warn('Notification background task: failed to process action', {
        actionId,
        err,
      });
    }
  },
);

Notifications.registerTaskAsync(BACKGROUND_NOTIFICATION_TASK).catch(() => {});
