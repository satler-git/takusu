import {
  recordProgressWithTotal,
  makeProgressOperationId,
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
    ...overrides,
  } as unknown as TakusuClient;
}

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
