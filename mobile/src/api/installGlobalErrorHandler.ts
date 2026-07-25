// Install a global error handler that forwards uncaught JS exceptions and
// unhandled promise rejections to the native log ring buffer so they appear
// in the same export as server-side logs.
//
// Call `installGlobalErrorHandler()` once at app startup (e.g. from the
// root layout). It is idempotent.

import TakusuServerModule from '../../modules/takusu-server/src/TakusuServerModule';

let installed = false;

function pushClientLog(line: string): void {
  // pushLog is a synchronous native Function; a thrown native exception
  // propagates synchronously, so use try/catch rather than
  // Promise.resolve().catch().
  try {
    TakusuServerModule.pushLog(line);
  } catch {
    // native module not ready — drop silently
  }
}

export function installGlobalErrorHandler(): void {
  if (installed) return;
  installed = true;

  // React Native exposes a global `ErrorUtils` with setGlobalHandler.
  // The default handler logs to console.error (logcat); we wrap it so the
  // formatted error is also pushed to the ring buffer.
  const g = globalThis as unknown as {
    ErrorUtils?: {
      setGlobalHandler?: (
        handler: (e: Error, isFatal?: boolean) => void,
      ) => void;
      getGlobalHandler?: () => (e: Error, isFatal?: boolean) => void;
    };
  };

  if (g.ErrorUtils?.setGlobalHandler) {
    const previous = g.ErrorUtils.getGlobalHandler?.();
    g.ErrorUtils.setGlobalHandler((err, isFatal) => {
      const line = `[client][error] unhandled exception${isFatal ? ' (fatal)' : ''}: ${err?.stack ?? String(err)}`;
      pushClientLog(line);
      previous?.(err, isFatal);
    });
  }

  // Unhandled promise rejections — React Native emits a `unhandledRejection`
  // event on the global in newer Hermes versions. Fall back to patching
  // Promise.onUnhandledRejection if available.
  const promiseGlobal = globalThis as unknown as {
    Promise?: { onUnhandledRejection?: (id: number, reason: unknown) => void };
  };
  if (promiseGlobal.Promise?.onUnhandledRejection !== undefined) {
    const orig = promiseGlobal.Promise.onUnhandledRejection.bind(
      promiseGlobal.Promise,
    );
    promiseGlobal.Promise.onUnhandledRejection = (
      id: number,
      reason: unknown,
    ) => {
      const msg =
        reason instanceof Error
          ? (reason.stack ?? reason.message)
          : String(reason);
      pushClientLog(`[client][error] unhandled rejection: ${msg}`);
      orig(id, reason);
    };
  }
}
