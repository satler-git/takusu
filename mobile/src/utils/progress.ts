import type { TakusuClient } from '@/src/api/client';
import type {
  RecordWorkSessionProgress,
  TaskStatus,
  WorkSessionRow,
} from '@/src/api/types';

export interface ProgressPayload {
  quantityDone: number;
  note?: string;
  quantityTotal?: number;
}

export interface RecordProgressOptions {
  operationId?: string;
}

const ALPHANUM =
  'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';

function randomAlphanumeric(length: number): string {
  let result = '';
  for (let i = 0; i < length; i += 1) {
    result += ALPHANUM[Math.floor(Math.random() * ALPHANUM.length)];
  }
  return result;
}

export function makeProgressOperationId(): string {
  try {
    const cryptoLike = (
      globalThis as { crypto?: { randomUUID?: () => string } }
    ).crypto;
    if (cryptoLike?.randomUUID) {
      return cryptoLike.randomUUID();
    }
  } catch {
    // fall through
  }
  return `${Date.now()}-${randomAlphanumeric(16)}`;
}

/** Stable idempotency key for a user-submitted task comment (WI-5). */
export function makeCommentOperationId(): string {
  return makeProgressOperationId();
}

export async function findOptionalOpenWorkSessionForTask(
  client: TakusuClient,
  taskId: string,
): Promise<WorkSessionRow | null> {
  const sessions = await client.listWorkSessions(taskId);
  const open = sessions.filter((s) => !s.ended_at);
  if (open.length > 1) {
    throw new Error(`multiple open work sessions for task ${taskId}`);
  }
  return open[0] ?? null;
}

// Find the open work session for a task. Throws if none exists or if more
// than one open session is found, so callers surface a clear error instead of
// silently failing or picking an arbitrary session.
export async function findOpenWorkSessionForTask(
  client: TakusuClient,
  taskId: string,
): Promise<WorkSessionRow> {
  const open = await findOptionalOpenWorkSessionForTask(client, taskId);
  if (!open) {
    throw new Error(`no open work session for task ${taskId}`);
  }
  return open;
}

export type TaskCompletionMode = 'direct' | 'work_session';

export interface TaskCompletionOptions {
  operationId?: string;
  quantityTotal?: number | null;
}

export async function completeTaskWithOptionalWorkSession(
  client: TakusuClient,
  taskId: string,
  options: TaskCompletionOptions = {},
): Promise<TaskCompletionMode> {
  const session = await findOptionalOpenWorkSessionForTask(client, taskId);
  if (session) {
    await client.completeWorkSession(
      session.id,
      options.operationId ?? makeProgressOperationId(),
    );
    return 'work_session';
  }
  await client.updateTask(taskId, {
    status: 'completed',
    ...(options.quantityTotal != null
      ? { quantity_done: options.quantityTotal }
      : {}),
  });
  return 'direct';
}

export async function restoreTaskAfterCompletion(
  client: TakusuClient,
  taskId: string,
  previousStatus: TaskStatus,
  previousQuantityDone: number,
): Promise<void> {
  await client.updateTask(taskId, {
    status: previousStatus,
    quantity_done: previousQuantityDone,
  });
  // A completed work session does not imply that the task was in progress
  // before completion. Recreating one for a scheduled task would promote it
  // to in_progress again, which is especially problematic after a bulk status
  // update left an inconsistent open session behind.
  if (previousStatus === 'in_progress') {
    await client.createWorkSession(
      { task_id: taskId },
      makeProgressOperationId(),
    );
  }
}

// Record progress against a work session. The server updates both the session
// and any linked task when quantity_total differs, so we can pass it directly
// in the RecordWorkSessionProgress body.
export async function recordProgressWithTotal(
  client: TakusuClient,
  session: WorkSessionRow,
  payload: ProgressPayload,
  options?: RecordProgressOptions,
): Promise<string> {
  const operationId = options?.operationId ?? makeProgressOperationId();

  const body: RecordWorkSessionProgress = {
    quantity_done: payload.quantityDone,
    note: payload.note,
  };

  if (
    payload.quantityTotal !== undefined &&
    payload.quantityTotal !== session.quantity_total
  ) {
    body.quantity_total = payload.quantityTotal;
  }

  await client.recordWorkSessionProgress(session.id, body, operationId);

  return operationId;
}
