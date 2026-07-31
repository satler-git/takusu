import { TakusuClient } from '@/src/api/client';

describe('TakusuClient', () => {
  const fetchMock = jest.fn();

  beforeEach(() => {
    jest.resetAllMocks();
    (globalThis as any).fetch = fetchMock;
    fetchMock.mockResolvedValue({
      status: 200,
      text: jest.fn().mockResolvedValue('[]'),
    });
  });

  afterEach(() => {
    fetchMock.mockRestore();
  });

  it('listTasks includes no_overdue query parameter', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    await client.listTasks({ no_overdue: true });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe('http://localhost/api/tasks?no_overdue=true');
  });

  it('listTasks sends no_overdue=false when explicitly false', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    await client.listTasks({ no_overdue: false });
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe('http://localhost/api/tasks?no_overdue=false');
  });

  it('listTasks omits no_overdue when not provided', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    await client.listTasks({});
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe('http://localhost/api/tasks');
  });

  it('encodes ids containing reserved URL characters', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    fetchMock.mockResolvedValueOnce({
      status: 200,
      text: jest.fn().mockResolvedValue('{}'),
    });
    await client.getTask('foo#bar');
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toBe('http://localhost/api/tasks/foo%23bar');
  });

  it('sends Idempotency-Key and Content-Type headers for createWorkSession when operationId is provided', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    fetchMock.mockResolvedValueOnce({
      status: 200,
      text: jest.fn().mockResolvedValue('{}'),
    });
    await client.createWorkSession({ task_id: 'task-1' }, 'op-1');
    const init = fetchMock.mock.calls[0][1] as RequestInit | undefined;
    expect(init?.headers).toMatchObject({
      Authorization: 'Bearer token',
      'Content-Type': 'application/json',
      'Idempotency-Key': 'op-1',
    });
  });

  it('sends Idempotency-Key and Content-Type headers for recordWorkSessionProgress when operationId is provided', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    fetchMock.mockResolvedValueOnce({
      status: 200,
      text: jest.fn().mockResolvedValue('{}'),
    });
    await client.recordWorkSessionProgress(
      'session-1',
      { quantity_done: 5, note: 'done' },
      'op-2',
    );
    const init = fetchMock.mock.calls[0][1] as RequestInit | undefined;
    expect(init?.headers).toMatchObject({
      Authorization: 'Bearer token',
      'Content-Type': 'application/json',
      'Idempotency-Key': 'op-2',
    });
  });

  it('omits Idempotency-Key header for createWorkSession when operationId is not provided', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    fetchMock.mockResolvedValueOnce({
      status: 200,
      text: jest.fn().mockResolvedValue('{}'),
    });
    await client.createWorkSession({ title: '作業' });
    const init = fetchMock.mock.calls[0][1] as RequestInit | undefined;
    expect(init?.headers).toMatchObject({
      Authorization: 'Bearer token',
      'Content-Type': 'application/json',
    });
    expect(init?.headers).not.toHaveProperty('Idempotency-Key');
  });

  it('returns a MoveEntryResponse from moveEntry with warnings defaulting to []', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    fetchMock.mockResolvedValueOnce({
      status: 200,
      text: jest.fn().mockResolvedValue(
        JSON.stringify({
          task_id: 'task-1',
          start_at: '2026-07-24T10:00:00Z',
          end_at: '2026-07-24T11:00:00Z',
          warnings: ['overlap ignored'],
        }),
      ),
    });
    const result = await client.moveEntry('task-1', {
      start_at: '2026-07-24T10:00:00Z',
      force: false,
    });
    expect(result.task_id).toBe('task-1');
    expect(result.start_at).toBe('2026-07-24T10:00:00Z');
    expect(result.end_at).toBe('2026-07-24T11:00:00Z');
    expect(result.warnings).toEqual(['overlap ignored']);
  });

  it('revokeToken rejects non-numeric token ids before making a request', async () => {
    const client = new TakusuClient('http://localhost', 'token');
    await expect(client.revokeToken('not-a-number' as any)).rejects.toThrow(
      'token id must be a positive integer',
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
