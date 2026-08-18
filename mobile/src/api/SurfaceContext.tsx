import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { AgentClient } from './agentClient';
import { DEFAULT_PORT, useServer } from './ServerProvider';
import type {
  SurfaceAudioCallback,
  SurfaceCommand,
  SurfaceCommandResponse,
  SurfaceEvent,
  SurfaceSnapshot,
} from './agentTypes';

function surfaceEventToSnapshot(event: SurfaceEvent): SurfaceSnapshot {
  const { type: _type, ...snapshot } = event;
  return snapshot as SurfaceSnapshot;
}

interface SurfaceContextValue {
  /** Latest device-scoped surface snapshot, or null before the first event. */
  snapshot: SurfaceSnapshot | null;
  /** Agent client bound to the local server, or null when not ready. */
  agentClient: AgentClient | null;
  /** Send a surface command; returns the server response. */
  sendCommand(
    command: SurfaceCommand,
    operationId?: number,
  ): Promise<SurfaceCommandResponse | null>;
  /** Report an audio lifecycle callback to the shared surface state machine. */
  reportAudio(
    callback: SurfaceAudioCallback,
    operationId?: number,
  ): Promise<SurfaceSnapshot | null>;
  /** True while a surface subscription is active. */
  connected: boolean;
  /** Fatal or persistent subscription error, if any. */
  error: string | null;
}

const SurfaceContext = createContext<SurfaceContextValue>({
  snapshot: null,
  agentClient: null,
  sendCommand: async () => null,
  reportAudio: async () => null,
  connected: false,
  error: null,
});

export function useSurface(): SurfaceContextValue {
  return useContext(SurfaceContext);
}

interface SurfaceProviderProps {
  children: ReactNode;
}

/**
 * Provides the device-scoped surface state shared by all resident-agent
 * surfaces on this device. Subscribes to `/api/agent/v1/surface/events` and
 * exposes helpers to send commands and report audio callbacks.
 */
export function SurfaceProvider({ children }: SurfaceProviderProps) {
  const { workersToken } = useServer();
  const agentClient = useMemo<AgentClient | null>(() => {
    if (!workersToken) return null;
    return new AgentClient(`http://127.0.0.1:${DEFAULT_PORT}`, workersToken);
  }, [workersToken]);

  const [snapshot, setSnapshot] = useState<SurfaceSnapshot | null>(null);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!agentClient) {
      setSnapshot(null);
      setConnected(false);
      setError(null);
      return;
    }

    setError(null);
    let active = true;
    const unsubscribe = agentClient.subscribeSurface((event: SurfaceEvent) => {
      if (!active) return;
      setSnapshot(surfaceEventToSnapshot(event));
      setConnected(true);
    });

    return () => {
      active = false;
      unsubscribe();
      setConnected(false);
    };
  }, [agentClient]);

  const sendCommand = useMemo(
    () =>
      async (
        command: SurfaceCommand,
        operationId?: number,
      ): Promise<SurfaceCommandResponse | null> => {
        if (!agentClient) return null;
        try {
          const response = await agentClient.sendSurfaceCommand(
            command,
            operationId,
          );
          if (response?.snapshot) {
            setSnapshot(response.snapshot);
          }
          return response;
        } catch (e) {
          const message = e instanceof Error ? e.message : String(e);
          setError(message);
          return null;
        }
      },
    [agentClient],
  );

  const reportAudio = useMemo(
    () =>
      async (
        callback: SurfaceAudioCallback,
        operationId?: number,
      ): Promise<SurfaceSnapshot | null> => {
        if (!agentClient) return null;
        try {
          const next = await agentClient.reportSurfaceAudio(
            callback,
            operationId,
          );
          if (next) {
            setSnapshot(next);
          }
          return next;
        } catch (e) {
          const message = e instanceof Error ? e.message : String(e);
          setError(message);
          return null;
        }
      },
    [agentClient],
  );

  const value = useMemo<SurfaceContextValue>(
    () => ({
      snapshot,
      agentClient,
      sendCommand,
      reportAudio,
      connected,
      error,
    }),
    [snapshot, agentClient, sendCommand, reportAudio, connected, error],
  );

  return (
    <SurfaceContext.Provider value={value}>{children}</SurfaceContext.Provider>
  );
}
