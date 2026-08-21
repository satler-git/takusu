import { Platform } from 'react-native';
import type { AgentClient } from '@/src/api/agentClient';
import type { AlarmEvaluationConfig } from './scheduler';
import {
  decodePresentation,
  type ActionCapability,
  type EventLedgerRow,
  type Presentation,
} from '@/src/api/agentTypes';
import TakusuAlarmsModule from '@/modules/takusu-alarms/src/TakusuAlarmsModule';
import TakusuServerModule from '@/modules/takusu-server/src/TakusuServerModule';
import * as Notifications from 'expo-notifications';
import { CHANNELS } from './channels';
import { CATEGORY_TASK_START } from './categories';
import { getNotificationIconColor } from './theme';

function bodyForPresentation(presentation: Presentation): string {
  switch (presentation.type) {
    case 'current_task':
      return `「${presentation.title}」の予定です`;
    case 'check_in':
      return presentation.question;
    case 'schedule_alert':
      return presentation.message;
    case 'text':
      return presentation.text;
    case 'work_transition':
      return `「${presentation.title}」${presentation.detail ?? ''}`;
    case 'schedule_summary':
    case 'progress_summary':
    case 'change_proposal':
    case 'clarification':
      return 'takusuからのお知らせがあります';
  }
}

function withCapability(
  presentation: Presentation,
  capability: ActionCapability,
): Presentation {
  if (presentation.type !== 'check_in') return presentation;
  return {
    ...presentation,
    act: {
      ...presentation.act,
      actions: presentation.act.actions.map((action, index) =>
        index === 0 ? { ...action, capability } : action,
      ),
    },
  };
}

function resolveLocalUrl(): string | undefined {
  try {
    const status = TakusuServerModule.status();
    if (status.running && status.port > 0) {
      return `http://127.0.0.1:${status.port}`;
    }
  } catch {
    // Module may be unavailable in tests or on non-Android platforms.
  }
  return undefined;
}

async function presentationForEvent(
  event: EventLedgerRow,
  agentClient: AgentClient,
  deviceId: string,
): Promise<Presentation> {
  let presentation: Presentation;
  try {
    presentation = decodePresentation(JSON.parse(event.presentation));
  } catch {
    return { type: 'text', text: 'takusuからのお知らせがあります' };
  }

  if (
    presentation.type === 'check_in' &&
    event.task_id &&
    (event.kind === 'task_start_time_reached' ||
      event.kind === 'task_non_start_continued')
  ) {
    const capability = await agentClient.mintCapability({
      task_id: event.task_id,
      action: 'start',
      device_id: deviceId,
      input_path: 'notification_capability',
      event_id: event.id,
    });
    return withCapability(presentation, capability);
  }
  return presentation;
}

export async function replayPlannerEvents(
  agentClient: AgentClient,
  alarmEvaluation?: AlarmEvaluationConfig,
): Promise<void> {
  const deviceId = alarmEvaluation?.deviceId ?? 'mobile';
  const evaluation = await agentClient.evaluatePlannerEvents(deviceId);
  if (Platform.OS === 'android' && TakusuAlarmsModule) {
    const next = evaluation.next_eval_at
      ? new Date(evaluation.next_eval_at).getTime()
      : Number.NaN;
    if (Number.isFinite(next) && next > Date.now()) {
      if (alarmEvaluation) {
        const localUrl = alarmEvaluation.localUrl ?? resolveLocalUrl() ?? '';
        await TakusuAlarmsModule.scheduleEvaluatorAlarm(
          next,
          alarmEvaluation.workersUrl,
          alarmEvaluation.rootToken,
          deviceId,
          localUrl,
        );
      }
    } else {
      await TakusuAlarmsModule.cancelEvaluatorAlarm();
    }
  }

  const events = await agentClient.listPlannerEvents(deviceId);
  const color = await getNotificationIconColor();
  for (const event of events) {
    if (
      event.delivery_state !== 'pending_delivery' &&
      event.delivery_state !== 'deferred_quiet_hours'
    ) {
      continue;
    }
    if (!(await agentClient.claimPlannerEvent(event.id, deviceId))) continue;
    const presentation = await presentationForEvent(
      event,
      agentClient,
      deviceId,
    );
    const taskId = event.task_id ?? undefined;
    await Notifications.scheduleNotificationAsync({
      content: {
        title: 'takusu',
        body: bodyForPresentation(presentation),
        data: {
          eventId: event.id,
          taskId,
          check_in: presentation,
        },
        color,
        categoryIdentifier: CATEGORY_TASK_START,
      },
      trigger: {
        type: Notifications.SchedulableTriggerInputTypes.TIME_INTERVAL,
        seconds: 1,
        channelId: CHANNELS.taskReminders,
      },
    });
    await agentClient.updatePlannerEventState(event.id, 'delivered');
  }
}
