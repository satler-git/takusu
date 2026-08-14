import { memo, useCallback, useMemo, useState } from 'react';
import {
  ActivityIndicator,
  Modal,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { PressableScale } from '@/src/components/PressableScale';
import { haptic } from '@/src/components/haptics';
import { useTheme, type ColorSet } from '@/src/theme';
import { AgentClient, AgentApiError } from '@/src/api/agentClient';
import { showError } from '@/src/api/errors';
import type { CapabilityRequest, Presentation } from '@/src/api/agentTypes';

interface AgentCompactPanelProps {
  visible: boolean;
  mode: 'complete' | 'delay' | 'progress';
  taskId: string;
  taskTitle: string;
  taskReference: string;
  agentClient: AgentClient;
  onClose: () => void;
  onSuccess: () => Promise<void>;
}

const SNOOZE_OPTIONS = [
  { minutes: 10, label: '10分後' },
  { minutes: 30, label: '30分後' },
  { minutes: 60, label: '1時間後' },
];

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      justifyContent: 'flex-end',
      backgroundColor: colors.overlay,
    },
    sheet: {
      backgroundColor: colors.surface,
      borderTopStartRadius: 16,
      borderTopEndRadius: 16,
      padding: 16,
      paddingBottom: 32,
      gap: 16,
    },
    header: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
    },
    headerTitle: {
      fontSize: 16,
      fontWeight: '700',
      color: colors.black,
    },
    reference: {
      fontSize: 12,
      color: colors.textOnCardSecondary,
    },
    body: {
      gap: 12,
    },
    prompt: {
      fontSize: 14,
      color: colors.textOnCard,
    },
    actions: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 8,
    },
    action: {
      paddingHorizontal: 16,
      paddingVertical: 10,
      borderRadius: 10,
      backgroundColor: colors.surfaceTint,
    },
    primaryAction: {
      backgroundColor: colors.brand,
    },
    actionText: {
      fontSize: 14,
      fontWeight: '600',
      color: colors.textOnCard,
    },
    primaryActionText: {
      color: colors.onBrand,
    },
    input: {
      borderWidth: 1,
      borderColor: colors.cardOutline,
      borderRadius: 8,
      padding: 10,
      color: colors.black,
      backgroundColor: colors.surfaceTint,
      fontSize: 14,
    },
    result: {
      padding: 12,
      borderRadius: 10,
      backgroundColor: colors.surfaceTint,
      gap: 6,
    },
    resultTitle: {
      fontSize: 15,
      fontWeight: '600',
      color: colors.black,
    },
    resultDetail: {
      fontSize: 13,
      color: colors.textOnCard,
    },
  });

function decodeError(e: unknown): string {
  if (e instanceof AgentApiError) {
    try {
      const parsed = JSON.parse(e.body);
      if (typeof parsed.error === 'string') return parsed.error;
    } catch {
      // fall through
    }
    return `Agent API error ${e.status}`;
  }
  return e instanceof Error ? e.message : String(e);
}

function AgentCompactPanelImpl({
  visible,
  mode,
  taskId,
  taskTitle,
  taskReference,
  agentClient,
  onClose,
  onSuccess,
}: AgentCompactPanelProps) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<Presentation | null>(null);
  const [quantity, setQuantity] = useState('');
  const [note, setNote] = useState('');

  const execute = useCallback(
    async (request: CapabilityRequest) => {
      if (loading) return;
      setLoading(true);
      try {
        const presentation = await agentClient.quickAction(request);
        setResult(presentation);
        await onSuccess();
      } catch (e) {
        showError(e, decodeError(e));
      } finally {
        setLoading(false);
      }
    },
    [agentClient, loading, onSuccess],
  );

  const handleComplete = useCallback(() => {
    haptic.light();
    execute({
      task_id: taskId,
      action: 'complete',
      device_id: 'mobile',
    });
  }, [execute, taskId]);

  const handleDelay = useCallback(
    (minutes: number) => {
      haptic.light();
      execute({
        task_id: taskId,
        action: 'delay',
        device_id: 'mobile',
        snooze_minutes: minutes,
      });
    },
    [execute, taskId],
  );

  const handleProgress = useCallback(() => {
    haptic.light();
    const n = Number(quantity);
    if (
      !Number.isFinite(n) ||
      !Number.isInteger(n) ||
      n <= 0 ||
      n > Number.MAX_SAFE_INTEGER
    ) {
      showError(
        new Error(
          '1 以上 ' + Number.MAX_SAFE_INTEGER + ' 以下の整数を入力してください',
        ),
        '進捗の記録に失敗',
      );
      return;
    }
    execute({
      task_id: taskId,
      action: 'progress',
      device_id: 'mobile',
      quantity_done: n,
      note: note.trim() || undefined,
    });
  }, [execute, taskId, quantity, note]);

  const handleClose = useCallback(() => {
    setResult(null);
    setQuantity('');
    setNote('');
    onClose();
  }, [onClose]);

  const title = useMemo(() => {
    switch (mode) {
      case 'complete':
        return '完了';
      case 'delay':
        return '延期';
      case 'progress':
        return '進捗';
    }
  }, [mode]);

  const renderBody = () => {
    if (result) {
      switch (result.type) {
        case 'work_transition':
          return (
            <View style={styles.result}>
              <Text style={styles.resultTitle}>
                {result.title} {result.reference}
              </Text>
              {result.detail ? (
                <Text style={styles.resultDetail}>{result.detail}</Text>
              ) : null}
            </View>
          );
        case 'text':
          return (
            <View style={styles.result}>
              <Text style={styles.resultDetail}>{result.text}</Text>
            </View>
          );
        default:
          return (
            <View style={styles.result}>
              <Text style={styles.resultDetail}>結果を受け取りました</Text>
            </View>
          );
      }
    }

    switch (mode) {
      case 'complete':
        return (
          <View style={styles.body}>
            <Text style={styles.prompt}>「{taskTitle}」を完了しますか？</Text>
            <PressableScale
              style={[styles.action, styles.primaryAction]}
              onPress={handleComplete}
              disabled={loading}
            >
              <Text style={[styles.actionText, styles.primaryActionText]}>
                完了
              </Text>
            </PressableScale>
          </View>
        );
      case 'delay':
        return (
          <View style={styles.body}>
            <Text style={styles.prompt}>
              「{taskTitle}」をいつまで延期しますか？
            </Text>
            <View style={styles.actions}>
              {SNOOZE_OPTIONS.map((opt) => (
                <PressableScale
                  key={opt.minutes}
                  style={styles.action}
                  onPress={() => handleDelay(opt.minutes)}
                  disabled={loading}
                >
                  <Text style={styles.actionText}>{opt.label}</Text>
                </PressableScale>
              ))}
            </View>
          </View>
        );
      case 'progress':
        return (
          <View style={styles.body}>
            <Text style={styles.prompt}>「{taskTitle}」の進捗を入力</Text>
            <TextInput
              style={styles.input}
              value={quantity}
              onChangeText={setQuantity}
              keyboardType="number-pad"
              placeholder="完了数"
              placeholderTextColor={colors.textOnCardSecondary}
            />
            <TextInput
              style={styles.input}
              value={note}
              onChangeText={setNote}
              placeholder="メモ（任意）"
              placeholderTextColor={colors.textOnCardSecondary}
            />
            <PressableScale
              style={[styles.action, styles.primaryAction]}
              onPress={handleProgress}
              disabled={loading}
            >
              <Text style={[styles.actionText, styles.primaryActionText]}>
                記録
              </Text>
            </PressableScale>
          </View>
        );
    }
  };

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={handleClose}
    >
      <Pressable style={styles.overlay} onPress={handleClose}>
        <View style={styles.sheet}>
          <View style={styles.header}>
            <View>
              <Text style={styles.headerTitle}>{title}</Text>
              <Text style={styles.reference}>{taskReference}</Text>
            </View>
            <PressableScale onPress={handleClose}>
              <Ionicons name="close" size={24} color={colors.textOnCard} />
            </PressableScale>
          </View>
          {loading && <ActivityIndicator color={colors.brand} />}
          {renderBody()}
        </View>
      </Pressable>
    </Modal>
  );
}

export const AgentCompactPanel = memo(AgentCompactPanelImpl);
