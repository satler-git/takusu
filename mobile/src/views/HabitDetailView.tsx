// HabitDetailView — view and edit a habit + recent generated tasks

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  Pressable,
  Modal,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useRouter, useLocalSearchParams } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import {
  Checkbox,
  IconButton,
  Menu,
  SegmentedButtons,
  TextInput as PaperTextInput,
} from 'react-native-paper';
import { Slider } from '@expo/ui/community/slider';
import { useServer } from '@/src/api/ServerProvider';
import { undoRedo } from '@/src/api/undoRedo';
import { showError, logError } from '@/src/api/errors';
import { parseDepends, parseDependsOn } from '@/src/api/types';
import type {
  HabitDetail,
  HabitScheduledSpanRow,
  TaskRow,
  WindowMode,
  RedundantDependency,
  HabitStepInput,
} from '@/src/api/types';
import { WINDOW_MODE_DAY, WINDOW_MODE_PERIOD } from '@/src/api/types';
import { BRAND_COLOR, useColors } from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { RruleBuilderModal } from '@/src/components/RruleBuilderModal';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { HabitEstimateModal } from '@/src/components/HabitEstimateModal';
import { HabitStepEditor } from '@/src/components/HabitStepEditor';
import { RedundantDepWarning } from '@/src/components/RedundantDepWarning';
import { parseRule, summarizeRule } from '@/src/api/rrule';
import { haptic } from '@/src/components/haptics';
import { useUndoableToast } from '@/src/hooks/useUndoableToast';
import { CancelConfirmButton } from '@/src/components/CancelConfirmButton';
import { DeleteConfirmButton } from '@/src/components/DeleteConfirmButton';
import { parseDuration } from '@/src/utils/duration';
import {
  type StepDraft,
  stepRowToDraft,
  saveHabitSteps,
} from '@/src/utils/habitSteps';
import { dateKey, todayDateKey } from '@/src/utils/dateKey';

export function HabitDetailView() {
  const { client } = useServer();
  const router = useRouter();
  const colors = useColors();
  const insets = useSafeAreaInsets();
  const showUndoToast = useUndoableToast();
  const { id } = useLocalSearchParams<{ id: string }>();
  const [habit, setHabit] = useState<HabitDetail | null>(null);
  const [tasks, setTasks] = useState<TaskRow[]>([]);
  const [spans, setSpans] = useState<HabitScheduledSpanRow[]>([]);

  // edit state
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [recurrence, setRecurrence] = useState('');
  const [showRruleBuilder, setShowRruleBuilder] = useState(false);
  const [startTime, setStartTime] = useState('09:00');
  const [endTime, setEndTime] = useState('10:00');
  const [avgMinutes, setAvgMinutes] = useState('60');
  const [sigmaMinutes, setSigmaMinutes] = useState('0');
  const [abandonability, setAbandonability] = useState(0.5);
  const [parallelizable, setParallelizable] = useState(false);
  const [allowsParallel, setAllowsParallel] = useState(false);
  const [fixed, setFixed] = useState(false);
  const [active, setActive] = useState(true);
  const [windowMode, setWindowMode] = useState<WindowMode>(WINDOW_MODE_DAY);
  const [stepDrafts, setStepDrafts] = useState<StepDraft[]>([]);
  const [stepRedundantEdges, setStepRedundantEdges] = useState<
    RedundantDependency[]
  >([]);
  const [simpleInfoExpanded, setSimpleInfoExpanded] = useState(false);
  const [showEstimateModal, setShowEstimateModal] = useState(false);
  const [saving, setSaving] = useState(false);
  const [menuVisible, setMenuVisible] = useState(false);
  const [pickerField, setPickerField] = useState<'start' | 'end' | null>(null);
  const [serverTz, setServerTz] = useState<string | undefined>(undefined);
  // Span-add modal state
  const [showSpanModal, setShowSpanModal] = useState(false);
  const [spanFrom, setSpanFrom] = useState<Date | null>(null);
  const [spanTo, setSpanTo] = useState<Date | null>(null);
  const [spanReason, setSpanReason] = useState('');
  const [spanPicker, setSpanPicker] = useState<'from' | 'to' | null>(null);
  // Ref mirror of `editing` so refresh() can skip overwriting unsaved edits
  // when called from menu actions (toggleActive) while editing.
  const editingRef = useRef(false);
  editingRef.current = editing;

  // "HH:MM" → Date (today at that time)
  function timeStringToDate(s: string): Date {
    const [h, m] = s.split(':').map((n) => parseInt(n, 10) || 0);
    const d = new Date();
    d.setHours(h, m, 0, 0);
    return d;
  }

  // Date → "HH:MM"
  function dateToTimeString(d: Date): string {
    return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;
  }

  function dateToYMD(d: Date): string {
    return dateKey(d.toISOString(), serverTz);
  }

  const refresh = useCallback(
    async (targetId = id) => {
      if (!client || !targetId) return;
      try {
        const settings = await client.getSettings().catch((e) => {
          logError('設定取得', e);
          return null;
        });
        setServerTz(settings?.tz ?? undefined);
      } catch {
        // settings are optional for viewing; keep serverTz as undefined
      }
      try {
        const h = await client.getHabit(targetId);
        setHabit(h);
        // Don't clobber the user's in-progress edits.
        if (!editingRef.current) {
          setTitle(h.title);
          setDescription(h.description ?? '');
          setRecurrence(h.recurrence);
          setStartTime(h.start_time);
          setEndTime(h.end_time);
          setAvgMinutes(String(h.avg_minutes));
          setSigmaMinutes(h.sigma_minutes > 0 ? String(h.sigma_minutes) : '');
          setAbandonability(h.abandonability);
          setParallelizable(h.parallelizable);
          setAllowsParallel(h.allows_parallel);
          setActive(h.active);
          setFixed(h.fixed);
          setWindowMode(
            (h.window_mode === WINDOW_MODE_PERIOD
              ? WINDOW_MODE_PERIOD
              : WINDOW_MODE_DAY) as WindowMode,
          );
          setStepDrafts(h.steps.map(stepRowToDraft));
        }
        // Fetch step dependency analysis (#355) — only meaningful for saved
        // steps, but we fetch always so the warning is available in view mode.
        if (h.steps.length > 0) {
          try {
            const analysis =
              await client.analyzeHabitStepDependencies(targetId);
            setStepRedundantEdges(analysis.redundant);
          } catch (e) {
            logError('ステップ依存分析の取得', e);
            setStepRedundantEdges([]);
          }
        } else {
          setStepRedundantEdges([]);
        }
      } catch (e) {
        showError(e, 'Habitの取得に失敗');
        return;
      }
      // Fetch scheduled spans (always, even while editing — span add/delete are
      // immediate actions outside the edit save flow).
      try {
        setSpans(await client.listHabitScheduledSpans(targetId));
      } catch (e) {
        logError('スケジュール済み期間の取得', e);
        setSpans([]);
      }
      try {
        const allTasks = await client.listTasks({ habit_id: targetId });
        // Show upcoming tasks in chronological order.
        // Server returns tasks ordered by created_at DESC (generation order),
        // not by date. Sort by start_at ascending so the user sees the earliest
        // upcoming task first. Exclude completed/skipped tasks so past finished
        // habit occurrences don't push upcoming ones out of the top 10.
        const sorted = [...allTasks]
          .filter((t) => t.status !== 'completed' && t.status !== 'skipped')
          .sort((a, b) => (a.start_at ?? '').localeCompare(b.start_at ?? ''))
          .slice(0, 10);
        setTasks(sorted);
      } catch (e) {
        logError('ハビットのタスク取得', e);
        setTasks([]);
      }
    },
    [client, id],
  );

  useEffect(() => {
    refresh();
  }, [refresh]);

  async function save() {
    if (!client || !habit || saving) return;
    const updates: Record<string, unknown> = {};
    if (title !== habit.title) updates.title = title;
    if (description !== (habit.description ?? ''))
      updates.description = description;
    if (recurrence !== habit.recurrence) updates.recurrence = recurrence;
    if (startTime !== habit.start_time) updates.start_time = startTime;
    if (endTime !== habit.end_time) updates.end_time = endTime;
    if (avgMinutes !== String(habit.avg_minutes)) {
      const v = parseDuration(avgMinutes);
      if (v !== null && v > 0) updates.avg_minutes = v;
    }
    if (
      sigmaMinutes !==
      (habit.sigma_minutes > 0 ? String(habit.sigma_minutes) : '')
    ) {
      const v = parseDuration(sigmaMinutes);
      if (v !== null && v >= 0) updates.sigma_minutes = v;
    }
    if (abandonability !== habit.abandonability)
      updates.abandonability = abandonability;
    if (parallelizable !== habit.parallelizable)
      updates.parallelizable = parallelizable;
    if (allowsParallel !== habit.allows_parallel)
      updates.allows_parallel = allowsParallel;
    if (active !== habit.active) updates.active = active;
    if (fixed !== habit.fixed) updates.fixed = fixed;
    if (windowMode !== habit.window_mode) updates.window_mode = windowMode;

    // Detect whether steps changed (compare count + per-field equality).
    const prevSteps = habit.steps;
    const stepsChanged =
      stepDrafts.length !== prevSteps.length ||
      stepDrafts.some((d, i) => {
        const r = prevSteps[i];
        if (!r) return true;
        let prevDeps: string[] = [];
        try {
          const parsed = JSON.parse(r.depends_on);
          if (Array.isArray(parsed)) prevDeps = parsed as string[];
        } catch {
          prevDeps = [];
        }
        return (
          d.id !== r.id ||
          d.title !== r.title ||
          (d.description ?? undefined) !== (r.description ?? undefined) ||
          d.start_time !== r.start_time ||
          d.end_time !== r.end_time ||
          d.avg_minutes !== r.avg_minutes ||
          d.sigma_minutes !== r.sigma_minutes ||
          d.parallelizable !== r.parallelizable ||
          d.allows_parallel !== r.allows_parallel ||
          d.abandonability !== r.abandonability ||
          d.fixed !== r.fixed ||
          JSON.stringify(d.depends_on) !== JSON.stringify(prevDeps)
        );
      });

    if (Object.keys(updates).length === 0 && !stepsChanged) {
      setEditing(false);
      return;
    }
    const prev = { ...habit };
    setSaving(true);
    let habitUpdated = false;
    try {
      if (Object.keys(updates).length > 0) {
        await client.updateHabit(habit.id, updates);
        habitUpdated = true;
      }
      if (stepsChanged) {
        await saveHabitSteps(client, habit.id, stepDrafts);
      }
      // Snapshot for undo/redo.
      const prevUpdates = { ...updates };
      const prevDrafts = prevSteps.map(stepRowToDraft);
      const newDrafts = stepDrafts;
      undoRedo.push({
        description: `edit habit: ${habit.title}`,
        undo: async () => {
          await client.updateHabit(habit.id, {
            title: prev.title,
            description: prev.description,
            recurrence: prev.recurrence,
            start_time: prev.start_time,
            end_time: prev.end_time,
            avg_minutes: prev.avg_minutes,
            sigma_minutes: prev.sigma_minutes,
            abandonability: prev.abandonability,
            parallelizable: prev.parallelizable,
            allows_parallel: prev.allows_parallel,
            active: prev.active,
            fixed: prev.fixed,
            window_mode: prev.window_mode,
          });
          if (stepsChanged) {
            await saveHabitSteps(client, habit.id, prevDrafts);
          }
          await refresh();
        },
        redo: async () => {
          if (Object.keys(prevUpdates).length > 0) {
            await client.updateHabit(habit.id, prevUpdates);
          }
          if (stepsChanged) {
            await saveHabitSteps(client, habit.id, newDrafts);
          }
          await refresh();
        },
      });
    } catch (e) {
      // If the habit body was updated but the step save failed, roll
      // back the body update so the habit isn't left in a partial state.
      if (habitUpdated) {
        await client
          .updateHabit(habit.id, {
            title: prev.title,
            description: prev.description,
            recurrence: prev.recurrence,
            start_time: prev.start_time,
            end_time: prev.end_time,
            avg_minutes: prev.avg_minutes,
            sigma_minutes: prev.sigma_minutes,
            abandonability: prev.abandonability,
            parallelizable: prev.parallelizable,
            allows_parallel: prev.allows_parallel,
            active: prev.active,
            fixed: prev.fixed,
            window_mode: prev.window_mode,
          })
          .catch(() => {});
      }
      showError(e, 'ハビットの保存に失敗');
      setSaving(false);
      return;
    }
    setSaving(false);
    setEditing(false);
    await refresh();
  }

  async function deleteHabit() {
    setMenuVisible(false);
    if (!client || !habit) return;
    // #240: confirm before deleting, showing how many associated
    // tasks will also be cascade-deleted. Fetch the task list first
    // so the confirmation is accurate and undo can restore them.
    let deletedTasks: TaskRow[];
    let deletedSpans: HabitScheduledSpanRow[];
    try {
      [deletedTasks, deletedSpans] = await Promise.all([
        client.listTasks({ habit_id: habit.id }),
        client
          .listHabitScheduledSpans(habit.id)
          .catch(() => [] as HabitScheduledSpanRow[]),
      ]);
    } catch (e) {
      showError(e, 'ハビットのタスク取得に失敗');
      return;
    }
    const taskCount = deletedTasks.length;
    const prev = { ...habit };
    // Track the current habit id: undo recreates the habit with a new id,
    // and redo must delete that new id (not the stale original).
    let currentId = habit.id;
    try {
      await client.deleteHabit(currentId);
    } catch (e) {
      showError(e, 'ハビットの削除に失敗');
      return;
    }

    const message =
      taskCount > 0
        ? `「${habit.title}」と関連する${taskCount}件のタスクを削除しました`
        : `「${habit.title}」を削除しました`;
    showUndoToast(message);

    // Track recreated task ids so redo deletes them, and so a retry
    // after partial failure doesn't create duplicates.
    const currentTaskIds: string[] = [...deletedTasks.map((t) => t.id)];
    const taskCreatedIdx = new Set<number>();
    // Guard habit creation so a retry after partial failure doesn't
    // create a duplicate habit (mirrors HabitView's createdIdx).
    let habitCreated = false;
    undoRedo.push({
      description:
        taskCount > 0
          ? `delete habit + ${taskCount} tasks: ${habit.title}`
          : `delete habit: ${habit.title}`,
      undo: async () => {
        if (!habitCreated) {
          const recreated = await client.createHabit({
            title: prev.title,
            description: prev.description,
            recurrence: prev.recurrence,
            start_time: prev.start_time,
            end_time: prev.end_time,
            avg_minutes: prev.avg_minutes,
            sigma_minutes: prev.sigma_minutes,
            parallelizable: prev.parallelizable,
            allows_parallel: prev.allows_parallel,
            abandonability: prev.abandonability,
            fixed: prev.fixed,
            window_mode: prev.window_mode,
          });
          // CreateHabit does not accept `active`; restore it via update.
          if (!prev.active) {
            await client.updateHabit(recreated.id, { active: prev.active });
          }
          // Restore steps (#95) — createHabit doesn't accept steps, so
          // bulk-replace them via the steps endpoint.
          if (prev.steps.length > 0) {
            await saveHabitSteps(
              client,
              recreated.id,
              prev.steps.map(stepRowToDraft),
            );
          }
          // Restore scheduled spans (#303 / #503).
          for (const p of deletedSpans) {
            await client.createHabitScheduledSpan(recreated.id, {
              start_date: p.start_date,
              end_date: p.end_date,
              reason: p.reason,
            });
          }
          currentId = recreated.id;
          habitCreated = true;
        }
        // Restore the cascade-deleted tasks, pointing them at the
        // recreated habit's new id. Two-pass: first create with no
        // deps (so server doesn't reject references to not-yet-existing
        // tasks), then remap deps to new ids — mirrors HomeView's
        // batch-delete undo pattern.
        const oldToNew = new Map<string, string>();
        for (let i = 0; i < deletedTasks.length; i++) {
          if (taskCreatedIdx.has(i)) {
            oldToNew.set(deletedTasks[i].id, currentTaskIds[i]);
            continue;
          }
          const t = deletedTasks[i];
          const recreatedTask = await client.createTask({
            title: t.title,
            description: t.description,
            start_at: t.start_at,
            end_at: t.end_at,
            avg_minutes: t.avg_minutes,
            sigma_minutes: t.sigma_minutes,
            depends: [],
            parallelizable: t.parallelizable,
            allows_parallel: t.allows_parallel,
            abandonability: t.abandonability,
            ical_uid: t.ical_uid,
            habit_id: currentId,
            fixed: t.fixed,
          });
          if (t.status !== 'pending') {
            await client.updateTask(recreatedTask.id, { status: t.status });
          }
          currentTaskIds[i] = recreatedTask.id;
          oldToNew.set(t.id, recreatedTask.id);
          taskCreatedIdx.add(i);
        }
        // Second pass: remap depends to new IDs for deps within the
        // deleted set.
        for (let i = 0; i < deletedTasks.length; i++) {
          const t = deletedTasks[i];
          const origDeps = parseDepends(t.depends);
          if (origDeps.length === 0) continue;
          const newId = oldToNew.get(t.id)!;
          const remapped = origDeps.map((d) => oldToNew.get(d) ?? d);
          await client.updateTask(newId, { depends: remapped });
        }
        await refresh(currentId);
      },
      redo: async () => {
        habitCreated = false;
        taskCreatedIdx.clear();
        await client.deleteHabit(currentId);
      },
    });
    router.back();
  }

  // Resolve a redundant step dependency edge by removing `toId` from the
  // `fromId` step's depends_on, then replacing all steps (#355).
  async function resolveStepRedundantEdge(fromId: string, toId: string) {
    if (!client || !habit) return;
    const prevSteps = habit.steps;
    const newSteps: HabitStepInput[] = prevSteps.map((s) => {
      const deps = parseDependsOn(s.depends_on);
      const filtered = s.id === fromId ? deps.filter((d) => d !== toId) : deps;
      return {
        id: s.id,
        position: s.position,
        title: s.title,
        description: s.description,
        start_time: s.start_time,
        end_time: s.end_time,
        avg_minutes: s.avg_minutes,
        sigma_minutes: s.sigma_minutes > 0 ? s.sigma_minutes : undefined,
        parallelizable: s.parallelizable,
        allows_parallel: s.allows_parallel,
        abandonability: s.abandonability,
        fixed: s.fixed,
        depends_on: filtered,
      };
    });
    try {
      await client.replaceHabitSteps(habit.id, newSteps);
    } catch (e) {
      showError(e, '冗長な依存の削除に失敗');
      throw e;
    }
    undoRedo.push({
      description: `remove redundant step dep`,
      undo: async () => {
        await client.replaceHabitSteps(
          habit.id,
          prevSteps.map((s) => ({
            id: s.id,
            position: s.position,
            title: s.title,
            description: s.description,
            start_time: s.start_time,
            end_time: s.end_time,
            avg_minutes: s.avg_minutes,
            sigma_minutes: s.sigma_minutes > 0 ? s.sigma_minutes : undefined,
            parallelizable: s.parallelizable,
            allows_parallel: s.allows_parallel,
            abandonability: s.abandonability,
            fixed: s.fixed,
            depends_on: parseDependsOn(s.depends_on),
          })),
        );
        await refresh();
      },
      redo: async () => {
        await client.replaceHabitSteps(habit.id, newSteps);
        await refresh();
      },
    });
    await refresh();
  }

  async function toggleActive() {
    setMenuVisible(false);
    if (!client || !habit) return;
    const next = !habit.active;
    const prev = habit.active;
    try {
      await client.updateHabit(habit.id, { active: next });
    } catch (e) {
      showError(e, 'アクティブ状態の変更に失敗');
      return;
    }
    undoRedo.push({
      description: `${next ? '有効化' : '無効化'} habit: ${habit.title}`,
      undo: async () => {
        await client.updateHabit(habit.id, { active: prev });
        await refresh();
      },
      redo: async () => {
        await client.updateHabit(habit.id, { active: next });
        await refresh();
      },
    });
    await refresh();
  }

  function openSpanModal() {
    setMenuVisible(false);
    setSpanFrom(null);
    setSpanTo(null);
    setSpanReason('');
    setShowSpanModal(true);
  }

  async function addSpan() {
    if (!client || !habit || !spanFrom || !spanTo) return;
    if (spanTo < spanFrom) {
      showError(
        '終了日は開始日以降にしてください',
        habit.active ? '休止期間' : 'アクティブ期間',
      );
      return;
    }
    const body = {
      start_date: dateToYMD(spanFrom),
      end_date: dateToYMD(spanTo),
      reason: spanReason.trim() || undefined,
    };
    let created: HabitScheduledSpanRow;
    try {
      created = await client.createHabitScheduledSpan(habit.id, body);
    } catch (e) {
      showError(
        e,
        habit.active ? '休止期間の追加に失敗' : 'アクティブ期間の追加に失敗',
      );
      return;
    }
    setShowSpanModal(false);
    let currentSpanId = created.id;
    undoRedo.push({
      description: `add ${habit.active ? 'pause' : 'activation window'}: ${habit.title}`,
      undo: async () => {
        await client.deleteHabitScheduledSpan(habit.id, currentSpanId);
        await refresh();
      },
      redo: async () => {
        const recreated = await client.createHabitScheduledSpan(habit.id, body);
        currentSpanId = recreated.id;
        await refresh();
      },
    });
    await refresh();
  }

  async function deleteSpan(spanId: string) {
    if (!client || !habit) return;
    const prev = spans.find((p) => p.id === spanId);
    if (!prev) return;
    try {
      await client.deleteHabitScheduledSpan(habit.id, spanId);
    } catch (e) {
      showError(
        e,
        habit.active ? '休止期間の削除に失敗' : 'アクティブ期間の削除に失敗',
      );
      return;
    }
    let currentSpanId = spanId;
    undoRedo.push({
      description: `delete ${habit.active ? 'pause' : 'activation window'}: ${habit.title}`,
      undo: async () => {
        const recreated = await client.createHabitScheduledSpan(habit.id, {
          start_date: prev.start_date,
          end_date: prev.end_date,
          reason: prev.reason,
        });
        currentSpanId = recreated.id;
        await refresh();
      },
      redo: async () => {
        await client.deleteHabitScheduledSpan(habit.id, currentSpanId);
        await refresh();
      },
    });
    await refresh();
  }

  // Is today within a scheduled span? (for highlighting the active span.)
  function spanIsActive(p: HabitScheduledSpanRow): boolean {
    const todayStr = todayDateKey(serverTz);
    return p.start_date <= todayStr && todayStr <= p.end_date;
  }

  function formatSpanDate(s: string): string {
    // YYYY-MM-DD → M/D
    const [, m, d] = s.split('-').map((n) => parseInt(n, 10));
    return `${m}/${d}`;
  }

  if (!habit) {
    return (
      <View style={[styles.container, { backgroundColor: colors.white }]}>
        <Text style={[styles.loading, { color: colors.gray }]}>
          読み込み中...
        </Text>
      </View>
    );
  }

  const hasSteps = stepDrafts.length > 0;
  const showSimpleInfo = !hasSteps || simpleInfoExpanded;

  // Labels for scheduled spans depend on `habit.active` (#503):
  // active habit → pause, disabled habit → activation window.
  const spanLabel = habit.active ? '休止期間' : 'アクティブ期間 (scheduled)';
  const spanAddLabel = habit.active ? '休止期間' : 'アクティブ期間';
  const spanMenuTitle = habit.active
    ? '休止期間を追加...'
    : 'アクティブ期間を追加...';
  const spanIcon = habit.active
    ? 'pause-circle-outline'
    : 'play-circle-outline';
  const spanActiveColor = habit.active ? colors.red : BRAND_COLOR;

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View style={[styles.topBar, { paddingTop: 4 + insets.top }]}>
        <IconButton
          icon="chevron-left"
          iconColor={BRAND_COLOR}
          size={28}
          onPress={() => {
            haptic.light();
            router.back();
          }}
        />
        <View style={{ flex: 1 }} />
        {editing && (
          <>
            <IconButton
              icon="check"
              iconColor={colors.white}
              containerColor={BRAND_COLOR}
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
                setEditing(false);
                refresh();
              }}
            />
          </>
        )}
        <Menu
          visible={menuVisible}
          onDismiss={() => setMenuVisible(false)}
          anchor={
            <IconButton
              icon="dots-vertical"
              iconColor={BRAND_COLOR}
              size={24}
              onPress={() => setMenuVisible(true)}
            />
          }
        >
          {editing ? (
            <>
              <Menu.Item
                onPress={() => {
                  setMenuVisible(false);
                  save();
                }}
                title="保存"
                leadingIcon="content-save-outline"
              />
              <Menu.Item
                onPress={() => {
                  setMenuVisible(false);
                  editingRef.current = false;
                  setEditing(false);
                  refresh();
                }}
                title="キャンセル"
                leadingIcon="close"
              />
            </>
          ) : (
            <>
              <Menu.Item
                onPress={() => {
                  setMenuVisible(false);
                  setEditing(true);
                }}
                title="編集"
                leadingIcon="pencil-outline"
              />
              <Menu.Item
                onPress={toggleActive}
                title={habit.active ? '無効化' : '有効化'}
                leadingIcon={
                  habit.active ? 'pause-circle-outline' : 'play-circle-outline'
                }
              />
              <Menu.Item
                onPress={openSpanModal}
                title={spanMenuTitle}
                leadingIcon={spanIcon}
              />
              <Menu.Item
                onPress={deleteHabit}
                title="削除"
                leadingIcon="trash-can-outline"
              />
            </>
          )}
        </Menu>
      </View>

      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: 16 + insets.bottom },
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
            activeOutlineColor={BRAND_COLOR}
            style={styles.titleInput}
            contentStyle={{ fontSize: 20, fontWeight: '600' }}
          />
        ) : (
          <Text style={[styles.title, { color: colors.black }]}>
            {habit.title}
          </Text>
        )}

        {/* Description */}
        <View style={styles.section}>
          <Text style={[styles.label, { color: colors.gray }]}>説明</Text>
          {editing ? (
            <PaperTextInput
              mode="outlined"
              value={description}
              onChangeText={setDescription}
              multiline
              numberOfLines={4}
              outlineColor={colors.separator}
              activeOutlineColor={BRAND_COLOR}
              style={styles.descriptionInput}
            />
          ) : (
            <Text style={[styles.value, { color: colors.black }]}>
              {habit.description || '(なし)'}
            </Text>
          )}
        </View>

        {/* Scheduled spans (#303 / #503) */}
        <View style={styles.section}>
          <Text style={[styles.label, { color: colors.gray }]}>
            {spanLabel}
          </Text>
          {spans.length === 0 ? (
            <Text style={[styles.value, { color: colors.black }]}>(なし)</Text>
          ) : (
            spans.map((p) => (
              <View
                key={p.id}
                style={[
                  styles.spanRow,
                  {
                    backgroundColor: spanIsActive(p)
                      ? colors.surfaceTint
                      : colors.surface,
                    borderColor: spanIsActive(p)
                      ? spanActiveColor
                      : colors.separator,
                  },
                ]}
              >
                <Ionicons
                  name={spanIcon}
                  size={18}
                  color={spanIsActive(p) ? spanActiveColor : colors.gray}
                />
                <Text style={[styles.spanText, { color: colors.black }]}>
                  {formatSpanDate(p.start_date)} 〜 {formatSpanDate(p.end_date)}
                  {p.reason ? `  ${p.reason}` : ''}
                </Text>
                <DeleteConfirmButton
                  onConfirm={() => deleteSpan(p.id)}
                  size={34}
                  iconSize={18}
                  hitSlop={8}
                />
              </View>
            ))
          )}
          <Pressable
            style={[styles.addSpanButton, { borderColor: BRAND_COLOR }]}
            onPress={openSpanModal}
          >
            <Ionicons name="add" size={18} color={BRAND_COLOR} />
            <Text style={styles.addSpanText}>{spanAddLabel}を追加</Text>
          </Pressable>
        </View>

        {/* Recurrence */}
        <View style={styles.section}>
          <View style={styles.rruleHeader}>
            <Text style={[styles.label, { color: colors.gray }]}>
              周期 (RRULE)
            </Text>
            {editing && (
              <Pressable
                style={styles.helpButton}
                onPress={() => setShowRruleBuilder(true)}
                hitSlop={8}
              >
                <Ionicons
                  name="help-circle-outline"
                  size={18}
                  color={BRAND_COLOR}
                />
              </Pressable>
            )}
          </View>
          {editing ? (
            <Pressable
              style={[
                styles.dateField,
                {
                  borderColor: colors.separator,
                  backgroundColor: colors.white,
                },
              ]}
              onPress={() => setShowRruleBuilder(true)}
            >
              <Ionicons name="repeat" size={20} color={BRAND_COLOR} />
              <Text
                style={[styles.dateText, { color: colors.black }]}
                numberOfLines={2}
              >
                {summarizeRule(parseRule(recurrence))}
              </Text>
              <Ionicons
                name="chevron-forward"
                size={18}
                color={colors.grayLight}
              />
            </Pressable>
          ) : (
            <Text style={[styles.value, { color: colors.black }]}>
              {summarizeRule(parseRule(habit.recurrence))}
            </Text>
          )}
        </View>

        {/* Window mode (#window_mode) */}
        <View style={styles.section}>
          <Text style={[styles.label, { color: colors.gray }]}>
            スケジュール枠
          </Text>
          {editing ? (
            <>
              <SegmentedButtons
                value={windowMode}
                onValueChange={(v) => setWindowMode(v as WindowMode)}
                buttons={[
                  {
                    value: WINDOW_MODE_DAY,
                    label: '当日',
                  },
                  {
                    value: WINDOW_MODE_PERIOD,
                    label: '期間内どこでも',
                  },
                ]}
                theme={{ colors: { primary: BRAND_COLOR } }}
              />
              {windowMode === WINDOW_MODE_PERIOD && (
                <Text style={[styles.hint, { color: colors.grayLight }]}>
                  次の周期の直前が締め切りになります
                  {stepDrafts.length > 0 && '・全ステップが期間枠を共有します'}
                </Text>
              )}
            </>
          ) : (
            <Text style={[styles.value, { color: colors.black }]}>
              {habit.window_mode === WINDOW_MODE_PERIOD
                ? '期間内どこでも'
                : '当日'}
            </Text>
          )}
        </View>

        {hasSteps && (
          <View style={styles.section}>
            <Pressable
              style={styles.foldToggle}
              onPress={() => {
                haptic.light();
                setSimpleInfoExpanded((v) => !v);
              }}
            >
              <Ionicons
                name={simpleInfoExpanded ? 'chevron-down' : 'chevron-forward'}
                size={16}
                color={BRAND_COLOR}
              />
              <Text style={[styles.label, { color: BRAND_COLOR }]}>
                Habit 本体の設定（ステップが有効なため無視）
              </Text>
            </Pressable>
          </View>
        )}

        {showSimpleInfo && (
          <>
            {/* Time */}
            <View
              style={[
                styles.section,
                hasSteps && editing && styles.sectionDimmed,
              ]}
              pointerEvents={hasSteps && editing ? 'none' : 'auto'}
            >
              <Text style={[styles.label, { color: colors.gray }]}>時間</Text>
              {editing ? (
                <View style={styles.row}>
                  <Pressable
                    style={[
                      styles.timeField,
                      {
                        borderColor: colors.separator,
                        backgroundColor: colors.white,
                      },
                    ]}
                    onPress={() => {
                      haptic.select();
                      setPickerField('start');
                    }}
                  >
                    <Text
                      style={[styles.timeFieldLabel, { color: colors.gray }]}
                    >
                      開始
                    </Text>
                    <Text
                      style={[styles.timeFieldValue, { color: colors.black }]}
                    >
                      {startTime}
                    </Text>
                  </Pressable>
                  <Pressable
                    style={[
                      styles.timeField,
                      {
                        borderColor: colors.separator,
                        backgroundColor: colors.white,
                      },
                      windowMode === WINDOW_MODE_PERIOD && { opacity: 0.4 },
                    ]}
                    disabled={windowMode === WINDOW_MODE_PERIOD}
                    onPress={() => {
                      haptic.select();
                      setPickerField('end');
                    }}
                  >
                    <Text
                      style={[styles.timeFieldLabel, { color: colors.gray }]}
                    >
                      終了
                    </Text>
                    <Text
                      style={[styles.timeFieldValue, { color: colors.black }]}
                    >
                      {endTime}
                    </Text>
                  </Pressable>
                </View>
              ) : (
                <Text style={[styles.value, { color: colors.black }]}>
                  {habit.start_time} → {habit.end_time}
                </Text>
              )}
              {editing && windowMode === WINDOW_MODE_PERIOD && (
                <Text style={[styles.hint, { color: colors.grayLight }]}>
                  終了時刻は次の周期の直前が自動設定されます
                </Text>
              )}
            </View>

            {/* Cost */}
            <View
              style={[
                styles.section,
                stepDrafts.length > 0 && editing && styles.sectionDimmed,
              ]}
              pointerEvents={stepDrafts.length > 0 && editing ? 'none' : 'auto'}
            >
              <Text style={[styles.label, { color: colors.gray }]}>コスト</Text>
              {editing ? (
                <View style={styles.row}>
                  <PaperTextInput
                    mode="outlined"
                    label="avg (1h30m / 90m / 90)"
                    value={avgMinutes}
                    onChangeText={setAvgMinutes}
                    autoCapitalize="none"
                    autoCorrect={false}
                    outlineColor={colors.separator}
                    activeOutlineColor={BRAND_COLOR}
                    style={[styles.costInput, { flex: 1 }]}
                    dense
                  />
                  <View style={[styles.costInput, { flex: 1 }]}>
                    <PaperTextInput
                      mode="outlined"
                      label="sigma (1h30m / 90m / 90)"
                      value={sigmaMinutes}
                      onChangeText={setSigmaMinutes}
                      autoCapitalize="none"
                      autoCorrect={false}
                      outlineColor={colors.separator}
                      activeOutlineColor={BRAND_COLOR}
                      dense
                    />
                    {sigmaMinutes === '' && (
                      <Text
                        style={[styles.costHint, { color: colors.grayLight }]}
                      >
                        {habit.sigma_minutes}m
                      </Text>
                    )}
                  </View>
                </View>
              ) : (
                <>
                  <Text style={[styles.value, { color: colors.black }]}>
                    avg: {habit.avg_minutes}m, sigma:{' '}
                    {habit.sigma_minutes > 0 ? (
                      `${habit.sigma_minutes}m`
                    ) : (
                      <Text style={{ color: colors.grayLight }}>0m</Text>
                    )}
                  </Text>
                  {!habit.fixed && (
                    <Pressable
                      style={[
                        styles.estimateButton,
                        { backgroundColor: colors.surfaceTint },
                      ]}
                      onPress={() => {
                        haptic.light();
                        setShowEstimateModal(true);
                      }}
                    >
                      <Ionicons
                        name="calculator-outline"
                        size={16}
                        color={BRAND_COLOR}
                      />
                      <Text
                        style={[
                          styles.estimateButtonText,
                          { color: BRAND_COLOR },
                        ]}
                      >
                        実績から見積もり
                      </Text>
                    </Pressable>
                  )}
                </>
              )}
            </View>

            {/* Abandonability */}
            <View
              style={[
                styles.section,
                stepDrafts.length > 0 && editing && styles.sectionDimmed,
              ]}
              pointerEvents={stepDrafts.length > 0 && editing ? 'none' : 'auto'}
            >
              <Text style={[styles.label, { color: colors.gray }]}>
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
                    minimumTrackTintColor={BRAND_COLOR}
                    style={styles.slider}
                  />
                  <Text style={[styles.sliderValue, { color: BRAND_COLOR }]}>
                    {abandonability.toFixed(2)}
                  </Text>
                </View>
              ) : (
                <Text style={[styles.value, { color: colors.black }]}>
                  {habit.abandonability.toFixed(2)}
                </Text>
              )}
            </View>

            {/* Parallel config */}
            <View
              style={[
                styles.section,
                stepDrafts.length > 0 && editing && styles.sectionDimmed,
              ]}
              pointerEvents={stepDrafts.length > 0 && editing ? 'none' : 'auto'}
            >
              <Text style={[styles.label, { color: colors.gray }]}>
                並列設定
              </Text>
              {editing ? (
                <View style={styles.toggleRow}>
                  <Pressable
                    style={styles.toggleItem}
                    onPress={() => setParallelizable(!parallelizable)}
                  >
                    <Text style={[styles.toggleLabel, { color: colors.black }]}>
                      並列実行可能
                    </Text>
                    <Checkbox
                      status={parallelizable ? 'checked' : 'unchecked'}
                      onPress={() => setParallelizable(!parallelizable)}
                      color={BRAND_COLOR}
                    />
                  </Pressable>
                  <Pressable
                    style={styles.toggleItem}
                    onPress={() => setAllowsParallel(!allowsParallel)}
                  >
                    <Text style={[styles.toggleLabel, { color: colors.black }]}>
                      並列受け入れ
                    </Text>
                    <Checkbox
                      status={allowsParallel ? 'checked' : 'unchecked'}
                      onPress={() => setAllowsParallel(!allowsParallel)}
                      color={BRAND_COLOR}
                    />
                  </Pressable>
                </View>
              ) : (
                <View style={styles.toggleRow}>
                  <View style={styles.toggleItem}>
                    <Text style={[styles.toggleLabel, { color: colors.black }]}>
                      並列実行可能
                    </Text>
                    <Checkbox
                      status={habit.parallelizable ? 'checked' : 'unchecked'}
                      disabled
                      color={BRAND_COLOR}
                    />
                  </View>
                  <View style={styles.toggleItem}>
                    <Text style={[styles.toggleLabel, { color: colors.black }]}>
                      並列受け入れ
                    </Text>
                    <Checkbox
                      status={habit.allows_parallel ? 'checked' : 'unchecked'}
                      disabled
                      color={BRAND_COLOR}
                    />
                  </View>
                </View>
              )}
            </View>

            {/* Fixed */}
            <View
              style={[
                styles.section,
                stepDrafts.length > 0 && editing && styles.sectionDimmed,
              ]}
              pointerEvents={stepDrafts.length > 0 && editing ? 'none' : 'auto'}
            >
              <Text style={[styles.label, { color: colors.gray }]}>
                時間固定
              </Text>
              {editing ? (
                <>
                  <Checkbox
                    status={fixed ? 'checked' : 'unchecked'}
                    onPress={() => setFixed(!fixed)}
                    color={BRAND_COLOR}
                  />
                  <Text style={[styles.hint, { color: colors.grayLight }]}>
                    開始時刻を固定し、スケジューラの移動を許可しない
                  </Text>
                </>
              ) : (
                <Checkbox
                  status={habit.fixed ? 'checked' : 'unchecked'}
                  disabled
                  color={BRAND_COLOR}
                />
              )}
            </View>
          </>
        )}

        {/* Active */}
        <View style={styles.section}>
          <Text style={[styles.label, { color: colors.gray }]}>アクティブ</Text>
          {editing ? (
            <Checkbox
              status={active ? 'checked' : 'unchecked'}
              onPress={() => setActive(!active)}
              color={BRAND_COLOR}
            />
          ) : (
            <Checkbox
              status={habit.active ? 'checked' : 'unchecked'}
              disabled
              color={BRAND_COLOR}
            />
          )}
        </View>

        {/* Steps (#95) */}
        <View style={styles.section}>
          <Text style={[styles.label, { color: colors.gray }]}>ステップ</Text>
          {stepDrafts.length > 0 && editing && (
            <Text style={[styles.hint, { color: colors.grayLight }]}>
              ステップ設定が優先されます (habit 本体の時間帯・コストは無効)
            </Text>
          )}
          {!editing && stepDrafts.length > 0 && (
            <RedundantDepWarning
              edges={stepRedundantEdges}
              onResolve={resolveStepRedundantEdge}
              nodeLabel={(nid, ntitle) => {
                const idx = habit.steps.findIndex((s) => s.id === nid);
                return idx >= 0
                  ? `${idx + 1}. ${habit.steps[idx]!.title || '(無題)'}`
                  : ntitle;
              }}
            />
          )}
          {editing ? (
            <HabitStepEditor
              drafts={stepDrafts}
              onChange={setStepDrafts}
              stepsActive={stepDrafts.length > 0}
            />
          ) : stepDrafts.length === 0 ? (
            <Text style={[styles.value, { color: colors.black }]}>(なし)</Text>
          ) : (
            <View style={styles.stepList}>
              {stepDrafts.map((d, i) => {
                const depLabels = d.depends_on
                  .map((t) => stepDrafts.find((x) => x.tempId === t))
                  .filter(Boolean)
                  .map(
                    (x) =>
                      `${stepDrafts.indexOf(x!) + 1}.${x!.title || '(無題)'}`,
                  );
                return (
                  <View
                    key={d.tempId}
                    style={[
                      styles.stepViewCard,
                      { backgroundColor: colors.surface },
                    ]}
                  >
                    <Text style={[styles.stepViewIdx, { color: BRAND_COLOR }]}>
                      {i + 1}
                    </Text>
                    <View style={styles.stepViewBody}>
                      <Text
                        style={[styles.stepViewTitle, { color: colors.black }]}
                      >
                        {d.title || '(無題)'}
                      </Text>
                      <Text
                        style={[styles.stepViewMeta, { color: colors.gray }]}
                      >
                        {d.start_time}-{d.end_time} · {d.avg_minutes}m
                        {d.sigma_minutes > 0 ? `±${d.sigma_minutes}` : ''}
                        {depLabels.length > 0
                          ? ` · 依存: ${depLabels.join(', ')}`
                          : ''}
                      </Text>
                    </View>
                  </View>
                );
              })}
            </View>
          )}
        </View>

        {/* Recent generated tasks */}
        <View style={styles.section}>
          <Text style={[styles.label, { color: colors.gray }]}>
            直近のタスク
          </Text>
          {tasks.length === 0 ? (
            <Text style={[styles.value, { color: colors.black }]}>(なし)</Text>
          ) : (
            tasks.map((t) => (
              <Pressable
                key={t.id}
                style={[styles.taskItem, { backgroundColor: colors.surface }]}
                onPress={() => {
                  haptic.light();
                  router.push(`/task/${t.id}`);
                }}
              >
                <Text style={[styles.taskItemTitle, { color: colors.black }]}>
                  {t.title}
                </Text>
                <Text style={[styles.taskItemStatus, { color: colors.gray }]}>
                  {t.status}
                </Text>
              </Pressable>
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
          <Pressable
            style={[styles.saveBarButton, { backgroundColor: BRAND_COLOR }]}
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

      <RruleBuilderModal
        visible={showRruleBuilder}
        value={recurrence}
        onConfirm={(json) => {
          setRecurrence(json);
          setShowRruleBuilder(false);
        }}
        onCancel={() => setShowRruleBuilder(false)}
      />

      {habit && client && (
        <HabitEstimateModal
          visible={showEstimateModal}
          habitId={habit.id}
          client={client}
          onClose={() => setShowEstimateModal(false)}
          onApplied={async () => {
            await refresh();
          }}
        />
      )}

      <DateTimePickerModal
        visible={pickerField !== null}
        mode="time"
        label={pickerField === 'start' ? '開始時刻' : '終了時刻'}
        value={timeStringToDate(pickerField === 'start' ? startTime : endTime)}
        onConfirm={(date) => {
          if (date) {
            const s = dateToTimeString(date);
            if (pickerField === 'start') setStartTime(s);
            else setEndTime(s);
          }
          setPickerField(null);
        }}
        onCancel={() => setPickerField(null)}
      />

      {/* Scheduled-span add modal (#303 / #503) */}
      <Modal
        visible={showSpanModal}
        transparent
        animationType="slide"
        onRequestClose={() => setShowSpanModal(false)}
      >
        <Pressable
          style={spanStyles.overlay}
          onPress={() => setShowSpanModal(false)}
        >
          <Pressable
            style={[
              spanStyles.sheet,
              {
                backgroundColor: colors.white,
                paddingBottom: 24 + insets.bottom,
              },
            ]}
            onPress={(e) => e.stopPropagation()}
          >
            <View style={spanStyles.header}>
              <Text style={[spanStyles.title, { color: colors.black }]}>
                {spanAddLabel}を追加
              </Text>
              <Pressable onPress={() => setShowSpanModal(false)}>
                <Ionicons name="close" size={24} color={colors.gray} />
              </Pressable>
            </View>

            <Pressable
              style={[spanStyles.fieldRow, { borderColor: colors.separator }]}
              onPress={() => {
                haptic.select();
                setSpanPicker('from');
              }}
            >
              <Ionicons name="calendar-outline" size={20} color={BRAND_COLOR} />
              <Text style={[spanStyles.fieldLabel, { color: colors.gray }]}>
                開始日
              </Text>
              <Text style={[spanStyles.fieldValue, { color: colors.black }]}>
                {spanFrom ? dateToYMD(spanFrom) : '選択…'}
              </Text>
            </Pressable>

            <Pressable
              style={[spanStyles.fieldRow, { borderColor: colors.separator }]}
              onPress={() => {
                haptic.select();
                setSpanPicker('to');
              }}
            >
              <Ionicons name="calendar-outline" size={20} color={BRAND_COLOR} />
              <Text style={[spanStyles.fieldLabel, { color: colors.gray }]}>
                終了日
              </Text>
              <Text style={[spanStyles.fieldValue, { color: colors.black }]}>
                {spanTo ? dateToYMD(spanTo) : '選択…'}
              </Text>
            </Pressable>

            <PaperTextInput
              mode="outlined"
              label="理由 (任意)"
              value={spanReason}
              onChangeText={setSpanReason}
              outlineColor={colors.separator}
              activeOutlineColor={BRAND_COLOR}
              style={{ marginTop: 8 }}
            />

            <View style={spanStyles.actionRow}>
              <Pressable
                style={[
                  spanStyles.cancelButton,
                  { borderColor: colors.separator },
                ]}
                onPress={() => setShowSpanModal(false)}
              >
                <Text
                  style={[spanStyles.cancelText, { color: colors.grayDark }]}
                >
                  キャンセル
                </Text>
              </Pressable>
              <Pressable
                style={[
                  spanStyles.confirmButton,
                  { backgroundColor: BRAND_COLOR },
                  (!spanFrom || !spanTo) && { opacity: 0.4 },
                ]}
                disabled={!spanFrom || !spanTo}
                onPress={() => {
                  haptic.medium();
                  addSpan();
                }}
              >
                <Text style={[spanStyles.confirmText, { color: colors.white }]}>
                  追加
                </Text>
              </Pressable>
            </View>
          </Pressable>
        </Pressable>
      </Modal>

      <DateTimePickerModal
        visible={spanPicker !== null}
        mode="date"
        label={spanPicker === 'from' ? '開始日' : '終了日'}
        value={
          spanPicker === 'from'
            ? (spanFrom ?? new Date())
            : (spanTo ?? spanFrom ?? new Date())
        }
        minimumDate={spanPicker === 'to' ? (spanFrom ?? undefined) : undefined}
        onConfirm={(date) => {
          if (date) {
            if (spanPicker === 'from') setSpanFrom(date);
            else setSpanTo(date);
          }
          setSpanPicker(null);
        }}
        onCancel={() => setSpanPicker(null)}
      />
    </View>
  );
}

const styles = StyleSheet.create({
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
  section: {
    gap: 4,
  },
  foldToggle: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  label: {
    fontSize: 13,
    fontWeight: '500',
  },
  hint: {
    fontSize: 11,
    marginTop: 2,
  },
  value: {
    fontSize: 16,
  },
  estimateButton: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    alignSelf: 'flex-start',
    marginTop: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderRadius: 10,
  },
  estimateButtonText: {
    fontSize: 13,
    fontWeight: '600',
  },
  descriptionInput: {
    minHeight: 80,
  },
  rruleHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
  },
  helpButton: {
    padding: 2,
  },
  dateField: {
    flexDirection: 'row',
    alignItems: 'center',
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 12,
    gap: 8,
  },
  dateText: {
    flex: 1,
    fontSize: 16,
  },
  row: {
    flexDirection: 'row',
    gap: 12,
  },
  timeField: {
    flex: 1,
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    gap: 2,
  },
  timeFieldLabel: {
    fontSize: 12,
    fontWeight: '500',
  },
  timeFieldValue: {
    fontSize: 16,
  },
  costInput: {},
  costHint: {
    fontSize: 11,
    marginTop: 2,
    marginLeft: 4,
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
  taskItem: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    paddingVertical: 12,
    paddingHorizontal: 12,
    borderRadius: 8,
    marginTop: 4,
  },
  taskItemTitle: {
    fontSize: 14,
    flex: 1,
  },
  taskItemStatus: {
    fontSize: 12,
  },
  saveBar: {
    paddingHorizontal: 16,
    paddingTop: 8,
    borderTopWidth: 1,
    borderTopColor: '#eee',
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
  sectionDimmed: {
    opacity: 0.45,
  },
  spanRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 10,
    marginTop: 4,
  },
  spanText: {
    flex: 1,
    fontSize: 14,
  },
  addSpanButton: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderRadius: 8,
    paddingVertical: 8,
    marginTop: 6,
  },
  addSpanText: {
    color: BRAND_COLOR,
    fontSize: 13,
    fontWeight: '500',
  },
  stepList: {
    gap: 6,
    marginTop: 4,
  },
  stepViewCard: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
  },
  stepViewIdx: {
    fontSize: 16,
    fontWeight: '700',
    minWidth: 20,
  },
  stepViewBody: {
    flex: 1,
    gap: 2,
  },
  stepViewTitle: {
    fontSize: 15,
    fontWeight: '500',
  },
  stepViewMeta: {
    fontSize: 12,
  },
});

const spanStyles = StyleSheet.create({
  overlay: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.4)',
    justifyContent: 'flex-end',
  },
  sheet: {
    borderTopLeftRadius: 20,
    borderTopRightRadius: 20,
    padding: 20,
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
  fieldRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    borderWidth: 1,
    borderRadius: 10,
    paddingHorizontal: 12,
    paddingVertical: 14,
    marginBottom: 8,
  },
  fieldLabel: {
    flex: 1,
    fontSize: 15,
  },
  fieldValue: {
    fontSize: 15,
    fontWeight: '500',
  },
  actionRow: {
    flexDirection: 'row',
    gap: 12,
    marginTop: 16,
  },
  cancelButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 10,
    borderWidth: 1,
    alignItems: 'center',
  },
  cancelText: {
    fontSize: 15,
  },
  confirmButton: {
    flex: 1,
    paddingVertical: 12,
    borderRadius: 10,
    alignItems: 'center',
  },
  confirmText: {
    fontSize: 15,
    fontWeight: '600',
  },
});
