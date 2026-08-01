// TaskProgressSheet — bottom sheet for recording work session progress
// Supports entering either delta (this-time) or cumulative quantity, and
// allows editing the total. Used from HomeView and TaskDetailView.

import { useEffect, useMemo, useRef, useState } from 'react';
import {
  Modal,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import Reanimated, {
  Easing as ReanimatedEasing,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
} from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { WorkSessionRow } from '@/src/api/types';
import { useTheme, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { type ProgressPayload } from '@/src/utils/progress';
import { useTaskProgress } from '@/src/hooks/useTaskProgress';

// Kept for callers that previously imported this shape from this file.
export type TaskProgressSheetPayload = ProgressPayload;

const LONG_PRESS_MS = 600;

interface TaskProgressSheetProps {
  visible: boolean;
  session: WorkSessionRow;
  mode: 'record' | 'pause' | 'complete';
  onConfirm: (payload: ProgressPayload) => void | Promise<void>;
  onCancel: () => void;
  // Optional record-only action for pause/complete mode so users can record
  // progress without pausing or completing the work session.
  onRecord?: (payload: ProgressPayload) => void | Promise<void>;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      ...StyleSheet.absoluteFill,
      backgroundColor: colors.overlay,
    },
    sheet: {
      borderTopLeftRadius: 20,
      borderTopRightRadius: 20,
      padding: 20,
      gap: 10,
    },
    header: {
      marginBottom: 2,
    },
    title: {
      fontSize: 20,
      fontWeight: '800',
      letterSpacing: -0.3,
    },
    note: {
      borderWidth: 1,
      borderRadius: 12,
      paddingHorizontal: 12,
      paddingVertical: 8,
      fontSize: 15,
      lineHeight: 22,
    },
    preview: {
      alignItems: 'center',
      justifyContent: 'center',
      borderRadius: 12,
      paddingVertical: 10,
      paddingHorizontal: 12,
      gap: 6,
    },
    previewRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 4,
    },
    previewText: {
      fontSize: 15,
      fontWeight: '800',
      fontVariant: ['tabular-nums'],
    },
    previewUnit: {
      fontSize: 12,
      fontWeight: '600',
    },
    track: {
      width: '100%',
      maxWidth: 120,
      height: 6,
      borderRadius: 3,
      overflow: 'hidden',
      backgroundColor: colors.separator,
    },
    trackFill: {
      height: '100%',
      borderRadius: 3,
    },
    totalField: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      borderWidth: 1,
      borderRadius: 12,
      paddingHorizontal: 12,
      paddingVertical: 6,
    },
    totalInput: {
      flex: 1,
      fontSize: 18,
      fontWeight: '700',
      textAlign: 'right',
      fontVariant: ['tabular-nums'],
    },
    totalUnit: {
      fontSize: 13,
      fontWeight: '600',
    },
    qtyField: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      borderWidth: 1,
      borderRadius: 16,
      paddingHorizontal: 10,
      paddingVertical: 8,
    },
    qtyStep: {
      width: 40,
      height: 40,
      borderRadius: 20,
      alignItems: 'center',
      justifyContent: 'center',
      borderWidth: 1,
    },
    qtyInput: {
      flex: 1,
      fontSize: 24,
      fontWeight: '800',
      textAlign: 'right',
      fontVariant: ['tabular-nums'],
    },
    qtyUnit: {
      fontSize: 14,
      fontWeight: '600',
    },
    toggle: {
      flexDirection: 'row',
      gap: 4,
      borderWidth: 1,
      borderRadius: 12,
      padding: 4,
    },
    toggleButton: {
      flex: 1,
      alignItems: 'center',
      justifyContent: 'center',
      paddingVertical: 7,
      borderRadius: 8,
    },
    toggleText: {
      fontSize: 13,
      fontWeight: '700',
    },
    actions: {
      flexDirection: 'row',
      gap: 10,
      marginTop: 4,
    },
    cancelBtn: {
      flex: 1,
      alignItems: 'center',
      justifyContent: 'center',
      height: 52,
      borderRadius: 14,
      borderWidth: 1,
    },
    primaryBtn: {
      position: 'relative',
      flex: 2,
      alignItems: 'center',
      justifyContent: 'center',
      height: 52,
      borderRadius: 14,
      overflow: 'hidden',
    },
    primaryText: {
      fontSize: 16,
      fontWeight: '800',
    },
    pressFill: {
      position: 'absolute',
      left: 0,
      right: 0,
      bottom: 0,
      height: 4,
      transformOrigin: 'left',
    },
    hint: {
      textAlign: 'center',
      fontSize: 11,
      fontWeight: '700',
      marginTop: 4,
    },
  });

export function TaskProgressSheet({
  visible,
  session,
  mode,
  onConfirm,
  onCancel,
  onRecord,
}: TaskProgressSheetProps) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();

  const currentDone = useMemo(() => session.quantity_done ?? 0, [session]);
  const currentTotal = useMemo(() => session.quantity_total ?? 0, [session]);
  const unit = useMemo(() => session.quantity_unit ?? '', [session]);

  const [isSubmitting, setIsSubmitting] = useState(false);

  const pressProgress = useSharedValue(0);
  const pressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const longPressFiredRef = useRef(false);
  const mountedRef = useRef(true);

  const pressFillStyle = useAnimatedStyle(() => ({
    transform: [{ scaleX: pressProgress.value }],
  }));

  const canToggle = useMemo(
    () => mode !== 'complete' && onRecord != null,
    [mode, onRecord],
  );

  const progress = useTaskProgress({
    session,
    mode,
    allowToggle: canToggle,
  });

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (pressTimerRef.current) {
        clearTimeout(pressTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (visible) {
      progress.reset();
    }
  }, [visible, progress]);

  function switchInputMode(next: 'delta' | 'cumulative') {
    haptic.light();
    progress.switchInputMode(next);
  }

  function adjustQty(d: number) {
    haptic.light();
    progress.adjustQty(d);
  }

  async function handlePrimaryPress() {
    if (isSubmitting) {
      return;
    }
    haptic.medium();
    setIsSubmitting(true);
    try {
      const payload = progress.buildPayload();
      if (progress.action === 'record' && onRecord) {
        await onRecord(payload);
      } else {
        await onConfirm(payload);
      }
    } finally {
      if (mountedRef.current) {
        setIsSubmitting(false);
      }
    }
  }

  function startPress() {
    if (!canToggle) {
      return;
    }
    longPressFiredRef.current = false;
    pressProgress.value = withTiming(1, {
      duration: LONG_PRESS_MS,
      easing: ReanimatedEasing.linear,
    });
    pressTimerRef.current = setTimeout(() => {
      longPressFiredRef.current = true;
      pressTimerRef.current = null;
      haptic.medium();
      progress.toggleAction();
    }, LONG_PRESS_MS);
  }

  function endPress() {
    if (pressTimerRef.current) {
      clearTimeout(pressTimerRef.current);
      pressTimerRef.current = null;
    }
    pressProgress.value = 0;
  }

  function onPressablePress() {
    pressProgress.value = 0;
    longPressFiredRef.current = false;
    void handlePrimaryPress();
  }

  const { afterDone, afterTotal, previewPct } = progress;

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={onCancel}
    >
      <View style={{ flex: 1 }}>
        <Pressable style={styles.overlay} onPress={onCancel} />
        <View
          style={[
            styles.sheet,
            {
              backgroundColor: colors.surface,
              paddingBottom: 16 + insets.bottom,
            },
          ]}
        >
          <View style={styles.header}>
            <Text
              style={[styles.title, { color: colors.black }]}
              numberOfLines={2}
            >
              {session.title || '作業'}
            </Text>
          </View>

          <TextInput
            style={[
              styles.note,
              {
                borderColor: colors.separator,
                color: colors.black,
                backgroundColor: colors.white,
              },
            ]}
            placeholder="メモ（任意）"
            placeholderTextColor={colors.grayLight}
            value={progress.note}
            onChangeText={progress.handleNoteChange}
            accessibilityLabel="メモ（任意）"
          />

          <View
            style={[styles.preview, { backgroundColor: colors.surfaceTint }]}
          >
            <View style={styles.previewRow}>
              <Text style={[styles.previewText, { color: colors.black }]}>
                {currentDone}
              </Text>
              <Text style={[styles.previewText, { color: colors.gray }]}>
                {' / '}
              </Text>
              <Text style={[styles.previewText, { color: colors.black }]}>
                {currentTotal}
              </Text>
              <Ionicons
                name="arrow-forward"
                size={14}
                color={colors.gray}
                style={{ marginHorizontal: 4 }}
              />
              <Text style={[styles.previewText, { color: colors.brand }]}>
                {afterDone}
              </Text>
              <Text style={[styles.previewText, { color: colors.gray }]}>
                {' / '}
              </Text>
              <Text style={[styles.previewText, { color: colors.black }]}>
                {afterTotal}
              </Text>
              <Text
                style={[
                  styles.previewUnit,
                  { color: colors.gray, marginLeft: 2 },
                ]}
              >
                {unit}
              </Text>
            </View>
            <View style={styles.track}>
              <View
                style={[
                  styles.trackFill,
                  { width: `${previewPct}%`, backgroundColor: colors.brand },
                ]}
              />
            </View>
          </View>

          <View
            style={[
              styles.totalField,
              {
                borderColor: colors.separator,
                backgroundColor: colors.white,
              },
            ]}
          >
            <TextInput
              style={[styles.totalInput, { color: colors.black }]}
              value={progress.total}
              onChangeText={progress.handleTotalChange}
              keyboardType="number-pad"
              placeholder="目標"
              placeholderTextColor={colors.grayLight}
              accessibilityLabel="目標数量"
            />
            <Text style={[styles.totalUnit, { color: colors.gray }]}>
              {unit}
            </Text>
          </View>

          <View
            style={[
              styles.qtyField,
              {
                borderColor: colors.separator,
                backgroundColor: colors.white,
              },
            ]}
          >
            <Pressable
              style={[
                styles.qtyStep,
                {
                  borderColor: colors.separator,
                  backgroundColor: colors.surface,
                },
              ]}
              onPress={() => adjustQty(-1)}
              accessibilityLabel="1 減らす"
            >
              <Ionicons name="remove" size={18} color={colors.brand} />
            </Pressable>
            <TextInput
              style={[styles.qtyInput, { color: colors.black }]}
              value={progress.qty}
              onChangeText={progress.handleQtyChange}
              keyboardType="number-pad"
              accessibilityLabel={
                progress.inputMode === 'delta' ? '差分' : '累積'
              }
            />
            <Text style={[styles.qtyUnit, { color: colors.gray }]}>{unit}</Text>
            <Pressable
              style={[
                styles.qtyStep,
                {
                  borderColor: colors.separator,
                  backgroundColor: colors.surface,
                },
              ]}
              onPress={() => adjustQty(1)}
              accessibilityLabel="1 増やす"
            >
              <Ionicons name="add" size={18} color={colors.brand} />
            </Pressable>
          </View>

          <View
            style={[
              styles.toggle,
              {
                borderColor: colors.separator,
                backgroundColor: colors.white,
              },
            ]}
          >
            <Pressable
              style={[
                styles.toggleButton,
                progress.inputMode === 'delta' && {
                  backgroundColor: colors.brand,
                },
              ]}
              onPress={() => switchInputMode('delta')}
              accessibilityRole="tab"
              accessibilityState={{ selected: progress.inputMode === 'delta' }}
            >
              <Text
                style={[
                  styles.toggleText,
                  {
                    color:
                      progress.inputMode === 'delta'
                        ? colors.onBrand
                        : colors.black,
                  },
                ]}
              >
                差分
              </Text>
            </Pressable>
            <Pressable
              style={[
                styles.toggleButton,
                progress.inputMode === 'cumulative' && {
                  backgroundColor: colors.brand,
                },
              ]}
              onPress={() => switchInputMode('cumulative')}
              accessibilityRole="tab"
              accessibilityState={{
                selected: progress.inputMode === 'cumulative',
              }}
            >
              <Text
                style={[
                  styles.toggleText,
                  {
                    color:
                      progress.inputMode === 'cumulative'
                        ? colors.onBrand
                        : colors.black,
                  },
                ]}
              >
                累積
              </Text>
            </Pressable>
          </View>

          <View style={styles.actions}>
            <Pressable
              style={[
                styles.cancelBtn,
                {
                  borderColor: colors.separator,
                  backgroundColor: colors.white,
                },
              ]}
              onPress={onCancel}
            >
              <Text
                style={{ color: colors.black, fontWeight: '700', fontSize: 16 }}
              >
                キャンセル
              </Text>
            </Pressable>
            <Pressable
              style={[
                styles.primaryBtn,
                {
                  backgroundColor:
                    mode === 'complete' ? colors.green : colors.brand,
                  opacity: isSubmitting ? 0.6 : 1,
                },
              ]}
              onPressIn={startPress}
              onPressOut={endPress}
              onPress={onPressablePress}
              disabled={isSubmitting}
              accessibilityRole="button"
              accessibilityLabel={progress.primaryLabel}
            >
              <Text
                style={[
                  styles.primaryText,
                  {
                    color: mode === 'complete' ? colors.white : colors.onBrand,
                  },
                ]}
                numberOfLines={1}
              >
                {progress.primaryLabel}
              </Text>
              {canToggle && (
                <Reanimated.View
                  style={[
                    styles.pressFill,
                    pressFillStyle,
                    {
                      backgroundColor: colors.onBrand,
                      opacity: 0.4,
                    },
                  ]}
                />
              )}
            </Pressable>
          </View>

          {canToggle && (
            <Text style={[styles.hint, { color: colors.gray }]}>
              {progress.hintLabel}
            </Text>
          )}
        </View>
      </View>
    </Modal>
  );
}
