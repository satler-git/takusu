import { useEffect, useMemo, useState } from 'react';
import {
  ActivityIndicator,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  useWindowDimensions,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors, type ColorSet } from '@/src/theme';
import {
  type LlmModelSettings,
  type LlmProvider,
  type PermissionsMap,
} from '@/src/api/settingsStore';
import { showError } from '@/src/api/errors';
import { PermissionsEditor } from '@/src/components/PermissionsEditor';

interface Props {
  model: LlmModelSettings;
  providers: LlmProvider[];
  provider: LlmProvider;
  apiKey: string;
  onChangeModel: (model: LlmModelSettings) => void;
  onSave: () => void;
  onCancel: () => void;
  onDelete?: () => void;
  saving?: boolean;
}

export interface ModelPricing {
  prompt?: string | number;
  completion?: string | number;
}

interface ModelItem {
  id?: string;
  pricing?: ModelPricing;
}

interface ModelResponse {
  data?: ModelItem[];
}

const moneyFormatter = new Intl.NumberFormat('en-US', {
  style: 'currency',
  currency: 'USD',
  maximumFractionDigits: 9,
  minimumFractionDigits: 0,
});

function formatMoney(n: number): string {
  return moneyFormatter.format(n);
}

function parsePrice(value: string | number | undefined): number | undefined {
  if (value === undefined || value === null || value === '') return undefined;
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(n) || n < 0) return undefined;
  return n;
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
    readOnly: {
      fontSize: 13,
      paddingHorizontal: 4,
    },
    secondary: {
      minHeight: 44,
      borderWidth: 1,
      borderColor: colors.brand,
      borderRadius: 8,
      alignItems: 'center',
      justifyContent: 'center',
    },
    modelListHeader: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: 10,
      borderWidth: 1,
      borderRadius: 8,
    },
    modelListHeaderText: { fontSize: 15, fontWeight: '700' },
    modelListExpanded: { gap: 10 },
    modelListContent: { gap: 10 },
    modelListFooter: {
      minHeight: 44,
      flexDirection: 'row',
      borderWidth: 1,
      borderRadius: 8,
      alignItems: 'center',
      justifyContent: 'center',
      gap: 4,
    },
    modelRow: { padding: 10, borderWidth: 1, borderRadius: 8 },
    modelRowContent: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    modelName: { flex: 1 },
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
    sectionTitle: { fontSize: 15, fontWeight: '700', marginTop: 4 },
  });

export function formatCost(
  pricing: ModelPricing | undefined,
): string | undefined {
  const prompt = parsePrice(pricing?.prompt);
  const completion = parsePrice(pricing?.completion);
  if (prompt === undefined && completion === undefined) return undefined;
  if (prompt !== undefined && completion !== undefined) {
    return `in ${formatMoney(prompt * 1000000)}, out ${formatMoney(
      completion * 1000000,
    )} / 1M tokens`;
  }
  if (prompt !== undefined)
    return `${formatMoney(prompt * 1000000)} / 1M tokens`;
  if (completion !== undefined)
    return `${formatMoney(completion * 1000000)} / 1M tokens`;
}

export function LlmModelEditor({
  model,
  providers,
  provider,
  apiKey,
  onChangeModel,
  onSave,
  onCancel,
  onDelete,
  saving,
}: Props) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const { height: windowHeight } = useWindowDimensions();
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelFilter, setModelFilter] = useState('');
  const [modelCosts, setModelCosts] = useState<Record<string, string>>({});
  const [modelsExpanded, setModelsExpanded] = useState(true);
  const [providerDropdownOpen, setProviderDropdownOpen] = useState(false);
  const [modelListHeight, setModelListHeight] = useState(0);
  const showBottomFold =
    modelsExpanded && modelListHeight > (windowHeight * 3) / 5;

  useEffect(() => {
    setModelCosts({});
  }, [model.id, model.providerId]);

  useEffect(() => {
    const { selectedModel, cost } = model;
    if (selectedModel && cost) {
      setModelCosts((prev) => ({
        ...prev,
        [selectedModel]: cost,
      }));
    }
  }, [model.selectedModel, model.cost]);

  const filteredModels = useMemo(() => {
    const query = modelFilter.trim().toLowerCase();
    if (!query) return model.cachedModels;
    return model.cachedModels.filter((m) => m.toLowerCase().includes(query));
  }, [model.cachedModels, modelFilter]);

  async function fetchModels() {
    if (!provider.baseUrl.trim() || !apiKey.trim()) {
      void showError(
        'ProviderのBase URLとAPI keyを設定してください',
        '入力不足',
      );
      return;
    }
    setModelsLoading(true);
    try {
      const response = await fetch(
        `${provider.baseUrl.replace(/\/+$/, '')}/models`,
        {
          headers: { Authorization: `Bearer ${apiKey.trim()}` },
        },
      );
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const body = (await response.json()) as ModelResponse;
      const items = body.data ?? [];
      const models = [
        ...new Set(
          items
            .map((item) => item.id)
            .filter((id): id is string => Boolean(id)),
        ),
      ].sort();
      const costs: Record<string, string> = {};
      for (const item of items) {
        if (item.id) {
          const cost = formatCost(item.pricing);
          if (cost) {
            costs[item.id] = cost;
          }
        }
      }
      setModelCosts(costs);
      const nextSelected = model.selectedModel || models[0] || '';
      onChangeModel({
        ...model,
        cachedModels: models,
        selectedModel: nextSelected,
        modelsFetchedAt: new Date().toISOString(),
        cost: costs[nextSelected],
      });
      if (models.length === 0) {
        void showError('モデルIDを手入力してください', 'モデルなし');
      }
    } catch (e) {
      void showError(e, '取得失敗');
    } finally {
      setModelsLoading(false);
    }
  }

  function handleSave() {
    if (!model.selectedModel.trim()) {
      void showError('LLMモデルを選択または入力してください', '入力不足');
      return;
    }
    onSave();
  }

  return (
    <View style={[styles.editor, { borderColor: colors.separator }]}>
      <TextInput
        style={[
          styles.input,
          { color: colors.black, borderColor: colors.separator },
        ]}
        value={model.name}
        onChangeText={(name) => onChangeModel({ ...model, name })}
        placeholder="表示名"
      />
      <View>
        <Pressable
          onPress={() => setProviderDropdownOpen((v) => !v)}
          style={[styles.modelListHeader, { borderColor: colors.separator }]}
        >
          <Text style={[styles.modelListHeaderText, { color: colors.black }]}>
            Provider: {provider.name || '未設定'}
          </Text>
          <Ionicons
            name={providerDropdownOpen ? 'chevron-up' : 'chevron-down'}
            size={16}
            color={colors.gray}
          />
        </Pressable>
        {providerDropdownOpen && (
          <View style={styles.modelListContent}>
            {providers.map((p) => (
              <Pressable
                key={p.id}
                onPress={() => {
                  setProviderDropdownOpen(false);
                  if (p.id !== model.providerId) {
                    onChangeModel({
                      ...model,
                      providerId: p.id,
                      cachedModels: [],
                      modelsFetchedAt: undefined,
                      cost: undefined,
                    });
                  }
                }}
                style={[styles.modelRow, { borderColor: colors.separator }]}
              >
                <View style={styles.modelRowContent}>
                  <Ionicons
                    name={
                      model.providerId === p.id
                        ? 'checkmark-circle'
                        : 'ellipse-outline'
                    }
                    size={20}
                    color={
                      model.providerId === p.id ? colors.brand : colors.black
                    }
                  />
                  <Text style={{ color: colors.black }}>
                    {p.name || p.baseUrl || '未設定'}
                  </Text>
                </View>
              </Pressable>
            ))}
          </View>
        )}
      </View>
      <Text style={[styles.readOnly, { color: colors.gray }]} numberOfLines={1}>
        Base URL: {provider.baseUrl || '未設定'}
      </Text>
      <Text style={[styles.readOnly, { color: colors.gray }]}>
        API key: {apiKey ? '設定済み' : '未設定'}
      </Text>
      <Pressable
        onPress={fetchModels}
        style={styles.secondary}
        disabled={modelsLoading}
      >
        {modelsLoading ? (
          <ActivityIndicator />
        ) : (
          <Text style={{ color: colors.black }}>モデルを取得</Text>
        )}
      </Pressable>
      {model.cachedModels.length > 0 && (
        <Pressable
          onPress={() => setModelsExpanded((v) => !v)}
          style={[styles.modelListHeader, { borderColor: colors.separator }]}
        >
          <Text style={[styles.modelListHeaderText, { color: colors.black }]}>
            モデル一覧
          </Text>
          <Ionicons
            name={modelsExpanded ? 'chevron-down' : 'chevron-forward'}
            size={16}
            color={colors.gray}
          />
        </Pressable>
      )}
      {modelsExpanded && model.cachedModels.length > 0 && (
        <View
          testID="model-list-expanded"
          style={styles.modelListExpanded}
          onLayout={(e) => setModelListHeight(e.nativeEvent.layout.height)}
        >
          <View style={styles.modelListContent}>
            <TextInput
              style={[
                styles.input,
                { color: colors.black, borderColor: colors.separator },
              ]}
              value={modelFilter}
              onChangeText={setModelFilter}
              autoCapitalize="none"
              placeholder="モデルを検索"
            />
            {filteredModels.map((m) => (
              <Pressable
                key={m}
                onPress={() =>
                  onChangeModel({
                    ...model,
                    selectedModel: m,
                    cost: modelCosts[m],
                  })
                }
                style={[styles.modelRow, { borderColor: colors.separator }]}
              >
                <View style={styles.modelRowContent}>
                  <Ionicons
                    name={
                      model.selectedModel === m
                        ? 'checkmark-circle'
                        : 'ellipse-outline'
                    }
                    size={20}
                    color={
                      model.selectedModel === m ? colors.brand : colors.black
                    }
                  />
                  <Text
                    style={[styles.modelName, { color: colors.black }]}
                    numberOfLines={1}
                  >
                    {m}
                  </Text>
                  {modelCosts[m] && (
                    <Text style={{ color: colors.gray, fontSize: 12 }}>
                      {modelCosts[m]}
                    </Text>
                  )}
                </View>
              </Pressable>
            ))}
          </View>
          {showBottomFold && (
            <Pressable
              onPress={() => setModelsExpanded(false)}
              style={[
                styles.modelListFooter,
                { borderColor: colors.separator },
              ]}
            >
              <Ionicons name="chevron-up" size={16} color={colors.black} />
              <Text style={{ color: colors.black }}>畳む</Text>
            </Pressable>
          )}
        </View>
      )}
      <TextInput
        style={[
          styles.input,
          { color: colors.black, borderColor: colors.separator },
        ]}
        value={model.selectedModel}
        onChangeText={(selectedModel) =>
          onChangeModel({
            ...model,
            selectedModel,
            cost: modelCosts[selectedModel],
          })
        }
        autoCapitalize="none"
        placeholder="モデルID（手入力可）"
      />
      <Text style={[styles.sectionTitle, { color: colors.black }]}>
        自動承認する権限
      </Text>
      <PermissionsEditor
        permissions={model.permissions}
        onChange={(permissions: PermissionsMap) =>
          onChangeModel({ ...model, permissions })
        }
      />
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
