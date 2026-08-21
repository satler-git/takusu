import {
  completeTaskWithOptionalWorkSession,
  recordProgressWithTotal,
  makeProgressOperationId,
  restoreTaskAfterCompletion,
} from '@/src/utils/progress';
import type { TakusuClient } from '@/src/api/client';
import type { WorkSessionRow } from '@/src/api/types';

function makeSession(overrides?: Partial<WorkSessionRow>): WorkSessionRow {
  return {
    id: 'session-1',
    task_id: 'task-1',
    title: '作業',
    quantity_done: 0,
    quantity_total: 10,
    quantity_unit: '個',
    started_at: '2026-06-01T10:00:00Z',
    created_at: '2026-06-01T10:00:00Z',
    ...overrides,
  };
}

function makeClient(
  overrides?: Partial<Record<keyof TakusuClient, unknown>>,
): TakusuClient {
  return {
    recordWorkSessionProgress: jest.fn().mockResolvedValue({} as never),
    createWorkSession: jest.fn().mockResolvedValue({} as never),
    ...overrides,
  } as unknown as TakusuClient;
}

describe('completeTaskWithOptionalWorkSession', () => {
  it('updates the task directly when no work session is open', async () => {
    const client = makeClient({
      listWorkSessions: jest.fn().mockResolvedValue([]),
      updateTask: jest.fn().mockResolvedValue({} as never),
      completeWorkSession: jest.fn(),
    });

    const mode = await completeTaskWithOptionalWorkSession(client, 'task-1', {
      quantityTotal: 10,
    });

    expect(mode).toBe('direct');
    expect(client.updateTask).toHaveBeenCalledWith('task-1', {
      status: 'completed',
      quantity_done: 10,
    });
    expect(client.completeWorkSession).not.toHaveBeenCalled();
  });

  it('completes the open work session when one exists', async () => {
    const client = makeClient({
      listWorkSessions: jest.fn().mockResolvedValue([makeSession()]),
      updateTask: jest.fn(),
      completeWorkSession: jest.fn().mockResolvedValue({} as never),
    });

    const mode = await completeTaskWithOptionalWorkSession(client, 'task-1', {
      operationId: 'op-123',
      quantityTotal: 10,
    });

    expect(mode).toBe('work_session');
    expect(client.completeWorkSession).toHaveBeenCalledWith(
      'session-1',
      'op-123',
    );
    expect(client.updateTask).not.toHaveBeenCalled();
  });
});

describe('restoreTaskAfterCompletion', () => {
  it('reopens a work session when restoring direct completion of in_progress', async () => {
    const client = makeClient({
      updateTask: jest.fn().mockResolvedValue({} as never),
    });

    await restoreTaskAfterCompletion(client, 'task-1', 'in_progress', 3);

    expect(client.updateTask).toHaveBeenCalledWith('task-1', {
      status: 'in_progress',
      quantity_done: 3,
    });
    expect(client.createWorkSession).toHaveBeenCalledWith(
      { task_id: 'task-1' },
      expect.any(String),
    );
  });

  it('does not reopen a session when restoring a scheduled task', async () => {
    const client = makeClient({
      updateTask: jest.fn().mockResolvedValue({} as never),
    });

    await restoreTaskAfterCompletion(client, 'task-1', 'scheduled', 3);

    expect(client.updateTask).toHaveBeenCalledWith('task-1', {
      status: 'scheduled',
      quantity_done: 3,
    });
    expect(client.createWorkSession).not.toHaveBeenCalled();
  });
});

describe('recordProgressWithTotal', () => {
  it('generates and passes an operationId to recordWorkSessionProgress', async () => {
    const client = makeClient();
    const session = makeSession();
    const returned = await recordProgressWithTotal(client, session, {
      quantityDone: 5,
      note: 'done',
    });
    expect(client.recordWorkSessionProgress).toHaveBeenCalledWith(
      'session-1',
      { quantity_done: 5, note: 'done' },
      expect.stringMatching(/./),
    );
    expect(returned).toBe(
      (client.recordWorkSessionProgress as jest.Mock).mock.calls[0][2],
    );
  });

  it('uses the provided operationId when given', async () => {
    const client = makeClient();
    const session = makeSession();
    const operationId = 'op-123';
    const returned = await recordProgressWithTotal(
      client,
      session,
      { quantityDone: 3 },
      { operationId },
    );
    expect(client.recordWorkSessionProgress).toHaveBeenCalledWith(
      'session-1',
      { quantity_done: 3, note: undefined },
      operationId,
    );
    expect(returned).toBe(operationId);
  });

  it('skips quantity_total when it matches the current total', async () => {
    const client = makeClient();
    const session = makeSession({ quantity_total: 10 });
    await recordProgressWithTotal(client, session, {
      quantityDone: 5,
      quantityTotal: 10,
    });
    expect(client.recordWorkSessionProgress).toHaveBeenCalledWith(
      'session-1',
      { quantity_done: 5, note: undefined },
      expect.any(String),
    );
  });

  it('passes quantity_total in the progress body when it changed', async () => {
    const client = makeClient();
    const session = makeSession({ quantity_total: 10 });
    await recordProgressWithTotal(client, session, {
      quantityDone: 5,
      quantityTotal: 20,
    });
    expect(client.recordWorkSessionProgress).toHaveBeenCalledWith(
      'session-1',
      { quantity_done: 5, note: undefined, quantity_total: 20 },
      expect.any(String),
    );
  });

  it('passes quantity_total for a standalone session when it changed', async () => {
    const client = makeClient();
    const session = makeSession({
      task_id: undefined,
      quantity_total: undefined,
    });
    await recordProgressWithTotal(client, session, {
      quantityDone: 5,
      quantityTotal: 20,
    });
    expect(client.recordWorkSessionProgress).toHaveBeenCalledWith(
      'session-1',
      { quantity_done: 5, note: undefined, quantity_total: 20 },
      expect.any(String),
    );
  });

  it('does not pass quantity_total when it is undefined and the session has no total', async () => {
    const client = makeClient();
    const session = makeSession({
      quantity_total: undefined,
    });
    await recordProgressWithTotal(client, session, {
      quantityDone: 5,
    });
    expect(client.recordWorkSessionProgress).toHaveBeenCalledWith(
      'session-1',
      { quantity_done: 5, note: undefined },
      expect.any(String),
    );
  });

  it('does not update the task directly', async () => {
    const client = makeClient();
    const session = makeSession({ quantity_total: 10 });
    await recordProgressWithTotal(client, session, {
      quantityDone: 5,
      quantityTotal: 20,
    });
    expect(
      (client as unknown as { updateTask?: jest.Mock }).updateTask,
    ).toBeUndefined();
  });

  it('propagates recordWorkSessionProgress errors without rollback', async () => {
    const client = makeClient({
      recordWorkSessionProgress: jest
        .fn()
        .mockRejectedValue(new Error('network')),
    });
    const session = makeSession({ quantity_total: 10 });
    await expect(
      recordProgressWithTotal(client, session, {
        quantityDone: 5,
        quantityTotal: 20,
      }),
    ).rejects.toThrow('network');
    expect(client.recordWorkSessionProgress).toHaveBeenCalledTimes(1);
  });
});

describe('makeProgressOperationId', () => {
  it('returns a non-empty string', () => {
    const id = makeProgressOperationId();
    expect(typeof id).toBe('string');
    expect(id.length).toBeGreaterThan(0);
  });
});
