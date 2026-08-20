// Local takusu server lifecycle helpers.
//
// Used by ServerProvider (foreground) and the notification background task.

import TakusuServerModule, {
  type StartOptions,
} from '@/modules/takusu-server/src/TakusuServerModule';
import { TakusuClient } from '@/src/api/client';

export const DEFAULT_LOCAL_PORT = 3838;

export interface EnsureLocalServerOptions {
  port?: number;
  workersUrl: string;
  rootToken: string;
  agentConfigJson?: string;
}

function isAlreadyRunningError(err: unknown): boolean {
  const message = err instanceof Error ? err.message : String(err);
  // With the current native layer, start() returns success when the server is
  // already running, so these are only safety nets for stale exceptions.
  return (
    message.includes('already running') ||
    message.includes('ERR_ALREADY_RUNNING')
  );
}

const HEALTH_CHECK_INTERVAL_MS = 100;
const DEFAULT_HEALTH_CHECK_MAX_WAIT_MS = 5000;
const HEALTH_CHECK_TIMEOUT_MS = 1000;

export interface WaitForLocalServerReadyOptions {
  maxWaitMs?: number;
  signal?: AbortSignal;
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      'abort',
      () => {
        clearTimeout(timer);
        reject(new Error('aborted'));
      },
      { once: true },
    );
  });
}

/**
 * Perform a single health check with a per-call timeout.
 *
 * Creates a fresh `AbortController` for the timeout so that slow or hung
 * `fetch` calls cannot block the whole wait loop.
 */
async function healthWithTimeout(
  client: TakusuClient,
  signal?: AbortSignal,
): Promise<string> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), HEALTH_CHECK_TIMEOUT_MS);

  const onAbort = () => controller.abort();
  signal?.addEventListener('abort', onAbort, { once: true });

  try {
    return await client.health(controller.signal);
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', onAbort);
  }
}

/**
 * Poll the local server's `/health` endpoint until it responds.
 *
 * `TakusuServerModule.start()` returns as soon as the native runtime has bound
 * the port and spawned the axum task, but the server may not be accepting
 * requests yet. Without this wait, the first view that uses the client can fail
 * to load on cold start and remain broken until the server is restarted.
 */
export async function waitForLocalServerReady(
  client: TakusuClient,
  options: WaitForLocalServerReadyOptions = {},
): Promise<void> {
  const { maxWaitMs = DEFAULT_HEALTH_CHECK_MAX_WAIT_MS, signal } = options;
  const deadline = Date.now() + maxWaitMs;
  let lastError: Error | undefined;
  do {
    if (signal?.aborted) {
      throw new Error('aborted');
    }
    try {
      await healthWithTimeout(client, signal);
      return;
    } catch (e) {
      if (signal?.aborted) {
        throw new Error('aborted', { cause: e });
      }
      lastError = e instanceof Error ? e : new Error(String(e));
    }
    try {
      await sleep(HEALTH_CHECK_INTERVAL_MS, signal);
    } catch (e) {
      throw new Error('aborted', { cause: e });
    }
  } while (Date.now() < deadline);
  throw lastError ?? new Error('Local server did not become ready');
}

// Return the port the local server is currently running on, falling back
// to the default port if the module reports that it is not running.
export function getLocalServerPort(): number {
  try {
    const status = TakusuServerModule.status();
    if (status.running && status.port > 0) {
      return status.port;
    }
  } catch {
    // module may not be available in tests
  }
  return DEFAULT_LOCAL_PORT;
}

// Return a client for the local server, starting it if necessary.
// Throws if the server cannot be started.
export function ensureLocalServer(
  options: EnsureLocalServerOptions,
): TakusuClient {
  const {
    port = DEFAULT_LOCAL_PORT,
    workersUrl,
    rootToken,
    agentConfigJson,
  } = options;

  // Always call start(), even when status() reports running. A background
  // worker may own the Runtime; the foreground module must adopt it so this
  // process holds an Arc and the server stays alive after the worker stops.
  const status = TakusuServerModule.status();
  const startOptions: StartOptions = {
    port: status.running && status.port > 0 ? status.port : port,
    workersUrl,
    rootToken,
  };
  if (agentConfigJson !== undefined) {
    startOptions.agentConfigJson = agentConfigJson;
  }

  try {
    TakusuServerModule.start(startOptions);
  } catch (err) {
    if (!isAlreadyRunningError(err)) {
      throw err;
    }
  }

  const after = TakusuServerModule.status();
  if (!after.running) {
    throw new Error('Local server did not start');
  }

  return new TakusuClient(`http://127.0.0.1:${after.port}`, rootToken);
}
