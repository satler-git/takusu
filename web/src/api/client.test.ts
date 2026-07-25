import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: { 'Content-Type': 'application/json' },
    }),
  );
}

describe('getClient', () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('bootstraps a TakusuClient from /bootstrap', async () => {
    const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
      if (url === '/bootstrap') return jsonResponse({ token: 'tok_123' });
      return jsonResponse({});
    });
    vi.stubGlobal('fetch', fetchMock);

    const { getClient } = await import('./client');
    const client = await getClient();
    // Duck-type check (instanceof is unreliable across vi.resetModules() module
    // instances): the bootstrapped client exposes the TakusuClient surface.
    expect(typeof client.health).toBe('function');
    expect(client.baseUrl).toBe('');
    expect(fetchMock).toHaveBeenCalledWith('/bootstrap');
  });

  it('fetches /bootstrap only once across calls', async () => {
    const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
      if (url === '/bootstrap') return jsonResponse({ token: 'tok_123' });
      return jsonResponse([]);
    });
    vi.stubGlobal('fetch', fetchMock);

    const { getClient } = await import('./client');
    await getClient();
    await getClient();

    const bootstrapCalls = fetchMock.mock.calls.filter(
      ([u]) => u === '/bootstrap',
    );
    expect(bootstrapCalls).toHaveLength(1);
  });

  it('returns a client whose health() hits /health', async () => {
    const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
      if (url === '/bootstrap') return jsonResponse({ token: 'tok_abc' });
      if (url === '/health')
        return Promise.resolve(new Response('ok', { status: 200 }));
      return jsonResponse({});
    });
    vi.stubGlobal('fetch', fetchMock);

    const { getClient } = await import('./client');
    const client = await getClient();
    await expect(client.health()).resolves.toBe('ok');
  });
});
