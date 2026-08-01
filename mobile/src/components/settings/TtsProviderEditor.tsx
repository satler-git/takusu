import { useEffect, useState, useMemo } from 'react';
import {
  ActivityIndicator,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors, type ColorSet } from '@/src/theme';
import {
  TTS_PROVIDER_LABELS,
  type TtsProvider,
  type TtsProviderSettings,
} from '@/src/api/settingsStore';
import { showError } from '@/src/api/errors';
import TakusuAudioModule, {
  type TtsVoiceInfo,
} from '@/modules/takusu-server/src/TakusuAudioModule';

const TTS_PROVIDER_TYPES: TtsProvider[] = ['cartesia', 'fish', 'android'];

interface Props {
  provider: TtsProviderSettings;
  apiKey: string;
  onChangeProvider: (provider: TtsProviderSettings) => void;
  onChangeApiKey: (apiKey: string) => void;
  onSave: (provider: TtsProviderSettings) => void;
  onCancel: () => void;
  onDelete?: () => void;
  saving?: boolean;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
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
    dropdown: {
      minHeight: 44,
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
    },
    dropdownList: {
      marginTop: 4,
      borderWidth: 1,
      borderRadius: 8,
      overflow: 'hidden',
    },
    dropdownItem: {
      minHeight: 44,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      paddingHorizontal: 12,
    },
    row: { flexDirection: 'row', gap: 8 },
    language: { flex: 1 },
    sampleRate: { flex: 1.5 },
    speed: { flex: 1 },
    actions: { flexDirection: 'row', gap: 8, marginTop: 4 },
    save: {
      flex: 1,
      minHeight: 44,
      borderRadius: 8,
      backgroundColor: colors.brand,
      alignItems: 'center',
      justifyContent: 'center',
    },
    saveText: { fontWeight: '700' },
    cancel: {
      minHeight: 44,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.disabled,
      paddingHorizontal: 16,
      alignItems: 'center',
      justifyContent: 'center',
    },
    remove: {
      minHeight: 44,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.destructive,
      paddingHorizontal: 16,
      alignItems: 'center',
      justifyContent: 'center',
    },
    removeText: { color: colors.destructive },
  });

export function TtsProviderEditor({
  provider,
  apiKey,
  onChangeProvider,
  onChangeApiKey,
  onSave,
  onCancel,
  onDelete,
  saving,
}: Props) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const [isExpanded, setIsExpanded] = useState(false);
  const [sampleRate, setSampleRate] = useState(String(provider.sampleRate));
  const [speed, setSpeed] = useState(
    provider.speed !== undefined ? String(provider.speed) : '',
  );
  const [model, setModel] = useState(provider.model ?? '');
  const [voices, setVoices] = useState<TtsVoiceInfo[]>([]);
  const [voicesLoading, setVoicesLoading] = useState(false);
  const [voicesExpanded, setVoicesExpanded] = useState(false);

  const isAndroid = provider.provider === 'android';

  useEffect(() => {
    if (!isAndroid) {
      setVoices([]);
      return;
    }
    let cancelled = false;
    setVoicesLoading(true);
    TakusuAudioModule.getAvailableVoices()
      .then((result) => {
        if (!cancelled) setVoices(result);
      })
      .catch((error: unknown) => {
        console.error('Failed to load TTS voices:', error);
      })
      .finally(() => {
        if (!cancelled) setVoicesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isAndroid]);

  function updateSampleRate(text: string) {
    setSampleRate(text);
    const parsed = parseInt(text, 10);
    if (Number.isFinite(parsed) && parsed > 0) {
      onChangeProvider({ ...provider, sampleRate: parsed });
    }
  }

  function updateSpeed(text: string) {
    setSpeed(text);
    if (text.trim() === '') {
      onChangeProvider({ ...provider, speed: undefined });
      return;
    }
    const parsed = parseFloat(text);
    if (Number.isFinite(parsed) && parsed > 0) {
      onChangeProvider({ ...provider, speed: parsed });
    }
  }

  function updateModel(text: string) {
    setModel(text);
    onChangeProvider({
      ...provider,
      model: text.trim() === '' ? undefined : text.trim(),
    });
  }

  function selectProvider(type: TtsProvider) {
    setIsExpanded(false);
    onChangeApiKey('');
    const nextModel = type === 'fish' ? 's2.1-pro-free' : '';
    setModel(nextModel);
    onChangeProvider({
      ...provider,
      provider: type,
      name: TTS_PROVIDER_LABELS[type],
      voiceId: '',
      model: nextModel || undefined,
    });
  }

  function selectVoice(voiceId: string) {
    setVoicesExpanded(false);
    onChangeProvider({ ...provider, voiceId });
  }

  function handleSave() {
    if (provider.provider === 'cartesia' && !provider.voiceId.trim()) {
      void showError('Voice IDを入力してください', '入力不足');
      return;
    }
    if (provider.provider === 'fish' && !model.trim()) {
      void showError('Modelを入力してください', '入力不足');
      return;
    }

    let nextProvider = provider;

    if (!isAndroid) {
      const parsedSampleRate = parseInt(sampleRate, 10);
      if (
        sampleRate.trim() === '' ||
        !Number.isFinite(parsedSampleRate) ||
        parsedSampleRate <= 0
      ) {
        void showError(
          'サンプルレートは正の整数を入力してください',
          '入力不足',
        );
        return;
      }
      nextProvider = { ...nextProvider, sampleRate: parsedSampleRate };
    }

    if (speed.trim() !== '') {
      const parsedSpeed = parseFloat(speed);
      if (!Number.isFinite(parsedSpeed) || parsedSpeed <= 0) {
        void showError('速度は正の数値を入力してください', '入力不足');
        return;
      }
      nextProvider = { ...nextProvider, speed: parsedSpeed };
    } else {
      nextProvider = { ...nextProvider, speed: undefined };
    }

    nextProvider = {
      ...nextProvider,
      model: model.trim() === '' ? undefined : model.trim(),
    };

    onSave(nextProvider);
  }

  return (
    <View style={[styles.editor, { borderColor: colors.separator }]}>
      <TextInput
        style={[
          styles.input,
          { color: colors.black, borderColor: colors.separator },
        ]}
        value={provider.name}
        onChangeText={(name) => onChangeProvider({ ...provider, name })}
        placeholder="表示名"
      />

      <View>
        <Pressable
          onPress={() => setIsExpanded((prev) => !prev)}
          style={[styles.dropdown, { borderColor: colors.separator }]}
        >
          <Text style={{ color: colors.black }}>
            {TTS_PROVIDER_LABELS[provider.provider]}
          </Text>
          <Ionicons
            name={isExpanded ? 'chevron-up' : 'chevron-down'}
            size={16}
            color={colors.gray}
          />
        </Pressable>

        {isExpanded && (
          <View
            style={[
              styles.dropdownList,
              {
                backgroundColor: colors.surface,
                borderColor: colors.separator,
              },
            ]}
          >
            {TTS_PROVIDER_TYPES.map((type) => (
              <Pressable
                key={type}
                onPress={() => selectProvider(type)}
                style={[
                  styles.dropdownItem,
                  provider.provider === type && {
                    backgroundColor: colors.surfaceTint,
                  },
                ]}
              >
                <Ionicons
                  name={
                    provider.provider === type
                      ? 'checkmark-circle'
                      : 'ellipse-outline'
                  }
                  size={20}
                  color={
                    provider.provider === type ? colors.brand : colors.black
                  }
                />
                <Text style={{ color: colors.black }}>
                  {TTS_PROVIDER_LABELS[type]}
                </Text>
              </Pressable>
            ))}
          </View>
        )}
      </View>

      {isAndroid ? (
        <View>
          <Pressable
            onPress={() => setVoicesExpanded((prev) => !prev)}
            style={[styles.dropdown, { borderColor: colors.separator }]}
            disabled={voicesLoading}
          >
            {voicesLoading ? (
              <ActivityIndicator size="small" color={colors.gray} />
            ) : (
              <Text style={{ color: colors.black }}>
                {provider.voiceId.trim() === ''
                  ? '自動（最初の声）'
                  : provider.voiceId}
              </Text>
            )}
            <Ionicons
              name={voicesExpanded ? 'chevron-up' : 'chevron-down'}
              size={16}
              color={colors.gray}
            />
          </Pressable>

          {voicesExpanded && (
            <View
              style={[
                styles.dropdownList,
                {
                  backgroundColor: colors.surface,
                  borderColor: colors.separator,
                },
              ]}
            >
              <Pressable
                onPress={() => selectVoice('')}
                style={[
                  styles.dropdownItem,
                  provider.voiceId.trim() === '' && {
                    backgroundColor: colors.surfaceTint,
                  },
                ]}
              >
                <Ionicons
                  name={
                    provider.voiceId.trim() === ''
                      ? 'checkmark-circle'
                      : 'ellipse-outline'
                  }
                  size={20}
                  color={
                    provider.voiceId.trim() === '' ? colors.brand : colors.black
                  }
                />
                <Text style={{ color: colors.black }}>自動（最初の声）</Text>
              </Pressable>
              {voices.map((voice) => (
                <Pressable
                  key={`${voice.name}-${voice.locale}`}
                  onPress={() => selectVoice(voice.name)}
                  style={[
                    styles.dropdownItem,
                    provider.voiceId === voice.name && {
                      backgroundColor: colors.surfaceTint,
                    },
                  ]}
                >
                  <Ionicons
                    name={
                      provider.voiceId === voice.name
                        ? 'checkmark-circle'
                        : 'ellipse-outline'
                    }
                    size={20}
                    color={
                      provider.voiceId === voice.name
                        ? colors.brand
                        : colors.black
                    }
                  />
                  <Text style={{ color: colors.black }}>
                    {voice.name}
                    {voice.locale ? ` (${voice.locale})` : ''}
                  </Text>
                </Pressable>
              ))}
            </View>
          )}
        </View>
      ) : (
        <TextInput
          style={[
            styles.input,
            { color: colors.black, borderColor: colors.separator },
          ]}
          value={provider.voiceId}
          onChangeText={(voiceId) => onChangeProvider({ ...provider, voiceId })}
          autoCapitalize="none"
          placeholder={
            provider.provider === 'fish'
              ? 'Voice / Reference ID (optional)'
              : 'Voice ID'
          }
        />
      )}

      {!isAndroid && (
        <>
          <TextInput
            style={[
              styles.input,
              { color: colors.black, borderColor: colors.separator },
            ]}
            value={apiKey}
            onChangeText={onChangeApiKey}
            autoCapitalize="none"
            secureTextEntry
            placeholder="API key"
          />
          <TextInput
            style={[
              styles.input,
              { color: colors.black, borderColor: colors.separator },
            ]}
            value={model}
            onChangeText={updateModel}
            autoCapitalize="none"
            placeholder={
              provider.provider === 'fish'
                ? 'Fish model (s2.1-pro-free)'
                : 'Model (optional)'
            }
          />
        </>
      )}

      <View style={styles.row}>
        <TextInput
          style={[
            styles.input,
            styles.language,
            { color: colors.black, borderColor: colors.separator },
          ]}
          value={provider.language}
          onChangeText={(language) =>
            onChangeProvider({ ...provider, language })
          }
          autoCapitalize="none"
          placeholder="言語"
        />
        {!isAndroid && (
          <TextInput
            style={[
              styles.input,
              styles.sampleRate,
              { color: colors.black, borderColor: colors.separator },
            ]}
            value={sampleRate}
            onChangeText={updateSampleRate}
            onBlur={() => {
              const parsed = parseInt(sampleRate, 10);
              if (
                sampleRate.trim() === '' ||
                !Number.isFinite(parsed) ||
                parsed <= 0
              ) {
                setSampleRate(String(provider.sampleRate));
              }
            }}
            keyboardType="numeric"
            placeholder="サンプルレート"
          />
        )}
        <TextInput
          style={[
            styles.input,
            styles.speed,
            { color: colors.black, borderColor: colors.separator },
          ]}
          value={speed}
          onChangeText={updateSpeed}
          onBlur={() => {
            if (speed.trim() === '') return;
            const parsed = parseFloat(speed);
            if (!Number.isFinite(parsed) || parsed <= 0) {
              setSpeed(
                provider.speed !== undefined ? String(provider.speed) : '',
              );
            }
          }}
          keyboardType="numeric"
          placeholder="速度"
        />
      </View>

      <View style={styles.actions}>
        <Pressable onPress={handleSave} style={styles.save} disabled={saving}>
          {saving ? (
            <ActivityIndicator color={colors.white} />
          ) : (
            <Text style={[styles.saveText, { color: colors.white }]}>保存</Text>
          )}
        </Pressable>
        <Pressable onPress={onCancel} style={styles.cancel}>
          <Text style={{ color: colors.black }}>キャンセル</Text>
        </Pressable>
        {onDelete && (
          <Pressable onPress={onDelete} style={styles.remove}>
            <Text style={styles.removeText}>削除</Text>
          </Pressable>
        )}
      </View>
    </View>
  );
}
