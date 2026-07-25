import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  TakusuClient,
  parseDepends,
  parseDependsOn,
  parseSchedule,
} from './index';

describe('parse helpers', () => {
  it('parseDepends parses a JSON array', () => {
    expect(parseDepends('["a","b"]')).toEqual(['a', 'b']);
  });

  it('parseDepends returns [] on invalid JSON', () => {
    expect(parseDepends('not json')).toEqual([]);
  });

  it('parseDependsOn parses a JSON array', () => {
    expect(parseDependsOn('["s1"]')).toEqual(['s1']);
  });

  it('parseSchedule parses a JSON array of entries', () => {
    const json = JSON.stringify([{ task_id: 't', start_at: 'a', end_at: 'b' }]);
    expect(parseSchedule(json)).toEqual([
      { task_id: 't', start_at: 'a', end_at: 'b' },
    ]);
  });
});

describe('TakusuClient', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('sends the bearer token and builds query strings', async () => {
    const fetchMock = vi.fn((_url: string, _init: RequestInit) =>
      Promise.resolve(new Response('[]', { status: 200 })),
    );
    vi.stubGlobal('fetch', fetchMock);

    const client = new TakusuClient('http://localhost:3000', 'tok');
    await client.listTasks({ status: 'pending', no_overdue: true });

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(
      'http://localhost:3000/api/tasks?status=pending&no_overdue=true',
    );
    const headers = init.headers as Record<string, string>;
    expect(headers.Authorization).toBe('Bearer tok');
  });
});
