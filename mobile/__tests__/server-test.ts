import TakusuServerModule from '@/modules/takusu-server/src/TakusuServerModule';
import { TakusuClient } from '@/src/api/client';
import {
  ensureLocalServer,
  getLocalServerPort,
  waitForLocalServerReady,
  DEFAULT_LOCAL_PORT,
} from '@/src/api/server';

jest.mock('@/modules/takusu-server/src/TakusuServerModule', () => ({
  status: jest.fn(),
  start: jest.fn(),
}));

const mockedModule = TakusuServerModule as unknown as {
  status: jest.Mock;
  start: jest.Mock;
};

describe('ensureLocalServer', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it('reuses a running server and returns a client for the reported port', () => {
    mockedModule.status.mockReturnValue({ running: true, port: 4242 });
    const client = ensureLocalServer({
      workersUrl: 'https://example.com',
      rootToken: 'token',
    });
    expect(mockedModule.start).not.toHaveBeenCalled();
    expect((client as any).baseUrl).toBe('http://127.0.0.1:4242');
  });

  it('starts the server on the default port when it is not running', () => {
    mockedModule.status
      .mockReturnValueOnce({ running: false, port: 0 })
      .mockReturnValueOnce({ running: true, port: DEFAULT_LOCAL_PORT });
    const client = ensureLocalServer({
      workersUrl: 'https://example.com',
      rootToken: 'token',
    });
    expect(mockedModule.start).toHaveBeenCalledWith(
      expect.objectContaining({
        port: DEFAULT_LOCAL_PORT,
        workersUrl: 'https://example.com',
        rootToken: 'token',
      }),
    );
    expect((client as any).baseUrl).toBe(
      `http://127.0.0.1:${DEFAULT_LOCAL_PORT}`,
    );
  });

  it('uses a custom port when provided', () => {
    mockedModule.status
      .mockReturnValueOnce({ running: false, port: 0 })
      .mockReturnValueOnce({ running: true, port: 5000 });
    const client = ensureLocalServer({
      port: 5000,
      workersUrl: 'https://example.com',
      rootToken: 'token',
    });
    expect(mockedModule.start).toHaveBeenCalledWith(
      expect.objectContaining({ port: 5000 }),
    );
    expect((client as any).baseUrl).toBe('http://127.0.0.1:5000');
  });

  it('reuses the server when start reports it is already running', () => {
    mockedModule.status
      .mockReturnValueOnce({ running: false, port: 0 })
      .mockReturnValueOnce({ running: true, port: DEFAULT_LOCAL_PORT });
    mockedModule.start.mockImplementation(() => {
      throw new Error('already running');
    });
    const client = ensureLocalServer({
      workersUrl: 'https://example.com',
      rootToken: 'token',
    });
    expect(mockedModule.start).toHaveBeenCalled();
    expect((client as any).baseUrl).toBe(
      `http://127.0.0.1:${DEFAULT_LOCAL_PORT}`,
    );
  });

  it('rethrows non-already-running start errors', () => {
    mockedModule.status.mockReturnValue({ running: false, port: 0 });
    mockedModule.start.mockImplementation(() => {
      throw new Error('port in use');
    });
    expect(() =>
      ensureLocalServer({
        workersUrl: 'https://example.com',
        rootToken: 'token',
      }),
    ).toThrow('port in use');
  });

  it('throws when the server does not report running after start', () => {
    mockedModule.status
      .mockReturnValueOnce({ running: false, port: 0 })
      .mockReturnValueOnce({ running: false, port: 0 });
    mockedModule.start.mockReturnValue(true);
    expect(() =>
      ensureLocalServer({
        workersUrl: 'https://example.com',
        rootToken: 'token',
      }),
    ).toThrow('Local server did not start');
  });

  it('passes agentConfigJson when provided', () => {
    mockedModule.status
      .mockReturnValueOnce({ running: false, port: 0 })
      .mockReturnValueOnce({ running: true, port: DEFAULT_LOCAL_PORT });
    ensureLocalServer({
      workersUrl: 'https://example.com',
      rootToken: 'token',
      agentConfigJson: '{"llm":{}}',
    });
    expect(mockedModule.start).toHaveBeenCalledWith(
      expect.objectContaining({ agentConfigJson: '{"llm":{}}' }),
    );
  });
});

describe('getLocalServerPort', () => {
  beforeEach(() => {
    jest.clearAllMocks();
  });

  it('returns the reported port when the server is running', () => {
    mockedModule.status.mockReturnValue({ running: true, port: 4242 });
    expect(getLocalServerPort()).toBe(4242);
  });

  it('falls back to the default port when the server is not running', () => {
    mockedModule.status.mockReturnValue({ running: false, port: 0 });
    expect(getLocalServerPort()).toBe(DEFAULT_LOCAL_PORT);
  });

  it('falls back to the default port when the reported port is zero', () => {
    mockedModule.status.mockReturnValue({ running: true, port: 0 });
    expect(getLocalServerPort()).toBe(DEFAULT_LOCAL_PORT);
  });

  it('falls back to the default port when the native module throws', () => {
    mockedModule.status.mockImplementation(() => {
      throw new Error('not available');
    });
    expect(getLocalServerPort()).toBe(DEFAULT_LOCAL_PORT);
  });
});

describe('waitForLocalServerReady', () => {
  let healthSpy: jest.SpyInstance<ReturnType<TakusuClient['health']>>;

  beforeEach(() => {
    healthSpy = jest
      .spyOn(TakusuClient.prototype, 'health')
      .mockRejectedValue(new Error('not ready'));
  });

  afterEach(() => {
    healthSpy.mockRestore();
  });

  it('returns when health() succeeds', async () => {
    const client = new TakusuClient('http://127.0.0.1:3838', 'token');
    healthSpy.mockResolvedValueOnce('ok');
    await expect(
      waitForLocalServerReady(client, { maxWaitMs: 1000 }),
    ).resolves.toBeUndefined();
  });

  it('retries until health() succeeds', async () => {
    const client = new TakusuClient('http://127.0.0.1:3838', 'token');
    healthSpy
      .mockRejectedValueOnce(new Error('not ready'))
      .mockRejectedValueOnce(new Error('not ready'))
      .mockResolvedValueOnce('ok');

    await expect(
      waitForLocalServerReady(client, { maxWaitMs: 1000 }),
    ).resolves.toBeUndefined();
    expect(healthSpy).toHaveBeenCalledTimes(3);
  });

  it('throws the last health error when the deadline passes', async () => {
    const client = new TakusuClient('http://127.0.0.1:3838', 'token');
    const lastError = new Error('connection refused');
    healthSpy.mockRejectedValue(lastError);

    await expect(
      waitForLocalServerReady(client, { maxWaitMs: 50 }),
    ).rejects.toThrow('connection refused');
  });

  it('tries once even when maxWaitMs is 0', async () => {
    const client = new TakusuClient('http://127.0.0.1:3838', 'token');
    healthSpy.mockResolvedValueOnce('ok');
    await expect(
      waitForLocalServerReady(client, { maxWaitMs: 0 }),
    ).resolves.toBeUndefined();
    expect(healthSpy).toHaveBeenCalledTimes(1);
  });

  it('stops polling when the signal is aborted', async () => {
    const client = new TakusuClient('http://127.0.0.1:3838', 'token');
    const controller = new AbortController();
    controller.abort();
    await expect(
      waitForLocalServerReady(client, { signal: controller.signal }),
    ).rejects.toThrow('aborted');
  });
});
