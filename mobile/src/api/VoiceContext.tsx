import {
  createContext,
  useContext,
  useState,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
} from 'react';

const IDLE_TIMEOUT_MS = 30000;

interface VoiceContextValue {
  /** Whether any voice button is currently recording. */
  isRecording: boolean;
  setIsRecording: (value: boolean) => void;
  /** Whether a continuous voice session is active (multi-turn continuation). */
  sessionActive: boolean;
  /** Begin a continuous voice session; recording re-arms after each turn. */
  startSession: () => void;
  /** End the continuous voice session after the current turn. */
  stopSession: () => void;
  /** Reset the idle timeout that ends an active continuous session. */
  resetIdleTimeout: () => void;
  /** Session id queued by the floating voice button for AgentView to activate as a new session. */
  pendingSessionId: string | null;
  setPendingSessionId: (value: string | null) => void;
}

const VoiceContext = createContext<VoiceContextValue>({
  isRecording: false,
  setIsRecording: () => {},
  sessionActive: false,
  startSession: () => {},
  stopSession: () => {},
  resetIdleTimeout: () => {},
  pendingSessionId: null,
  setPendingSessionId: () => {},
});

export function VoiceProvider({
  children,
  onRecordingChange,
}: {
  children: ReactNode;
  onRecordingChange?: (
    listener: (recording: boolean) => void,
  ) => (() => void) | void;
}) {
  const [isRecording, setIsRecording] = useState(false);
  const [sessionActive, setSessionActive] = useState(false);
  const [pendingSessionId, setPendingSessionIdState] = useState<string | null>(
    null,
  );
  const idleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setIsRecordingStable = useCallback((value: boolean) => {
    setIsRecording(value);
  }, []);

  useEffect(() => {
    if (!onRecordingChange) return;
    return onRecordingChange(setIsRecordingStable);
  }, [onRecordingChange, setIsRecordingStable]);

  const setPendingSessionId = useCallback((value: string | null) => {
    setPendingSessionIdState(value);
  }, []);

  const clearIdleTimeout = useCallback(() => {
    if (idleTimerRef.current) {
      clearTimeout(idleTimerRef.current);
      idleTimerRef.current = null;
    }
  }, []);

  const startSession = useCallback(() => {
    setSessionActive(true);
  }, []);

  const stopSession = useCallback(() => {
    setSessionActive(false);
    clearIdleTimeout();
  }, [clearIdleTimeout]);

  const resetIdleTimeout = useCallback(() => {
    clearIdleTimeout();
    if (!sessionActive || isRecording) return;
    idleTimerRef.current = setTimeout(() => {
      stopSession();
    }, IDLE_TIMEOUT_MS);
  }, [clearIdleTimeout, sessionActive, isRecording, stopSession]);

  // Start the idle timeout when a continuous session is active but not
  // currently recording; clear it while the user is holding the microphone.
  useEffect(() => {
    if (sessionActive && !isRecording) {
      resetIdleTimeout();
    } else {
      clearIdleTimeout();
    }
    return clearIdleTimeout;
  }, [sessionActive, isRecording, resetIdleTimeout, clearIdleTimeout]);

  const value = useMemo<VoiceContextValue>(
    () => ({
      isRecording,
      setIsRecording: setIsRecordingStable,
      sessionActive,
      startSession,
      stopSession,
      resetIdleTimeout,
      pendingSessionId,
      setPendingSessionId,
    }),
    [
      isRecording,
      setIsRecordingStable,
      sessionActive,
      startSession,
      stopSession,
      resetIdleTimeout,
      pendingSessionId,
      setPendingSessionId,
    ],
  );

  return (
    <VoiceContext.Provider value={value}>{children}</VoiceContext.Provider>
  );
}

export function useVoice(): VoiceContextValue {
  return useContext(VoiceContext);
}
