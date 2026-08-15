// Notification action button handler (START / SNOOZE / RESCHEDULE / DONE / CANCEL).
// Used both in the foreground UI and in the background notification task.

import * as Linking from 'expo-linking';
import * as Notifications from 'expo-notifications';
import * as Sentry from '@sentry/react-native';
import type { TakusuClient } from '@/src/api/client';
import { AgentClient } from '@/src/api/agentClient';
import type { CheckInCard } from '@/src/api/agentTypes';
import { decodePresentation } from '@/src/api/agentTypes';
import { haptic as defaultHaptic } from '@/src/components/haptics';
import {
  ACTION_DONE,
  ACTION_CANCEL,
  ACTION_START,
  ACTION_SNOOZE,
  ACTION_RESCHEDULE,
} from './categories';
import {
  postInProgressNotification,
  dismissInProgressNotification,
  dismissTaskNotifications,
  cancelScheduledTaskNotifications,
  cancelScheduledStartNotifications,
  postResultNotification,
} from './scheduler';

export interface ActionHandlerHaptic {
  medium: () => void;
  success: () => void;
  warning: () => void;
}

export const NOOP_HAPTIC: ActionHandlerHaptic = {
  medium: () => {},
  success: () => {},
  warning: () => {},
};

export interface ActionHandlerOptions {
  client: TakusuClient;
  /** Agent client for capability-authorized start/snooze actions (WI-4). */
  agentClient?: AgentClient;
  inProgressNotifications: boolean;
  haptic?: ActionHandlerHaptic;
  /** Called for RESCHEDULE when the app is already in the foreground. */
  onReschedule?: (taskId: string) => void;
}

function logActionError(
  action: string,
  taskId: string | undefined,
  err: unknown,
): void {
  Sentry.withScope((scope) => {
    scope.setTag('action', action);
    scope.setExtra('taskId', taskId ?? null);
    Sentry.captureException(err);
  });
  console.warn('Notification action failed', { action, taskId, err });
}

// Process a notification action button (START / DONE / CANCEL).
// Returns true if the response was a recognized action button, false otherwise.
export async function handleActionButtonResponse(
  response: Notifications.NotificationResponse,
  options: ActionHandlerOptions,
): Promise<boolean> {
  const {
    client,
    agentClient,
    inProgressNotifications,
    haptic = defaultHaptic,
  } = options;
  const actionId = response.actionIdentifier;

  const data = response.notification.request.content.data;
  const checkIn = parseCheckIn(data);
  const notificationTaskId =
    typeof data?.taskId === 'string' ? data.taskId : undefined;

  // Handle START / SNOOZE / RESCHEDULE actions from the start-time check-in card (WI-4)
  if (
    actionId === ACTION_START ||
    actionId === ACTION_SNOOZE ||
    actionId === ACTION_RESCHEDULE
  ) {
    if (!notificationTaskId) return true;

    // Legacy fallback for pre-WI-4 start reminders that carry only a task id.
    if (!checkIn) {
      if (actionId === ACTION_START) {
        return await handleLegacyStart(
          client,
          notificationTaskId,
          inProgressNotifications,
          haptic,
        );
      }
      return true;
    }

    if (actionId === ACTION_RESCHEDULE) {
      // 組み直す opens the app so the user can use the compact rescheduling panel.
      haptic.medium();
      if (notificationTaskId) {
        if (options.onReschedule) {
          options.onReschedule(notificationTaskId);
        } else {
          await Linking.openURL(
            Linking.createURL('/reschedule', {
              queryParams: { taskId: notificationTaskId },
            }),
          );
        }
      }
      return true;
    }

    const capabilityAction = findActionByCategoryId(checkIn, actionId);
    if (!capabilityAction?.capability || !agentClient) return true;

    haptic.medium();
    try {
      if (
        actionId === ACTION_SNOOZE &&
        capabilityAction.capability.snooze_minutes != null
      ) {
        capabilityAction.capability.snooze_target = new Date(
          Date.now() + capabilityAction.capability.snooze_minutes * 60_000,
        ).toISOString();
      }
      await agentClient.authorizeAction(capabilityAction.capability);
      await dismissTaskNotifications(notificationTaskId);
      await cancelScheduledStartNotifications(notificationTaskId);
      if (inProgressNotifications && actionId === ACTION_START) {
        const task = await client.getTask(notificationTaskId);
        await postInProgressNotification(task);
      } else if (actionId === ACTION_SNOOZE) {
        // Snoozed; the agent moved the task and the scheduler will pick up the new time.
        await cancelScheduledTaskNotifications(notificationTaskId);
      }
    } catch (err) {
      logActionError(actionId, notificationTaskId, err);
    }
    return true;
  }

  // Handle action button taps (DONE / CANCEL for in-progress tasks)
  if (actionId === ACTION_DONE || actionId === ACTION_CANCEL) {
    const taskId = response.notification.request.content.data?.taskId;
    if (typeof taskId !== 'string' || !taskId) return true;
    const newStatus = actionId === ACTION_DONE ? 'completed' : 'skipped';
    if (actionId === ACTION_DONE) haptic.success();
    else haptic.warning();
    const title = response.notification.request.content.title ?? '';
    const taskTitle = title.replace(/^実行中: /, '') || 'タスク';
    try {
      await client.updateTask(taskId, { status: newStatus });
      await Promise.all([
        postResultNotification(taskId, taskTitle, newStatus),
        dismissInProgressNotification(taskId),
        dismissTaskNotifications(taskId),
        cancelScheduledTaskNotifications(taskId),
      ]);
    } catch (err) {
      logActionError(actionId, taskId, err);
    }
    return true;
  }

  return false;
}

async function handleLegacyStart(
  client: TakusuClient,
  taskId: string,
  inProgressNotifications: boolean,
  haptic: ActionHandlerHaptic,
): Promise<boolean> {
  haptic.medium();
  try {
    await client.updateTask(taskId, { status: 'in_progress' });
    await dismissTaskNotifications(taskId);
    await cancelScheduledStartNotifications(taskId);
    if (inProgressNotifications) {
      const task = await client.getTask(taskId);
      await postInProgressNotification(task);
    }
  } catch (err) {
    logActionError(ACTION_START, taskId, err);
  }
  return true;
}

function parseCheckIn(data: unknown): CheckInCard | null {
  if (typeof data !== 'object' || data === null) return null;
  const raw = (data as Record<string, unknown>).check_in;
  if (!raw) return null;
  const presentation = decodePresentation(raw);
  return presentation.type === 'check_in' ? presentation : null;
}

function findActionByCategoryId(checkIn: CheckInCard, actionId: string) {
  // Match by semantic kind and capability instead of human-readable labels,
  // so label changes (i18n, design) do not break the handler.
  if (actionId === ACTION_START) {
    return checkIn.act.actions.find(
      (a) => a.kind === 'immediate' && a.capability,
    );
  }
  if (actionId === ACTION_SNOOZE) {
    return checkIn.shift.actions.find(
      (a) => a.kind === 'immediate' && a.capability,
    );
  }
  if (actionId === ACTION_RESCHEDULE) {
    return checkIn.shift.actions.find((a) => a.kind === 'panel');
  }
  return undefined;
}
