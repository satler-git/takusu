// HabitView — list of habit cards with add button
// Habits are selectable, context menu (left) changes with selection

import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  FlatList,
  Pressable,
  RefreshControl,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useRouter } from 'expo-router';
import { IconButton } from 'react-native-paper';
import { Ionicons } from '@expo/vector-icons';
import type { TakusuClient } from '@/src/api/client';
import { showError, logError } from '@/src/api/errors';
import { parseDepends } from '@/src/api/types';
import type {
  HabitScheduledSpanRow,
  HabitRow,
  HabitStepRow,
  TaskRow,
} from '@/src/api/types';
import { WINDOW_MODE_PERIOD } from '@/src/api/types';
import { BRAND_COLOR, useColors } from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { ContextMenu } from '@/src/components/ContextMenu';
import { haptic } from '@/src/components/haptics';
import { useUndoableToast } from '@/src/hooks/useUndoableToast';
import { undoRedo } from '@/src/api/undoRedo';
import { parseRule, summarizeRule } from '@/src/api/rrule';
import { stepRowToDraft, saveHabitSteps } from '@/src/utils/habitSteps';
import { todayDateKey } from '@/src/utils/dateKey';

interface HabitViewProps {
  client: TakusuClient | null;
  refreshKey?: number | null;
}

export function HabitView({ client, refreshKey }: HabitViewProps) {
  const router = useRouter();
  const colors = useColors();
  const insets = useSafeAreaInsets();
  const showUndoToast = useUndoableToast();
  const [habits, setHabits] = useState<HabitRow[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  // Badge data: step counts per habit id, and active scheduled span per habit id.
  const [stepCounts, setStepCounts] = useState<Map<string, number>>(new Map());
  const [activeSpans, setActiveSpans] = useState<
    Map<string, HabitScheduledSpanRow>
  >(new Map());
  const [spanHabits, setSpanHabits] = useState<Set<string>>(new Set());

  const refresh = useCallback(async () => {
    if (!client) return;
    setRefreshing(true);
    try {
      const [habitsData, allSteps, allSpans, settings] = await Promise.all([
        client.listHabits(),
        client.listAllHabitSteps().catch((e) => {
          logError('ステップ一覧取得', e);
          return [];
        }),
        client.listAllHabitScheduledSpans().catch((e) => {
          logError('スケジュール済み期間一覧取得', e);
          return [];
        }),
        client.getSettings().catch((e) => {
          logError('設定取得', e);
          return null;
        }),
      ]);
      setHabits(habitsData);
      // Build step count map.
      const counts = new Map<string, number>();
      for (const s of allSteps) {
        counts.set(s.habit_id, (counts.get(s.habit_id) ?? 0) + 1);
      }
      setStepCounts(counts);
      // Build active-span map: a span whose [start, end] contains today.
      const todayStr = todayDateKey(settings?.tz ?? undefined);
      const active = new Map<string, HabitScheduledSpanRow>();
      const spanHabitsSet = new Set<string>();
      for (const s of allSpans) {
        spanHabitsSet.add(s.habit_id);
        if (s.start_date <= todayStr && todayStr <= s.end_date) {
          // Keep the latest-ending active span if multiple.
          const prev = active.get(s.habit_id);
          if (!prev || s.end_date > prev.end_date) {
            active.set(s.habit_id, s);
          }
        }
      }
      setActiveSpans(active);
      setSpanHabits(spanHabitsSet);
    } catch (e) {
      showError(e, 'Habit一覧の取得に失敗');
    } finally {
      setRefreshing(false);
    }
  }, [client]);

  useEffect(() => {
    refresh();
  }, [refresh, refreshKey]);

  // Refresh the habit list whenever an undoable action is pushed/undone/redone
  // so undo from another view (e.g. HabitDetailView) is reflected here.
  useEffect(() => {
    return undoRedo.subscribe(() => {
      if (client) refresh();
    });
  }, [client, refresh]);

  async function deleteSelected() {
    if (!client) return;
    const toDelete = habits.filter((h) => selected.has(h.id));
    if (toDelete.length === 0) return;
    // #240: confirm before batch-deleting, showing how many associated
    // tasks will also be cascade-deleted. Capture the tasks per habit
    // so undo can restore them alongside the recreated habits.
    let tasksPerHabit: TaskRow[][];
    let stepsPerHabit: HabitStepRow[][];
    let spansPerHabit: HabitScheduledSpanRow[][];
    try {
      [tasksPerHabit, stepsPerHabit, spansPerHabit] = await Promise.all([
        Promise.all(toDelete.map((h) => client!.listTasks({ habit_id: h.id }))),
        Promise.all(
          toDelete.map((h) =>
            client!.listHabitSteps(h.id).catch(() => [] as HabitStepRow[]),
          ),
        ),
        Promise.all(
          toDelete.map((h) =>
            client!
              .listHabitScheduledSpans(h.id)
              .catch(() => [] as HabitScheduledSpanRow[]),
          ),
        ),
      ]);
    } catch (e) {
      showError(e, 'ハビットのタスク取得に失敗');
      return;
    }
    const deleted: HabitRow[] = [];
    const deletedTasksPerHabit: TaskRow[][] = [];
    const deletedStepsPerHabit: HabitStepRow[][] = [];
    const deletedSpansPerHabit: HabitScheduledSpanRow[][] = [];
    let failed = 0;
    for (let i = 0; i < toDelete.length; i++) {
      const h = toDelete[i];
      try {
        await client.deleteHabit(h.id);
        deleted.push(h);
        deletedTasksPerHabit.push(tasksPerHabit[i] ?? []);
        deletedStepsPerHabit.push(stepsPerHabit[i] ?? []);
        deletedSpansPerHabit.push(spansPerHabit[i] ?? []);
      } catch (e) {
        failed++;
        logError(`ハビット削除 (${h.id})`, e);
      }
    }
    if (failed > 0) {
      showError(`${failed}件の削除に失敗しました`, 'Habitの削除');
    }
    if (deleted.length === 0) return;
    // Track the ids assigned by the server when undo recreates the habits,
    // so redo deletes the recreated (not the stale original) ids.
    // Push a single grouped undo entry so one undo restores all habits.
    const currentIds: string[] = [...deleted.map((h) => h.id)];
    // Track which habits have been recreated so a retry after partial
    // failure doesn't create duplicates.
    const createdIdx = new Set<number>();
    // Flatten all cascade-deleted tasks across habits into a single
    // list so dependency remapping can handle deps that span habits.
    // Each entry remembers which habit it belonged to so we can set
    // habit_id correctly after the habit is recreated.
    const flatTasks: { task: TaskRow; habitIdx: number }[] = [];
    for (let i = 0; i < deletedTasksPerHabit.length; i++) {
      for (const t of deletedTasksPerHabit[i]) {
        flatTasks.push({ task: t, habitIdx: i });
      }
    }
    const currentTaskIds: string[] = flatTasks.map((ft) => ft.task.id);
    const taskCreatedIdx = new Set<number>();
    const totalTasks = flatTasks.length;

    const message =
      deleted.length === 1
        ? totalTasks > 0
          ? `「${deleted[0].title}」と関連する${totalTasks}件のタスクを削除しました`
          : `「${deleted[0].title}」を削除しました`
        : totalTasks > 0
          ? `${deleted.length}件のハビットと関連する${totalTasks}件のタスクを削除しました`
          : `${deleted.length}件のハビットを削除しました`;
    showUndoToast(message);

    undoRedo.push({
      description:
        deleted.length === 1
          ? totalTasks > 0
            ? `delete habit + ${totalTasks} tasks: ${deleted[0].title}`
            : `delete habit: ${deleted[0].title}`
          : totalTasks > 0
            ? `delete ${deleted.length} habits + ${totalTasks} tasks`
            : `delete ${deleted.length} habits`,
      undo: async () => {
        // First pass: recreate any habits not yet restored.
        for (let i = 0; i < deleted.length; i++) {
          if (createdIdx.has(i)) continue;
          const h = deleted[i];
          const recreated = await client.createHabit({
            title: h.title,
            description: h.description,
            recurrence: h.recurrence,
            start_time: h.start_time,
            end_time: h.end_time,
            avg_minutes: h.avg_minutes,
            sigma_minutes: h.sigma_minutes,
            parallelizable: h.parallelizable,
            allows_parallel: h.allows_parallel,
            abandonability: h.abandonability,
            fixed: h.fixed,
            window_mode: h.window_mode,
          });
          if (!h.active) {
            await client.updateHabit(recreated.id, { active: h.active });
          }
          // Restore steps (#95).
          const steps = deletedStepsPerHabit[i] ?? [];
          if (steps.length > 0) {
            await saveHabitSteps(
              client,
              recreated.id,
              steps.map(stepRowToDraft),
            );
          }
          // Restore scheduled spans (#303 / #503).
          const spans = deletedSpansPerHabit[i] ?? [];
          for (const s of spans) {
            await client.createHabitScheduledSpan(recreated.id, {
              start_date: s.start_date,
              end_date: s.end_date,
              reason: s.reason,
            });
          }
          currentIds[i] = recreated.id;
          createdIdx.add(i);
        }
        // Second pass: recreate tasks with no deps (two-pass dep remap
        // mirrors HomeView's batch-delete undo). All habits are now
        // recreated so habit_id is always valid.
        const oldToNew = new Map<string, string>();
        for (let i = 0; i < flatTasks.length; i++) {
          if (taskCreatedIdx.has(i)) {
            oldToNew.set(flatTasks[i].task.id, currentTaskIds[i]);
            continue;
          }
          const { task: t, habitIdx } = flatTasks[i];
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
            habit_id: currentIds[habitIdx],
            fixed: t.fixed,
          });
          if (t.status !== 'pending') {
            await client.updateTask(recreatedTask.id, { status: t.status });
          }
          currentTaskIds[i] = recreatedTask.id;
          oldToNew.set(t.id, recreatedTask.id);
          taskCreatedIdx.add(i);
        }
        // Third pass: remap depends to new IDs for deps within the
        // deleted set (deps on tasks outside the set are left as-is).
        for (let i = 0; i < flatTasks.length; i++) {
          const t = flatTasks[i].task;
          const origDeps = parseDepends(t.depends);
          if (origDeps.length === 0) continue;
          const newId = oldToNew.get(t.id)!;
          const remapped = origDeps.map((d) => oldToNew.get(d) ?? d);
          await client.updateTask(newId, { depends: remapped });
        }
      },
      redo: async () => {
        createdIdx.clear();
        taskCreatedIdx.clear();
        for (const id of currentIds) {
          await client.deleteHabit(id);
        }
      },
    });
    setSelected(new Set());
  }

  const toggleSelection = useCallback((id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const handleHabitPress = useCallback(
    (habit: HabitRow) => {
      if (selectedRef.current.size > 0) {
        haptic.light();
        toggleSelection(habit.id);
      } else {
        haptic.light();
        router.push(`/habit/${habit.id}`);
      }
    },
    [toggleSelection, router],
  );

  const handleHabitLongPress = useCallback(
    (habit: HabitRow) => {
      haptic.medium();
      toggleSelection(habit.id);
    },
    [toggleSelection],
  );

  const keyExtractor = useCallback((h: HabitRow) => h.id, []);

  const contentContainerStyle = useMemo(
    () => [styles.listContent, { paddingBottom: 100 + insets.bottom }],
    [insets.bottom],
  );

  const renderItem = useCallback(
    ({ item: h }: { item: HabitRow }) => {
      const isSelected = selectedRef.current.has(h.id);
      const hasSpan = activeSpans.has(h.id);
      const isActive = h.active || hasSpan;
      const span = activeSpans.get(h.id);
      const stepCount = stepCounts.get(h.id) ?? 0;
      const isScheduled =
        (h.active && hasSpan) || (!h.active && spanHabits.has(h.id));

      return (
        <HabitCard
          habit={h}
          selected={isSelected}
          isActive={isActive}
          isScheduled={isScheduled}
          span={span}
          stepCount={stepCount}
          colors={colors}
          onPress={handleHabitPress}
          onLongPress={handleHabitLongPress}
        />
      );
    },
    [
      activeSpans,
      spanHabits,
      stepCounts,
      colors,
      handleHabitPress,
      handleHabitLongPress,
    ],
  );

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View style={[styles.topBar, { paddingTop: 8 + insets.top }]}>
        <ContextMenu
          hasSelection={selected.size > 0}
          onSettings={() => router.push('/settings')}
          onStats={() => router.push('/stats')}
          onUndo={() =>
            undoRedo.undo().catch((e) => showError(e, 'アンドゥに失敗'))
          }
          onRedo={() =>
            undoRedo.redo().catch((e) => showError(e, 'リドゥに失敗'))
          }
          onSelectAll={() => setSelected(new Set(habits.map((h) => h.id)))}
          onClearSelection={() => setSelected(new Set())}
          onDeleteSelected={deleteSelected}
        />
        <View style={styles.topBarCenter}>
          <Text style={[styles.title, { color: colors.black }]}>Habit</Text>
        </View>
        <IconButton
          icon="plus"
          iconColor={colors.white}
          size={24}
          containerColor={BRAND_COLOR}
          onPress={() => {
            haptic.light();
            router.push('/habit/add');
          }}
          style={styles.addButton}
        />
      </View>

      <FlatList
        data={habits}
        keyExtractor={keyExtractor}
        renderItem={renderItem}
        refreshControl={
          <RefreshControl refreshing={refreshing} onRefresh={refresh} />
        }
        contentContainerStyle={contentContainerStyle}
        extraData={selected}
        initialNumToRender={10}
        maxToRenderPerBatch={10}
        windowSize={7}
      />
    </View>
  );
}

interface HabitCardProps {
  habit: HabitRow;
  selected: boolean;
  isActive: boolean;
  isScheduled: boolean;
  span: HabitScheduledSpanRow | undefined;
  stepCount: number;
  colors: ReturnType<typeof useColors>;
  onPress: (habit: HabitRow) => void;
  onLongPress: (habit: HabitRow) => void;
}

const HabitCard = memo(function HabitCardImpl({
  habit,
  selected,
  isActive,
  isScheduled,
  span,
  stepCount,
  colors,
  onPress,
  onLongPress,
}: HabitCardProps) {
  return (
    <Pressable
      style={[
        styles.habitCard,
        {
          backgroundColor: colors.surface,
          borderColor: colors.separator,
        },
        selected && styles.habitCardSelected,
        habit.active && span && styles.habitCardPaused,
      ]}
      onPress={() => onPress(habit)}
      onLongPress={() => onLongPress(habit)}
    >
      <View style={styles.habitHeader}>
        <Text
          style={[
            styles.habitTitle,
            {
              color: isActive ? colors.black : colors.gray,
              textDecorationLine: isActive ? 'none' : 'line-through',
            },
          ]}
        >
          {habit.title}
        </Text>
        <View style={styles.badgeRow}>
          {habit.window_mode === WINDOW_MODE_PERIOD && (
            <View
              style={[styles.chip, { backgroundColor: colors.surfaceTint }]}
            >
              <Ionicons
                name="calendar-number-outline"
                size={11}
                color={BRAND_COLOR}
              />
              <Text style={[styles.chipText, { color: BRAND_COLOR }]}>
                自由枠
              </Text>
            </View>
          )}
          {stepCount > 0 && (
            <View
              style={[styles.chip, { backgroundColor: colors.surfaceTint }]}
            >
              <Ionicons name="layers-outline" size={11} color={BRAND_COLOR} />
              <Text style={[styles.chipText, { color: BRAND_COLOR }]}>
                {stepCount} steps
              </Text>
            </View>
          )}
          {isScheduled && (
            <View
              style={[styles.chip, { backgroundColor: colors.surfaceTint }]}
            >
              {habit.active ? (
                <>
                  <Ionicons name="pause-circle" size={11} color={colors.red} />
                  <Text style={[styles.chipText, { color: colors.red }]}>
                    〜{span ? formatSpanShort(span.end_date) : ''}
                  </Text>
                </>
              ) : (
                <>
                  <Ionicons name="play-circle" size={11} color={BRAND_COLOR} />
                  <Text style={[styles.chipText, { color: BRAND_COLOR }]}>
                    {span
                      ? `scheduled 〜${formatSpanShort(span.end_date)}`
                      : 'scheduled'}
                  </Text>
                </>
              )}
            </View>
          )}
        </View>
      </View>
      {(() => {
        const hasSteps = stepCount > 0;
        return (
          <>
            {!hasSteps && (
              <Text style={[styles.habitTime, { color: colors.gray }]}>
                時間: {habit.start_time} → {habit.end_time}
              </Text>
            )}
            <Text style={[styles.habitRecurrence, { color: colors.gray }]}>
              周期: {summarizeRule(parseRule(habit.recurrence))}
            </Text>
            {!hasSteps && (
              <>
                <Text style={[styles.habitCost, { color: colors.gray }]}>
                  {habit.avg_minutes}m ±{habit.sigma_minutes}
                </Text>
                <Text style={[styles.habitParallel, { color: colors.gray }]}>
                  parallel:{' '}
                  {habit.parallelizable && habit.allows_parallel
                    ? 'host+guest'
                    : habit.parallelizable
                      ? 'guest'
                      : habit.allows_parallel
                        ? 'host'
                        : 'none'}
                  {habit.fixed ? ' · fixed' : ''}
                </Text>
                <Text style={[styles.habitAbandon, { color: colors.gray }]}>
                  abandon: {habit.abandonability.toFixed(2)}
                </Text>
              </>
            )}
          </>
        );
      })()}
    </Pressable>
  );
});

// YYYY-MM-DD → M/D
function formatSpanShort(s: string): string {
  const [, m, d] = s.split('-').map((n) => parseInt(n, 10));
  return `${m}/${d}`;
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  topBar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 8,
    paddingBottom: 8,
    gap: 4,
  },
  topBarCenter: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  title: {
    fontSize: 18,
    fontWeight: '600',
  },
  addButton: {
    width: 40,
    height: 40,
    margin: 0,
  },
  listContent: {
    padding: 12,
    gap: 8,
  },
  habitCard: {
    borderRadius: 12,
    padding: 16,
    gap: 4,
    borderWidth: 2,
  },
  habitCardSelected: {
    borderColor: BRAND_COLOR,
  },
  habitHeader: {
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    gap: 8,
  },
  habitTitle: {
    fontSize: 16,
    fontWeight: '600',
    flex: 1,
  },
  badgeRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 4,
    alignItems: 'center',
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 3,
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 10,
  },
  chipText: {
    fontSize: 10,
    fontWeight: '500',
  },
  habitCardPaused: {
    opacity: 0.6,
  },
  habitRecurrence: {
    fontSize: 13,
  },
  habitTime: {
    fontSize: 13,
  },
  habitCost: {
    fontSize: 13,
  },
  habitParallel: {
    fontSize: 13,
  },
  habitAbandon: {
    fontSize: 13,
  },
});
