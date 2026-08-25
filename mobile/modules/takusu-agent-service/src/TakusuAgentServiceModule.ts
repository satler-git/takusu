import { NativeModule, requireOptionalNativeModule } from 'expo';

export interface AmbientStartOptions {
  workersUrl: string;
  rootToken: string;
  deviceId?: string;
  localUrl?: string;
  modelDir?: string;
  asrModel?: string;
  language?: string;
  wakeWordBackend?: 'sherpa_kws' | 'asr_text_match';
}

interface TakusuAgentServiceModuleType extends NativeModule {
  startAmbient(options: AmbientStartOptions): boolean;
  stopAmbient(): boolean;
  isRunning(): boolean;
  setAmbientEnabled(enabled: boolean): boolean;
  isAmbientEnabled(): boolean;
}

const TakusuAgentServiceModule =
  requireOptionalNativeModule<TakusuAgentServiceModuleType>(
    'TakusuAgentService',
  );

export default TakusuAgentServiceModule;
