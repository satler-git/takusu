import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  useMemo,
  type ReactNode,
} from 'react';
import Constants from 'expo-constants';
import { TakusuClient } from './client';
import { AgentClient, type AgentUpdateSettings } from './agentClient';
import {
  DEFAULT_LOCAL_PORT,
  ensureLocalServer,
  getLocalServerPort,
  waitForLocalServerReady,
} from './server';
import TakusuServerModule from '@/modules/takusu-server/src/TakusuServerModule';
import TakusuAppIconModule from '@/modules/takusu-app-icon/src/TakusuAppIconModule';
import TakusuWidgetModule from '../../modules/takusu-widget/src/TakusuWidgetModule';
import {
  loadSettings,
  loadAgentApiKey,
  saveWorkersUrl,
  saveWorkersToken,
  saveTheme,
  saveUndoSteps,
  saveNotificationSettings,
  systemInitialTheme,
  type PersistedSettings,
  type NotificationSettings,
} from './settingsStore';
import { APP_THEMES, type AppTheme } from '@/src/theme';
import { undoRedo, DEFAULT_MAX_HISTORY } from './undoRedo';
import { useAmbient } from '@/src/hooks/useAmbient';

interface ServerState {
  ready: boolean;
  settingsLoaded: boolean;
  error: string | null;
  client: TakusuClient | null;
  workersUrl: string;
  workersToken: string;
  theme: AppTheme;
  undoSteps: number;
  notifications: NotificationSettings;
  restarting: boolean;
}

interface ServerContextValue extends ServerState {
  restartServer: (url?: string, token?: string) => Promise<void>;
  setWorkersUrl: (url: string) => Promise<void>;
  setWorkersToken: (token: string) => Promise<void>;
  setTheme: (theme: AppTheme) => Promise<void>;
  setUndoSteps: (steps: number) => Promise<void>;
  setNotifications: (settings: NotificationSettings) => Promise<void>;
  pushAgentConfig: () => Promise<void>;
}

const ServerContext = createContext<ServerContextValue>({
  ready: false,
  settingsLoaded: false,
  error: null,
  client: null,
  workersUrl: '',
  workersToken: '',
  theme: 'light',
  undoSteps: DEFAULT_MAX_HISTORY,
  notifications: {} as NotificationSettings,
  restarting: false,
  restartServer: async () => {},
  setWorkersUrl: async () => {},
  setWorkersToken: async () => {},
  setTheme: async () => {},
  setUndoSteps: async () => {},
  setNotifications: async () => {},
  pushAgentConfig: async () => {},
});

export const DEFAULT_PORT = DEFAULT_LOCAL_PORT;

async function buildAgentUpdateSettings(): Promise<AgentUpdateSettings> {
  const settings = await loadSettings();
  const activeLlmModel = settings.llmModels.find(
    (m) => m.id === settings.activeLlmModel,
  );
  const activeLlmProvider = activeLlmModel
    ? settings.llmProviders.find((p) => p.id === activeLlmModel.providerId)
    : undefined;
  const activeTts = settings.ttsProviders.find(
    (p) => p.id === settings.activeTtsProvider,
  );
  const [llmKey, ttsKey] = await Promise.all([
    activeLlmProvider
      ? loadAgentApiKey('llm', activeLlmProvider.id)
      : Promise.resolve(''),
    activeTts ? loadAgentApiKey('tts', activeTts.id) : Promise.resolve(''),
  ]);
  const body: AgentUpdateSettings = {};
  if (activeLlmModel && activeLlmProvider) {
    body.llm = {
      base_url: activeLlmProvider.baseUrl,
      model: activeLlmModel.selectedModel,
    };
    if (llmKey) {
      body.llm.api_key = llmKey;
    }
    if (
      activeLlmModel.permissions &&
      Object.keys(activeLlmModel.permissions).length > 0
    ) {
      body.llm.permissions = activeLlmModel.permissions;
    }
  }
  if (activeTts) {
    body.audio = {
      tts: {
        backend: activeTts.provider,
        voice_id: activeTts.voiceId,
        language: activeTts.language,
        sample_rate: activeTts.sampleRate,
        model: activeTts.model,
        speed: activeTts.speed,
      },
    };
    if (ttsKey) {
      body.audio.tts.api_key = ttsKey;
    }
  }
  return body;
}

export function ServerProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ServerState>({
    ready: false,
    settingsLoaded: false,
    error: null,
    client: null,
    workersUrl: '',
    workersToken: '',
    theme: systemInitialTheme(),
    undoSteps: DEFAULT_MAX_HISTORY,
    notifications: {} as NotificationSettings,
    restarting: false,
  });

  const {
    enabled: ambientEnabled,
    running: ambientRunning,
    start: startAmbient,
  } = useAmbient();

  const startServer = useCallback(
    async (
      url: string,
      token: string,
      signal?: AbortSignal,
    ): Promise<TakusuClient | null> => {
      const finalUrl = url || process.env.EXPO_PUBLIC_WORKERS_URL || '';
      const finalToken = token || process.env.EXPO_PUBLIC_ROOT_TOKEN || '';

      if (!finalUrl || !finalToken) {
        throw new Error('Workers URL and token are required');
      }

      const agentConfigJson = JSON.stringify(await buildAgentUpdateSettings());

      // Retry starting the local server a few times. A background WorkManager
      // worker may have just stopped its temporary server, and the tokio
      // runtime (with its TcpListener) can take a moment to fully release the
      // port on a cold start triggered by a notification tap.
      const client = await (async (): Promise<TakusuClient> => {
        const maxAttempts = 10;
        const delayMs = 200;
        let lastError: unknown = null;
        for (let attempt = 0; attempt < maxAttempts; attempt++) {
          if (signal?.aborted) {
            throw new Error('aborted');
          }
          try {
            const c = ensureLocalServer({
              workersUrl: finalUrl,
              rootToken: finalToken,
              agentConfigJson,
            });
            await waitForLocalServerReady(c, { signal });
            return c;
          } catch (e) {
            lastError = e;
            if (attempt < maxAttempts - 1) {
              await new Promise((resolve) => setTimeout(resolve, delayMs));
            }
          }
        }
        throw lastError ?? new Error('Local server did not start');
      })();

      // Persist credentials for the home screen widget so the
      // WorkManager worker can start the local server independently.
      try {
        const scheme = Constants.expoConfig?.scheme;
        const port = getLocalServerPort();
        TakusuWidgetModule.saveConfig({
          workersUrl: finalUrl,
          token: finalToken,
          scheme: Array.isArray(scheme) ? scheme[0] : scheme,
          port,
        });
      } catch {
        // widget module not available during dev builds — ignore
      }

      return client;
    },
    [],
  );

  const restartServer = useCallback(
    async (url?: string, token?: string) => {
      const newUrl = url ?? state.workersUrl;
      const newToken = token ?? state.workersToken;

      setState((prev) => ({ ...prev, restarting: true, error: null }));

      try {
        try {
          await TakusuServerModule.stop();
        } catch {
          // server may not be running, ignore
        }

        const client = await startServer(newUrl, newToken);

        setState((prev) => ({
          ...prev,
          ready: true,
          error: null,
          client,
          workersUrl: newUrl,
          workersToken: newToken,
          restarting: false,
        }));
      } catch (e) {
        setState((prev) => ({
          ...prev,
          error: e instanceof Error ? e.message : String(e),
          restarting: false,
        }));
      }
    },
    [state.workersUrl, state.workersToken, startServer],
  );

  const setWorkersUrl = useCallback(async (url: string) => {
    await saveWorkersUrl(url);
    setState((prev) => ({ ...prev, workersUrl: url }));
  }, []);

  // Clearing the token also drops the client so that subsequent local API
  // calls do not run with an empty/invalid bearer token.
  const setWorkersToken = useCallback(async (token: string) => {
    await saveWorkersToken(token);
    setState((prev) => {
      const client =
        prev.client && token
          ? new TakusuClient(prev.client.baseUrl, token)
          : null;
      return { ...prev, workersToken: token, client };
    });
  }, []);

  const setTheme = useCallback(async (newTheme: AppTheme) => {
    if (!APP_THEMES.includes(newTheme)) return;
    await saveTheme(newTheme);
    setState((prev) => ({ ...prev, theme: newTheme }));
    try {
      TakusuAppIconModule.setTheme(newTheme);
    } catch {
      // icon module may not be available during dev builds
    }
  }, []);

  const setUndoSteps = useCallback(async (steps: number) => {
    if (!Number.isFinite(steps) || steps <= 0) return;
    const n = Math.floor(steps);
    await saveUndoSteps(n);
    undoRedo.setMaxHistory(n);
    setState((prev) => ({ ...prev, undoSteps: n }));
  }, []);

  const setNotifications = useCallback(
    async (settings: NotificationSettings) => {
      await saveNotificationSettings(settings);
      setState((prev) => ({ ...prev, notifications: settings }));
    },
    [],
  );

  useEffect(() => {
    let cancelled = false;
    const controller = new AbortController();

    async function init() {
      const settings: PersistedSettings = await loadSettings();

      if (cancelled || controller.signal.aborted) return;

      setState((prev) => ({
        ...prev,
        settingsLoaded: true,
        workersUrl: settings.workersUrl,
        workersToken: settings.workersToken,
        theme: settings.theme,
        undoSteps: settings.undoSteps,
        notifications: settings.notifications,
      }));

      try {
        TakusuAppIconModule.setTheme(settings.theme);
      } catch {
        // icon module may not be available during dev builds
      }

      undoRedo.setMaxHistory(settings.undoSteps);

      try {
        const client = await startServer(
          settings.workersUrl,
          settings.workersToken,
          controller.signal,
        );
        if (cancelled || controller.signal.aborted) return;
        setState((prev) => ({
          ...prev,
          ready: true,
          error: null,
          client,
        }));
      } catch (e) {
        if (cancelled || controller.signal.aborted) return;
        setState((prev) => ({
          ...prev,
          settingsLoaded: true,
          ready: false,
          error: e instanceof Error ? e.message : String(e),
          client: null,
        }));
      }
    }

    init();

    return () => {
      cancelled = true;
      controller.abort();
      // stop() is a synchronous native Function; a thrown native
      // exception (e.g. "Server not running") propagates synchronously,
      // so use try/catch rather than Promise.resolve().catch().
      try {
        TakusuServerModule.stop();
      } catch {
        // server may not be running
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Resume the ambient foreground service when the server is ready and the
  // user has previously enabled it. The service persists across the app being
  // backgrounded, and the boot receiver posts a re-arm notification if it is
  // killed.
  useEffect(() => {
    if (!state.ready || !state.workersUrl || !state.workersToken) return;
    if (!ambientEnabled || ambientRunning) return;
    startAmbient(
      {
        workersUrl: state.workersUrl,
        rootToken: state.workersToken,
      },
      // Auto-resume must not pop permission dialogs while the user is doing
      // something else; the manual toggle in AgentSettingsView requests them.
      { requestPermissions: false },
    ).catch((error) => {
      // The user may have revoked the microphone permission; a manual
      // toggle in AgentSettingsView will surface the error. Log it for
      // debugging.
      console.warn('auto-resume ambient failed:', error);
    });
  }, [
    state.ready,
    state.workersUrl,
    state.workersToken,
    ambientEnabled,
    ambientRunning,
    startAmbient,
  ]);

  const pushAgentConfig = useCallback(async () => {
    if (!state.ready || !state.workersToken || !state.client) {
      return;
    }
    const agentClient = new AgentClient(
      state.client.baseUrl,
      state.workersToken,
    );
    await agentClient.updateSettings(await buildAgentUpdateSettings());
  }, [state.ready, state.workersToken, state.client]);

  const contextValue = useMemo<ServerContextValue>(
    () => ({
      ...state,
      restartServer,
      setWorkersUrl,
      setWorkersToken,
      setTheme,
      setUndoSteps,
      setNotifications,
      pushAgentConfig,
    }),
    [
      state,
      restartServer,
      setWorkersUrl,
      setWorkersToken,
      setTheme,
      setUndoSteps,
      setNotifications,
      pushAgentConfig,
    ],
  );

  return (
    <ServerContext.Provider value={contextValue}>
      {children}
    </ServerContext.Provider>
  );
}

export function useServer() {
  return useContext(ServerContext);
}

export { saveWorkersUrl, saveWorkersToken, saveNotificationSettings };
