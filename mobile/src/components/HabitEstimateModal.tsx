// HabitEstimateModal — preview and apply a habit's avg/sigma estimate from
// completed task actuals, with automatic outlier detection and per-step
// estimates (#919).

import { useCallback, useEffect, useState } from 'react';
import {
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { BRAND_COLOR, useTheme } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { TakusuClient } from '@/src/api/client';
import type { HabitEstimateResult, HabitEstimateStep } from '@/src/api/types';
import { formatDuration } from '@/src/utils/duration';
import { showError } from '@/src/api/errors';

interface HabitEstimateModalProps {
  visible: boolean;
  habitId: string;
  client: TakusuClient;
  onClose: () => void;
  onApplied: () => Promise<void>;
}

export function HabitEstimateModal({
  visible,
  habitId,
  client,
  onClose,
  onApplied,
}: HabitEstimateModalProps) {
  const { colors } = useTheme();
  const insets = useSafeAreaInsets();
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [detectOutliers, setDetectOutliers] = useState(false);
  const [result, setResult] = useState<HabitEstimateResult | null>(null);

  const loadPreview = useCallback(
    async (useOutliers: boolean) => {
      setLoading(true);
      try {
        const preview = await client.estimateHabit(habitId, {
          detect_outliers: useOutliers,
          apply: false,
        });
        setResult(preview);
      } catch (e) {
        showError(e, '見積もりの取得に失敗');
      } finally {
        setLoading(false);
      }
    },
    [client, habitId],
  );

  useEffect(() => {
    if (visible) {
      setDetectOutliers(false);
      loadPreview(false);
    }
  }, [visible, habitId, loadPreview]);

  async function toggleOutliers(value: boolean) {
    haptic.light();
    setDetectOutliers(value);
    await loadPreview(value);
  }

  async function applyEstimate() {
    if (!result || applying || loading) return;
    setApplying(true);
    try {
      await client.estimateHabit(habitId, {
        detect_outliers: detectOutliers,
        apply: true,
      });
      haptic.medium();
      await onApplied();
      onClose();
    } catch (e) {
      showError(e, '見積もりの適用に失敗');
    } finally {
      setApplying(false);
    }
  }

  const noData = !loading && result && result.sample_count === 0;

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={onClose}
    >
      <Pressable style={styles.overlay} onPress={onClose}>
        <Pressable
          style={[
            styles.sheet,
            {
              backgroundColor: colors.white,
              paddingBottom: 32 + insets.bottom,
            },
          ]}
          onPress={(e) => e.stopPropagation()}
        >
          <View style={styles.header}>
            <Text style={[styles.title, { color: colors.black }]}>
              実績から見積もり
            </Text>
            <Pressable
              onPress={() => {
                haptic.light();
                onClose();
              }}
            >
              <Ionicons name="close" size={24} color={colors.gray} />
            </Pressable>
          </View>

          {loading && (
            <Text style={[styles.message, { color: colors.grayDark }]}>
              読み込み中…
            </Text>
          )}

          {!loading && result && (
            <>
              <View
                style={[
                  styles.summary,
                  { backgroundColor: colors.surfaceTint },
                ]}
              >
                <Text style={[styles.summaryLabel, { color: colors.grayDark }]}>
                  推定値
                </Text>
                <Text
                  style={[styles.summaryValue, { color: colors.black }]}
                >{`${formatDuration(result.avg_minutes)} ± ${formatDuration(result.sigma_minutes)}`}</Text>
                <Text style={[styles.summaryMeta, { color: colors.gray }]}>
                  {`${result.sample_count}件の実績（${result.excluded_count}件除外）`}
                </Text>
              </View>

              <View
                style={[
                  styles.toggleRow,
                  { backgroundColor: colors.surfaceTint },
                ]}
              >
                <Text style={[styles.toggleLabel, { color: colors.black }]}>
                  外れ値を自動検出
                </Text>
                <Switch
                  value={detectOutliers}
                  onValueChange={toggleOutliers}
                  disabled={loading || applying}
                  trackColor={{ false: colors.grayLight, true: BRAND_COLOR }}
                  thumbColor={colors.white}
                />
              </View>

              {noData ? (
                <Text style={[styles.message, { color: colors.grayDark }]}>
                  完了済みの非固定タスクが見つかりません。
                </Text>
              ) : (
                <ScrollView style={styles.list}>
                  {result.steps.length > 0 ? (
                    result.steps.map((step) => (
                      <StepEstimateRow key={step.step_id} step={step} />
                    ))
                  ) : (
                    <Text style={[styles.noSteps, { color: colors.gray }]}>
                      Step なしの習慣です。全体の実績から推定しています。
                    </Text>
                  )}
                </ScrollView>
              )}

              <View style={styles.actionRow}>
                <Pressable
                  style={[
                    styles.cancelButton,
                    { borderColor: colors.separator },
                  ]}
                  onPress={() => {
                    haptic.light();
                    onClose();
                  }}
                >
                  <Text style={[styles.cancelText, { color: colors.grayDark }]}>
                    閉じる
                  </Text>
                </Pressable>
                <Pressable
                  style={[styles.confirmButton, applying && { opacity: 0.6 }]}
                  onPress={applyEstimate}
                  disabled={applying || loading || noData}
                >
                  <Text style={[styles.confirmText, { color: colors.white }]}>
                    {applying ? '適用中…' : '適用'}
                  </Text>
                </Pressable>
              </View>
            </>
          )}
        </Pressable>
      </Pressable>
    </Modal>
  );
}

function StepEstimateRow({ step }: { step: HabitEstimateStep }) {
  const { colors } = useTheme();
  return (
    <View style={[styles.stepRow, { backgroundColor: colors.surfaceTint }]}>
      <Text style={[styles.stepTitle, { color: colors.black }]}>
        {step.title || '(no title)'}
      </Text>
      <Text style={[styles.stepValue, { color: colors.black }]}>
        {`${formatDuration(step.avg_minutes)} ± ${formatDuration(step.sigma_minutes)}`}
      </Text>
      <Text style={[styles.stepMeta, { color: colors.gray }]}>
        {`${step.sample_count}件（${step.excluded_count}件除外）`}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create({
  overlay: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.4)',
    justifyContent: 'flex-end',
  },
  sheet: {
    borderTopLeftRadius: 20,
    borderTopRightRadius: 20,
    padding: 20,
    paddingBottom: 32,
    maxHeight: '80%',
  },
  header: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    marginBottom: 16,
  },
  title: {
    fontSize: 18,
    fontWeight: '600',
  },
  summary: {
    borderRadius: 12,
    padding: 16,
    marginBottom: 12,
    alignItems: 'center',
  },
  summaryLabel: {
    fontSize: 12,
    marginBottom: 4,
  },
  summaryValue: {
    fontSize: 22,
    fontWeight: '700',
    marginBottom: 4,
  },
  summaryMeta: {
    fontSize: 12,
  },
  toggleRow: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    borderRadius: 12,
    paddingHorizontal: 16,
    paddingVertical: 12,
    marginBottom: 12,
  },
  toggleLabel: {
    fontSize: 15,
    fontWeight: '500',
  },
  message: {
    textAlign: 'center',
    marginVertical: 24,
    fontSize: 14,
  },
  list: {
    maxHeight: 240,
    marginBottom: 16,
  },
  noSteps: {
    textAlign: 'center',
    marginVertical: 24,
    fontSize: 14,
  },
  stepRow: {
    borderRadius: 12,
    padding: 14,
    marginBottom: 8,
  },
  stepTitle: {
    fontSize: 15,
    fontWeight: '600',
    marginBottom: 4,
  },
  stepValue: {
    fontSize: 18,
    fontWeight: '700',
    marginBottom: 2,
  },
  stepMeta: {
    fontSize: 12,
  },
  actionRow: {
    flexDirection: 'row',
    gap: 12,
  },
  cancelButton: {
    flex: 1,
    padding: 14,
    borderRadius: 12,
    borderWidth: 1,
    alignItems: 'center',
  },
  cancelText: {
    fontSize: 15,
    fontWeight: '600',
  },
  confirmButton: {
    flex: 1,
    padding: 14,
    borderRadius: 12,
    backgroundColor: BRAND_COLOR,
    alignItems: 'center',
  },
  confirmText: {
    fontSize: 15,
    fontWeight: '600',
  },
});
