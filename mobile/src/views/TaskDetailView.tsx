// TaskDetailView — view and edit a single task
// Elements from top to bottom:
//   title, status, time -> time (if not pending), parallel task, cost (avg, sigma),
//   abandonability (5-step slider), habit (if generated from habit),
//   description, parallel config, deps graph (related only)

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { useRouter, useLocalSearchParams } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import {
  Button,
  Checkbox,
  IconButton,
  List,
  Menu,
  Modal,
  Portal,
  TextInput as PaperTextInput,
  Divider,
} from 'react-native-paper';
import { Slider } from '@expo/ui/community/slider';
import { useServer } from '@/src/api/ServerProvider';
import { undoRedo } from '@/src/api/undoRedo';
import { showError, logError } from '@/src/api/errors';
import { parseDepends, parseSchedule } from '@/src/api/types';
import type {
  TaskRow,
  HabitDetail,
  ScheduleEntry,
  TaskStatus,
  RedundantDependency,
} from '@/src/api/types';
import { useColors, type ColorSet } from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { haptic } from '@/src/components/haptics';
import { TaskProgressSheet } from '@/src/components/TaskProgressSheet';
import { SplitTaskModal } from '@/src/components/SplitTaskModal';
import { CancelConfirmButton } from '@/src/components/CancelConfirmButton';
import { DeleteConfirmMenuItem } from '@/src/components/DeleteConfirmMenuItem';
import { RedundantDepWarning } from '@/src/components/RedundantDepWarning';
import { formatDate } from '@/src/formatDate';
import { parseDuration, formatDuration } from '@/src/utils/duration';
import {
  makeProgressOperationId,
  recordProgressWithTotal,
  type ProgressPayload,
} from '@/src/utils/progress';
import {
  DependencyGraph,
  type GraphNode,
  type GraphEdge,
} from '@/src/components/graph/DependencyGraph';
import {
  postInProgressNotification,
  dismissInProgressNotification,
  dismissTaskNotifications,
  cancelScheduledTaskNotifications,
  cancelScheduledStartNotifications,
} from '@/src/notifications';

const STATUS_LABELS: Record<TaskStatus, string> = {
  pending: '未スケジュール',
  scheduled: 'スケジュール済',
  in_progress: '進行中',
  completed: '完了',
  skipped: 'スキップ',
};

const STATUS_ICONS: Record<TaskStatus, keyof typeof Ionicons.glyphMap> = {
  pending: 'hourglass-outline',
  scheduled: 'calendar-outline',
  in_progress: 'play-circle-outline',
  completed: 'checkmark-circle-outline',
  skipped: 'play-skip-forward-outline',
};

function formatTime(iso?: string): string {
  if (!iso) return '';
  const d = new Date(iso);
  return `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()} ${d
    .getHours()
    .toString()
    .padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      flex: 1,
    },
    topBar: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingHorizontal: 4,
      paddingBottom: 4,
    },
    content: {
      flex: 1,
    },
    contentContainer: {
      padding: 16,
      gap: 16,
    },
    loading: {
      textAlign: 'center',
      marginTop: 40,
    },
    title: {
      fontSize: 24,
      fontWeight: '600',
    },
    titleInput: {
      fontSize: 20,
    },
    statusRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
    },
    statusText: {
      flex: 1,
      fontSize: 15,
      fontWeight: '500',
    },
    timeText: {
      fontSize: 14,
    },
    section: {
      gap: 4,
    },
    sectionLabel: {
      fontSize: 13,
      fontWeight: '500',
    },
    hint: {
      fontSize: 11,
      marginTop: 2,
      flex: 1,
    },
    sectionValue: {
      fontSize: 16,
    },
    sliderContainer: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
    },
    slider: {
      flex: 1,
    },
    sliderValue: {
      fontSize: 14,
      fontVariant: ['tabular-nums'],
    },
    habitLink: {
      fontSize: 16,
      color: colors.brand,
    },
    descriptionInput: {
      minHeight: 80,
    },
    depLink: {
      fontSize: 14,
      color: colors.brand,
      paddingVertical: 4,
    },
    depRow: {
      flexDirection: 'row',
      alignItems: 'center',
    },
    timeEditContainer: {
      gap: 8,
    },
    dateField: {
      flexDirection: 'row',
      alignItems: 'center',
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
      gap: 8,
    },
    dateText: {
      flex: 1,
      fontSize: 15,
    },
    costEditContainer: {
      flexDirection: 'row',
      gap: 12,
    },
    avgInputContainer: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
    },
    costInput: {
      flex: 1,
    },
    costHint: {
      fontSize: 11,
      marginTop: 2,
      marginLeft: 4,
    },
    toggleRow: {
      flexDirection: 'row',
      gap: 24,
    },
    toggleItem: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    toggleLabel: {
      fontSize: 14,
    },
    miniGraph: {
      marginTop: 12,
      paddingTop: 12,
      borderTopWidth: 1,
      gap: 4,
    },
    miniGraphLabel: {
      fontSize: 12,
      marginBottom: 4,
    },
    depHeader: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
    },
    depModal: {
      margin: 24,
      borderRadius: 12,
      padding: 16,
      maxHeight: '70%',
    },
    depModalTitle: {
      fontSize: 18,
      fontWeight: '600',
      marginBottom: 8,
    },
    depModalSearch: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      borderBottomWidth: 1,
      paddingBottom: 8,
      marginBottom: 8,
    },
    depModalSearchInput: {
      flex: 1,
      fontSize: 15,
    },
    depModalList: {
      maxHeight: 400,
    },
    depModalEmpty: {
      textAlign: 'center',
      paddingVertical: 24,
    },
    depModalClose: {
      marginTop: 8,
    },
    saveBar: {
      paddingHorizontal: 16,
      paddingTop: 8,
      borderTopWidth: 1,
      borderTopColor: colors.separator,
    },
    saveBarButton: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 8,
      paddingVertical: 14,
      borderRadius: 12,
    },
    saveBarText: {
      fontSize: 18,
      fontWeight: '700',
    },
    progressHeader: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      marginBottom: 8,
    },
    progressActionsRow: {
      flexDirection: 'row',
      gap: 8,
      marginBottom: 8,
    },
    progressButton: {
      flex: 1,
      height: 48,
      borderRadius: 10,
      alignItems: 'center',
      justifyContent: 'center',
      flexDirection: 'row',
      borderWidth: 1,
      borderColor: 'transparent',
    },
    progressButtonFlex: {
      flex: 1,
    },
    progressButtonFull: {
      marginBottom: 8,
    },
  });

export function TaskDetailView() {
  const { client, notifications } = useServer();
  const router = useRouter();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const { id } = useLocalSearchParams<{ id: string }>();
  const [task, setTask] = useState<TaskRow | null>(null);
  const [habit, setHabit] = useState<HabitDetail | null>(null);
  const [parallelTask, setParallelTask] = useState<TaskRow | null>(null);
  const [allTasks, setAllTasks] = useState<TaskRow[]>([]);
  const [editing, setEditing] = useState(false);
  // Ref mirror of `editing` so refresh() can skip overwriting unsaved edits
  // (matching HabitDetailView's pattern).
  const editingRef = useRef(false);
  editingRef.current = editing;
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [abandonability, setAbandonability] = useState(0.5);
  const [avgMinutes, setAvgMinutes] = useState('');
  const [sigmaMinutes, setSigmaMinutes] = useState('');
  const [quantityTotal, setQuantityTotal] = useState('');
  const [quantityUnit, setQuantityUnit] = useState('');
  const [startAt, setStartAt] = useState<Date | null>(null);
  const [endAt, setEndAt] = useState<Date | null>(null);
  const [parallelizable, setParallelizable] = useState(false);
  const [allowsParallel, setAllowsParallel] = useState(false);
  const [fixed, setFixed] = useState(false);
  const [deps, setDeps] = useState<string[]>([]);
  const [pickerField, setPickerField] = useState<'start' | 'end' | null>(null);
  const [statusMenuVisible, setStatusMenuVisible] = useState(false);
  const [depModalVisible, setDepModalVisible] = useState(false);
  const [depSearch, setDepSearch] = useState('');
  const [redundantEdges, setRedundantEdges] = useState<RedundantDependency[]>(
    [],
  );
  const [status, setStatus] = useState<TaskStatus>('pending');
  const [menuVisible, setMenuVisible] = useState(false);
  const [progressSheetVisible, setProgressSheetVisible] = useState(false);
  const [progressSheetMode, setProgressSheetMode] = useState<
    'record' | 'pause'
  >('record');
  const [splitModalVisible, setSplitModalVisible] = useState(false);
  // Double-tap detection ref — must be before the early return to satisfy
  // React's Rules of Hooks (hooks must be called unconditionally).
  const lastTapRef = useRef(0);
  const lastSectionRef = useRef('');

  const refresh = useCallback(async () => {
    if (!client || !id) return;
    let t: TaskRow;
    try {
      t = await client.getTask(id);
    } catch (e) {
      showError(e, 'タスクの取得に失敗');
      return;
    }
    // Don't clobber the user's in-progress edits.
    if (!editingRef.current) {
      setTask(t);
      setTitle(t.title);
      setDescription(t.description ?? '');
      setAbandonability(t.abandonability);
      setAvgMinutes(String(t.avg_minutes));
      setSigmaMinutes(t.sigma_minutes > 0 ? String(t.sigma_minutes) : '');
      setQuantityTotal(
        t.quantity_total !== undefined && t.quantity_total > 0
          ? String(t.quantity_total)
          : '',
      );
      setQuantityUnit(t.quantity_unit ?? '');
      setStartAt(t.start_at ? new Date(t.start_at) : null);
      setEndAt(new Date(t.end_at));
      setParallelizable(t.parallelizable);
      setAllowsParallel(t.allows_parallel);
      setFixed(t.fixed);
      setDeps(parseDepends(t.depends));
      setStatus(t.status);
    }
    if (t.habit_id) {
      try {
        setHabit(await client.getHabit(t.habit_id));
      } catch (e) {
        logError('ハビット取得', e);
        setHabit(null);
      }
    }

    // Load all tasks for deps editing and parallel task lookup
    try {
      const [tasks, sched, analysis] = await Promise.all([
        client.listTasks(),
        client.getSchedule().catch((e) => {
          logError('スケジュール取得', e);
          return null;
        }),
        client.analyzeTaskDependencies().catch((e) => {
          logError('依存分析取得', e);
          return null;
        }),
      ]);
      setAllTasks(tasks);
      if (analysis) {
        setRedundantEdges(
          analysis.redundant.filter((e) => e.from === id || e.to === id),
        );
      } else {
        setRedundantEdges([]);
      }
      const entries: ScheduleEntry[] = sched
        ? parseSchedule(sched.schedule)
        : [];
      const myEntry = entries.find((e) => e.task_id === id);
      if (myEntry) {
        const myStart = new Date(myEntry.start_at).getTime();
        const myEnd = new Date(myEntry.end_at).getTime();
        const isReceiver = t.allows_parallel;
        const isParallelizable = t.parallelizable;
        for (const other of tasks) {
          if (other.id === id) continue;
          if (other.status === 'completed' || other.status === 'skipped')
            continue;
          const isMatch = isReceiver
            ? other.parallelizable
            : isParallelizable
              ? other.allows_parallel
              : false;
          if (!isMatch) continue;
          const otherEntry = entries.find((e) => e.task_id === other.id);
          if (!otherEntry) continue;
          const otherStart = new Date(otherEntry.start_at).getTime();
          const otherEnd = new Date(otherEntry.end_at).getTime();
          if (otherStart < myEnd && otherEnd > myStart) {
            setParallelTask(other);
            break;
          }
        }
      }
    } catch (e) {
      logError('タスク一覧取得', e);
      setParallelTask(null);
    }
  }, [client, id]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  function toISO(d: Date): string {
    return d.toISOString();
  }

  async function save() {
    if (!client || !task) return;
    const updates: Record<string, unknown> = {};
    if (title !== task.title) updates.title = title;
    if (description !== (task.description ?? ''))
      updates.description = description;
    if (abandonability !== task.abandonability)
      updates.abandonability = abandonability;
    if (avgMinutes !== String(task.avg_minutes)) {
      const v = parseDuration(avgMinutes);
      if (v !== null && v > 0) updates.avg_minutes = v;
    }
    if (
      sigmaMinutes !==
      (task.sigma_minutes > 0 ? String(task.sigma_minutes) : '')
    ) {
      const v = parseDuration(sigmaMinutes);
      if (v !== null && v >= 0) updates.sigma_minutes = v;
    }
    if (quantityTotal !== String(task.quantity_total ?? '')) {
      const v = parseInt(quantityTotal, 10);
      if (!isNaN(v) && v > 0) updates.quantity_total = v;
      else updates.quantity_total = undefined;
    }
    if (quantityUnit !== (task.quantity_unit ?? '')) {
      updates.quantity_unit = quantityUnit.trim() || undefined;
    }
    const prevStart = task.start_at ? new Date(task.start_at) : null;
    if (startAt?.getTime() !== prevStart?.getTime()) {
      updates.start_at = startAt ? toISO(startAt) : null;
    }
    const prevEnd = new Date(task.end_at);
    if (endAt && endAt.getTime() !== prevEnd.getTime()) {
      updates.end_at = toISO(endAt);
    }
    if (parallelizable !== task.parallelizable)
      updates.parallelizable = parallelizable;
    if (allowsParallel !== task.allows_parallel)
      updates.allows_parallel = allowsParallel;
    if (fixed !== task.fixed) updates.fixed = fixed;
    if (status !== task.status) updates.status = status;
    const prevDeps = parseDepends(task.depends);
    if (JSON.stringify(deps) !== JSON.stringify(prevDeps)) {
      updates.depends = deps;
    }

    if (Object.keys(updates).length === 0) {
      editingRef.current = false;
      setEditing(false);
      return;
    }
    const prev = { ...task };
    try {
      await client.updateTask(task.id, updates);
    } catch (e) {
      showError(e, 'タスクの保存に失敗');
      return;
    }
    undoRedo.push({
      description: `edit task: ${task.title}`,
      undo: async () => {
        await client.updateTask(task.id, {
          title: prev.title,
          description: prev.description,
          abandonability: prev.abandonability,
          avg_minutes: prev.avg_minutes,
          sigma_minutes: prev.sigma_minutes,
          quantity_total: prev.quantity_total,
          quantity_unit: prev.quantity_unit,
          start_at: prev.start_at,
          end_at: prev.end_at,
          parallelizable: prev.parallelizable,
          allows_parallel: prev.allows_parallel,
          fixed: prev.fixed,
          status: prev.status,
          depends: prevDeps,
        });
        await refresh();
      },
      redo: async () => {
        await client.updateTask(task.id, updates);
        await refresh();
      },
    });
    editingRef.current = false;
    setEditing(false);
    await refresh();
  }

  // Resolve a redundant dependency edge by removing `toId` from the
  // `fromId` task's depends list (#355).
  async function resolveRedundantEdge(fromId: string, toId: string) {
    if (!client) return;
    const fromTask = allTasks.find((t) => t.id === fromId);
    if (!fromTask) return;
    const prevDeps = parseDepends(fromTask.depends);
    const newDeps = prevDeps.filter((d) => d !== toId);
    try {
      await client.updateTask(fromId, { depends: newDeps });
    } catch (e) {
      showError(e, '冗長な依存の削除に失敗');
      throw e;
    }
    undoRedo.push({
      description: `remove redundant dep: ${fromTask.title}`,
      undo: async () => {
        await client.updateTask(fromId, { depends: prevDeps });
        await refresh();
      },
      redo: async () => {
        await client.updateTask(fromId, { depends: newDeps });
        await refresh();
      },
    });
    await refresh();
  }

  async function changeStatus(newStatus: TaskStatus) {
    if (!client || !task) return;
    const prevStatus = task.status;
    setStatusMenuVisible(false);
    if (newStatus === prevStatus) return;

    // In edit mode, only update local state — persisted on Save
    if (editing) {
      setStatus(newStatus);
      return;
    }

    if (newStatus === 'in_progress') {
      await startTask();
      return;
    }
    if (newStatus === 'completed') {
      await completeTask();
      return;
    }
    if (newStatus === 'scheduled' && prevStatus === 'in_progress') {
      await pauseTask();
      return;
    }

    try {
      await client.updateTask(task.id, { status: newStatus });
    } catch (e) {
      showError(e, 'ステータス変更に失敗');
      return;
    }
    setStatus(newStatus);

    if (prevStatus === 'in_progress') {
      dismissInProgressNotification(task.id).catch((e) =>
        logError('通知の消去', e),
      );
    }
    if (newStatus === 'skipped') {
      dismissTaskNotifications(task.id).catch((e) => logError('通知の消去', e));
      cancelScheduledTaskNotifications(task.id).catch((e) =>
        logError('通知のキャンセル', e),
      );
    }

    undoRedo.push({
      description: `status → ${STATUS_LABELS[newStatus]}: ${task.title}`,
      undo: async () => {
        await client.updateTask(task.id, { status: prevStatus });
        await refresh();
      },
      redo: async () => {
        await client.updateTask(task.id, { status: newStatus });
        await refresh();
      },
    });
    await refresh();
  }

  async function revertToHabit() {
    setMenuVisible(false);
    if (!client || !task || !task.habit_id) return;
    const prev = { ...task };
    try {
      await client.updateTask(task.id, { user_edited: false });
    } catch (e) {
      showError(e, 'habitへの追従設定に失敗');
      return;
    }
    undoRedo.push({
      description: `revert to habit: ${task.title}`,
      undo: async () => {
        await client.updateTask(task.id, {
          title: prev.title,
          description: prev.description,
          avg_minutes: prev.avg_minutes,
          sigma_minutes: prev.sigma_minutes,
          start_at: prev.start_at,
          end_at: prev.end_at,
          parallelizable: prev.parallelizable,
          allows_parallel: prev.allows_parallel,
          abandonability: prev.abandonability,
          user_edited: true,
        });
        await refresh();
      },
      redo: async () => {
        await client.updateTask(task.id, { user_edited: false });
        await refresh();
      },
    });
    await refresh();
  }

  async function deleteTask() {
    setMenuVisible(false);
    if (!client || !task) return;
    let currentId = task.id;
    try {
      await client.deleteTask(currentId);
    } catch (e) {
      showError(e, 'タスクの削除に失敗');
      return;
    }
    undoRedo.push({
      description: `delete task: ${task.title}`,
      undo: async () => {
        const recreated = await client.createTask({
          title: task.title,
          description: task.description,
          start_at: task.start_at,
          end_at: task.end_at,
          avg_minutes: task.avg_minutes,
          sigma_minutes: task.sigma_minutes,
          depends: parseDepends(task.depends),
          parallelizable: task.parallelizable,
          allows_parallel: task.allows_parallel,
          abandonability: task.abandonability,
          habit_id: task.habit_id,
          fixed: task.fixed,
        });
        currentId = recreated.id;
        if (task.user_edited) {
          await client.updateTask(currentId, { user_edited: true });
        }
      },
      redo: async () => {
        await client.deleteTask(currentId);
      },
    });
    router.back();
  }

  // ── Task progress actions (#757) ──

  async function startTask() {
    if (!client || !task) return;
    const prevStatus = task.status;
    const operationId = makeProgressOperationId();
    try {
      await client.startTaskWork(task.id, operationId);
    } catch (e) {
      showError(e, 'タスクの開始に失敗');
      return;
    }
    cancelScheduledStartNotifications(task.id).catch((e) =>
      logError('通知のキャンセル', e),
    );
    dismissTaskNotifications(task.id).catch((e) => logError('通知の消去', e));
    if (notifications.inProgress) {
      postInProgressNotification({ ...task, status: 'in_progress' }).catch(
        (e) => logError('通知の投稿', e),
      );
    }
    await refresh();

    undoRedo.push({
      description: `start: ${task.title}`,
      undo: async () => {
        try {
          await client.pauseTaskWork(task.id, makeProgressOperationId());
        } catch (e) {
          showError(e, 'タスクの巻き戻しに失敗');
          return;
        }
        if (prevStatus !== 'scheduled') {
          try {
            await client.updateTask(task.id, { status: prevStatus });
          } catch (e) {
            showError(e, 'タスクの巻き戻しに失敗');
          }
        }
        await refresh();
      },
      redo: async () => {
        try {
          await client.startTaskWork(task.id, makeProgressOperationId());
        } catch (e) {
          showError(e, 'タスクの再開に失敗');
          return;
        }
        await refresh();
      },
    });
  }

  async function pauseTask(payload?: ProgressPayload) {
    if (!client || !task) return;
    const prevQuantityDone = task.quantity_done;
    const prevQuantityTotal = task.quantity_total;
    const recordOperationId = payload ? makeProgressOperationId() : undefined;
    const pauseOperationId = makeProgressOperationId();
    if (payload && recordOperationId) {
      try {
        await recordProgressWithTotal(client, task, payload, {
          operationId: recordOperationId,
        });
      } catch (e) {
        showError(e, '進捗の記録に失敗');
        return;
      }
    }
    try {
      await client.pauseTaskWork(task.id, pauseOperationId);
    } catch (e) {
      showError(e, 'タスクの一時停止に失敗');
      return;
    }
    dismissInProgressNotification(task.id).catch((e) =>
      logError('通知の消去', e),
    );
    await refresh();

    undoRedo.push({
      description: `pause: ${task.title}`,
      undo: async () => {
        try {
          await client.startTaskWork(task.id, makeProgressOperationId());
        } catch (e) {
          showError(e, 'タスクの巻き戻しに失敗');
          return;
        }
        if (payload) {
          try {
            await client.updateTask(task.id, {
              quantity_done: prevQuantityDone,
              quantity_total: prevQuantityTotal,
            });
          } catch (e) {
            showError(e, '数量の巻き戻しに失敗');
          }
        }
        await refresh();
      },
      redo: async () => {
        try {
          await client.pauseTaskWork(task.id, makeProgressOperationId());
        } catch (e) {
          showError(e, 'タスクの再停止に失敗');
          return;
        }
        await refresh();
      },
    });
  }

  async function completeTask() {
    if (!client || !task) return;
    const prevQuantityDone = task.quantity_done;
    const total = task.quantity_total;
    const operationId = makeProgressOperationId();
    try {
      await client.completeTaskWork(task.id, operationId);
    } catch (e) {
      showError(e, 'タスクの完了に失敗');
      return;
    }
    dismissTaskNotifications(task.id).catch((e) => logError('通知の消去', e));
    cancelScheduledTaskNotifications(task.id).catch((e) =>
      logError('通知のキャンセル', e),
    );
    await refresh();

    undoRedo.push({
      description: `complete: ${task.title}`,
      undo: async () => {
        try {
          await client.updateTask(task.id, {
            status: 'in_progress',
            quantity_done: prevQuantityDone,
          });
        } catch (e) {
          showError(e, 'タスクの巻き戻しに失敗');
          return;
        }
        await refresh();
      },
      redo: async () => {
        try {
          await client.updateTask(task.id, {
            status: 'completed',
            quantity_done: total ?? prevQuantityDone,
          });
        } catch (e) {
          showError(e, 'タスクの再完了に失敗');
          return;
        }
        await refresh();
      },
    });
  }

  async function recordProgress(payload: ProgressPayload) {
    if (!client || !task) return;
    try {
      await recordProgressWithTotal(client, task, payload);
    } catch (e) {
      showError(e, '進捗の記録に失敗');
      return;
    }
    await refresh();
  }

  async function adjustProgress(delta: number) {
    if (!task) return;
    const next = Math.max(0, (task.quantity_done ?? 0) + delta);
    await recordProgress({ quantityDone: next });
  }

  async function splitTask(payload: {
    retainedQuantity: number;
    setDependency: boolean;
    title?: string;
    description?: string;
    endAt?: string;
  }) {
    if (!client || !task) return;
    const operationId = makeProgressOperationId();
    try {
      await client.splitTask(
        task.id,
        {
          retained_quantity: payload.retainedQuantity,
          set_dependency: payload.setDependency,
          title: payload.title,
          description: payload.description,
          end_at: payload.endAt,
        },
        operationId,
      );
    } catch (e) {
      showError(e, 'タスクの分割に失敗');
      return;
    }
    await refresh();
  }

  function openProgressSheet(mode: 'record' | 'pause') {
    setProgressSheetMode(mode);
    setProgressSheetVisible(true);
  }

  function openSplitModal() {
    setSplitModalVisible(true);
  }

  async function handleProgressConfirm(payload: ProgressPayload) {
    if (progressSheetMode === 'pause') {
      await pauseTask(payload);
    } else {
      await recordProgress(payload);
    }
    setProgressSheetVisible(false);
  }

  async function handleProgressRecord(payload: ProgressPayload) {
    await recordProgress(payload);
    setProgressSheetVisible(false);
  }

  // Build the connected component dependency graph for the current task.
  // Traverses both forward (deps) and reverse (dependents) transitively,
  // following edges in both directions until no new nodes are discovered.
  // Must be BEFORE the early return to satisfy Rules of Hooks.
  const { detailGraphNodes, detailGraphEdges } = useMemo(() => {
    if (!task) return { detailGraphNodes: [], detailGraphEdges: [] };
    const taskMap = new Map(allTasks.map((t) => [t.id, t]));

    // Build bidirectional adjacency: for each task, store its forward deps
    // and reverse deps (tasks that depend on it).
    const forwardAdj = new Map<string, string[]>();
    const reverseAdj = new Map<string, string[]>();
    for (const t of allTasks) {
      const tDeps = parseDepends(t.depends);
      forwardAdj.set(t.id, tDeps);
      for (const depId of tDeps) {
        const rev = reverseAdj.get(depId) ?? [];
        rev.push(t.id);
        reverseAdj.set(depId, rev);
      }
    }

    // BFS from the current task, following both forward and reverse edges.
    const visited = new Set<string>();
    const queue: string[] = [task.id];
    const edges: GraphEdge[] = [];
    while (queue.length > 0) {
      const nodeId = queue.shift()!;
      if (visited.has(nodeId)) continue;
      visited.add(nodeId);
      // Enqueue forward deps
      for (const depId of forwardAdj.get(nodeId) ?? []) {
        edges.push({ source: depId, target: nodeId });
        if (!visited.has(depId)) queue.push(depId);
      }
      // Enqueue reverse deps
      for (const revId of reverseAdj.get(nodeId) ?? []) {
        if (!visited.has(revId)) queue.push(revId);
      }
    }

    // Build nodes in visitation order
    const nodes: GraphNode[] = [];
    for (const nodeId of visited) {
      const t = taskMap.get(nodeId);
      if (!t) continue;
      const isDone = t.status === 'completed' || t.status === 'skipped';
      nodes.push({
        id: t.id,
        label: t.title,
        color: isDone ? colors.done : colors.brand,
        x: 0,
        y: 0,
        vx: 0,
        vy: 0,
      });
    }

    return { detailGraphNodes: nodes, detailGraphEdges: edges };
  }, [allTasks, colors, task]);

  if (!task) {
    return (
      <View style={[styles.container, { backgroundColor: colors.white }]}>
        <Text style={[styles.loading, { color: colors.gray }]}>
          読み込み中...
        </Text>
      </View>
    );
  }

  const isPending = task.status === 'pending';
  const availableDeps = allTasks.filter(
    (t) => t.id !== task.id && !deps.includes(t.id),
  );

  // Double-tap (or single tap on a section) enters edit mode.
  function enterEdit() {
    if (!editing) {
      haptic.light();
      setEditing(true);
    }
  }
  function handleSectionTap(section: string) {
    const now = Date.now();
    if (now - lastTapRef.current < 300 && lastSectionRef.current === section) {
      enterEdit();
      lastTapRef.current = 0;
      lastSectionRef.current = '';
    } else {
      lastTapRef.current = now;
      lastSectionRef.current = section;
    }
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      {/* Top bar */}
      <View style={[styles.topBar, { paddingTop: 4 + insets.top }]}>
        <IconButton
          icon="chevron-left"
          iconColor={colors.brand}
          size={28}
          onPress={() => {
            haptic.light();
            router.back();
          }}
        />
        <View style={{ flex: 1 }} />
        {editing ? (
          <>
            <IconButton
              icon="check"
              iconColor={colors.white}
              containerColor={colors.brand}
              size={22}
              onPress={() => {
                haptic.medium();
                save();
              }}
            />
            <CancelConfirmButton
              onConfirm={() => {
                haptic.light();
                editingRef.current = false;
                refresh();
                setEditing(false);
              }}
            />
          </>
        ) : (
          <IconButton
            icon="pencil-outline"
            iconColor={colors.brand}
            size={22}
            onPress={() => {
              haptic.light();
              setEditing(true);
            }}
          />
        )}
        <Menu
          visible={menuVisible}
          onDismiss={() => setMenuVisible(false)}
          anchor={
            <IconButton
              icon="dots-vertical"
              iconColor={colors.brand}
              size={24}
              onPress={() => setMenuVisible(true)}
            />
          }
        >
          {task.habit_id && task.user_edited && (
            <Menu.Item
              onPress={revertToHabit}
              title="habitの設定に戻す"
              leadingIcon="restore"
            />
          )}
          <DeleteConfirmMenuItem onConfirm={deleteTask} visible={menuVisible} />
        </Menu>
      </View>

      <ScrollView
        style={styles.content}
        contentContainerStyle={[
          styles.contentContainer,
          { paddingBottom: 40 + insets.bottom },
        ]}
      >
        {/* Title */}
        {editing ? (
          <PaperTextInput
            mode="outlined"
            value={title}
            onChangeText={setTitle}
            label="タイトル"
            outlineColor={colors.separator}
            activeOutlineColor={colors.brand}
            style={styles.titleInput}
            contentStyle={{ fontSize: 20, fontWeight: '600' }}
          />
        ) : (
          <Pressable onPress={() => handleSectionTap('title')}>
            <Text style={[styles.title, { color: colors.black }]}>
              {task.title}
            </Text>
          </Pressable>
        )}

        {/* Status */}
        <View style={styles.section}>
          <Menu
            visible={statusMenuVisible}
            onDismiss={() => setStatusMenuVisible(false)}
            anchor={
              <Pressable
                style={[styles.statusRow, { borderColor: colors.separator }]}
                onPress={() => {
                  haptic.light();
                  setStatusMenuVisible(true);
                }}
              >
                <Ionicons
                  name={STATUS_ICONS[editing ? status : task.status]}
                  size={20}
                  color={colors.brand}
                />
                <Text style={[styles.statusText, { color: colors.black }]}>
                  {STATUS_LABELS[editing ? status : task.status]}
                </Text>
                <Ionicons name="chevron-down" size={16} color={colors.gray} />
              </Pressable>
            }
          >
            {(Object.keys(STATUS_LABELS) as TaskStatus[])
              .filter((s) => {
                // pending (未スケジュール) のタスクは done/skip のみ変更可能。
                // scheduled/in_progress への手動変更はスケジューラの役割。
                if (task.status === 'pending') {
                  return s === 'completed' || s === 'skipped';
                }
                return true;
              })
              .map((s) => (
                <Menu.Item
                  key={s}
                  onPress={() => {
                    haptic.medium();
                    changeStatus(s);
                  }}
                  title={STATUS_LABELS[s]}
                  leadingIcon={({ color, size }) => (
                    <Ionicons
                      name={STATUS_ICONS[s]}
                      color={color}
                      size={size}
                    />
                  )}
                />
              ))}
          </Menu>
        </View>

        {/* Progress */}
        {!editing &&
          task.status === 'in_progress' &&
          task.quantity_total !== undefined &&
          task.quantity_total > 0 && (
            <View style={styles.section}>
              <View style={styles.progressHeader}>
                <Text style={[styles.sectionLabel, { color: colors.gray }]}>
                  進捗
                </Text>
                <Text style={{ color: colors.black, fontWeight: '600' }}>
                  {task.quantity_done}/{task.quantity_total}
                </Text>
              </View>
              <View style={styles.progressActionsRow}>
                <Pressable
                  style={({ pressed }) => [
                    styles.progressButton,
                    { backgroundColor: colors.red, opacity: pressed ? 0.8 : 1 },
                  ]}
                  onPress={() => openProgressSheet('pause')}
                >
                  <Ionicons name="pause" size={20} color={colors.white} />
                </Pressable>
                <Pressable
                  style={({ pressed }) => [
                    styles.progressButton,
                    {
                      backgroundColor: colors.green,
                      opacity: pressed ? 0.8 : 1,
                    },
                  ]}
                  onPress={completeTask}
                >
                  <Ionicons name="checkmark" size={24} color={colors.white} />
                </Pressable>
              </View>
              <Pressable
                style={({ pressed }) => [
                  styles.progressButton,
                  styles.progressButtonFull,
                  { borderColor: colors.separator, opacity: pressed ? 0.8 : 1 },
                ]}
                onPress={openSplitModal}
              >
                <Ionicons name="cut" size={20} color={colors.black} />
                <Text style={{ color: colors.black, marginLeft: 6 }}>分割</Text>
              </Pressable>
              <View style={styles.progressActionsRow}>
                <Pressable
                  style={({ pressed }) => [
                    styles.progressButton,
                    styles.progressButtonFlex,
                    {
                      borderColor: colors.separator,
                      opacity: pressed ? 0.8 : 1,
                    },
                  ]}
                  onPress={() => adjustProgress(-1)}
                >
                  <Text style={{ color: colors.black, fontWeight: '600' }}>
                    -1
                  </Text>
                </Pressable>
                <Pressable
                  style={({ pressed }) => [
                    styles.progressButton,
                    styles.progressButtonFlex,
                    {
                      backgroundColor: colors.brand,
                      opacity: pressed ? 0.8 : 1,
                    },
                  ]}
                  onPress={() => openProgressSheet('record')}
                >
                  <Text style={{ color: colors.white, fontWeight: '600' }}>
                    記録
                  </Text>
                </Pressable>
                <Pressable
                  style={({ pressed }) => [
                    styles.progressButton,
                    styles.progressButtonFlex,
                    {
                      borderColor: colors.separator,
                      opacity: pressed ? 0.8 : 1,
                    },
                  ]}
                  onPress={() => adjustProgress(1)}
                >
                  <Text style={{ color: colors.black, fontWeight: '600' }}>
                    +1
                  </Text>
                </Pressable>
              </View>
            </View>
          )}

        {/* Time */}
        {!isPending && (
          <View style={styles.section}>
            <Text style={[styles.sectionLabel, { color: colors.gray }]}>
              時間
            </Text>
            {editing ? (
              <View style={styles.timeEditContainer}>
                <Pressable
                  style={[styles.dateField, { borderColor: colors.separator }]}
                  onPress={() => {
                    haptic.select();
                    setPickerField('start');
                  }}
                >
                  <Ionicons
                    name="calendar-outline"
                    size={18}
                    color={colors.brand}
                  />
                  <Text
                    style={[
                      styles.dateText,
                      { color: startAt ? colors.black : colors.grayLight },
                    ]}
                  >
                    {formatDate(startAt)}
                  </Text>
                  {startAt && (
                    <Pressable
                      onPress={() => {
                        haptic.light();
                        setStartAt(null);
                      }}
                    >
                      <Ionicons
                        name="close-circle"
                        size={16}
                        color={colors.grayLight}
                      />
                    </Pressable>
                  )}
                </Pressable>
                <Pressable
                  style={[styles.dateField, { borderColor: colors.separator }]}
                  onPress={() => {
                    haptic.select();
                    setPickerField('end');
                  }}
                >
                  <Ionicons
                    name="calendar-outline"
                    size={18}
                    color={colors.brand}
                  />
                  <Text
                    style={[
                      styles.dateText,
                      { color: endAt ? colors.black : colors.grayLight },
                    ]}
                  >
                    {formatDate(endAt)}
                  </Text>
                </Pressable>
              </View>
            ) : (
              <Pressable onPress={() => handleSectionTap('time')}>
                <Text style={[styles.timeText, { color: colors.gray }]}>
                  {formatTime(task.start_at)} → {formatTime(task.end_at)}
                </Text>
              </Pressable>
            )}
          </View>
        )}

        {/* Parallel task */}
        {(task.allows_parallel || task.parallelizable) && (
          <View style={styles.section}>
            <Text style={[styles.sectionLabel, { color: colors.gray }]}>
              並列タスク
            </Text>
            {parallelTask ? (
              <Pressable
                onPress={() => {
                  haptic.light();
                  router.push(`/task/${parallelTask.id}`);
                }}
              >
                <Text style={styles.habitLink}>{parallelTask.title} ›</Text>
              </Pressable>
            ) : (
              <Text style={[styles.sectionValue, { color: colors.black }]}>
                {task.allows_parallel
                  ? '受け皿タスク (重なるタスクなし)'
                  : 'なし'}
              </Text>
            )}
          </View>
        )}

        {/* Cost */}
        <View style={styles.section}>
          <Text style={[styles.sectionLabel, { color: colors.gray }]}>
            コスト
          </Text>
          {editing ? (
            <View style={styles.costEditContainer}>
              <View style={styles.avgInputContainer}>
                <PaperTextInput
                  mode="outlined"
                  label="avg (1h30m / 90m / 90)"
                  value={avgMinutes}
                  onChangeText={setAvgMinutes}
                  autoCapitalize="none"
                  autoCorrect={false}
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  style={styles.costInput}
                  dense
                />
                <IconButton
                  icon="arrow-expand"
                  size={18}
                  iconColor={colors.brand}
                  disabled={!startAt || !endAt}
                  onPress={() => {
                    if (!startAt || !endAt) return;
                    haptic.light();
                    const diffMin = Math.max(
                      1,
                      Math.round((endAt.getTime() - startAt.getTime()) / 60000),
                    );
                    setAvgMinutes(String(diffMin));
                  }}
                />
              </View>
              <View style={styles.costInput}>
                <PaperTextInput
                  mode="outlined"
                  label="sigma (1h30m / 90m / 90)"
                  value={sigmaMinutes}
                  onChangeText={setSigmaMinutes}
                  autoCapitalize="none"
                  autoCorrect={false}
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  dense
                />
                {sigmaMinutes === '' && (
                  <Text style={[styles.costHint, { color: colors.grayLight }]}>
                    {task.sigma_minutes}m
                  </Text>
                )}
              </View>
            </View>
          ) : (
            <Pressable onPress={() => handleSectionTap('cost')}>
              <Text style={[styles.sectionValue, { color: colors.black }]}>
                avg: {task.avg_minutes}m, sigma:{' '}
                {task.sigma_minutes > 0 ? (
                  `${task.sigma_minutes}m`
                ) : (
                  <Text style={{ color: colors.grayLight }}>0m</Text>
                )}
                {task.actual_minutes !== undefined &&
                task.actual_minutes > 0 ? (
                  <Text style={{ color: colors.gray }}>
                    {' '}
                    (実績: {formatDuration(task.actual_minutes)})
                  </Text>
                ) : null}
              </Text>
            </Pressable>
          )}
        </View>

        {/* Quantity */}
        <View style={styles.section}>
          <Text style={[styles.sectionLabel, { color: colors.gray }]}>
            数量
          </Text>
          {editing ? (
            <View style={styles.costEditContainer}>
              <View style={styles.costInput}>
                <PaperTextInput
                  mode="outlined"
                  label="全体"
                  value={quantityTotal}
                  onChangeText={(text) =>
                    setQuantityTotal(text.replace(/[^0-9]/g, ''))
                  }
                  keyboardType="number-pad"
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  dense
                />
              </View>
              <View style={styles.costInput}>
                <PaperTextInput
                  mode="outlined"
                  label="単位"
                  value={quantityUnit}
                  onChangeText={setQuantityUnit}
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  dense
                />
              </View>
            </View>
          ) : (
            <Pressable onPress={() => handleSectionTap('quantity')}>
              <Text style={[styles.sectionValue, { color: colors.black }]}>
                {task.quantity_total !== undefined
                  ? `${task.quantity_total} ${task.quantity_unit ?? ''}`
                  : '未設定'}
              </Text>
            </Pressable>
          )}
        </View>

        {/* Abandonability */}
        <View style={styles.section}>
          <Text style={[styles.sectionLabel, { color: colors.gray }]}>
            abandonability
          </Text>
          {editing ? (
            <View style={styles.sliderContainer}>
              <Slider
                value={abandonability}
                onValueChange={setAbandonability}
                minimumValue={0}
                maximumValue={1}
                step={0.25}
                minimumTrackTintColor={colors.brand}
                style={styles.slider}
              />
              <Text style={[styles.sliderValue, { color: colors.brand }]}>
                {abandonability.toFixed(2)}
              </Text>
            </View>
          ) : (
            <Pressable onPress={() => handleSectionTap('abandonability')}>
              <Text style={[styles.sectionValue, { color: colors.black }]}>
                {task.abandonability.toFixed(2)}
              </Text>
            </Pressable>
          )}
        </View>

        {/* Habit */}
        {habit && (
          <Pressable
            style={styles.section}
            onPress={() => {
              haptic.light();
              router.push(`/habit/${habit.id}`);
            }}
          >
            <Text style={[styles.sectionLabel, { color: colors.gray }]}>
              Habit
            </Text>
            <Text style={styles.habitLink}>
              {task.habit_step_id
                ? (() => {
                    const step = habit.steps.find(
                      (s) => s.id === task.habit_step_id,
                    );
                    return step
                      ? `${habit.title} › ${step.title}`
                      : `${habit.title} ›`;
                  })()
                : `${habit.title} ›`}
            </Text>
          </Pressable>
        )}

        {/* Description */}
        <View style={styles.section}>
          <Text style={[styles.sectionLabel, { color: colors.gray }]}>
            説明
          </Text>
          {editing ? (
            <PaperTextInput
              mode="outlined"
              value={description}
              onChangeText={setDescription}
              multiline
              numberOfLines={4}
              outlineColor={colors.separator}
              activeOutlineColor={colors.brand}
              style={styles.descriptionInput}
            />
          ) : (
            <Pressable onPress={() => handleSectionTap('description')}>
              <Text style={[styles.sectionValue, { color: colors.black }]}>
                {task.description || '(なし)'}
              </Text>
            </Pressable>
          )}
        </View>

        {/* Parallel config */}
        <View style={styles.section}>
          <Text style={[styles.sectionLabel, { color: colors.gray }]}>
            並列設定
          </Text>
          {editing ? (
            <View style={styles.toggleRow}>
              <Pressable
                style={styles.toggleItem}
                onPress={() => {
                  haptic.select();
                  setParallelizable(!parallelizable);
                }}
              >
                <Text style={[styles.toggleLabel, { color: colors.black }]}>
                  並列実行可能
                </Text>
                <Checkbox
                  status={parallelizable ? 'checked' : 'unchecked'}
                  onPress={() => {
                    haptic.select();
                    setParallelizable(!parallelizable);
                  }}
                  color={colors.brand}
                />
              </Pressable>
              <Pressable
                style={styles.toggleItem}
                onPress={() => {
                  haptic.select();
                  setAllowsParallel(!allowsParallel);
                }}
              >
                <Text style={[styles.toggleLabel, { color: colors.black }]}>
                  並列受け入れ
                </Text>
                <Checkbox
                  status={allowsParallel ? 'checked' : 'unchecked'}
                  onPress={() => {
                    haptic.select();
                    setAllowsParallel(!allowsParallel);
                  }}
                  color={colors.brand}
                />
              </Pressable>
            </View>
          ) : (
            <Pressable
              style={styles.toggleRow}
              onPress={() => handleSectionTap('parallel')}
            >
              <View style={styles.toggleItem}>
                <Text style={[styles.toggleLabel, { color: colors.black }]}>
                  並列実行可能
                </Text>
                <Checkbox
                  status={task.parallelizable ? 'checked' : 'unchecked'}
                  disabled
                  color={colors.brand}
                />
              </View>
              <View style={styles.toggleItem}>
                <Text style={[styles.toggleLabel, { color: colors.black }]}>
                  並列受け入れ
                </Text>
                <Checkbox
                  status={task.allows_parallel ? 'checked' : 'unchecked'}
                  disabled
                  color={colors.brand}
                />
              </View>
            </Pressable>
          )}
        </View>

        {/* Fixed */}
        <View style={styles.section}>
          <Text style={[styles.sectionLabel, { color: colors.gray }]}>
            時間固定
          </Text>
          {editing ? (
            <View style={styles.toggleItem}>
              <Checkbox
                status={fixed ? 'checked' : 'unchecked'}
                onPress={() => {
                  haptic.select();
                  setFixed(!fixed);
                }}
                color={colors.brand}
              />
              <Text style={[styles.hint, { color: colors.grayLight }]}>
                開始時刻を固定し、スケジューラの移動を許可しない
              </Text>
            </View>
          ) : (
            <Pressable
              style={styles.toggleItem}
              onPress={() => handleSectionTap('fixed')}
            >
              <Checkbox
                status={task.fixed ? 'checked' : 'unchecked'}
                disabled
                color={colors.brand}
              />
            </Pressable>
          )}
        </View>

        {/* Deps — editable list + mini graph */}
        <View style={styles.section}>
          <View style={styles.depHeader}>
            <Text style={[styles.sectionLabel, { color: colors.gray }]}>
              依存 ({deps.length})
            </Text>
            {editing && (
              <Button
                mode="text"
                compact
                onPress={() => {
                  haptic.light();
                  setDepSearch('');
                  setDepModalVisible(true);
                }}
                textColor={colors.brand}
              >
                + 追加
              </Button>
            )}
          </View>
          {!editing && (
            <RedundantDepWarning
              edges={redundantEdges}
              onResolve={resolveRedundantEdge}
              nodeLabel={(nid, ntitle) => {
                const nt = allTasks.find((t) => t.id === nid);
                return nt ? `#${nt.display_id} ${nt.title}` : ntitle;
              }}
            />
          )}
          {deps.length > 0 ? (
            deps.map((depId) => {
              const depTask = allTasks.find((t) => t.id === depId);
              return (
                <View key={depId} style={styles.depRow}>
                  <Pressable
                    style={{ flex: 1 }}
                    onPress={() => {
                      if (!editing) {
                        haptic.light();
                        router.push(`/task/${depId}`);
                      }
                    }}
                  >
                    <Text style={styles.depLink}>
                      •{' '}
                      {depTask
                        ? `#${depTask.display_id} ${depTask.title}`
                        : depId.slice(0, 8) + '...'}{' '}
                      ›
                    </Text>
                  </Pressable>
                  {editing && (
                    <IconButton
                      icon="close"
                      size={18}
                      iconColor={colors.red}
                      onPress={() => {
                        haptic.light();
                        setDeps(deps.filter((d) => d !== depId));
                      }}
                    />
                  )}
                </View>
              );
            })
          ) : (
            <Text style={[styles.sectionValue, { color: colors.black }]}>
              (なし)
            </Text>
          )}

          {/* Dependency graph: connected component around this task */}
          {detailGraphNodes.length > 1 && (
            <View
              style={[styles.miniGraph, { borderTopColor: colors.separator }]}
            >
              <Text style={[styles.miniGraphLabel, { color: colors.gray }]}>
                依存グラフ
              </Text>
              <DependencyGraph
                nodes={detailGraphNodes}
                edges={detailGraphEdges}
                highlightTaskId={task.id}
                height={240}
                onTapNode={(tappedId) => {
                  if (!editing) {
                    haptic.light();
                    router.push(`/task/${tappedId}`);
                  }
                }}
              />
            </View>
          )}
        </View>
      </ScrollView>

      {/* Big save button — visible only in edit mode */}
      {editing && (
        <View
          style={[
            styles.saveBar,
            {
              paddingBottom: 8 + insets.bottom,
              backgroundColor: colors.white,
              borderTopColor: colors.separator,
            },
          ]}
        >
          <Pressable
            style={[styles.saveBarButton, { backgroundColor: colors.brand }]}
            onPress={() => {
              haptic.medium();
              save();
            }}
          >
            <Ionicons name="checkmark-circle" size={22} color={colors.white} />
            <Text style={[styles.saveBarText, { color: colors.white }]}>
              保存
            </Text>
          </Pressable>
        </View>
      )}

      {/* DateTime Picker Modals */}
      <DateTimePickerModal
        visible={pickerField === 'start'}
        value={startAt}
        mode="datetime"
        label="開始日時"
        optional
        onConfirm={(date) => {
          setStartAt(date);
          setPickerField(null);
        }}
        onCancel={() => setPickerField(null)}
      />
      <DateTimePickerModal
        visible={pickerField === 'end'}
        value={endAt}
        mode="datetime"
        label="期限日時"
        minimumDate={startAt ?? undefined}
        shortcuts={[
          {
            label: '1時間後',
            compute: () => new Date(Date.now() + 60 * 60 * 1000),
          },
          {
            label: '今日23:59',
            compute: () => {
              const d = new Date();
              d.setHours(23, 59, 0, 0);
              return d;
            },
          },
          {
            label: '明日',
            compute: () => {
              const d = new Date();
              d.setDate(d.getDate() + 1);
              d.setHours(23, 59, 0, 0);
              return d;
            },
          },
          {
            label: '明後日',
            compute: () => {
              const d = new Date();
              d.setDate(d.getDate() + 2);
              d.setHours(23, 59, 0, 0);
              return d;
            },
          },
          {
            label: '1週間後',
            compute: () => {
              const d = new Date();
              d.setDate(d.getDate() + 7);
              d.setHours(23, 59, 0, 0);
              return d;
            },
          },
        ]}
        onConfirm={(date) => {
          setEndAt(date);
          setPickerField(null);
        }}
        onCancel={() => setPickerField(null)}
      />

      {/* Progress sheet */}
      {task && (
        <TaskProgressSheet
          visible={progressSheetVisible}
          task={task}
          mode={progressSheetMode}
          onConfirm={handleProgressConfirm}
          onRecord={
            progressSheetMode === 'pause' ? handleProgressRecord : undefined
          }
          onCancel={() => setProgressSheetVisible(false)}
        />
      )}

      {/* Split modal */}
      {task && (
        <SplitTaskModal
          visible={splitModalVisible}
          task={task}
          onConfirm={splitTask}
          onCancel={() => setSplitModalVisible(false)}
        />
      )}

      {/* Dep selection modal (Paper) */}
      <Portal>
        <Modal
          visible={depModalVisible}
          onDismiss={() => setDepModalVisible(false)}
          contentContainerStyle={[
            styles.depModal,
            { backgroundColor: colors.white },
          ]}
        >
          <Text style={[styles.depModalTitle, { color: colors.black }]}>
            依存先を選択
          </Text>
          <View
            style={[
              styles.depModalSearch,
              { borderBottomColor: colors.separator },
            ]}
          >
            <Ionicons name="search" size={18} color={colors.gray} />
            <PaperTextInput
              mode="outlined"
              value={depSearch}
              onChangeText={setDepSearch}
              placeholder="タイトルで検索"
              placeholderTextColor={colors.grayLight}
              outlineColor={colors.separator}
              activeOutlineColor={colors.brand}
              style={styles.depModalSearchInput}
              dense
              autoFocus
            />
            {depSearch.length > 0 && (
              <Pressable
                onPress={() => {
                  haptic.light();
                  setDepSearch('');
                }}
              >
                <Ionicons
                  name="close-circle"
                  size={18}
                  color={colors.grayLight}
                />
              </Pressable>
            )}
          </View>
          <ScrollView style={styles.depModalList}>
            {availableDeps.length === 0 ? (
              <Text style={[styles.depModalEmpty, { color: colors.gray }]}>
                追加可能なタスクがありません
              </Text>
            ) : (
              availableDeps
                .filter((t) =>
                  depSearch.length === 0
                    ? true
                    : t.title.toLowerCase().includes(depSearch.toLowerCase()),
                )
                .map((t) => (
                  <List.Item
                    key={t.id}
                    title={t.title}
                    description={`#${t.display_id}${t.status !== 'pending' ? ' · ' + STATUS_LABELS[t.status] : ''}`}
                    onPress={() => {
                      haptic.medium();
                      setDeps([...deps, t.id]);
                      setDepModalVisible(false);
                    }}
                    left={() => (
                      <List.Icon
                        icon={STATUS_ICONS[t.status] as string}
                        color={colors.brand}
                      />
                    )}
                  />
                ))
            )}
          </ScrollView>
          <Divider />
          <Button
            mode="text"
            onPress={() => {
              haptic.light();
              setDepModalVisible(false);
            }}
            textColor={colors.brand}
            style={styles.depModalClose}
          >
            閉じる
          </Button>
        </Modal>
      </Portal>
    </View>
  );
}
