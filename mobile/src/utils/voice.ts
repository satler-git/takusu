import { PermissionsAndroid } from 'react-native';
import {
  loadSettings,
  loadAgentApiKey,
  loadTtsMuted,
} from '@/src/api/settingsStore';
import TakusuAudioModule from '../../modules/takusu-server/src/TakusuAudioModule';

let configurePromise: Promise<void> | null = null;
let lastConfigKey = '';
let isRecording = false;
let onRecordingChange: ((recording: boolean) => void) | null = null;
let stopPromise: Promise<string> | null = null;

function hashString(input: string): number {
  let h = 0;
  for (let i = 0; i < input.length; i++) {
    h = (h << 5) - h + input.charCodeAt(i);
    h |= 0;
  }
  return h >>> 0;
}

function notifyRecordingChanged(recording: boolean) {
  onRecordingChange?.(recording);
}

export function setRecordingChangeListener(
  listener: ((recording: boolean) => void) | null,
): () => void {
  onRecordingChange = listener;
  return () => {
    if (onRecordingChange === listener) {
      onRecordingChange = null;
    }
  };
}

async function doConfigure(): Promise<void> {
  const settings = await loadSettings();
  const provider = settings.ttsProviders.find(
    (p) => p.id === settings.activeTtsProvider,
  );
  if (!provider) {
    throw new Error('TTS provider is not configured');
  }
  const [apiKey, muted] = await Promise.all([
    loadAgentApiKey('tts', provider.id),
    loadTtsMuted(),
  ]);
  // Use a hash of the API key so key-only changes trigger reconfiguration
  // without keeping the raw key in the cache key.
  const configKey = `${provider.id}:${provider.provider}:${provider.model ?? ''}:${provider.voiceId}:${provider.language}:${provider.sampleRate}:${provider.speed}:${hashString(apiKey)}:${muted ? '1' : '0'}`;
  if (configKey === lastConfigKey) return;
  await TakusuAudioModule.configure({
    provider: provider.provider,
    modelDir: '',
    model: provider.model ?? '',
    apiKey,
    voiceId: provider.voiceId,
    language: provider.language,
    sampleRate: provider.sampleRate,
    speed: provider.speed == null ? 1 : provider.speed,
    mute: muted,
  });
  lastConfigKey = configKey;
}

export async function ensureAudioConfigured(): Promise<void> {
  if (!configurePromise) {
    configurePromise = doConfigure().finally(() => {
      // Allow retry on next call if configuration failed.
      configurePromise = null;
    });
  }
  return configurePromise;
}

export function isRecordingActive(): boolean {
  return isRecording;
}

export async function startRecording(): Promise<void> {
  if (isRecording) {
    throw new Error('既に録音中です');
  }
  isRecording = true;
  notifyRecordingChanged(true);
  try {
    const permission = await PermissionsAndroid.request(
      PermissionsAndroid.PERMISSIONS.RECORD_AUDIO,
    );
    if (permission !== PermissionsAndroid.RESULTS.GRANTED) {
      isRecording = false;
      notifyRecordingChanged(false);
      throw new Error('マイク権限が許可されていません');
    }
    // Another stopAndTranscribe may have cancelled this start while permission was pending.
    if (!isRecording) {
      notifyRecordingChanged(false);
      throw new Error('録音がキャンセルされました');
    }
    TakusuAudioModule.stopPlayback();
    TakusuAudioModule.startRecording();
  } catch (e) {
    if (isRecording) {
      isRecording = false;
      notifyRecordingChanged(false);
    }
    throw e;
  }
}

export async function stopAndTranscribe(): Promise<string> {
  if (stopPromise) return stopPromise;
  if (!isRecording) return '';
  stopPromise = (async () => {
    try {
      await ensureAudioConfigured();
      const transcript = await TakusuAudioModule.stopAndTranscribe();
      return transcript.trim();
    } finally {
      isRecording = false;
      notifyRecordingChanged(false);
      stopPromise = null;
    }
  })();
  return stopPromise;
}

export interface VoiceResult {
  transcript: string;
  sendNow: boolean;
}

class VoiceBridge {
  private current: VoiceResult | null = null;
  private listeners = new Set<(result: VoiceResult | null) => void>();

  setResult(result: VoiceResult): void {
    this.current = result;
    this.listeners.forEach((l) => l(result));
  }

  consume(): VoiceResult | null {
    const r = this.current;
    this.current = null;
    return r;
  }

  subscribe(listener: (result: VoiceResult | null) => void): () => void {
    this.listeners.add(listener);
    if (this.current) listener(this.current);
    return () => this.listeners.delete(listener);
  }
}

export const voiceBridge = new VoiceBridge();
