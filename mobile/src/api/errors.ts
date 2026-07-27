// Shared error display helpers.
// All async UI actions should surface failures to the user via `showError`
// rather than silently swallowing them. Notification side-effects (which are
// non-critical and should not interrupt the user) use `logError`.
//
// Both helpers also forward the formatted message to the native log ring
// buffer (via TakusuServerModule.pushLog) so client-side errors appear in
// the same export as server logs.

import { Alert } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { getTopToastRef } from '@/src/api/topToastRef';
import { ApiError } from './client';

/** Format an unknown error into a human-readable string. */
export function formatError(e: unknown): string {
  if (e instanceof ApiError) {
    // Try to parse the response body as JSON for a structured message,
    // otherwise fall back to the raw body.
    try {
      const parsed = JSON.parse(e.body);
      if (typeof parsed === 'string') return parsed;
      if (parsed && typeof parsed.error === 'string') return parsed.error;
      if (parsed && typeof parsed.message === 'string') return parsed.message;
    } catch {
      // not JSON
    }
    if (e.body) return e.body;
    return `HTTP ${e.status}`;
  }
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * Format an error for the log buffer, including the stack trace when
 * available. The stack trace is essential for debugging exported logs
 * (issue #90).
 */
function formatErrorForLog(e: unknown): string {
  const base = formatError(e);
  if (e instanceof Error && e.stack && e.stack !== `Error: ${e.message}`) {
    return `${base}\n${e.stack}`;
  }
  if (e instanceof ApiError && e.stack) {
    return `${base}\n${e.stack}`;
  }
  return base;
}

/**
 * Forward a log line to the native ring buffer.
 * Silently no-ops if the native module is unavailable.
 */
function pushClientLog(level: string, context: string, message: string): void {
  const line = `[client][${level}] ${context}: ${message}`;
  // pushLog is a synchronous native Function; a thrown native exception
  // propagates synchronously, so use try/catch rather than
  // Promise.resolve().catch().
  try {
    const TakusuServerModule =
      require('../../modules/takusu-server/src/TakusuServerModule').default as {
        pushLog: (line: string) => void;
      };
    TakusuServerModule.pushLog(line);
  } catch {
    // native module not ready — drop silently
  }
}

// Track active error toasts so repeated identical errors replace the
// existing toast instead of filling the screen. A hard cap also prevents
// an unrelated burst of errors from covering the UI.
const MAX_ERROR_TOASTS = 3;

interface ActiveErrorToast {
  id: string;
  count: number;
}

const activeErrorToasts = new Map<string, ActiveErrorToast>();
const activeErrorToastIds = new Map<string, string>();

function removeActiveErrorToast(id: string): void {
  const key = activeErrorToastIds.get(id);
  if (!key) return;
  activeErrorToastIds.delete(id);
  const current = activeErrorToasts.get(key);
  if (current && current.id === id) {
    activeErrorToasts.delete(key);
  }
}

/**
 * Show a non-blocking top toast for an operation failure.
 * `title` defaults to "エラー" but can be overridden for context
 * (e.g. "タスクの削除に失敗").
 *
 * The toast includes a "コピー" action so the user can copy the full
 * error message (including stack trace when available) to the clipboard
 * for bug reports (issue #216). Falls back to an alert when no toast
 * provider is mounted.
 *
 * Identical errors replace the previous toast for that error and count up,
 * so the screen cannot be filled with repeated failures.
 */
export async function showError(
  e: unknown,
  title = 'エラー',
): Promise<string | undefined> {
  const msg = formatError(e);
  const fullMsg = formatErrorForLog(e);
  pushClientLog('error', title, fullMsg);

  const toastRef = getTopToastRef();
  if (toastRef) {
    const key = `${title}\n${msg}`;
    const existing = activeErrorToasts.get(key);
    const count = existing ? existing.count + 1 : 1;

    if (existing) {
      toastRef.hideTopToast(existing.id);
      removeActiveErrorToast(existing.id);
    }

    while (activeErrorToasts.size >= MAX_ERROR_TOASTS) {
      const firstKey = activeErrorToasts.keys().next().value as string;
      const first = activeErrorToasts.get(firstKey);
      if (first) {
        toastRef.hideTopToast(first.id);
        removeActiveErrorToast(first.id);
      }
    }

    const displayMessage =
      count > 1 ? `${title}: ${msg} (${count})` : `${title}: ${msg}`;

    let toastId = '';
    toastId = toastRef.showTopToast(displayMessage, {
      type: 'error',
      duration: Infinity,
      action: {
        label: 'コピー',
        onPress: () => {
          void (async () => {
            try {
              await Clipboard.setStringAsync(fullMsg);
              toastRef.hideTopToast(toastId);
            } catch {
              toastRef.showTopToast('コピーに失敗しました', {
                type: 'error',
                duration: 3000,
              });
            }
          })();
        },
      },
      onDismiss: () => {
        removeActiveErrorToast(toastId);
      },
    });

    activeErrorToasts.set(key, { id: toastId, count });
    activeErrorToastIds.set(toastId, key);
    return toastId;
  }

  Alert.alert(
    title,
    msg,
    [
      {
        text: 'コピー',
        onPress: () => {
          void Clipboard.setStringAsync(fullMsg);
        },
      },
      { text: 'OK', style: 'cancel' },
    ],
    { cancelable: true },
  );
  return undefined;
}

/**
 * Log a non-critical error without interrupting the user.
 * Used for notification side-effects where a failure should not block the UI.
 */
export function logError(context: string, e: unknown): void {
  const msg = formatError(e);
  pushClientLog('warn', context, formatErrorForLog(e));
  console.warn(`${context}:`, msg);
}
