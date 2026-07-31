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
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { WorkSessionRow } from '@/src/api/types';
import { useTheme, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { type ProgressPayload } from '@/src/utils/progress';

// Kept for callers that previously imported this shape from this file.
export type TaskProgressSheetPayload = ProgressPayload;

interface TaskProgressSheetProps {
  visible: boolean;
  session: WorkSessionRow;
  mode: 'record' | 'pause';
  onConfirm: (payload: ProgressPayload) => void | Promise<void>;
  onCancel: () => void;
  // Optional record-only action for pause mode so users can record progress
  // without pausing the work session.
  onRecord?: (payload: ProgressPayload) => void | Promise<void>;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      justifyContent: 'flex-end',
      backgroundColor: colors.overlay,
    },
    sheet: {
      borderTopLeftRadius: 20,
      borderTopRightRadius: 20,
      padding: 20,
      gap: 12,
    },
    title: {
      fontSize: 16,
      fontWeight: '700',
      marginBottom: 4,
    },
    row: {
      flexDirection: 'row',
      gap: 8,
    },
    input: {
      borderWidth: 1,
      borderRadius: 10,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 15,
    },
    inputFlex: {
      flex: 1,
    },
    note: {
      marginTop: 4,
    },
    actions: {
      flexDirection: 'row',
      gap: 8,
      marginTop: 8,
    },
    button: {
      flex: 1,
      alignItems: 'center',
      justifyContent: 'center',
      paddingVertical: 12,
      borderRadius: 10,
    },
    secondary: {
      borderWidth: 1,
    },
    primaryText: {
      color: colors.onBrand,
      fontWeight: '600',
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
  const currentTotal = useMemo(() => session.quantity_total, [session]);

  const [delta, setDelta] = useState('');
  const [cumulative, setCumulative] = useState(String(currentDone));
  const [total, setTotal] = useState(
    currentTotal !== undefined ? String(currentTotal) : '',
  );
  const [note, setNote] = useState('');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (visible) {
      setDelta('');
      setCumulative(String(currentDone));
      setTotal(currentTotal !== undefined ? String(currentTotal) : '');
      setNote('');
    }
  }, [visible, currentDone, currentTotal, session]);

  function digitsOnly(text: string) {
    return text.replace(/[^0-9]/g, '');
  }

  function updateFromCumulative(text: string) {
    const filtered = digitsOnly(text);
    setCumulative(filtered);
    const q = parseInt(filtered, 10);
    if (Number.isNaN(q)) return;
    setDelta(String(q - currentDone));
  }

  function updateFromDelta(text: string) {
    const filtered = digitsOnly(text);
    setDelta(filtered);
    const d = parseInt(filtered, 10);
    if (Number.isNaN(d)) return;
    setCumulative(String(currentDone + d));
  }

  function buildPayload(): TaskProgressSheetPayload {
    const cumulativeNum = parseInt(cumulative, 10);
    const quantityDone = Number.isNaN(cumulativeNum)
      ? currentDone
      : cumulativeNum;
    const totalNum = parseInt(total, 10);
    const quantityTotal =
      !Number.isNaN(totalNum) && totalNum > 0 ? totalNum : undefined;
    return {
      quantityDone,
      note: note.trim() || undefined,
      quantityTotal,
    };
  }

  async function handleConfirm() {
    if (isSubmitting) return;
    haptic.medium();
    setIsSubmitting(true);
    try {
      await onConfirm(buildPayload());
    } finally {
      if (mountedRef.current) {
        setIsSubmitting(false);
      }
    }
  }

  async function handleRecord() {
    if (!onRecord || isSubmitting) return;
    haptic.medium();
    setIsSubmitting(true);
    try {
      await onRecord(buildPayload());
    } finally {
      if (mountedRef.current) {
        setIsSubmitting(false);
      }
    }
  }

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={onCancel}
    >
      <Pressable style={styles.overlay} onPress={onCancel}>
        <View
          style={[
            styles.sheet,
            {
              backgroundColor: colors.surface,
              paddingBottom: 16 + insets.bottom,
            },
          ]}
        >
          <Text style={[styles.title, { color: colors.black }]}>
            {session.title || '作業'}
          </Text>
          <Text style={{ color: colors.gray, fontSize: 13, marginBottom: 8 }}>
            {mode === 'pause' ? '進捗を記録して一時停止' : '進捗を記録'}
          </Text>

          <View style={styles.row}>
            <TextInput
              style={[
                styles.input,
                styles.inputFlex,
                {
                  borderColor: colors.separator,
                  color: colors.black,
                  backgroundColor: colors.white,
                },
              ]}
              placeholder="差分"
              placeholderTextColor={colors.grayLight}
              keyboardType="number-pad"
              value={delta}
              onChangeText={updateFromDelta}
            />
            <TextInput
              style={[
                styles.input,
                styles.inputFlex,
                {
                  borderColor: colors.separator,
                  color: colors.black,
                  backgroundColor: colors.white,
                },
              ]}
              placeholder="累積"
              placeholderTextColor={colors.grayLight}
              keyboardType="number-pad"
              value={cumulative}
              onChangeText={updateFromCumulative}
            />
          </View>

          <View style={styles.row}>
            <TextInput
              style={[
                styles.input,
                styles.inputFlex,
                {
                  borderColor: colors.separator,
                  color: colors.black,
                  backgroundColor: colors.white,
                },
              ]}
              placeholder="全体"
              placeholderTextColor={colors.grayLight}
              keyboardType="number-pad"
              value={total}
              onChangeText={(text) => setTotal(text.replace(/[^0-9]/g, ''))}
            />
          </View>

          <TextInput
            style={[
              styles.input,
              styles.note,
              {
                borderColor: colors.separator,
                color: colors.black,
                backgroundColor: colors.white,
              },
            ]}
            placeholder="メモ（任意）"
            placeholderTextColor={colors.grayLight}
            value={note}
            onChangeText={setNote}
          />

          <View style={styles.actions}>
            <Pressable
              style={[
                styles.button,
                styles.secondary,
                { borderColor: colors.separator },
              ]}
              onPress={onCancel}
            >
              <Text style={{ color: colors.black }} numberOfLines={1}>
                キャンセル
              </Text>
            </Pressable>
            {mode === 'pause' && onRecord && (
              <Pressable
                style={[
                  styles.button,
                  styles.secondary,
                  {
                    borderColor: colors.separator,
                    opacity: isSubmitting ? 0.6 : 1,
                  },
                ]}
                onPress={handleRecord}
                disabled={isSubmitting}
              >
                <Text style={{ color: colors.black }} numberOfLines={1}>
                  記録
                </Text>
              </Pressable>
            )}
            <Pressable
              style={[
                styles.button,
                {
                  backgroundColor: colors.brand,
                  opacity: isSubmitting ? 0.6 : 1,
                },
              ]}
              onPress={handleConfirm}
              disabled={isSubmitting}
            >
              <Text
                style={styles.primaryText}
                numberOfLines={1}
                adjustsFontSizeToFit
                minimumFontScale={0.75}
              >
                {mode === 'pause' ? '記録して一時停止' : '記録'}
              </Text>
            </Pressable>
          </View>
        </View>
      </Pressable>
    </Modal>
  );
}
