import { useEffect, useState, useMemo } from 'react';
import {
  asBoolean,
  asNumber,
  asString,
  asStringArray,
  asArray,
  isRecord,
} from '@/src/components/ApprovalPanel';
import { formatDuration } from '@/src/utils/duration';
import {
  ActivityIndicator,
  Modal,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { PressableScale } from '@/src/components/PressableScale';
import { AgentClient } from '@/src/api/agentClient';
import { showError } from '@/src/api/errors';
import type {
  HabitPreviewTask,
  HabitStepInput,
  WindowMode,
} from '@/src/api/types';

interface HabitPreviewModalProps {
  visible: boolean;
  onClose: () => void;
  client?: AgentClient;
  habit: Record<string, unknown>;
  title?: string;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      backgroundColor: colors.overlay,
      justifyContent: 'center',
      paddingHorizontal: 16,
    },
    container: {
      borderRadius: 16,
      borderWidth: 1,
      maxHeight: '80%',
      overflow: 'hidden',
    },
    header: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      paddingHorizontal: 16,
      paddingVertical: 12,
      borderBottomWidth: 1,
    },
    headerTitle: {
      fontSize: 16,
      fontWeight: '700',
    },
    closeButton: {
      padding: 4,
    },
    list: {
      padding: 12,
      gap: 8,
    },
    loader: {
      padding: 24,
    },
    empty: {
      textAlign: 'center',
      padding: 24,
      fontSize: 14,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      borderRadius: 8,
      borderWidth: 1,
      padding: 8,
    },
    index: {
      fontSize: 14,
      fontWeight: '700',
      minWidth: 20,
    },
    rowBody: {
      flex: 1,
      gap: 0,
    },
    title: {
      fontSize: 13,
      fontWeight: '600',
    },
    time: {
      fontSize: 11,
    },
  });

export function HabitPreviewModal({
  visible,
  onClose,
  client,
  habit,
  title,
}: HabitPreviewModalProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const { tasks, loading } = usePreviewTasks(visible, client, habit, title);

  return (
    <Modal
      visible={visible}
      transparent
      animationType="fade"
      onRequestClose={onClose}
    >
      <View style={[styles.overlay, { paddingTop: 24 + insets.top }]}>
        <View
          style={[
            styles.container,
            {
              backgroundColor: colors.surface,
              borderColor: colors.separator,
              paddingBottom: 16 + insets.bottom,
            },
          ]}
        >
          <View
            style={[styles.header, { borderBottomColor: colors.separator }]}
          >
            <Text style={[styles.headerTitle, { color: colors.black }]}>
              タスク生成プレビュー
            </Text>
            <PressableScale
              onPress={() => {
                haptic.light();
                onClose();
              }}
              hitSlop={8}
              style={styles.closeButton}
            >
              <Ionicons name="close" size={24} color={colors.gray} />
            </PressableScale>
          </View>

          <ScrollView contentContainerStyle={styles.list}>
            {loading ? (
              <ActivityIndicator color={colors.brand} style={styles.loader} />
            ) : tasks.length === 0 ? (
              <Text style={[styles.empty, { color: colors.gray }]}>
                プレビューできるタスクがありません
              </Text>
            ) : (
              tasks.map((task, index) => (
                <TaskRow
                  key={index}
                  task={task}
                  index={index}
                  colors={colors}
                />
              ))
            )}
          </ScrollView>
        </View>
      </View>
    </Modal>
  );
}

function usePreviewTasks(
  visible: boolean,
  client: AgentClient | undefined,
  habit: Record<string, unknown>,
  title: string | undefined,
): { tasks: HabitPreviewTask[]; loading: boolean } {
  const [tasks, setTasks] = useState<HabitPreviewTask[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!visible || !client) {
      setTasks([]);
      return;
    }

    const recurrence = asString(habit.recurrence) ?? '';
    if (!recurrence) {
      setTasks([]);
      return;
    }

    const steps = (
      asArray<Record<string, unknown>>(habit.steps, isRecord) ?? []
    ).map(
      (s): HabitStepInput => ({
        id: typeof s.id === 'string' ? s.id : undefined,
        position:
          typeof s.position === 'number'
            ? s.position
            : (asNumber(s.position) ?? 0),
        title: asString(s.title) ?? '',
        description: asString(s.description) ?? undefined,
        start_time: asString(s.start_time) ?? '09:00',
        end_time: asString(s.end_time) ?? '10:00',
        avg_minutes: asNumber(s.avg_minutes) ?? 60,
        sigma_minutes: asNumber(s.sigma_minutes),
        parallelizable: asBoolean(s.parallelizable),
        allows_parallel: asBoolean(s.allows_parallel),
        abandonability: asNumber(s.abandonability),
        fixed: asBoolean(s.fixed),
        depends_on: asStringArray(s.depends_on),
      }),
    );

    const avg = asNumber(habit.avg_minutes);
    const body = {
      title: title ?? asString(habit.title) ?? 'Habit',
      description: asString(habit.description) ?? undefined,
      recurrence,
      start_time: asString(habit.start_time) ?? '09:00',
      end_time: asString(habit.end_time) ?? '10:00',
      avg_minutes: avg ?? 60,
      sigma_minutes: asNumber(habit.sigma_minutes),
      parallelizable: asBoolean(habit.parallelizable),
      allows_parallel: asBoolean(habit.allows_parallel),
      abandonability: asNumber(habit.abandonability),
      fixed: asBoolean(habit.fixed),
      window_mode: (asString(habit.window_mode) as WindowMode) ?? undefined,
      steps,
    };

    let cancelled = false;
    setLoading(true);
    client
      .previewHabit(body)
      .then((result) => {
        if (!cancelled) setTasks(result);
      })
      .catch((e) => {
        if (!cancelled) {
          showError(e, 'プレビューの取得に失敗');
          setTasks([]);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [visible, client, habit, title]);

  return { tasks, loading };
}

interface TaskRowProps {
  task: HabitPreviewTask;
  index: number;
  colors: ColorSet;
}

function TaskRow({ task, index, colors }: TaskRowProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const start = parseIso(task.start_at);
  const end = parseIso(task.end_at);
  const durationMin =
    start && end
      ? Math.max(0, Math.round((end.getTime() - start.getTime()) / 60000))
      : 0;
  return (
    <View
      style={[
        styles.row,
        {
          backgroundColor: colors.surfaceTint,
          borderColor: colors.separator,
        },
      ]}
    >
      <Text style={[styles.index, { color: colors.brand }]}>{index + 1}</Text>
      <View style={styles.rowBody}>
        <Text style={[styles.title, { color: colors.black }]} numberOfLines={1}>
          {task.title}
        </Text>
        <Text style={[styles.time, { color: colors.gray }]} numberOfLines={1}>
          {start ? formatDateTime(start) : task.start_at} 〜{' '}
          {end ? formatTimeOnly(end) : task.end_at}
          {durationMin > 0 && ` · 所要 ${formatDuration(durationMin)}`}
        </Text>
      </View>
    </View>
  );
}

function parseIso(iso: string): Date | null {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? null : d;
}

function formatDateTime(d: Date): string {
  const WEEKDAYS = ['日', '月', '火', '水', '木', '金', '土'];
  const y = d.getFullYear();
  const mo = d.getMonth() + 1;
  const day = d.getDate();
  const wd = WEEKDAYS[d.getDay()];
  const h = String(d.getHours()).padStart(2, '0');
  const m = String(d.getMinutes()).padStart(2, '0');
  return `${y}/${mo}/${day} (${wd}) ${h}:${m}`;
}

function formatTimeOnly(d: Date): string {
  const h = String(d.getHours()).padStart(2, '0');
  const m = String(d.getMinutes()).padStart(2, '0');
  return `${h}:${m}`;
}
