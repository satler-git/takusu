import { useEffect, useState, useMemo } from 'react';
import {
  ActivityIndicator,
  Alert,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors, type ColorSet } from '@/src/theme';
import { useServer } from '@/src/api/ServerProvider';
import { useTopToast } from '@/src/components/TopToast';
import { haptic } from '@/src/components/haptics';
import { showError } from '@/src/api/errors';
import TakusuServerModule from '../../modules/takusu-server/src/TakusuServerModule';
import TakusuAudioModule from '../../modules/takusu-server/src/TakusuAudioModule';
import {
  AGENT_SESSION_HISTORY_DEFAULT,
  AGENT_SESSION_HISTORY_MAX,
  AGENT_SESSION_HISTORY_MIN,
  deleteAgentApiKey,
  loadAgentApiKey,
  loadSettings,
  newId,
  saveAgentApiKey,
  saveAgentProviders,
  saveAgentSessionHistoryCount,
  type LlmModelSettings,
  type LlmProvider,
  type TtsProviderSettings,
} from '@/src/api/settingsStore';
import {
  DEFAULT_ASR_MODEL,
  loadAsrModel,
  saveAsrModel,
} from '@/src/utils/voice';
import { LlmModelEditor } from '@/src/components/settings/LlmModelEditor';
import { TtsProviderEditor } from '@/src/components/settings/TtsProviderEditor';

const ASR_MODELS = [
  'sherpa-sense-voice-int8',
  'sherpa-parakeet-ctc-ja-0.6b',
  'sherpa-nemotron-ja-0.6b',
] as const;

type AsrModelId = (typeof ASR_MODELS)[number];

const SPEAKER_MODEL_ID = 'sherpa-speaker-campplus-zh-en';

const MODEL_SIZES: Record<string, string> = {
  hush: '約8 MB',
  'sherpa-sense-voice-int8': '約160 MB',
  'sherpa-parakeet-ctc-ja-0.6b': '約470 MB',
  'sherpa-nemotron-ja-0.6b': '約470 MB',
  [SPEAKER_MODEL_ID]: '約27 MB',
};

const MODEL_NAMES: Record<string, string> = {
  hush: 'Hushノイズ除去',
  'sherpa-sense-voice-int8': 'SenseVoice音声認識',
  'sherpa-parakeet-ctc-ja-0.6b': 'Parakeet 日本語 CTC',
  'sherpa-nemotron-ja-0.6b': 'Nemotron ストリーミング',
  [SPEAKER_MODEL_ID]: 'CAM++ 話者認証',
};

function newLlmProvider(): LlmProvider {
  return {
    id: newId('llm'),
    name: 'Custom',
    baseUrl: '',
  };
}

function newLlmModel(providerId: string): LlmModelSettings {
  return {
    id: newId('llm-model'),
    name: 'Custom',
    providerId,
    selectedModel: '',
    cachedModels: [],
    permissions: {},
  };
}

function newTtsProvider(): TtsProviderSettings {
  return {
    id: newId('tts'),
    name: 'Cartesia',
    provider: 'cartesia',
    voiceId: '',
    language: 'ja',
    sampleRate: 44100,
  };
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    loading: { flex: 1, alignItems: 'center', justifyContent: 'center' },
    content: { padding: 16, gap: 10 },
    heading: { fontSize: 18, fontWeight: '700', marginTop: 12 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      borderWidth: 1,
      borderRadius: 8,
      gap: 12,
    },
    rowMain: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      padding: 10,
      gap: 8,
    },
    rowMainPressed: { backgroundColor: colors.pressed },
    rowText: { flex: 1 },
    editButton: {
      paddingVertical: 6,
      paddingHorizontal: 12,
      marginEnd: 10,
      borderWidth: 1,
      borderRadius: 8,
    },
    addButton: {
      minHeight: 44,
      borderWidth: 1,
      borderRadius: 8,
      alignItems: 'center',
      justifyContent: 'center',
    },
    editor: {
      padding: 12,
      borderWidth: 1,
      borderRadius: 12,
      gap: 10,
      marginTop: 8,
    },
    input: {
      minHeight: 44,
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
    },
    secondary: {
      minHeight: 44,
      borderWidth: 1,
      borderColor: colors.brand,
      borderRadius: 8,
      alignItems: 'center',
      justifyContent: 'center',
    },
    actions: { flexDirection: 'row', gap: 12, marginTop: 4 },
    save: {
      flex: 1,
      minHeight: 44,
      borderRadius: 8,
      backgroundColor: colors.brand,
      alignItems: 'center',
      justifyContent: 'center',
    },
    saveText: { color: colors.onBrand, fontWeight: '700' },
    cancel: {
      minHeight: 44,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.disabled,
      paddingHorizontal: 16,
      alignItems: 'center',
      justifyContent: 'center',
    },
    remove: { alignItems: 'center', padding: 12, marginTop: 8 },
    removeText: { color: colors.destructive },
    countInput: {
      minWidth: 48,
      minHeight: 36,
      paddingHorizontal: 8,
      borderWidth: 1,
      borderRadius: 8,
      textAlign: 'center',
    },
    voiceModelList: {
      borderWidth: 1,
      borderRadius: 12,
      overflow: 'hidden',
    },
    voiceModelRow: {
      flexDirection: 'row',
      alignItems: 'center',
      padding: 10,
      gap: 10,
      borderBottomWidth: 1,
    },
    voiceModelRowLast: { borderBottomWidth: 0 },
    voiceModelRowPressed: { backgroundColor: colors.pressed },
    voiceModelRowContent: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    voiceModelText: { flex: 1 },
    voiceModelName: { fontWeight: '600', fontSize: 15 },
    voiceModelMeta: { fontSize: 12, color: colors.gray },
    voiceModelStatus: { fontSize: 12, color: colors.gray, flexShrink: 0 },
    voiceModelAction: {
      minHeight: 44,
      borderRadius: 8,
      backgroundColor: colors.brand,
      alignItems: 'center',
      justifyContent: 'center',
    },
    voiceModelActionPressed: { opacity: 0.85 },
    voiceModelActionDisabled: { opacity: 0.4 },
    voiceModelActionText: { color: colors.onBrand, fontWeight: '700' },
  });

export function AgentSettingsView() {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const { client, pushAgentConfig } = useServer();
  const { showTopToast } = useTopToast();

  const [llmProviders, setLlmProviders] = useState<LlmProvider[]>([]);
  const [llmModels, setLlmModels] = useState<LlmModelSettings[]>([]);
  const [activeLlmModel, setActiveLlmModel] = useState<string | null>(null);
  const [ttsProviders, setTtsProviders] = useState<TtsProviderSettings[]>([]);
  const [activeTts, setActiveTts] = useState<string | null>(null);

  const [sessionHistoryCount, setSessionHistoryCount] = useState(
    AGENT_SESSION_HISTORY_DEFAULT,
  );

  const [editingLlmProvider, setEditingLlmProvider] =
    useState<LlmProvider | null>(null);
  const [editingLlmProviderKey, setEditingLlmProviderKey] = useState('');
  const [editingLlmModel, setEditingLlmModel] =
    useState<LlmModelSettings | null>(null);
  const [editingLlmModelKey, setEditingLlmModelKey] = useState('');
  const [editingTts, setEditingTts] = useState<TtsProviderSettings | null>(
    null,
  );
  const [editingTtsKey, setEditingTtsKey] = useState('');

  const [saving, setSaving] = useState(false);
  const [loading, setLoading] = useState(true);
  const [cachedModels, setCachedModels] = useState<Record<string, boolean>>({});
  const [downloadingModels, setDownloadingModels] = useState<
    Record<string, boolean>
  >({});
  const [asrModel, setAsrModel] = useState<AsrModelId>(DEFAULT_ASR_MODEL);
  const [savedAsrModel, setSavedAsrModel] =
    useState<AsrModelId>(DEFAULT_ASR_MODEL);

  const [speakerName, setSpeakerName] = useState('default');
  const [isSpeakerRecording, setIsSpeakerRecording] = useState(false);
  const [enrolledSpeakers, setEnrolledSpeakers] = useState<string[]>([]);
  const [lastVerifyResult, setLastVerifyResult] = useState<{
    score: number;
    accepted: boolean;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function checkCachedModels() {
      const next: Record<string, boolean> = {};
      for (const id of Object.keys(MODEL_SIZES)) {
        try {
          next[id] = await TakusuServerModule.isModelCached(id);
        } catch (e) {
          next[id] = false;
          console.error('isModelCached failed:', e);
        }
      }
      if (!cancelled) {
        setCachedModels(next);
      }
    }
    checkCachedModels();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const pending = Object.keys(downloadingModels).filter(
      (id) => downloadingModels[id],
    );
    if (pending.length === 0) {
      return;
    }
    const interval = setInterval(async () => {
      for (const id of pending) {
        try {
          const cached = await TakusuServerModule.isModelCached(id);
          if (cached) {
            setCachedModels((prev) => ({ ...prev, [id]: true }));
            setDownloadingModels((prev) => ({ ...prev, [id]: false }));
            if (asrModel === id) {
              try {
                await persistAsrModel(id as AsrModelId);
              } catch (e) {
                void showError(e, '保存失敗');
              }
            }
          }
        } catch (e) {
          console.error('isModelCached polling failed:', e);
        }
      }
    }, 1000);
    return () => clearInterval(interval);
  }, [downloadingModels, asrModel]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([loadSettings(), loadAsrModel()])
      .then(([settings, loadedAsrModel]) => {
        if (cancelled) return;
        setLlmProviders(settings.llmProviders);
        setLlmModels(settings.llmModels);
        setActiveLlmModel(settings.activeLlmModel || null);
        setTtsProviders(settings.ttsProviders);
        setActiveTts(settings.activeTtsProvider || null);
        setSessionHistoryCount(settings.agentSessionHistoryCount);
        setAsrModel(loadedAsrModel as AsrModelId);
        setSavedAsrModel(loadedAsrModel as AsrModelId);
      })
      .catch((e) => {
        void showError(e, '読み込み失敗');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const editingLlmProviderId = editingLlmProvider?.id;
  useEffect(() => {
    let cancelled = false;
    if (!editingLlmProviderId) {
      setEditingLlmProviderKey('');
      return;
    }
    loadAgentApiKey('llm', editingLlmProviderId).then((key) => {
      if (!cancelled) setEditingLlmProviderKey(key);
    });
    return () => {
      cancelled = true;
    };
  }, [editingLlmProviderId]);

  const editingLlmModelProviderId = editingLlmModel?.providerId;
  useEffect(() => {
    let cancelled = false;
    if (!editingLlmModelProviderId) {
      setEditingLlmModelKey('');
      return;
    }
    loadAgentApiKey('llm', editingLlmModelProviderId).then((key) => {
      if (!cancelled) setEditingLlmModelKey(key);
    });
    return () => {
      cancelled = true;
    };
  }, [editingLlmModelProviderId]);

  const editingTtsId = editingTts?.id;
  useEffect(() => {
    let cancelled = false;
    if (!editingTtsId) {
      setEditingTtsKey('');
      return;
    }
    loadAgentApiKey('tts', editingTtsId).then((key) => {
      if (!cancelled) setEditingTtsKey(key);
    });
    return () => {
      cancelled = true;
    };
  }, [editingTtsId]);

  async function setActiveLlmModelAndSave(id: string | null) {
    try {
      await saveAgentProviders(
        llmProviders,
        llmModels,
        id,
        ttsProviders,
        activeTts,
      );
      setActiveLlmModel(id);
      await pushAgentConfig();
    } catch (e) {
      void showError(e, '保存失敗');
    }
  }

  async function setActiveTtsAndSave(id: string | null) {
    try {
      await saveAgentProviders(
        llmProviders,
        llmModels,
        activeLlmModel,
        ttsProviders,
        id,
      );
      setActiveTts(id);
      await pushAgentConfig();
    } catch (e) {
      void showError(e, '保存失敗');
    }
  }

  async function saveLlmProvider(provider: LlmProvider, key: string) {
    setSaving(true);
    try {
      const existing = llmProviders.find((p) => p.id === provider.id);
      const updatedProviders = existing
        ? llmProviders.map((p) => (p.id === provider.id ? provider : p))
        : [...llmProviders, provider];
      await saveAgentApiKey('llm', provider.id, key);
      await saveAgentProviders(
        updatedProviders,
        llmModels,
        activeLlmModel,
        ttsProviders,
        activeTts,
      );
      setLlmProviders(updatedProviders);
      setEditingLlmProvider(null);
      setEditingLlmProviderKey('');
      await pushAgentConfig();
      haptic.success();
    } catch (e) {
      void showError(e, '保存失敗');
    } finally {
      setSaving(false);
    }
  }

  async function saveLlmModel(model: LlmModelSettings, key: string) {
    setSaving(true);
    try {
      const provider = llmProviders.find((p) => p.id === model.providerId);
      if (!provider) {
        void showError('選択されたProviderが見つかりません');
        return;
      }
      const existing = llmModels.find((m) => m.id === model.id);
      const updatedModels = existing
        ? llmModels.map((m) => (m.id === model.id ? model : m))
        : [...llmModels, model];
      const newActive = activeLlmModel ?? model.id;
      await saveAgentApiKey('llm', provider.id, key);
      await saveAgentProviders(
        llmProviders,
        updatedModels,
        newActive,
        ttsProviders,
        activeTts,
      );
      setLlmModels(updatedModels);
      setActiveLlmModel(newActive);
      setEditingLlmModel(null);
      setEditingLlmModelKey('');
      await pushAgentConfig();
      haptic.success();
    } catch (e) {
      void showError(e, '保存失敗');
    } finally {
      setSaving(false);
    }
  }

  async function saveTts(provider: TtsProviderSettings, key: string) {
    setSaving(true);
    try {
      const existing = ttsProviders.find((p) => p.id === provider.id);
      const updated = existing
        ? ttsProviders.map((p) => (p.id === provider.id ? provider : p))
        : [...ttsProviders, provider];
      const newActive = activeTts ?? provider.id;
      await saveAgentApiKey('tts', provider.id, key);
      await saveAgentProviders(
        llmProviders,
        llmModels,
        activeLlmModel,
        updated,
        newActive,
      );
      setTtsProviders(updated);
      setActiveTts(newActive);
      setEditingTts(null);
      setEditingTtsKey('');
      await pushAgentConfig();
      haptic.success();
    } catch (e) {
      void showError(e, '保存失敗');
    } finally {
      setSaving(false);
    }
  }

  function deleteLlmProvider(id: string) {
    Alert.alert('削除', 'このLLM Providerを削除しますか？', [
      { text: 'キャンセル', style: 'cancel' },
      {
        text: '削除',
        style: 'destructive',
        onPress: async () => {
          setSaving(true);
          try {
            const modelsUsingProvider = llmModels.filter(
              (m) => m.providerId === id,
            );
            if (modelsUsingProvider.length > 0) {
              showTopToast(
                'このProviderを使用しているモデルがあるため削除できません',
              );
              return;
            }
            const updatedProviders = llmProviders.filter((p) => p.id !== id);
            await deleteAgentApiKey('llm', id);
            await saveAgentProviders(
              updatedProviders,
              llmModels,
              activeLlmModel,
              ttsProviders,
              activeTts,
            );
            setLlmProviders(updatedProviders);
            if (editingLlmProvider?.id === id) setEditingLlmProvider(null);
            await pushAgentConfig();
          } catch (e) {
            void showError(e, '削除失敗');
          } finally {
            setSaving(false);
          }
        },
      },
    ]);
  }

  function deleteLlmModel(id: string) {
    Alert.alert('削除', 'このLLM Modelを削除しますか？', [
      { text: 'キャンセル', style: 'cancel' },
      {
        text: '削除',
        style: 'destructive',
        onPress: async () => {
          setSaving(true);
          try {
            const updatedModels = llmModels.filter((m) => m.id !== id);
            const newActive =
              activeLlmModel === id
                ? (updatedModels[0]?.id ?? null)
                : activeLlmModel;
            await saveAgentProviders(
              llmProviders,
              updatedModels,
              newActive,
              ttsProviders,
              activeTts,
            );
            setLlmModels(updatedModels);
            if (newActive !== activeLlmModel) setActiveLlmModel(newActive);
            if (editingLlmModel?.id === id) setEditingLlmModel(null);
            await pushAgentConfig();
          } catch (e) {
            void showError(e, '削除失敗');
          } finally {
            setSaving(false);
          }
        },
      },
    ]);
  }

  function deleteTts(id: string) {
    Alert.alert('削除', 'このTTS Providerを削除しますか？', [
      { text: 'キャンセル', style: 'cancel' },
      {
        text: '削除',
        style: 'destructive',
        onPress: async () => {
          setSaving(true);
          try {
            const updated = ttsProviders.filter((p) => p.id !== id);
            const newActive =
              activeTts === id ? (updated[0]?.id ?? null) : activeTts;
            await deleteAgentApiKey('tts', id);
            await saveAgentProviders(
              llmProviders,
              llmModels,
              activeLlmModel,
              updated,
              newActive,
            );
            setTtsProviders(updated);
            if (newActive !== activeTts) setActiveTts(newActive);
            if (editingTts?.id === id) setEditingTts(null);
            await pushAgentConfig();
          } catch (e) {
            void showError(e, '削除失敗');
          } finally {
            setSaving(false);
          }
        },
      },
    ]);
  }

  function removeAll() {
    Alert.alert('削除', 'すべてのProvider設定を削除しますか？', [
      { text: 'キャンセル', style: 'cancel' },
      {
        text: '削除',
        style: 'destructive',
        onPress: async () => {
          setSaving(true);
          try {
            await Promise.all(
              llmProviders.map((p) => deleteAgentApiKey('llm', p.id)),
            );
            await Promise.all(
              ttsProviders.map((p) => deleteAgentApiKey('tts', p.id)),
            );
            await saveAgentProviders([], [], null, [], null);
            setLlmProviders([]);
            setLlmModels([]);
            setActiveLlmModel(null);
            setTtsProviders([]);
            setActiveTts(null);
            setEditingLlmProvider(null);
            setEditingLlmProviderKey('');
            setEditingLlmModel(null);
            setEditingLlmModelKey('');
            setEditingTts(null);
            setEditingTtsKey('');
            await pushAgentConfig();
            haptic.success();
          } catch (e) {
            void showError(e, '削除失敗');
          } finally {
            setSaving(false);
          }
        },
      },
    ]);
  }

  function startModelDownload(modelId: string) {
    try {
      TakusuServerModule.startModelDownload(modelId);
      setDownloadingModels((prev) => ({ ...prev, [modelId]: true }));
      setCachedModels((prev) => ({ ...prev, [modelId]: false }));
      showTopToast(
        'バックグラウンドで音声モデルを準備します。通知で進捗を確認できます',
      );
    } catch (e) {
      void showError(e, '開始失敗');
    }
  }

  async function persistAsrModel(model: AsrModelId) {
    await saveAsrModel(model);
    setSavedAsrModel(model);
  }

  async function handleAsrAction() {
    if (downloadingModels[asrModel]) {
      return;
    }
    if (cachedModels[asrModel]) {
      try {
        await persistAsrModel(asrModel);
        haptic.success();
      } catch (e) {
        void showError(e, '保存失敗');
      }
      return;
    }
    const size = MODEL_SIZES[asrModel];
    const message = size
      ? `${size}のデータをダウンロードします。よろしいですか？`
      : 'データをダウンロードします。よろしいですか？';
    Alert.alert('ダウンロード確認', message, [
      { text: 'いいえ', style: 'cancel' },
      { text: 'はい', onPress: () => startModelDownload(asrModel) },
    ]);
  }

  function promptModelDownload(modelId: string) {
    if (cachedModels[modelId]) {
      showTopToast('このモデルはすでに準備されています');
      return;
    }
    const size = MODEL_SIZES[modelId];
    const message = size
      ? `${size}のデータをダウンロードします。よろしいですか？`
      : 'データをダウンロードします。よろしいですか？';
    Alert.alert('ダウンロード確認', message, [
      { text: 'いいえ', style: 'cancel' },
      { text: 'はい', onPress: () => startModelDownload(modelId) },
    ]);
  }

  async function startSpeakerRecording() {
    try {
      await TakusuAudioModule.startSpeakerRecording();
      setIsSpeakerRecording(true);
      setLastVerifyResult(null);
    } catch (e) {
      void showError(e, '録音開始失敗');
    }
  }

  async function stopAndEnrollSpeaker() {
    setIsSpeakerRecording(false);
    try {
      await TakusuAudioModule.stopAndEnrollSpeaker(speakerName);
      haptic.success();
      showTopToast('声紋を登録しました');
      await refreshSpeakers();
    } catch (e) {
      void showError(e, '登録失敗');
    }
  }

  async function stopAndVerifySpeaker() {
    setIsSpeakerRecording(false);
    try {
      const result = await TakusuAudioModule.stopAndVerifySpeaker(speakerName);
      setLastVerifyResult({
        score: result.score,
        accepted: result.accepted,
      });
      if (result.accepted) {
        haptic.success();
        showTopToast(`一致: ${(result.score * 100).toFixed(1)}%`);
      } else {
        showTopToast(`不一致: ${(result.score * 100).toFixed(1)}%`);
      }
    } catch (e) {
      void showError(e, '照合失敗');
    }
  }

  async function deleteSpeaker(name: string) {
    try {
      await TakusuAudioModule.deleteSpeaker(name);
      showTopToast('声紋を削除しました');
      await refreshSpeakers();
    } catch (e) {
      void showError(e, '削除失敗');
    }
  }

  async function refreshSpeakers() {
    try {
      const list = await TakusuAudioModule.listSpeakers();
      setEnrolledSpeakers(list);
    } catch (e) {
      console.error('refreshSpeakers failed:', e);
    }
  }

  useEffect(() => {
    let cancelled = false;
    async function init() {
      if (!cachedModels[SPEAKER_MODEL_ID] || cancelled) {
        return;
      }
      try {
        await TakusuAudioModule.configureSpeaker({
          modelDir: '',
          voiceDir: '',
          threshold: 0.5,
        });
        if (cancelled) return;
        const list = await TakusuAudioModule.listSpeakers();
        if (!cancelled) {
          setEnrolledSpeakers(list);
        }
      } catch (e) {
        // Not an error on first load; the user may not have downloaded the model yet.
        console.error('configureSpeaker failed:', e);
      }
    }
    init();
    return () => {
      cancelled = true;
    };
  }, [cachedModels]);

  if (loading) {
    return (
      <View style={[styles.loading, { backgroundColor: colors.white }]}>
        <ActivityIndicator />
      </View>
    );
  }

  const editingLlmModelProvider = editingLlmModel
    ? llmProviders.find((p) => p.id === editingLlmModel.providerId)
    : undefined;

  const isAsrActionDisabled =
    asrModel === savedAsrModel &&
    (cachedModels[asrModel] || downloadingModels[asrModel]);

  return (
    <ScrollView contentContainerStyle={styles.content}>
      <Text style={[styles.heading, { color: colors.black }]}>
        LLM Provider
      </Text>
      {llmProviders.length === 0 && (
        <Text style={{ color: colors.gray }}>Providerを追加してください</Text>
      )}
      {llmProviders.map((provider) => (
        <View
          key={provider.id}
          style={[styles.row, { borderColor: colors.separator, padding: 10 }]}
        >
          <View style={styles.rowText}>
            <Text style={{ color: colors.black, fontWeight: '600' }}>
              {provider.name}
            </Text>
            <Text style={{ color: colors.gray, fontSize: 12 }}>
              {provider.baseUrl || '未設定'}
            </Text>
          </View>
          <Pressable
            onPress={() => setEditingLlmProvider({ ...provider })}
            style={[styles.editButton, { borderColor: colors.separator }]}
          >
            <Text style={{ color: colors.black }}>編集</Text>
          </Pressable>
        </View>
      ))}
      <Pressable
        onPress={() => setEditingLlmProvider(newLlmProvider())}
        style={[styles.addButton, { borderColor: colors.brand }]}
      >
        <Text style={{ color: colors.brand }}>+ LLM Providerを追加</Text>
      </Pressable>
      {editingLlmProvider && (
        <View style={[styles.editor, { borderColor: colors.separator }]}>
          <TextInput
            style={[
              styles.input,
              { color: colors.black, borderColor: colors.separator },
            ]}
            value={editingLlmProvider.name}
            onChangeText={(name) =>
              setEditingLlmProvider({ ...editingLlmProvider, name })
            }
            placeholder="表示名"
          />
          <TextInput
            style={[
              styles.input,
              { color: colors.black, borderColor: colors.separator },
            ]}
            value={editingLlmProvider.baseUrl}
            onChangeText={(baseUrl) =>
              setEditingLlmProvider({ ...editingLlmProvider, baseUrl })
            }
            autoCapitalize="none"
            placeholder="Base URL"
          />
          <TextInput
            style={[
              styles.input,
              { color: colors.black, borderColor: colors.separator },
            ]}
            value={editingLlmProviderKey}
            onChangeText={setEditingLlmProviderKey}
            autoCapitalize="none"
            secureTextEntry
            placeholder="API key"
          />
          <View style={styles.actions}>
            <Pressable
              onPress={() =>
                saveLlmProvider(editingLlmProvider, editingLlmProviderKey)
              }
              style={styles.save}
              disabled={saving}
            >
              {saving ? (
                <ActivityIndicator color={colors.onBrand} />
              ) : (
                <Text style={styles.saveText}>保存</Text>
              )}
            </Pressable>
            <Pressable
              onPress={() => setEditingLlmProvider(null)}
              style={styles.cancel}
            >
              <Text style={{ color: colors.black }}>キャンセル</Text>
            </Pressable>
            {llmProviders.some((p) => p.id === editingLlmProvider.id) && (
              <Pressable
                onPress={() => deleteLlmProvider(editingLlmProvider.id)}
                style={styles.remove}
              >
                <Text style={styles.removeText}>削除</Text>
              </Pressable>
            )}
          </View>
        </View>
      )}

      <Text style={[styles.heading, { color: colors.black }]}>LLM Model</Text>
      {llmModels.length === 0 && (
        <Text style={{ color: colors.gray }}>Modelを追加してください</Text>
      )}
      {llmModels.map((model) => (
        <View
          key={model.id}
          style={[styles.row, { borderColor: colors.separator }]}
        >
          <Pressable
            onPress={() => setActiveLlmModelAndSave(model.id)}
            disabled={saving}
            style={({ pressed }) => [
              styles.rowMain,
              pressed && styles.rowMainPressed,
            ]}
          >
            <Ionicons
              name={
                activeLlmModel === model.id
                  ? 'checkmark-circle'
                  : 'ellipse-outline'
              }
              size={22}
              color={activeLlmModel === model.id ? colors.brand : colors.black}
            />
            <View style={styles.rowText}>
              <Text style={{ color: colors.black, fontWeight: '600' }}>
                {model.name}
              </Text>
              <Text style={{ color: colors.gray, fontSize: 12 }}>
                {llmProviders.find((p) => p.id === model.providerId)?.name ??
                  '未設定'}
                {' · '}
                {model.selectedModel || '未設定'}
                {model.cost ? ` · ${model.cost}` : ''}
                {model.permissions &&
                Object.values(model.permissions).some(Boolean)
                  ? ` · ${Object.values(model.permissions).filter(Boolean).length} 権限`
                  : ''}
              </Text>
            </View>
          </Pressable>
          <Pressable
            onPress={() => setEditingLlmModel({ ...model })}
            style={[styles.editButton, { borderColor: colors.separator }]}
          >
            <Text style={{ color: colors.black }}>編集</Text>
          </Pressable>
        </View>
      ))}
      <Pressable
        onPress={() => {
          const providerId = llmProviders[0]?.id ?? '';
          if (!providerId) {
            showTopToast('先にLLM Providerを追加してください');
            return;
          }
          setEditingLlmModel(newLlmModel(providerId));
        }}
        style={[styles.addButton, { borderColor: colors.brand }]}
      >
        <Text style={{ color: colors.brand }}>+ LLM Modelを追加</Text>
      </Pressable>
      {editingLlmModel && editingLlmModelProvider && (
        <LlmModelEditor
          model={editingLlmModel}
          providers={llmProviders}
          provider={editingLlmModelProvider}
          apiKey={editingLlmModelKey}
          onChangeModel={(next) => {
            setEditingLlmModel(next);
          }}
          onSave={() => saveLlmModel(editingLlmModel, editingLlmModelKey)}
          onCancel={() => setEditingLlmModel(null)}
          onDelete={
            llmModels.some((m) => m.id === editingLlmModel.id)
              ? () => deleteLlmModel(editingLlmModel.id)
              : undefined
          }
          saving={saving}
        />
      )}

      <Text style={[styles.heading, { color: colors.black }]}>音声モデル</Text>

      <View
        style={[
          styles.voiceModelList,
          { borderColor: colors.separator, backgroundColor: colors.surface },
        ]}
      >
        <Pressable
          onPress={() => promptModelDownload('hush')}
          disabled={cachedModels['hush'] || downloadingModels['hush']}
          style={({ pressed }) => [
            styles.voiceModelRow,
            { borderBottomColor: colors.separator },
            pressed && styles.voiceModelRowPressed,
          ]}
        >
          <View style={styles.voiceModelRowContent}>
            <Ionicons
              name={
                cachedModels['hush']
                  ? 'checkmark-circle'
                  : 'cloud-download-outline'
              }
              size={22}
              color={cachedModels['hush'] ? colors.success : colors.gray}
            />
            <View style={styles.voiceModelText}>
              <Text style={[styles.voiceModelName, { color: colors.black }]}>
                {MODEL_NAMES['hush']}
              </Text>
              <Text style={styles.voiceModelMeta}>{MODEL_SIZES['hush']}</Text>
            </View>
          </View>
          <Text style={styles.voiceModelStatus}>
            {downloadingModels['hush']
              ? '準備中'
              : cachedModels['hush']
                ? '準備済み'
                : '未ダウンロード'}
          </Text>
        </Pressable>
        <Pressable
          onPress={() => promptModelDownload(SPEAKER_MODEL_ID)}
          disabled={
            cachedModels[SPEAKER_MODEL_ID] ||
            downloadingModels[SPEAKER_MODEL_ID]
          }
          style={({ pressed }) => [
            styles.voiceModelRow,
            { borderBottomColor: colors.separator },
            pressed && styles.voiceModelRowPressed,
            styles.voiceModelRowLast,
          ]}
        >
          <View style={styles.voiceModelRowContent}>
            <Ionicons
              name={
                cachedModels[SPEAKER_MODEL_ID]
                  ? 'checkmark-circle'
                  : 'cloud-download-outline'
              }
              size={22}
              color={
                cachedModels[SPEAKER_MODEL_ID] ? colors.success : colors.gray
              }
            />
            <View style={styles.voiceModelText}>
              <Text style={[styles.voiceModelName, { color: colors.black }]}>
                {MODEL_NAMES[SPEAKER_MODEL_ID]}
              </Text>
              <Text style={styles.voiceModelMeta}>
                {MODEL_SIZES[SPEAKER_MODEL_ID]}
              </Text>
            </View>
          </View>
          <Text style={styles.voiceModelStatus}>
            {downloadingModels[SPEAKER_MODEL_ID]
              ? '準備中'
              : cachedModels[SPEAKER_MODEL_ID]
                ? '準備済み'
                : '未ダウンロード'}
          </Text>
        </Pressable>
      </View>

      {cachedModels[SPEAKER_MODEL_ID] && (
        <>
          <Text style={[styles.heading, { color: colors.black }]}>
            話者認証
          </Text>
          <Text style={{ color: colors.gray, fontSize: 12 }}>
            同じ話者の声を 1 回録音して登録・照合できます
          </Text>

          <View
            style={[
              styles.editor,
              {
                borderColor: colors.separator,
                backgroundColor: colors.surface,
              },
            ]}
          >
            <Text style={{ color: colors.black, fontWeight: '600' }}>
              話者名
            </Text>
            <TextInput
              style={[
                styles.input,
                { color: colors.black, borderColor: colors.separator },
              ]}
              value={speakerName}
              onChangeText={setSpeakerName}
              placeholder="default"
              placeholderTextColor={colors.gray}
            />

            {isSpeakerRecording ? (
              <View style={styles.actions}>
                <Pressable
                  onPress={stopAndEnrollSpeaker}
                  style={[
                    styles.save,
                    { backgroundColor: colors.brand, flex: 1 },
                  ]}
                >
                  <Text style={styles.saveText}>停止して登録</Text>
                </Pressable>
                <Pressable
                  onPress={stopAndVerifySpeaker}
                  style={[
                    styles.save,
                    { backgroundColor: colors.brand, flex: 1 },
                  ]}
                >
                  <Text style={styles.saveText}>停止して照合</Text>
                </Pressable>
              </View>
            ) : (
              <Pressable
                onPress={startSpeakerRecording}
                style={[styles.secondary, { borderColor: colors.brand }]}
              >
                <Text style={{ color: colors.brand, fontWeight: '600' }}>
                  録音開始
                </Text>
              </Pressable>
            )}

            {lastVerifyResult != null && (
              <Text
                style={{
                  color: lastVerifyResult.accepted
                    ? colors.success
                    : colors.destructive,
                  fontWeight: '600',
                }}
              >
                {lastVerifyResult.accepted ? '一致' : '不一致'}:{' '}
                {(lastVerifyResult.score * 100).toFixed(1)}%
              </Text>
            )}
          </View>

          {enrolledSpeakers.length > 0 && (
            <View
              style={[
                styles.voiceModelList,
                {
                  borderColor: colors.separator,
                  backgroundColor: colors.surface,
                },
              ]}
            >
              {enrolledSpeakers.map((name, index) => {
                const isLast = index === enrolledSpeakers.length - 1;
                return (
                  <View
                    key={name}
                    style={[
                      styles.voiceModelRow,
                      { borderBottomColor: colors.separator },
                      isLast && styles.voiceModelRowLast,
                    ]}
                  >
                    <View style={styles.voiceModelRowContent}>
                      <Ionicons
                        name="person-outline"
                        size={22}
                        color={colors.black}
                      />
                      <Text
                        style={[styles.voiceModelName, { color: colors.black }]}
                      >
                        {name}
                      </Text>
                    </View>
                    <Pressable
                      onPress={() => deleteSpeaker(name)}
                      style={[
                        styles.editButton,
                        { borderColor: colors.destructive },
                      ]}
                    >
                      <Text style={{ color: colors.destructive }}>削除</Text>
                    </Pressable>
                  </View>
                );
              })}
            </View>
          )}
        </>
      )}

      <Text style={[styles.heading, { color: colors.black }]}>
        音声認識モデル
      </Text>
      <Text style={{ color: colors.gray, fontSize: 12 }}>
        一覧から 1 つ選んで、保存またはダウンロードしてください
      </Text>

      <View
        style={[
          styles.voiceModelList,
          { borderColor: colors.separator, backgroundColor: colors.surface },
        ]}
      >
        {ASR_MODELS.map((id, index) => {
          const isLast = index === ASR_MODELS.length - 1;
          const isCached = cachedModels[id] ?? false;
          const isDownloading = downloadingModels[id] ?? false;
          const status = isDownloading
            ? '準備中'
            : isCached
              ? '準備済み'
              : '未ダウンロード';
          return (
            <Pressable
              key={id}
              onPress={() => setAsrModel(id)}
              style={({ pressed }) => [
                styles.voiceModelRow,
                { borderBottomColor: colors.separator },
                pressed && styles.voiceModelRowPressed,
                isLast && styles.voiceModelRowLast,
              ]}
            >
              <View style={styles.voiceModelRowContent}>
                <Ionicons
                  name={
                    asrModel === id ? 'checkmark-circle' : 'ellipse-outline'
                  }
                  size={22}
                  color={asrModel === id ? colors.brand : colors.black}
                />
                <View style={styles.voiceModelText}>
                  <Text
                    style={[styles.voiceModelName, { color: colors.black }]}
                  >
                    {MODEL_NAMES[id]}
                  </Text>
                  <Text style={styles.voiceModelMeta}>{MODEL_SIZES[id]}</Text>
                </View>
              </View>
              <Text style={styles.voiceModelStatus}>{status}</Text>
            </Pressable>
          );
        })}
      </View>

      <Pressable
        onPress={handleAsrAction}
        disabled={isAsrActionDisabled}
        style={({ pressed }) => [
          styles.voiceModelAction,
          pressed && !isAsrActionDisabled && styles.voiceModelActionPressed,
          isAsrActionDisabled && styles.voiceModelActionDisabled,
        ]}
      >
        <Text style={styles.voiceModelActionText}>
          {downloadingModels[asrModel]
            ? '準備中…'
            : cachedModels[asrModel]
              ? asrModel === savedAsrModel
                ? '保存済み'
                : '保存'
              : 'ダウンロード'}
        </Text>
      </Pressable>

      <Text style={[styles.heading, { color: colors.black }]}>
        TTS Provider
      </Text>
      {ttsProviders.length === 0 && (
        <Text style={{ color: colors.gray }}>Providerを追加してください</Text>
      )}
      {ttsProviders.map((provider) => (
        <View
          key={provider.id}
          style={[styles.row, { borderColor: colors.separator }]}
        >
          <Pressable
            onPress={() => setActiveTtsAndSave(provider.id)}
            disabled={saving}
            style={({ pressed }) => [
              styles.rowMain,
              pressed && styles.rowMainPressed,
            ]}
          >
            <Ionicons
              name={
                activeTts === provider.id
                  ? 'checkmark-circle'
                  : 'ellipse-outline'
              }
              size={22}
              color={activeTts === provider.id ? colors.brand : colors.black}
            />
            <View style={styles.rowText}>
              <Text style={{ color: colors.black, fontWeight: '600' }}>
                {provider.name}
              </Text>
              <Text style={{ color: colors.gray, fontSize: 12 }}>
                {provider.provider} · {provider.voiceId || '未設定'}
              </Text>
            </View>
          </Pressable>
          <Pressable
            onPress={() => setEditingTts({ ...provider })}
            style={[styles.editButton, { borderColor: colors.separator }]}
          >
            <Text style={{ color: colors.black }}>編集</Text>
          </Pressable>
        </View>
      ))}
      <Pressable
        onPress={() => setEditingTts(newTtsProvider())}
        style={[styles.addButton, { borderColor: colors.brand }]}
      >
        <Text style={{ color: colors.brand }}>+ TTS Providerを追加</Text>
      </Pressable>
      {editingTts && (
        <TtsProviderEditor
          key={editingTts.id}
          provider={editingTts}
          apiKey={editingTtsKey}
          onChangeProvider={setEditingTts}
          onChangeApiKey={setEditingTtsKey}
          onSave={(provider) => saveTts(provider, editingTtsKey)}
          onCancel={() => setEditingTts(null)}
          onDelete={
            ttsProviders.some((p) => p.id === editingTts.id)
              ? () => deleteTts(editingTts.id)
              : undefined
          }
          saving={saving}
        />
      )}

      <Text style={[styles.heading, { color: colors.black }]}>
        セッション履歴
      </Text>
      <View
        style={[styles.row, { borderColor: colors.separator, padding: 10 }]}
      >
        <Text style={{ flex: 1, color: colors.black }}>
          保持するセッション数（{AGENT_SESSION_HISTORY_MIN}-
          {AGENT_SESSION_HISTORY_MAX}）
        </Text>
        <TextInput
          style={[
            styles.countInput,
            { color: colors.black, borderColor: colors.separator },
          ]}
          value={String(sessionHistoryCount)}
          onChangeText={(value) => {
            if (value === '') {
              setSessionHistoryCount(AGENT_SESSION_HISTORY_DEFAULT);
              return;
            }
            const parsed = Number(value);
            if (Number.isInteger(parsed)) {
              setSessionHistoryCount(
                Math.max(
                  AGENT_SESSION_HISTORY_MIN,
                  Math.min(AGENT_SESSION_HISTORY_MAX, parsed),
                ),
              );
            }
          }}
          onBlur={async () => {
            try {
              await saveAgentSessionHistoryCount(sessionHistoryCount);
            } catch (e) {
              void showError(e, '保存失敗');
            }
          }}
          keyboardType="number-pad"
          maxLength={1}
        />
      </View>

      <Pressable onPress={removeAll} style={styles.remove}>
        <Text style={styles.removeText}>Provider設定をすべて削除</Text>
      </Pressable>
      {!client && (
        <Text style={{ color: colors.gray }}>
          Planner serverに接続していません
        </Text>
      )}
    </ScrollView>
  );
}
