import { NativeModule, requireNativeModule } from 'expo';

export interface AudioOptions {
  provider: string;
  modelDir: string;
  model: string;
  asrModel: string;
  apiKey: string;
  voiceId: string;
  language: string;
  sampleRate: number;
  speed: number;
  mute?: boolean;
}

export interface SpeakerOptions {
  modelDir: string;
  voiceDir: string;
  threshold: number;
}

export interface SpeakerVerifyResult {
  score: number;
  accepted: boolean;
  speaker: string;
}

export interface TtsVoiceInfo {
  name: string;
  locale: string;
  quality: number;
  latency: number;
  requiresNetworkConnection: boolean;
  features: string[];
}

interface TakusuAudioModuleType extends NativeModule {
  configure(options: AudioOptions): Promise<boolean>;
  setMuted(muted: boolean): Promise<boolean>;
  startRecording(): Promise<boolean>;
  startRecordingWithEndpointing(): Promise<boolean>;
  stopAndTranscribe(): Promise<string>;
  stopAndGetPcm(): Promise<number[]>;
  synthesizeAndPlay(text: string): Promise<boolean>;
  synthesizeToFile(text: string): Promise<string>;
  playFile(path: string): Promise<boolean>;
  deleteFile(path: string): Promise<boolean>;
  stopPlayback(): boolean;
  clearTtsStop(): boolean;
  getAvailableVoices(): Promise<TtsVoiceInfo[]>;
  configureSpeaker(options: SpeakerOptions): Promise<boolean>;
  startSpeakerRecording(): Promise<boolean>;
  stopAndEnrollSpeaker(name: string): Promise<boolean>;
  stopAndVerifySpeaker(name: string): Promise<SpeakerVerifyResult>;
  deleteSpeaker(name: string): Promise<boolean>;
  listSpeakers(): Promise<string[]>;
}

const TakusuAudioModule =
  requireNativeModule<TakusuAudioModuleType>('TakusuAudio');

export default TakusuAudioModule;
