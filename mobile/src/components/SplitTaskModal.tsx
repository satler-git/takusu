// SplitTaskModal — bottom sheet for splitting a task into retained + remainder

import { useEffect, useState, useMemo } from 'react';
import {
  Modal,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import type { TaskRow } from '@/src/api/types';
import { useTheme, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { Checkbox } from 'react-native-paper';

export interface SplitTaskPayload {
  retainedQuantity: number;
  setDependency: boolean;
  title?: string;
  description?: string;
  endAt?: string;
}

interface SplitTaskModalProps {
  visible: boolean;
  task: TaskRow;
  onConfirm: (payload: SplitTaskPayload) => void;
  onCancel: () => void;
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
    field: {
      gap: 4,
    },
    label: {
      fontSize: 12,
    },
    input: {
      borderWidth: 1,
      borderRadius: 10,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 15,
    },
    hint: {
      fontSize: 12,
      marginTop: 2,
    },
    checkboxRow: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingVertical: 4,
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

export function SplitTaskModal({
  visible,
  task,
  onConfirm,
  onCancel,
}: SplitTaskModalProps) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const total = task.quantity_total ?? 0;

  const [retained, setRetained] = useState(
    String(Math.max(1, Math.floor(total / 2))),
  );
  const [setDependency, setSetDependency] = useState(true);
  const [title, setTitle] = useState(`${task.title}（残り）`);
  const [description, setDescription] = useState('');
  const [endAt, setEndAt] = useState<Date | null>(null);
  const [showDatePicker, setShowDatePicker] = useState(false);

  useEffect(() => {
    if (visible) {
      setRetained(String(Math.max(1, Math.floor(total / 2))));
      setSetDependency(true);
      setTitle(`${task.title}（残り）`);
      setDescription('');
      setEndAt(null);
    }
  }, [visible, total, task.title]);

  const retainedNum = parseInt(retained, 10) || 1;
  const remainder = Math.max(0, total - retainedNum);
  const canConfirm =
    total > 1 &&
    retainedNum > 0 &&
    retainedNum < total &&
    retainedNum >= task.quantity_done;

  function handleConfirm() {
    haptic.medium();
    onConfirm({
      retainedQuantity: retainedNum,
      setDependency,
      title: title.trim() || undefined,
      description: description.trim() || undefined,
      endAt: endAt ? endAt.toISOString() : undefined,
    });
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
            タスクを分割
          </Text>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>
              元タスクに残す数量
            </Text>
            <TextInput
              style={[
                styles.input,
                {
                  borderColor: colors.separator,
                  color: colors.black,
                  backgroundColor: colors.white,
                },
              ]}
              placeholder="元タスクに残す数量"
              placeholderTextColor={colors.grayLight}
              keyboardType="number-pad"
              value={retained}
              onChangeText={(text) => setRetained(text.replace(/[^0-9]/g, ''))}
            />
            <Text style={[styles.hint, { color: colors.gray }]}>
              残り {remainder} を新しいタスクにします
            </Text>
          </View>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>全体</Text>
            <TextInput
              style={[
                styles.input,
                {
                  borderColor: colors.separator,
                  color: colors.black,
                  backgroundColor: colors.grayLight,
                },
              ]}
              editable={false}
              value={String(total)}
            />
          </View>

          <Pressable
            style={styles.checkboxRow}
            onPress={() => setSetDependency((v) => !v)}
          >
            <Checkbox
              status={setDependency ? 'checked' : 'unchecked'}
              onPress={() => setSetDependency((v) => !v)}
              color={colors.brand}
            />
            <Text style={{ color: colors.black, marginStart: 8 }}>
              残りを元タスクに依存させる
            </Text>
          </Pressable>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>
              残りタスクのタイトル（任意）
            </Text>
            <TextInput
              style={[
                styles.input,
                {
                  borderColor: colors.separator,
                  color: colors.black,
                  backgroundColor: colors.white,
                },
              ]}
              placeholder="残りタスクのタイトル"
              placeholderTextColor={colors.grayLight}
              value={title}
              onChangeText={setTitle}
            />
          </View>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>
              説明（任意）
            </Text>
            <TextInput
              style={[
                styles.input,
                {
                  borderColor: colors.separator,
                  color: colors.black,
                  backgroundColor: colors.white,
                },
              ]}
              placeholder="説明"
              placeholderTextColor={colors.grayLight}
              value={description}
              onChangeText={setDescription}
            />
          </View>

          <View style={styles.field}>
            <Text style={[styles.label, { color: colors.gray }]}>
              期限（任意）
            </Text>
            <Pressable
              style={[
                styles.input,
                {
                  borderColor: colors.separator,
                  backgroundColor: colors.white,
                  justifyContent: 'center',
                },
              ]}
              onPress={() => setShowDatePicker(true)}
            >
              <Text style={{ color: endAt ? colors.black : colors.grayLight }}>
                {endAt
                  ? `${endAt.getFullYear()}/${(endAt.getMonth() + 1).toString().padStart(2, '0')}/${endAt.getDate().toString().padStart(2, '0')} ${endAt.getHours().toString().padStart(2, '0')}:${endAt.getMinutes().toString().padStart(2, '0')}`
                  : '期限を設定'}
              </Text>
            </Pressable>
          </View>

          <View style={styles.actions}>
            <Pressable
              style={[
                styles.button,
                styles.secondary,
                { borderColor: colors.separator },
              ]}
              onPress={onCancel}
            >
              <Text style={{ color: colors.black }}>キャンセル</Text>
            </Pressable>
            <Pressable
              style={[
                styles.button,
                {
                  backgroundColor: canConfirm ? colors.brand : colors.grayLight,
                },
              ]}
              onPress={handleConfirm}
              disabled={!canConfirm}
            >
              <Text style={styles.primaryText}>分割</Text>
            </Pressable>
          </View>
        </View>
      </Pressable>

      <DateTimePickerModal
        visible={showDatePicker}
        value={endAt}
        mode="datetime"
        label="期限"
        onConfirm={(d) => {
          setEndAt(d);
          setShowDatePicker(false);
        }}
        onCancel={() => setShowDatePicker(false)}
        optional
      />
    </Modal>
  );
}
