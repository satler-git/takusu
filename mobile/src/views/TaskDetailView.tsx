// TaskDetailView — view and edit a single task (#1146 redesign).
// Layout from top to bottom:
//   header card (status + parallel-guest chips, title, schedule strip with a
//   now marker and remaining/until chip), state-dependent actions (a single
//   primary button), stats grid (cost / quantity + pace marker / flags &
//   metadata incl. abandonability), deps chips + embedded dependency graph,
//   habit link, collapsible description. Editing happens inline in place.

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
  WorkSessionRow,
  CommentRow,
  CommentAuthor,
} from '@/src/api/types';
import {
  useColors,
  useTheme,
  taskCardColor,
  filledPips,
  type ColorSet,
} from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { haptic } from '@/src/components/haptics';
import { PressableScale } from '@/src/components/PressableScale';
import { TaskProgressSheet } from '@/src/components/TaskProgressSheet';
import { SplitTaskModal } from '@/src/components/SplitTaskModal';
import { CancelConfirmButton } from '@/src/components/CancelConfirmButton';
import { DeleteConfirmMenuItem } from '@/src/components/DeleteConfirmMenuItem';
import { RedundantDepWarning } from '@/src/components/RedundantDepWarning';
import { formatDate } from '@/src/formatDate';
import { parseDuration, formatDuration } from '@/src/utils/duration';
import {
  makeProgressOperationId,
  makeCommentOperationId,
  recordProgressWithTotal,
  findOpenWorkSessionForTask,
  completeTaskWithOptionalWorkSession,
  restoreTaskAfterCompletion,
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

const AUTHOR_LABELS: Record<CommentAuthor, string> = {
  user: 'あなた',
  agent: 'エージェント',
  system: 'システム',
};

const WEEKDAY_JA = ['日', '月', '火', '水', '木', '金', '土'];

// "8/1 (金)" — compact date label for the schedule strip.
function formatStripDate(iso: string): string {
  const d = new Date(iso);
  return `${d.getMonth() + 1}/${d.getDate()} (${WEEKDAY_JA[d.getDay()]})`;
}

// "09:05" — HH:MM for the big time display.
function hm(iso?: string | null): string {
  if (!iso) return '';
  const d = new Date(iso);
  return `${d.getHours().toString().padStart(2, '0')}:${d
    .getMinutes()
    .toString()
    .padStart(2, '0')}`;
}

// Human-readable absolute minute span (e.g. 25 → "25分", 90 → "1h30m").
function relMinutes(ms: number): string {
  const mins = Math.max(0, Math.round(Math.abs(ms) / 60000));
  if (mins < 60) return `${mins}分`;
  return formatDuration(mins);
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
    // ── redesign (#1146) ──
    topBarId: {
      fontSize: 12,
      fontWeight: '700',
      fontVariant: ['tabular-nums'],
      marginStart: 2,
    },
    headerCard: {
      marginHorizontal: 12,
      marginTop: 6,
      borderRadius: 18,
      paddingHorizontal: 16,
      paddingTop: 14,
      paddingBottom: 16,
      overflow: 'hidden',
    },
    headerChips: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 6,
      marginBottom: 10,
    },
    tintChip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 5,
      paddingHorizontal: 10,
      paddingVertical: 4,
      borderRadius: 999,
    },
    tintChipText: {
      fontSize: 12,
      fontWeight: '700',
    },
    headerTitle: {
      fontSize: 22,
      fontWeight: '800',
      lineHeight: 28,
    },
    headerTitleInput: {
      fontSize: 18,
    },
    stripDate: {
      fontSize: 12,
      fontWeight: '700',
      marginBottom: 2,
      fontVariant: ['tabular-nums'],
    },
    timesRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    tBig: {
      fontSize: 27,
      fontWeight: '800',
      fontVariant: ['tabular-nums'],
      letterSpacing: -0.5,
    },
    tArrow: {
      fontSize: 15,
    },
    headChipRight: {
      marginStart: 'auto',
    },
    track: {
      height: 6,
      borderRadius: 3,
      marginTop: 10,
    },
    fill: {
      position: 'absolute',
      start: 0,
      top: 0,
      bottom: 0,
      borderRadius: 3,
    },
    nowdot: {
      position: 'absolute',
      top: -3.5,
      width: 13,
      height: 13,
      borderRadius: 7,
      borderWidth: 3,
    },
    pendingNote: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      marginTop: 12,
      paddingHorizontal: 12,
      paddingVertical: 10,
      borderRadius: 10,
      borderWidth: 1.5,
      borderStyle: 'dashed',
    },
    pendingNoteText: {
      flex: 1,
      fontSize: 13,
      fontWeight: '600',
    },
    timeFields: {
      flexDirection: 'row',
      gap: 8,
      marginTop: 12,
    },
    timeField: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      borderRadius: 10,
      paddingHorizontal: 11,
      paddingVertical: 8,
      borderWidth: 1.5,
    },
    timeFieldLabels: {
      gap: 1,
    },
    timeFieldLabel: {
      fontSize: 9,
      fontWeight: '800',
      letterSpacing: 0.5,
    },
    timeFieldValue: {
      fontSize: 13,
      fontWeight: '800',
      fontVariant: ['tabular-nums'],
    },
    slabel: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      marginTop: 18,
      marginBottom: 7,
      marginHorizontal: 12,
    },
    slabelText: {
      flex: 1,
      fontSize: 11,
      fontWeight: '800',
      letterSpacing: 1,
    },
    actions: {
      marginHorizontal: 12,
      gap: 8,
    },
    abtn: {
      height: 52,
      borderRadius: 14,
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 9,
    },
    abtnText: {
      fontSize: 17,
      fontWeight: '800',
    },
    actRow: {
      flexDirection: 'row',
      gap: 8,
    },
    hbtn: {
      flex: 1,
      height: 46,
      borderRadius: 12,
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 6,
      borderWidth: 1.5,
    },
    hbtnText: {
      fontSize: 14,
      fontWeight: '700',
    },
    hbtnSq: {
      flex: 0,
      width: 52,
      height: 46,
      borderRadius: 12,
      alignItems: 'center',
      justifyContent: 'center',
      borderWidth: 1.5,
    },
    hbtnSqText: {
      fontSize: 17,
      fontWeight: '700',
      fontVariant: ['tabular-nums'],
    },
    actMeta: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
    },
    linkBtnText: {
      fontSize: 13,
      fontWeight: '700',
    },
    doneBanner: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      borderRadius: 14,
      paddingHorizontal: 14,
      paddingVertical: 12,
      borderWidth: 1,
    },
    doneBannerTitle: {
      fontSize: 14,
      fontWeight: '700',
    },
    doneBannerSub: {
      fontSize: 11,
      fontWeight: '600',
    },
    statsRow: {
      flexDirection: 'row',
      gap: 8,
      marginHorizontal: 12,
      marginBottom: 8,
    },
    cell: {
      flex: 1,
      borderRadius: 13,
      borderWidth: 1,
      paddingHorizontal: 12,
      paddingVertical: 10,
    },
    cellWide: {
      marginHorizontal: 12,
      marginBottom: 8,
      borderRadius: 13,
      borderWidth: 1,
      paddingHorizontal: 12,
      paddingVertical: 10,
    },
    cellK: {
      fontSize: 10,
      fontWeight: '800',
      letterSpacing: 1,
    },
    cellV: {
      fontSize: 16,
      fontWeight: '800',
      marginTop: 3,
      fontVariant: ['tabular-nums'],
    },
    cellVUnit: {
      fontSize: 11,
      fontWeight: '600',
    },
    cellSub: {
      fontSize: 11,
      fontWeight: '600',
      marginTop: 2,
    },
    minibar: {
      height: 6,
      borderRadius: 3,
      marginTop: 8,
    },
    minibarFill: {
      position: 'absolute',
      start: 0,
      top: 0,
      bottom: 0,
      borderRadius: 3,
    },
    paceMarker: {
      position: 'absolute',
      top: -4,
      bottom: -4,
      width: 2,
      borderRadius: 1,
    },
    flagrow: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 6,
      marginTop: 5,
    },
    flagChip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 4,
      paddingHorizontal: 9,
      paddingVertical: 4,
      borderRadius: 999,
      borderWidth: 1,
    },
    flagChipText: {
      fontSize: 11,
      fontWeight: '700',
    },
    pips: {
      flexDirection: 'row',
      gap: 3,
      marginStart: 4,
    },
    pip: {
      width: 9,
      height: 9,
      borderRadius: 3,
    },
    costInputs: {
      flexDirection: 'row',
      gap: 6,
      marginTop: 7,
    },
    flagEditor: {
      gap: 9,
      marginTop: 8,
    },
    tog: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 9,
    },
    togLabel: {
      fontSize: 13,
      fontWeight: '600',
    },
    abandonEdit: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 9,
    },
    abandonLabel: {
      fontSize: 12,
      fontWeight: '600',
    },
    abandonSlider: {
      flex: 1,
    },
    abandonVal: {
      fontSize: 13,
      fontWeight: '800',
      fontVariant: ['tabular-nums'],
    },
    habitLinkRow: {
      marginHorizontal: 12,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
    },
    habitLinkText: {
      flex: 1,
      fontSize: 15,
      fontWeight: '600',
    },
    depChips: {
      marginHorizontal: 12,
      gap: 6,
    },
    depChip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 9,
      borderRadius: 11,
      borderWidth: 1,
      paddingHorizontal: 12,
      paddingVertical: 9,
    },
    depChipNo: {
      fontSize: 12,
      fontWeight: '700',
      fontVariant: ['tabular-nums'],
    },
    depChipTitle: {
      flex: 1,
      fontSize: 13,
      fontWeight: '600',
    },
    addDep: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 6,
      borderRadius: 11,
      borderWidth: 1.5,
      borderStyle: 'dashed',
      paddingVertical: 10,
    },
    addDepText: {
      fontSize: 13,
      fontWeight: '700',
    },
    graphBox: {
      marginHorizontal: 12,
      marginTop: 8,
      borderRadius: 13,
      borderWidth: 1,
      padding: 10,
    },
    descBox: {
      marginHorizontal: 12,
    },
    descText: {
      fontSize: 13,
      lineHeight: 22,
    },
    commentBox: {
      marginHorizontal: 12,
      gap: 10,
    },
    commentRow: {
      flexDirection: 'row',
      alignItems: 'flex-start',
      gap: 8,
    },
    commentHeader: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      marginBottom: 2,
    },
    commentAuthor: {
      fontSize: 12,
      fontWeight: '800',
    },
    commentTime: {
      fontSize: 11,
    },
    commentContent: {
      fontSize: 13,
      lineHeight: 20,
      flex: 1,
    },
    commentDelete: {
      padding: 2,
      marginTop: -2,
    },
    commentInputRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      marginTop: 4,
    },
    commentInput: {
      flex: 1,
    },
    commentEmpty: {
      fontSize: 13,
    },
  });

export function TaskDetailView() {
  const { client, notifications } = useServer();
  const router = useRouter();
  const colors = useColors();
  const { theme } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const { id } = useLocalSearchParams<{ id: string }>();
  const [descExpanded, setDescExpanded] = useState(false);
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
  const [workSession, setWorkSession] = useState<WorkSessionRow | null>(null);
  const [splitModalVisible, setSplitModalVisible] = useState(false);
  const [comments, setComments] = useState<CommentRow[]>([]);
  const [commentInput, setCommentInput] = useState('');
  const [commentsError, setCommentsError] = useState(false);
  const [commentSending, setCommentSending] = useState(false);
  const pendingCommentOpRef = useRef<string | null>(null);
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
      setSigmaMinutes(String(t.sigma_minutes));
      setQuantityTotal(
        t.quantity_total != null && t.quantity_total > 0
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
    // Load the open work session for this task, if any.
    try {
      const sessions = await client.listWorkSessions(t.id);
      setWorkSession(sessions.find((s) => !s.ended_at) ?? null);
    } catch (e) {
      logError('作業セッション取得', e);
      setWorkSession(null);
    }

    // Load the comment timeline (WI-5).
    try {
      setComments(await client.listComments(t.id));
      setCommentsError(false);
    } catch (e) {
      logError('コメント取得', e);
      setCommentsError(true);
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
    // Reset per-task state so a previous task's comments never leak into the
    // freshly opened task while it is loading (WI-5).
    setComments([]);
    setCommentsError(false);
    setCommentInput('');
    pendingCommentOpRef.current = null;
    setCommentSending(false);
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
    const sigma = parseDuration(sigmaMinutes);
    if (sigma !== null && sigma !== task.sigma_minutes)
      updates.sigma_minutes = sigma;
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

  async function addComment() {
    if (!client || !task) return;
    const content = commentInput.trim();
    if (!content) return;
    // Guard against double-submit while a request is already in flight; the
    // input is also disabled, but a double keypress could otherwise send twice.
    if (commentSending) return;
    // Stable idempotency key: reuse a previously failed attempt's key so a
    // retry after a lost response does not double-post (WI-5).
    const operationId = pendingCommentOpRef.current ?? makeCommentOperationId();
    if (!pendingCommentOpRef.current) {
      pendingCommentOpRef.current = operationId;
    }
    setCommentSending(true);
    setCommentInput('');
    try {
      const created = await client.createComment(
        task.id,
        { content },
        operationId,
      );
      pendingCommentOpRef.current = null;
      setCommentSending(false);
      setComments((prev) => [...prev, created]);
    } catch (e) {
      showError(e, 'コメントの追加に失敗');
      setCommentSending(false);
      setCommentInput(content);
    }
  }

  async function deleteComment(commentId: string) {
    if (!client) return;
    try {
      await client.deleteComment(commentId);
      setComments((prev) => prev.filter((c) => c.id !== commentId));
    } catch (e) {
      showError(e, 'コメントの削除に失敗');
    }
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
    let startedSession: WorkSessionRow | null = null;
    try {
      startedSession = await client.createWorkSession(
        { task_id: task.id },
        operationId,
      );
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
          if (startedSession) {
            await client.pauseWorkSession(
              startedSession.id,
              makeProgressOperationId(),
            );
          }
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
          await client.createWorkSession(
            { task_id: task.id },
            makeProgressOperationId(),
          );
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
    const session = await findOpenWorkSessionForTask(client, task.id);
    const prevQuantityDone = task.quantity_done;
    const prevQuantityTotal = task.quantity_total;
    const recordOperationId = payload ? makeProgressOperationId() : undefined;
    const pauseOperationId = makeProgressOperationId();
    if (payload && recordOperationId) {
      try {
        // Record progress first, then pause. If pause fails after a
        // successful record, the progress is retained and the session remains
        // open; the user is shown the error and can retry pausing.
        await recordProgressWithTotal(client, session, payload, {
          operationId: recordOperationId,
        });
      } catch (e) {
        showError(e, '進捗の記録に失敗');
        return;
      }
    }
    try {
      await client.pauseWorkSession(session.id, pauseOperationId);
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
          await client.createWorkSession(
            { task_id: task.id },
            makeProgressOperationId(),
          );
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
        const redoSession = await findOpenWorkSessionForTask(client, task.id);
        try {
          await client.pauseWorkSession(
            redoSession.id,
            makeProgressOperationId(),
          );
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
    const prevStatus = task.status;
    const prevQuantityDone = task.quantity_done;
    const total = task.quantity_total;
    const operationId = makeProgressOperationId();
    try {
      await completeTaskWithOptionalWorkSession(client, task.id, {
        operationId,
        quantityTotal: total,
      });
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
          await restoreTaskAfterCompletion(
            client,
            task.id,
            prevStatus,
            prevQuantityDone,
          );
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
    const session = await findOpenWorkSessionForTask(client, task.id);
    try {
      await recordProgressWithTotal(client, session, payload);
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

  // ── redesign (#1146): derived display values ──
  const headerBg = taskCardColor(
    task.abandonability,
    task.habit_id,
    habit?.display_id,
    theme,
  );
  const isLight = theme === 'light';
  const headerText = colors.textOnCard;
  const headerSub = colors.textOnCardSecondary;
  const chipOnTintBg = isLight ? 'rgba(255,255,255,0.5)' : 'rgba(0,0,0,0.28)';
  const trackOnTint = isLight ? 'rgba(28,24,36,0.15)' : 'rgba(255,255,255,0.2)';
  const nowMs = Date.now();
  const startMs = task.start_at ? new Date(task.start_at).getTime() : null;
  const endMs = new Date(task.end_at).getTime();
  const qTotal = task.quantity_total ?? 0;
  const qDone = task.quantity_done ?? 0;
  const actualFrac = qTotal > 0 ? Math.min(1, qDone / qTotal) : 0;
  const paceFrac =
    qTotal > 0 &&
    startMs != null &&
    endMs > startMs &&
    task.status === 'in_progress'
      ? Math.max(0, Math.min(1, (nowMs - startMs) / (endMs - startMs)))
      : null;
  const shownAbandon = editing ? abandonability : task.abandonability;
  const filledPipCount = filledPips(shownAbandon);

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
        <Text style={[styles.topBarId, { color: colors.gray }]}>
          #{task.display_id}
        </Text>
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
        {/* Header card (#1146): status + parallel guest + title + schedule strip */}
        <View style={[styles.headerCard, { backgroundColor: headerBg }]}>
          <View style={styles.headerChips}>
            <Menu
              visible={statusMenuVisible}
              onDismiss={() => setStatusMenuVisible(false)}
              anchor={
                <Pressable
                  style={[styles.tintChip, { backgroundColor: chipOnTintBg }]}
                  onPress={() => {
                    haptic.light();
                    setStatusMenuVisible(true);
                  }}
                >
                  <Ionicons
                    name={STATUS_ICONS[editing ? status : task.status]}
                    size={13}
                    color={headerText}
                  />
                  <Text style={[styles.tintChipText, { color: headerText }]}>
                    {STATUS_LABELS[editing ? status : task.status]}
                  </Text>
                  <Ionicons name="chevron-down" size={12} color={headerText} />
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
            {!editing && parallelTask && (
              <Pressable
                style={[styles.tintChip, { backgroundColor: chipOnTintBg }]}
                onPress={() => {
                  haptic.light();
                  router.push(`/task/${parallelTask.id}`);
                }}
              >
                <Ionicons name="swap-horizontal" size={13} color={headerText} />
                <Text
                  style={[styles.tintChipText, { color: headerText }]}
                  numberOfLines={1}
                >
                  並列: {parallelTask.title}
                </Text>
              </Pressable>
            )}
          </View>

          {editing ? (
            <PaperTextInput
              mode="outlined"
              value={title}
              onChangeText={setTitle}
              label="タイトル"
              outlineColor={headerSub}
              activeOutlineColor={headerText}
              textColor={headerText}
              style={styles.headerTitleInput}
              contentStyle={{ fontSize: 18, fontWeight: '700' }}
            />
          ) : (
            <Pressable onPress={() => handleSectionTap('title')}>
              <Text style={[styles.headerTitle, { color: headerText }]}>
                {task.title}
              </Text>
            </Pressable>
          )}

          {!isPending &&
            (editing ? (
              <View style={styles.timeFields}>
                <Pressable
                  style={[
                    styles.timeField,
                    { backgroundColor: chipOnTintBg, borderColor: headerSub },
                  ]}
                  onPress={() => {
                    haptic.select();
                    setPickerField('start');
                  }}
                >
                  <Ionicons
                    name="calendar-outline"
                    size={18}
                    color={headerText}
                  />
                  <View style={styles.timeFieldLabels}>
                    <Text style={[styles.timeFieldLabel, { color: headerSub }]}>
                      開始
                    </Text>
                    <Text
                      style={[styles.timeFieldValue, { color: headerText }]}
                    >
                      {formatDate(startAt)}
                    </Text>
                  </View>
                </Pressable>
                <Pressable
                  style={[
                    styles.timeField,
                    { backgroundColor: chipOnTintBg, borderColor: headerSub },
                  ]}
                  onPress={() => {
                    haptic.select();
                    setPickerField('end');
                  }}
                >
                  <Ionicons name="flag-outline" size={18} color={headerText} />
                  <View style={styles.timeFieldLabels}>
                    <Text style={[styles.timeFieldLabel, { color: headerSub }]}>
                      期限
                    </Text>
                    <Text
                      style={[styles.timeFieldValue, { color: headerText }]}
                    >
                      {formatDate(endAt)}
                    </Text>
                  </View>
                </Pressable>
              </View>
            ) : (
              <Pressable onPress={() => handleSectionTap('time')}>
                <Text style={[styles.stripDate, { color: headerSub }]}>
                  {formatStripDate(task.start_at ?? task.end_at)}
                </Text>
                <View style={styles.timesRow}>
                  <Text style={[styles.tBig, { color: headerText }]}>
                    {hm(task.start_at)}
                  </Text>
                  <Text style={[styles.tArrow, { color: headerSub }]}>→</Text>
                  <Text style={[styles.tBig, { color: headerText }]}>
                    {hm(task.end_at)}
                  </Text>
                  {task.status === 'in_progress' && (
                    <Text
                      style={[
                        styles.tintChipText,
                        styles.headChipRight,
                        { color: headerText },
                      ]}
                    >
                      {endMs - nowMs > 0
                        ? `残り ${relMinutes(endMs - nowMs)}`
                        : `${relMinutes(nowMs - endMs)} 超過`}
                    </Text>
                  )}
                  {task.status === 'scheduled' && startMs != null && (
                    <Text
                      style={[
                        styles.tintChipText,
                        styles.headChipRight,
                        { color: headerSub },
                      ]}
                    >
                      開始まで {relMinutes(startMs - nowMs)}
                    </Text>
                  )}
                  {task.status === 'completed' && (
                    <Text
                      style={[
                        styles.tintChipText,
                        styles.headChipRight,
                        { color: headerText },
                      ]}
                    >
                      ✓ {hm(task.completed_at ?? undefined)}
                    </Text>
                  )}
                </View>
                <View style={[styles.track, { backgroundColor: trackOnTint }]}>
                  <View
                    style={[
                      styles.fill,
                      {
                        backgroundColor:
                          task.status === 'completed'
                            ? colors.green
                            : colors.brand,
                        width: `${
                          task.status === 'completed'
                            ? 100
                            : task.status === 'in_progress' &&
                                startMs != null &&
                                endMs > startMs
                              ? Math.max(
                                  0,
                                  Math.min(
                                    100,
                                    ((nowMs - startMs) / (endMs - startMs)) *
                                      100,
                                  ),
                                )
                              : 0
                        }%`,
                      },
                    ]}
                  />
                  {task.status === 'in_progress' &&
                    startMs != null &&
                    endMs > startMs && (
                      <View
                        style={[
                          styles.nowdot,
                          {
                            borderColor: colors.brand,
                            backgroundColor: headerBg,
                            start: `${Math.max(
                              0,
                              Math.min(
                                100,
                                ((nowMs - startMs) / (endMs - startMs)) * 100,
                              ),
                            )}%`,
                          },
                        ]}
                      />
                    )}
                </View>
              </Pressable>
            ))}

          {isPending && !editing && (
            <View style={[styles.pendingNote, { borderColor: headerSub }]}>
              <Ionicons name="help-buoy-outline" size={18} color={headerText} />
              <Text style={[styles.pendingNoteText, { color: headerText }]}>
                期限未設定 — 期限を決めるとスケジューラが自動配置します
              </Text>
            </View>
          )}
          {isPending && editing && (
            <View style={styles.timeFields}>
              <Pressable
                style={[
                  styles.timeField,
                  { backgroundColor: chipOnTintBg, borderColor: headerSub },
                ]}
                onPress={() => {
                  haptic.select();
                  setPickerField('end');
                }}
              >
                <Ionicons name="flag-outline" size={18} color={headerText} />
                <View style={styles.timeFieldLabels}>
                  <Text style={[styles.timeFieldLabel, { color: headerSub }]}>
                    期限
                  </Text>
                  <Text style={[styles.timeFieldValue, { color: headerText }]}>
                    {formatDate(endAt)}
                  </Text>
                </View>
              </Pressable>
            </View>
          )}
        </View>

        {/* Actions (#1146): one primary action per state */}
        {!editing && (
          <>
            <View style={styles.slabel}>
              <Text style={[styles.slabelText, { color: colors.gray }]}>
                アクション
              </Text>
            </View>
            <View style={styles.actions}>
              {task.status === 'in_progress' && (
                <>
                  <PressableScale
                    style={({ pressed }) => [
                      styles.abtn,
                      {
                        backgroundColor: colors.green,
                        opacity: pressed ? 0.85 : 1,
                      },
                    ]}
                    onPress={completeTask}
                  >
                    <Ionicons
                      name="checkmark-circle"
                      size={22}
                      color={colors.white}
                    />
                    <Text style={[styles.abtnText, { color: colors.white }]}>
                      完了
                    </Text>
                  </PressableScale>
                  <View style={styles.actRow}>
                    <PressableScale
                      style={({ pressed }) => [
                        styles.hbtn,
                        {
                          borderColor: colors.separator,
                          backgroundColor: colors.white,
                          opacity: pressed ? 0.8 : 1,
                        },
                      ]}
                      onPress={() => openProgressSheet('pause')}
                    >
                      <Ionicons name="pause" size={17} color={colors.black} />
                      <Text style={[styles.hbtnText, { color: colors.black }]}>
                        停止
                      </Text>
                    </PressableScale>
                    <PressableScale
                      style={({ pressed }) => [
                        styles.hbtn,
                        {
                          borderColor: colors.brand,
                          backgroundColor: colors.brand,
                          opacity: pressed ? 0.85 : 1,
                        },
                      ]}
                      onPress={() => openProgressSheet('record')}
                    >
                      <Ionicons name="add" size={17} color={colors.white} />
                      <Text style={[styles.hbtnText, { color: colors.white }]}>
                        記録
                      </Text>
                    </PressableScale>
                    <PressableScale
                      style={({ pressed }) => [
                        styles.hbtnSq,
                        {
                          borderColor: colors.separator,
                          backgroundColor: colors.white,
                          opacity: pressed ? 0.8 : 1,
                        },
                      ]}
                      onPress={() => adjustProgress(-1)}
                    >
                      <Text
                        style={[styles.hbtnSqText, { color: colors.black }]}
                      >
                        −1
                      </Text>
                    </PressableScale>
                    <PressableScale
                      style={({ pressed }) => [
                        styles.hbtnSq,
                        {
                          borderColor: colors.separator,
                          backgroundColor: colors.white,
                          opacity: pressed ? 0.8 : 1,
                        },
                      ]}
                      onPress={() => adjustProgress(1)}
                    >
                      <Text
                        style={[styles.hbtnSqText, { color: colors.black }]}
                      >
                        +1
                      </Text>
                    </PressableScale>
                  </View>
                  <PressableScale
                    style={({ pressed }) => [
                      styles.hbtn,
                      {
                        borderColor: colors.brand,
                        borderStyle: 'dashed',
                        backgroundColor: colors.white,
                        opacity: pressed ? 0.8 : 1,
                      },
                    ]}
                    onPress={openSplitModal}
                  >
                    <Ionicons name="cut" size={17} color={colors.brand} />
                    <Text style={[styles.hbtnText, { color: colors.brand }]}>
                      分割
                    </Text>
                  </PressableScale>
                </>
              )}

              {task.status === 'scheduled' && (
                <>
                  <PressableScale
                    style={({ pressed }) => [
                      styles.abtn,
                      {
                        backgroundColor: colors.brand,
                        opacity: pressed ? 0.85 : 1,
                      },
                    ]}
                    onPress={startTask}
                  >
                    <Ionicons name="play" size={22} color={colors.white} />
                    <Text style={[styles.abtnText, { color: colors.white }]}>
                      開始
                    </Text>
                  </PressableScale>
                  <View style={styles.actMeta}>
                    <Pressable
                      onPress={() => {
                        haptic.light();
                        setEditing(true);
                        setPickerField('end');
                      }}
                    >
                      <Text
                        style={[styles.linkBtnText, { color: colors.brand }]}
                      >
                        時間を変更
                      </Text>
                    </Pressable>
                    <Pressable
                      onPress={() => {
                        haptic.medium();
                        changeStatus('skipped');
                      }}
                    >
                      <Text
                        style={[styles.linkBtnText, { color: colors.gray }]}
                      >
                        スキップ
                      </Text>
                    </Pressable>
                  </View>
                </>
              )}

              {task.status === 'pending' && (
                <PressableScale
                  style={({ pressed }) => [
                    styles.abtn,
                    {
                      backgroundColor: colors.brand,
                      opacity: pressed ? 0.85 : 1,
                    },
                  ]}
                  onPress={() => {
                    haptic.light();
                    setEditing(true);
                    setPickerField('end');
                  }}
                >
                  <Ionicons
                    name="calendar-outline"
                    size={22}
                    color={colors.white}
                  />
                  <Text style={[styles.abtnText, { color: colors.white }]}>
                    期限を設定
                  </Text>
                </PressableScale>
              )}

              {(task.status === 'completed' || task.status === 'skipped') && (
                <>
                  <View
                    style={[
                      styles.doneBanner,
                      {
                        backgroundColor: colors.success + '22',
                        borderColor: colors.success + '55',
                      },
                    ]}
                  >
                    <Ionicons
                      name={
                        task.status === 'completed'
                          ? 'checkmark-done-circle'
                          : 'play-skip-forward'
                      }
                      size={26}
                      color={colors.success}
                    />
                    <View>
                      <Text
                        style={[
                          styles.doneBannerTitle,
                          { color: colors.success },
                        ]}
                      >
                        {STATUS_LABELS[task.status]}
                      </Text>
                      {(task.completed_at ||
                        (task.actual_minutes ?? 0) > 0) && (
                        <Text
                          style={[
                            styles.doneBannerSub,
                            { color: colors.success },
                          ]}
                        >
                          {task.completed_at
                            ? `${hm(task.completed_at)} · `
                            : ''}
                          {(task.actual_minutes ?? 0) > 0
                            ? `実績 ${formatDuration(task.actual_minutes!)}`
                            : ''}
                        </Text>
                      )}
                    </View>
                  </View>
                  <View style={styles.actMeta}>
                    <Pressable
                      onPress={() => {
                        haptic.medium();
                        changeStatus('in_progress');
                      }}
                    >
                      <Text
                        style={[styles.linkBtnText, { color: colors.brand }]}
                      >
                        進行中に戻す
                      </Text>
                    </Pressable>
                  </View>
                </>
              )}
            </View>
          </>
        )}

        {/* (Time + parallel task moved into the header card, #1146) */}

        {/* Stats grid (#1146) */}
        <View style={styles.slabel}>
          <Text style={[styles.slabelText, { color: colors.gray }]}>統計</Text>
        </View>
        <View style={styles.statsRow}>
          {/* Cost */}
          <View
            style={[
              styles.cell,
              { backgroundColor: colors.white, borderColor: colors.separator },
            ]}
          >
            <Text style={[styles.cellK, { color: colors.gray }]}>コスト</Text>
            {editing ? (
              <View style={styles.costInputs}>
                <PaperTextInput
                  mode="outlined"
                  label="avg"
                  value={avgMinutes}
                  onChangeText={setAvgMinutes}
                  autoCapitalize="none"
                  autoCorrect={false}
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  style={{ flex: 1 }}
                  dense
                />
                <PaperTextInput
                  mode="outlined"
                  label="sigma"
                  value={sigmaMinutes}
                  onChangeText={setSigmaMinutes}
                  autoCapitalize="none"
                  autoCorrect={false}
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  style={{ flex: 1 }}
                  dense
                />
              </View>
            ) : (
              <Pressable onPress={() => handleSectionTap('cost')}>
                <Text style={[styles.cellV, { color: colors.black }]}>
                  {formatDuration(task.avg_minutes)}{' '}
                  <Text style={[styles.cellVUnit, { color: colors.gray }]}>
                    ±{formatDuration(task.sigma_minutes)}
                  </Text>
                </Text>
                {task.actual_minutes != null && task.actual_minutes > 0 && (
                  <Text style={[styles.cellSub, { color: colors.grayLight }]}>
                    {task.status === 'completed' ? '実績' : '経過'}{' '}
                    {formatDuration(task.actual_minutes)}
                  </Text>
                )}
              </Pressable>
            )}
          </View>

          {/* Quantity */}
          <View
            style={[
              styles.cell,
              { backgroundColor: colors.white, borderColor: colors.separator },
            ]}
          >
            <Text style={[styles.cellK, { color: colors.gray }]}>数量</Text>
            {editing ? (
              <View style={styles.costInputs}>
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
                  style={{ flex: 1 }}
                  dense
                />
                <PaperTextInput
                  mode="outlined"
                  label="単位"
                  value={quantityUnit}
                  onChangeText={setQuantityUnit}
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  style={{ flex: 1 }}
                  dense
                />
              </View>
            ) : (
              <Pressable onPress={() => handleSectionTap('quantity')}>
                {qTotal > 0 ? (
                  <Text style={[styles.cellV, { color: colors.black }]}>
                    {qDone}
                    <Text style={[styles.cellVUnit, { color: colors.gray }]}>
                      {' '}
                      / {qTotal} {task.quantity_unit ?? ''}
                    </Text>
                  </Text>
                ) : (
                  <Text style={[styles.cellVUnit, { color: colors.gray }]}>
                    未設定
                  </Text>
                )}
                {qTotal > 0 && (
                  <View
                    style={[
                      styles.minibar,
                      { backgroundColor: colors.separator },
                    ]}
                  >
                    <View
                      style={[
                        styles.minibarFill,
                        {
                          backgroundColor: colors.brand,
                          width: `${actualFrac * 100}%`,
                        },
                      ]}
                    />
                    {paceFrac != null && (
                      <View
                        style={[
                          styles.paceMarker,
                          {
                            backgroundColor: colors.warning,
                            start: `${paceFrac * 100}%`,
                          },
                        ]}
                      />
                    )}
                  </View>
                )}
                {paceFrac != null && (
                  <Text style={[styles.cellSub, { color: colors.grayLight }]}>
                    ▸ ペース {Math.round(paceFrac * 100)}%
                    {actualFrac < paceFrac - 0.02
                      ? ' · わずかに遅れ'
                      : actualFrac > paceFrac + 0.02
                        ? ' · 先行'
                        : ''}
                  </Text>
                )}
              </Pressable>
            )}
          </View>
        </View>

        {/* Flags / metadata (wide) */}
        <View
          style={[
            styles.cellWide,
            { backgroundColor: colors.white, borderColor: colors.separator },
          ]}
        >
          <Text style={[styles.cellK, { color: colors.gray }]}>
            フラグ・メタデータ
          </Text>
          {editing ? (
            <View style={styles.flagEditor}>
              <Pressable
                style={styles.tog}
                onPress={() => {
                  haptic.select();
                  setParallelizable(!parallelizable);
                }}
              >
                <Checkbox
                  status={parallelizable ? 'checked' : 'unchecked'}
                  onPress={() => {
                    haptic.select();
                    setParallelizable(!parallelizable);
                  }}
                  color={colors.brand}
                />
                <Text style={[styles.togLabel, { color: colors.black }]}>
                  並列実行可能
                </Text>
              </Pressable>
              <Pressable
                style={styles.tog}
                onPress={() => {
                  haptic.select();
                  setAllowsParallel(!allowsParallel);
                }}
              >
                <Checkbox
                  status={allowsParallel ? 'checked' : 'unchecked'}
                  onPress={() => {
                    haptic.select();
                    setAllowsParallel(!allowsParallel);
                  }}
                  color={colors.brand}
                />
                <Text style={[styles.togLabel, { color: colors.black }]}>
                  並列受け入れ
                </Text>
              </Pressable>
              <Pressable
                style={styles.tog}
                onPress={() => {
                  haptic.select();
                  setFixed(!fixed);
                }}
              >
                <Checkbox
                  status={fixed ? 'checked' : 'unchecked'}
                  onPress={() => {
                    haptic.select();
                    setFixed(!fixed);
                  }}
                  color={colors.brand}
                />
                <Text style={[styles.togLabel, { color: colors.black }]}>
                  時間固定
                </Text>
              </Pressable>
              <View style={styles.abandonEdit}>
                <Text style={[styles.abandonLabel, { color: colors.gray }]}>
                  捨てづらさ
                </Text>
                <Slider
                  value={abandonability}
                  onValueChange={setAbandonability}
                  minimumValue={0}
                  maximumValue={1}
                  step={0.25}
                  minimumTrackTintColor={colors.brand}
                  style={styles.abandonSlider}
                />
                <Text style={[styles.abandonVal, { color: colors.brand }]}>
                  {abandonability.toFixed(2)}
                </Text>
              </View>
            </View>
          ) : (
            <Pressable
              style={styles.flagrow}
              onPress={() => handleSectionTap('flags')}
            >
              <View
                style={[
                  styles.flagChip,
                  {
                    backgroundColor: colors.surfaceTint,
                    borderColor: colors.separator,
                  },
                ]}
              >
                <Text style={[styles.flagChipText, { color: colors.gray }]}>
                  並列実行
                </Text>
                <Text
                  style={[
                    styles.flagChipText,
                    {
                      color: task.parallelizable ? colors.brand : colors.red,
                    },
                  ]}
                >
                  {task.parallelizable ? '✓' : 'off'}
                </Text>
              </View>
              <View
                style={[
                  styles.flagChip,
                  {
                    backgroundColor: colors.surfaceTint,
                    borderColor: colors.separator,
                  },
                ]}
              >
                <Text style={[styles.flagChipText, { color: colors.gray }]}>
                  並列受入
                </Text>
                <Text
                  style={[
                    styles.flagChipText,
                    {
                      color: task.allows_parallel ? colors.brand : colors.red,
                    },
                  ]}
                >
                  {task.allows_parallel ? '✓' : 'off'}
                </Text>
              </View>
              <View
                style={[
                  styles.flagChip,
                  {
                    backgroundColor: colors.surfaceTint,
                    borderColor: colors.separator,
                  },
                ]}
              >
                <Text style={[styles.flagChipText, { color: colors.gray }]}>
                  時間固定
                </Text>
                <Text
                  style={[
                    styles.flagChipText,
                    { color: task.fixed ? colors.brand : colors.red },
                  ]}
                >
                  {task.fixed ? '✓' : 'off'}
                </Text>
              </View>
              <View
                style={[
                  styles.flagChip,
                  {
                    backgroundColor: colors.surfaceTint,
                    borderColor: colors.separator,
                  },
                ]}
              >
                <Text style={[styles.flagChipText, { color: colors.gray }]}>
                  捨てづらさ
                </Text>
                <Text style={[styles.flagChipText, { color: colors.black }]}>
                  {task.abandonability.toFixed(2)}
                </Text>
                <View style={styles.pips}>
                  {[0, 1, 2, 3, 4].map((i) => (
                    <View
                      key={i}
                      style={[
                        styles.pip,
                        {
                          backgroundColor:
                            i < filledPipCount
                              ? colors.brand
                              : colors.separator,
                        },
                      ]}
                    />
                  ))}
                </View>
              </View>
            </Pressable>
          )}
        </View>

        {/* Habit link (#1146) */}
        {habit && (
          <>
            <View style={styles.slabel}>
              <Text style={[styles.slabelText, { color: colors.gray }]}>
                HABIT
              </Text>
            </View>
            <Pressable
              style={styles.habitLinkRow}
              onPress={() => {
                haptic.light();
                router.push(`/habit/${habit.id}`);
              }}
            >
              <Ionicons name="repeat" size={16} color={colors.brand} />
              <Text
                style={[styles.habitLinkText, { color: colors.brand }]}
                numberOfLines={1}
              >
                {task.habit_step_id
                  ? (() => {
                      const step = habit.steps.find(
                        (s) => s.id === task.habit_step_id,
                      );
                      return step
                        ? `${habit.title} › ${step.title}`
                        : habit.title;
                    })()
                  : habit.title}
              </Text>
              <Ionicons name="chevron-forward" size={16} color={colors.brand} />
            </Pressable>
          </>
        )}

        {/* (Parallel config + fixed moved into the stats grid, #1146) */}

        {/* Deps (#1146): chips + always-embedded graph */}
        <View style={styles.slabel}>
          <Text style={[styles.slabelText, { color: colors.gray }]}>
            依存 ({deps.length})
          </Text>
        </View>
        {!editing && redundantEdges.length > 0 && (
          <View style={{ marginHorizontal: 12, marginBottom: 4 }}>
            <RedundantDepWarning
              edges={redundantEdges}
              onResolve={resolveRedundantEdge}
              nodeLabel={(nid, ntitle) => {
                const nt = allTasks.find((t) => t.id === nid);
                return nt ? `#${nt.display_id} ${nt.title}` : ntitle;
              }}
            />
          </View>
        )}
        <View style={styles.depChips}>
          {deps.map((depId) => {
            const depTask = allTasks.find((t) => t.id === depId);
            const depDone =
              depTask?.status === 'completed' || depTask?.status === 'skipped';
            return (
              <View
                key={depId}
                style={[
                  styles.depChip,
                  {
                    backgroundColor: colors.white,
                    borderColor: colors.separator,
                  },
                ]}
              >
                <Ionicons
                  name={depDone ? 'checkmark-circle' : 'ellipse-outline'}
                  size={16}
                  color={depDone ? colors.green : colors.brand}
                />
                <Text style={[styles.depChipNo, { color: colors.grayLight }]}>
                  #{depTask?.display_id ?? depId.slice(0, 6)}
                </Text>
                <Pressable
                  style={{ flex: 1 }}
                  onPress={() => {
                    if (!editing) {
                      haptic.light();
                      router.push(`/task/${depId}`);
                    }
                  }}
                >
                  <Text
                    style={[styles.depChipTitle, { color: colors.black }]}
                    numberOfLines={1}
                  >
                    {depTask?.title ?? depId.slice(0, 8) + '...'}
                  </Text>
                </Pressable>
                {editing ? (
                  <Pressable
                    onPress={() => {
                      haptic.light();
                      setDeps(deps.filter((d) => d !== depId));
                    }}
                  >
                    <Ionicons
                      name="close-circle"
                      size={18}
                      color={colors.red}
                    />
                  </Pressable>
                ) : (
                  <Ionicons
                    name="chevron-forward"
                    size={15}
                    color={colors.grayLight}
                  />
                )}
              </View>
            );
          })}
          {deps.length === 0 && !editing && (
            <Text style={[styles.cellSub, { color: colors.gray }]}>(なし)</Text>
          )}
          {editing && (
            <Pressable
              style={[styles.addDep, { borderColor: colors.brand }]}
              onPress={() => {
                haptic.light();
                setDepSearch('');
                setDepModalVisible(true);
              }}
            >
              <Ionicons name="add" size={18} color={colors.brand} />
              <Text style={[styles.addDepText, { color: colors.brand }]}>
                依存を追加
              </Text>
            </Pressable>
          )}
        </View>

        {/* Dependency graph: always embedded when connected (#1146) */}
        {detailGraphNodes.length > 1 && (
          <View
            style={[
              styles.graphBox,
              { backgroundColor: colors.white, borderColor: colors.separator },
            ]}
          >
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

        {/* Description (#1146) */}
        <View style={styles.slabel}>
          <Text style={[styles.slabelText, { color: colors.gray }]}>説明</Text>
        </View>
        {editing ? (
          <PaperTextInput
            mode="outlined"
            value={description}
            onChangeText={setDescription}
            multiline
            numberOfLines={4}
            outlineColor={colors.separator}
            activeOutlineColor={colors.brand}
            style={{ marginHorizontal: 12, minHeight: 84 }}
          />
        ) : (
          <View style={styles.descBox}>
            <Text
              style={[styles.descText, { color: colors.gray }]}
              numberOfLines={descExpanded ? undefined : 3}
            >
              {task.description || '(なし)'}
            </Text>
            {(task.description?.length ?? 0) > 60 && (
              <Pressable onPress={() => setDescExpanded((v) => !v)}>
                <Text
                  style={[
                    styles.linkBtnText,
                    { color: colors.brand, marginTop: 4 },
                  ]}
                >
                  {descExpanded ? '閉じる' : '続きを読む'}
                </Text>
              </Pressable>
            )}
          </View>
        )}

        {/* Comment timeline (WI-5) — alongside the description */}
        <View style={styles.slabel}>
          <Text style={[styles.slabelText, { color: colors.gray }]}>
            {`コメント (${comments.length})`}
          </Text>
        </View>
        <View style={styles.commentBox}>
          <PaperTextInput
            mode="outlined"
            value={commentInput}
            onChangeText={setCommentInput}
            placeholder="メモを追加"
            onSubmitEditing={addComment}
            editable={!commentSending}
            placeholderTextColor={colors.gray}
            outlineColor={colors.separator}
            activeOutlineColor={colors.brand}
            style={styles.commentInput}
          />
          {commentsError ? (
            <Text style={[styles.commentEmpty, { color: colors.red }]}>
              コメントの取得に失敗しました
            </Text>
          ) : comments.length === 0 ? (
            <Text style={[styles.commentEmpty, { color: colors.gray }]}>
              コメントはありません
            </Text>
          ) : (
            comments.map((c) => (
              <View key={c.id} style={styles.commentRow}>
                <View style={{ flex: 1 }}>
                  <View style={styles.commentHeader}>
                    <Text
                      style={[styles.commentAuthor, { color: colors.black }]}
                    >
                      {AUTHOR_LABELS[c.author] ?? c.author}
                    </Text>
                    <Text style={[styles.commentTime, { color: colors.gray }]}>
                      {formatDate(new Date(c.created_at))}
                    </Text>
                  </View>
                  <Text style={[styles.commentContent, { color: colors.gray }]}>
                    {c.content}
                  </Text>
                </View>
                {c.author !== 'system' && (
                  <IconButton
                    icon="close"
                    size={16}
                    iconColor={colors.gray}
                    onPress={() => deleteComment(c.id)}
                    style={styles.commentDelete}
                  />
                )}
              </View>
            ))
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
          <PressableScale
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
          </PressableScale>
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
      {workSession && (
        <TaskProgressSheet
          visible={progressSheetVisible}
          session={workSession}
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
              <PressableScale
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
              </PressableScale>
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
