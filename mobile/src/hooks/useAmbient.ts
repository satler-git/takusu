import { useCallback, useEffect, useRef, useState } from 'react';
import { PermissionsAndroid, Platform } from 'react-native';
import TakusuAgentServiceModule, {
  type AmbientStartOptions,
} from '@/modules/takusu-agent-service/src/TakusuAgentServiceModule';
import { getLocalServerPort } from '@/src/api/server';
import { loadAsrModel } from '@/src/utils/voice';

function isAmbientEnabled(): boolean {
  try {
    return TakusuAgentServiceModule?.isAmbientEnabled() ?? false;
  } catch {
    return false;
  }
}

function isAmbientRunning(): boolean {
  try {
    return TakusuAgentServiceModule?.isRunning() ?? false;
  } catch {
    return false;
  }
}

export type { AmbientStartOptions };

interface StartOptions {
  /** When false (auto-resume), never show a permission prompt; start only if
   *  the microphone permission is already granted. */
  requestPermissions?: boolean;
}

export function useAmbient() {
  const [enabled, setEnabled] = useState(false);
  const [running, setRunning] = useState(false);
  const [processing, setProcessing] = useState(false);
  const inProgressRef = useRef(false);

  useEffect(() => {
    let cancelled = false;

    const refreshState = () => {
      if (cancelled || inProgressRef.current) {
        return;
      }
      setEnabled(isAmbientEnabled());
      setRunning(isAmbientRunning());
    };

    refreshState();
    const interval = setInterval(refreshState, 2000);

    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  const buildOptions = useCallback(
    async (base: AmbientStartOptions): Promise<AmbientStartOptions> => {
      const asrModel = (await loadAsrModel()).trim();
      const localUrl = `http://127.0.0.1:${getLocalServerPort()}`;
      return {
        ...base,
        asrModel: asrModel || undefined,
        localUrl: base.localUrl || localUrl,
        deviceId: base.deviceId || 'mobile',
        language: base.language || 'ja',
        wakeWordBackend: base.wakeWordBackend || 'asr_text_match',
      };
    },
    [],
  );

  const start = useCallback(
    async (baseOptions: AmbientStartOptions, opts?: StartOptions) => {
      if (Platform.OS !== 'android' || !TakusuAgentServiceModule) {
        throw new Error('常時聞き取りは Android のみで利用できます');
      }

      if (inProgressRef.current) {
        return;
      }
      inProgressRef.current = true;
      setProcessing(true);

      try {
        const requestPermissions = opts?.requestPermissions ?? true;
        if (requestPermissions) {
          const audioPermission = await PermissionsAndroid.request(
            PermissionsAndroid.PERMISSIONS.RECORD_AUDIO,
          );
          if (audioPermission !== PermissionsAndroid.RESULTS.GRANTED) {
            throw new Error('マイク権限が許可されていません');
          }

          // READ_PHONE_STATE is used to pause recording during phone calls.
          // The service will skip call-state monitoring if the permission is
          // denied, so this request is best-effort.
          await PermissionsAndroid.request(
            PermissionsAndroid.PERMISSIONS.READ_PHONE_STATE,
          );

          // Android 13+ can show a notification permission prompt, but ambient
          // should still be able to start if it is denied. The foreground
          // notification may not be visible, which is acceptable.
          if (
            Platform.Version >= 33 &&
            PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS
          ) {
            await PermissionsAndroid.request(
              PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS,
            );
          }
        } else {
          // Auto-resume path from ServerProvider: never prompt. If the user
          // revoked the microphone permission, skip quietly; the manual toggle
          // in AgentSettingsView surfaces the error.
          const granted = await PermissionsAndroid.check(
            PermissionsAndroid.PERMISSIONS.RECORD_AUDIO,
          );
          if (!granted) {
            throw new Error('マイク権限が許可されていません');
          }
        }

        const options = await buildOptions(baseOptions);
        await TakusuAgentServiceModule.startAmbient(options);
        setRunning(true);
        setEnabled(true);
      } finally {
        inProgressRef.current = false;
        setProcessing(false);
      }
    },
    [buildOptions],
  );

  const stop = useCallback(async () => {
    if (Platform.OS !== 'android' || !TakusuAgentServiceModule) {
      return;
    }

    if (inProgressRef.current) {
      return;
    }
    inProgressRef.current = true;
    setProcessing(true);

    try {
      await TakusuAgentServiceModule.stopAmbient();
      setRunning(false);
      setEnabled(false);
    } finally {
      inProgressRef.current = false;
      setProcessing(false);
    }
  }, []);

  const toggle = useCallback(
    async (baseOptions: AmbientStartOptions) => {
      if (inProgressRef.current) {
        return;
      }
      // Key the toggle on the *running* state, not on the persisted "enabled"
      // flag: after the OS kills the service it shows "有効（停止中）", and a
      // tap there is a resume, not a disable.
      if (running) {
        await stop();
      } else {
        await start(baseOptions);
      }
    },
    [running, start, stop],
  );

  return { enabled, running, processing, start, stop, toggle };
}
