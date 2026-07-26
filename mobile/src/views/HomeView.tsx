// Home (Task) view — the main screen
// Top bar: hamburger menu, search button, sync button
// Middle: task cards in chronological order (pending on top, date separators)
// Bottom: add button (center), start&done button (right)
// Pull-down-to-reveal past days

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import Constants from 'expo-constants';
import {
  Alert,
  FlatList,
  Pressable,
  StyleSheet,
  Text,
  View,
  RefreshControl,
  type NativeSyntheticEvent,
  type NativeScrollEvent,
  type LayoutChangeEvent,
} from 'react-native';
import { useRouter, useFocusEffect } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useServer } from '@/src/api/ServerProvider';
import { TakusuClient } from '@/src/api/client';
import { undoRedo } from '@/src/api/undoRedo';
import { showError, logError } from '@/src/api/errors';
import type { TaskRow, TaskStatus, ScheduleEntry } from '@/src/api/types';
import { parseDepends, parseSchedule } from '@/src/api/types';
import { TaskCard, ParallelGroupCard } from '@/src/components/TaskCard';
import { NavigationButtons } from '@/src/components/NavigationButtons';
import { ViewChanger, type ViewType } from '@/src/components/ViewChanger';
import { ContextMenu } from '@/src/components/ContextMenu';
import { TaskSearchBar } from '@/src/components/TaskSearchBar';
import { Ionicons } from '@expo/vector-icons';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  withTiming,
  withSpring,
  runOnJS,
} from 'react-native-reanimated';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { useTopToast } from '@/src/components/TopToast';
import { useUndoableToast } from '@/src/hooks/useUndoableToast';
import { TaskProgressSheet } from '@/src/components/TaskProgressSheet';
import { dateKey, todayDateKey } from '@/src/utils/dateKey';
import TakusuWidgetModule from '../../modules/takusu-widget/src/TakusuWidgetModule';
import { useScheduleOperation } from '@/src/hooks/useScheduleOperation';
import {
  rescheduleFromRaw,
  postInProgressNotification,
  dismissInProgressNotification,
  dismissTaskNotifications,
  cancelScheduledTaskNotifications,
  cancelScheduledStartNotifications,
} from '@/src/notifications';
import {
  makeProgressOperationId,
  recordProgressWithTotal,
  type ProgressPayload,
} from '@/src/utils/progress';
import type { HabitRow } from '@/src/api/types';

interface TaskItem {
  type: 'task';
  task: TaskRow;
  scheduleStart?: string;
  scheduleEnd?: string;
  isDone: boolean;
  dateKey: string;
}

interface ParallelGroupItem {
  type: 'parallelGroup';
  host: TaskRow;
  guests: TaskRow[];
  hostScheduleStart?: string;
  hostScheduleEnd?: string;
  guestScheduleStarts: (string | undefined)[];
  guestScheduleEnds: (string | undefined)[];
  dateKey: string;
}

interface DateSeparator {
  type: 'separator';
  label: string;
}

type ListItem = TaskItem | ParallelGroupItem | DateSeparator;

function dateLabel(key: string, tz?: string): string {
  // Compare the date key (already in server tz) against "today" in the
  // same server tz, so the 今日/明日/昨日 labels are consistent with the
  // date separators built by dateKey.
  const d = new Date(key + 'T00:00:00');
  const todayKey = todayDateKey(tz);
  const today = new Date(todayKey + 'T00:00:00');
  const diff = Math.round(
    (d.getTime() - today.getTime()) / (1000 * 60 * 60 * 24),
  );
  if (diff === 0) return '今日';
  if (diff === 1) return '明日';
  if (diff === -1) return '昨日';
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

// Number of days between two YYYY-MM-DD keys, computed without interpreting
// the keys in the device timezone. This avoids DST/local-midnight issues when
// comparing server-timezone date keys.
function daysBetweenDateKeys(a: string, b: string): number {
  const [ay, am, ad] = a.split('-').map(Number);
  const [by, bm, bd] = b.split('-').map(Number);
  return Math.round(
    (Date.UTC(ay, am - 1, ad) - Date.UTC(by, bm - 1, bd)) /
      (1000 * 60 * 60 * 24),
  );
}

// Human-readable label for a future task's scheduled date, relative to today.
// Only called when taskKey > todayKey, so diff is always positive.
function futureTaskDateLabel(taskKey: string, todayKey: string): string {
  const diff = daysBetweenDateKeys(taskKey, todayKey);
  if (diff === 1) return '明日';
  if (diff > 1 && diff <= 6) return `${diff}日後`;
  const [y, m, d] = taskKey.split('-').map(Number);
  const [ty] = todayKey.split('-').map(Number);
  if (y === ty) return `${m}/${d}`;
  return `${y}/${m}/${d}`;
}

// A separator that marks a real day boundary (今日 / 明日 / M/D). Excludes
// the non-day separators: 'pending', '過去', and the "load more past" row.
function isDaySeparator(item: DateSeparator): boolean {
  return (
    item.label !== 'pending' &&
    item.label !== '過去' &&
    !item.label.startsWith('過去をさらに読み込む')
  );
}

// viewabilityConfig for tracking the topmost visible item index. Module-level
// so the object identity stays stable across renders (FlatList requirement).
const VIEWABILITY_CONFIG = {
  minimumViewTime: 0,
  viewAreaCoveragePercentThreshold: 0,
} as const;
const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      flex: 1,
    },
    pastToggle: {
      flexDirection: 'row',
      paddingVertical: 10,
      alignItems: 'center',
      justifyContent: 'center',
      gap: 6,
    },
    pastToggleText: {
      fontSize: 13,
      color: colors.brand,
      fontWeight: '500',
    },
    topBar: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingHorizontal: 8,
      paddingBottom: 8,
      gap: 4,
    },
    topButton: {
      width: 40,
      height: 40,
      borderRadius: 20,
      alignItems: 'center',
      justifyContent: 'center',
    },
    topButtonPressed: {
      backgroundColor: colors.brandPressed,
    },
    topButtonDisabled: {
      opacity: 0.4,
    },
    topButtonText: {
      fontSize: 20,
    },
    listContent: {
      paddingBottom: 100,
    },
    separator: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingHorizontal: 16,
      paddingVertical: 8,
      gap: 8,
    },
    separatorBar: {
      flex: 1,
      height: 1,
    },
    separatorText: {
      fontSize: 12,
      fontWeight: '500',
    },
    bottomBar: {
      position: 'absolute',
      bottom: 0,
      left: 0,
      right: 0,
      flexDirection: 'row',
      justifyContent: 'center',
      alignItems: 'center',
      paddingVertical: 16,
      paddingHorizontal: 24,
      gap: 16,
    },
    startDoneButton: {
      position: 'absolute',
      right: 24,
      bottom: 16,
      width: 48,
      height: 48,
      borderRadius: 24,
      alignItems: 'center',
      justifyContent: 'center',
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 2 },
      shadowOpacity: 0.3,
      shadowRadius: 4,
      elevation: 4,
    },
    startDoneText: {
      fontSize: 20,
    },
    startDoneHint: {
      position: 'absolute',
      width: 56,
      height: 56,
      borderRadius: 28,
      backgroundColor: colors.surfaceTranslucent,
      alignItems: 'center',
      justifyContent: 'center',
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 2 },
      shadowOpacity: 0.2,
      shadowRadius: 3,
      elevation: 3,
    },
  });

export function HomeView() {
  const { client, notifications, workersUrl, workersToken } = useServer();
  const router = useRouter();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();

  const [tasks, setTasks] = useState<TaskRow[]>([]);
  const [schedule, setSchedule] = useState<ScheduleEntry[]>([]);
  const [habits, setHabits] = useState<HabitRow[]>([]);
  // habit_id (UUID) → display_id map for habit-based task coloring (#309)
  // and h1#5 ID labels (#305).
  const habitDisplayIdMap = useMemo(
    () => new Map(habits.map((h) => [h.id, h.display_id])),
    [habits],
  );
  // task_id → number of tasks that depend on it (reverse dependency count)
  const dependentCountMap = useMemo(() => {
    const counts = new Map<string, number>();
    for (const t of tasks) {
      for (const depId of parseDepends(t.depends)) {
        counts.set(depId, (counts.get(depId) ?? 0) + 1);
      }
    }
    return counts;
  }, [tasks]);
  const habitDisplayIdMapRef = useRef(habitDisplayIdMap);
  habitDisplayIdMapRef.current = habitDisplayIdMap;
  const dependentCountMapRef = useRef(dependentCountMap);
  dependentCountMapRef.current = dependentCountMap;
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [refreshing, setRefreshing] = useState(false);
  // Server-configured timezone (from GET /api/settings). Used by dateKey
  // so date separators match the server's habit sync date grouping.
  const [serverTz, setServerTz] = useState<string | undefined>(undefined);
  const [view, setView] = useState<ViewType>('task');
  const viewChanger = useMemo(
    () => <ViewChanger current={view} onChange={setView} />,
    [view],
  );
  const [searchQuery, setSearchQuery] = useState('');
  const searchQueryRef = useRef(searchQuery);
  searchQueryRef.current = searchQuery;
  const [showPast, setShowPast] = useState(false);
  // #206: past tasks load 1 week at a time
  const [pastWeeks, setPastWeeks] = useState(1);
  const [progressSheetVisible, setProgressSheetVisible] = useState(false);
  const [progressTask, setProgressTask] = useState<TaskRow | null>(null);
  const startDoneButtonY = useSharedValue(0);
  const startDoneButtonPressed = useSharedValue(0);
  const listRef = useRef<FlatList<ListItem>>(null);
  const scrollOffsetRef = useRef(0);
  // Viewport height of the FlatList (for page-sized scrolls). Captured via
  // onLayout so it stays correct across rotation / keyboard changes.
  const listLayoutHeightRef = useRef(0);
  // Index of the topmost visible item, kept in sync via
  // onViewableItemsChanged. Used by scrollByDay to find the next/previous
  // day separator relative to the current scroll position.
  const visibleTopIndexRef = useRef(0);

  // Stable refs for state values that callbacks need to read without
  // becoming dependencies of those callbacks. Updated synchronously during
  // render so handlers always see the latest value in the current render.
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const tasksRef = useRef(tasks);
  tasksRef.current = tasks;
  const notificationsRef = useRef(notifications);
  notificationsRef.current = notifications;
  const clientRef = useRef(client);
  clientRef.current = client;
  const serverTzRef = useRef(serverTz);
  serverTzRef.current = serverTz;

  // Navigation buttons visibility — shown when scrolling, hidden when idle
  // (#308). Uses a shared value for smooth opacity animation.
  const navOpacity = useSharedValue(0);
  const [navButtonsVisible, setNavButtonsVisible] = useState(false);
  const navHideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const navDisableTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showNavButtons = useCallback(() => {
    if (navHideTimer.current) {
      clearTimeout(navHideTimer.current);
      navHideTimer.current = null;
    }
    if (navDisableTimer.current) {
      clearTimeout(navDisableTimer.current);
      navDisableTimer.current = null;
    }
    navOpacity.value = withTiming(1, { duration: 200 });
    setNavButtonsVisible(true);
  }, [navOpacity]);

  const scheduleHideNavButtons = useCallback(() => {
    if (navHideTimer.current) clearTimeout(navHideTimer.current);
    if (navDisableTimer.current) {
      clearTimeout(navDisableTimer.current);
      navDisableTimer.current = null;
    }
    navHideTimer.current = setTimeout(() => {
      navOpacity.value = withTiming(0, { duration: 300 });
      // Disable taps after the fade-out animation completes
      navDisableTimer.current = setTimeout(
        () => setNavButtonsVisible(false),
        350,
      );
    }, 1500);
  }, [navOpacity]);

  const navButtonsStyle = useAnimatedStyle(() => ({
    opacity: navOpacity.value,
  }));

  // Stable callback for FlatList's onViewableItemsChanged. React Native
  // warns ("Changing onViewableItemsChanged on the fly is not supported")
  // when the callback identity changes after mount, so it must be wrapped
  // in useCallback with an empty dependency array. The callback only writes
  // to a ref, so capturing it once is safe.
  const handleViewableItemsChanged = useCallback(
    ({ viewableItems }: { viewableItems: Array<{ index: number | null }> }) => {
      if (viewableItems.length > 0) {
        visibleTopIndexRef.current = viewableItems[0].index ?? 0;
      }
    },
    [],
  );

  // Animated chevron rotation for past-day toggle
  const chevronRotate = useSharedValue(0);
  const chevronStyle = useAnimatedStyle(() => ({
    transform: [{ rotate: `${chevronRotate.value}deg` }],
  }));
  const togglePast = useCallback(() => {
    haptic.select();
    setShowPast((v) => {
      const next = !v;
      chevronRotate.value = withTiming(next ? 180 : 0, { duration: 250 });
      if (!next) setPastWeeks(1); // reset pagination when collapsing
      return next;
    });
  }, [chevronRotate]);

  const refresh = useCallback(async () => {
    if (!client) return;
    setRefreshing(true);
    try {
      const [taskList, sched, habitList, settings] = await Promise.all([
        client.listTasks({ q: searchQueryRef.current }),
        client.getSchedule().catch((e) => {
          logError('スケジュール取得', e);
          return null;
        }),
        client.listHabits().catch((e) => {
          logError('Habit取得', e);
          return [] as HabitRow[];
        }),
        client.getSettings().catch(() => null),
      ]);
      setTasks(taskList);
      setSchedule(sched ? parseSchedule(sched.schedule) : []);
      setHabits(habitList);
      setServerTz(settings?.tz);
      // Push a fresh snapshot to the home screen widget so it shows
      // current data immediately (without waiting for WorkManager).
      // The native side separates the in-progress task as `doing` and keeps
      // scheduled tasks as `upcoming`, sorted by scheduled start time.
      try {
        const schedEntries = sched ? parseSchedule(sched.schedule) : [];
        const schedMap = new Map(schedEntries.map((e) => [e.task_id, e]));
        let unscheduledCount = 0;
        const scheduled: {
          id: string;
          title: string;
          startAt: string | null;
          endAt: string;
          abandonability: number;
          fixed: boolean;
        }[] = [];
        let doing: {
          id: string;
          title: string;
          startAt: string | null;
          endAt: string;
          abandonability: number;
          fixed: boolean;
        } | null = null;

        for (const t of taskList) {
          if (t.status === 'pending') {
            unscheduledCount++;
          } else if (t.status === 'scheduled' || t.status === 'in_progress') {
            const entry = schedMap.get(t.id);
            const startAt = entry?.start_at ?? t.start_at ?? null;
            const endAt = entry?.end_at ?? t.end_at;
            const task = {
              id: t.id,
              title: t.title,
              startAt,
              endAt,
              abandonability: t.abandonability,
              fixed: t.fixed,
            };
            if (t.status === 'in_progress' && doing == null) {
              doing = task;
            } else {
              scheduled.push(task);
            }
          }
        }

        scheduled.sort((a, b) => {
          const ta = new Date(a.startAt ?? a.endAt).getTime();
          const tb = new Date(b.startAt ?? b.endAt).getTime();
          return ta - tb;
        });

        const scheme = Constants.expoConfig?.scheme;
        TakusuWidgetModule.saveSnapshot({
          doing,
          upcoming: scheduled,
          unscheduledCount,
          serverTz,
          scheme: Array.isArray(scheme) ? scheme[0] : scheme,
        });
      } catch {
        // widget module not available (non-Android) — ignore
      }
    } catch (e) {
      showError(e, 'タスク一覧の取得に失敗');
    } finally {
      setRefreshing(false);
    }
  }, [client, serverTz]);

  // Debounced server-side search. The first run is skipped because the initial
  // focus effect already fetches the task list. Only mark the search as
  // started once we have an actual client, otherwise a later client
  // availability change would trigger a duplicate request.
  const searchStartedRef = useRef(false);
  useEffect(() => {
    if (!client) return;
    if (!searchStartedRef.current) {
      searchStartedRef.current = true;
      return;
    }
    const id = setTimeout(() => {
      client
        .listTasks({ q: searchQuery })
        .then(setTasks)
        .catch((e) => {
          logError('タスク検索', e);
        });
    }, 250);
    return () => clearTimeout(id);
  }, [searchQuery, client]);

  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  const { showTopToast, hideTopToast } = useTopToast();
  const showUndoToast = useUndoableToast();
  const { startScheduleOperation, scheduleOperation, lastCompletedAt } =
    useScheduleOperation({
      client,
      workersUrl,
      workersToken,
      refresh,
      showTopToast,
      hideTopToast,
    });

  useFocusEffect(
    useCallback(() => {
      refresh();
    }, [refresh]),
  );

  // Reschedule notifications when tasks, schedule, or notification
  // settings change. This is separate from refresh() to avoid triggering a
  // full server refetch when only notification settings are toggled.
  useEffect(() => {
    if (tasks.length === 0) return;
    rescheduleFromRaw(
      tasks,
      schedule.length > 0 ? JSON.stringify(schedule) : null,
      notifications,
      serverTz,
    ).catch((e) => logError('通知の再スケジュール', e));
  }, [tasks, schedule, notifications, serverTz]);

  const scheduleMap = useMemo(() => {
    const m = new Map<string, ScheduleEntry>();
    for (const e of schedule) m.set(e.task_id, e);
    return m;
  }, [schedule]);

  // Build parallel groups: host (allows_parallel=true) → overlapping guests
  // (parallelizable=true). Each guest is assigned to at most one host (the
  // first one found) to avoid duplicate rendering across groups.
  // Active hosts (in_progress or scheduled) form groups regardless of end
  // time, so scheduled tasks with a past end time remain visible in the main
  // list and their guests are not orphaned. (#472)
  const { parallelGroups, groupedGuestIds } = useMemo(() => {
    const groups = new Map<string, TaskRow[]>();
    const guestIds = new Set<string>();
    const hosts = tasks.filter(
      (t) =>
        t.allows_parallel &&
        (t.status === 'in_progress' || t.status === 'scheduled'),
    );
    const guests = tasks.filter(
      (t) =>
        t.parallelizable &&
        t.status !== 'pending' &&
        t.status !== 'completed' &&
        t.status !== 'skipped',
    );
    for (const host of hosts) {
      // Skip hosts that have already been claimed as a guest by another
      // host — otherwise their own guests would be orphaned (claimed but
      // never rendered, since this host is skipped in the upcoming loop).
      if (guestIds.has(host.id)) continue;
      const hostEntry = scheduleMap.get(host.id);
      if (!hostEntry) continue;
      const hostStart = new Date(hostEntry.start_at).getTime();
      const hostEnd = new Date(hostEntry.end_at).getTime();
      const overlapping: TaskRow[] = [];
      for (const guest of guests) {
        if (guest.id === host.id) continue;
        // Skip guests already claimed by another host.
        if (guestIds.has(guest.id)) continue;
        const guestEntry = scheduleMap.get(guest.id);
        if (!guestEntry) continue;
        const gStart = new Date(guestEntry.start_at).getTime();
        const gEnd = new Date(guestEntry.end_at).getTime();
        if (gStart < hostEnd && gEnd > hostStart) {
          overlapping.push(guest);
          guestIds.add(guest.id);
        }
      }
      if (overlapping.length > 0) {
        overlapping.sort(
          (a, b) =>
            new Date(scheduleMap.get(a.id)!.start_at).getTime() -
            new Date(scheduleMap.get(b.id)!.start_at).getTime(),
        );
        groups.set(host.id, overlapping);
      }
    }
    return { parallelGroups: groups, groupedGuestIds: guestIds };
  }, [tasks, scheduleMap]);
  const parallelGroupsRef = useRef(parallelGroups);
  parallelGroupsRef.current = parallelGroups;
  const guestToAllIdsRef = useRef(new Map<string, string[]>());
  guestToAllIdsRef.current = useMemo(() => {
    const map = new Map<string, string[]>();
    for (const [hostId, groupGuests] of parallelGroups) {
      const allIds = [hostId, ...groupGuests.map((g) => g.id)];
      for (const g of groupGuests) {
        map.set(g.id, allIds);
      }
    }
    return map;
  }, [parallelGroups]);

  const items: ListItem[] = useMemo(() => {
    const searching = searchQuery.length > 0;
    const filtered = tasks;

    const pending = filtered.filter((t) => t.status === 'pending');
    const scheduled = filtered
      .filter((t) => t.status !== 'pending')
      .sort((a, b) => {
        // Sort by scheduled start time (or task end_at as fallback).
        // Use timestamp comparison instead of string localeCompare to
        // avoid date-boundary sorting issues (#210): localeCompare on
        // ISO strings with different dates can produce wrong order when
        // the strings have different lengths or timezone offsets.
        const sa = scheduleMap.get(a.id)?.start_at ?? a.end_at;
        const sb = scheduleMap.get(b.id)?.start_at ?? b.end_at;
        const ta = new Date(sa).getTime();
        const tb = new Date(sb).getTime();
        return ta - tb;
      });

    // Past completed/skipped tasks — only include in list when showPast
    const now = Date.now();
    // #254: 完了/スキップ済みタスクは end_at に関わらず過去セクションへ。
    // fixed タスクは完了後も schedule の end_at が未来になりうるため、
    // status ベースで過去判定しないと upcoming に残り続ける。
    // #472: scheduled タスクも過去扱いにしない。未完了なのでメインリストに残す。
    const isPast = (t: TaskRow): boolean => {
      if (t.status === 'in_progress') return false;
      if (t.status === 'scheduled') return false;
      if (t.status === 'completed' || t.status === 'skipped') return true;
      const entry = scheduleMap.get(t.id);
      const end = entry?.end_at ?? t.end_at;
      return new Date(end).getTime() < now;
    };
    const pastAll = scheduled.filter(isPast);
    // When searching, show all matching past tasks regardless of the toggle.
    const past = searching || showPast ? pastAll : [];

    // Upcoming = always exclude past tasks, regardless of showPast
    const upcoming = scheduled.filter((t) => !isPast(t));

    const result: ListItem[] = [];

    // Past section (when revealed) — no date separators, 1 week at a time (#206)
    if (past.length > 0) {
      const weekCutoff = now - pastWeeks * 7 * 24 * 60 * 60 * 1000;
      const pastVisible = searching
        ? past
        : past.filter((t) => {
            const entry = scheduleMap.get(t.id);
            const end = entry?.end_at ?? t.end_at;
            return new Date(end).getTime() >= weekCutoff;
          });
      const hasOlder = pastVisible.length < past.length;
      if (pastVisible.length > 0 || hasOlder) {
        result.push({ type: 'separator', label: '過去' });
        // Put "load more past" right below the '過去' separator header.
        if (hasOlder) {
          result.push({
            type: 'separator',
            label: '過去をさらに読み込む',
          });
        }
      }
      for (const t of pastVisible) {
        if (groupedGuestIds.has(t.id)) continue;
        const entry = scheduleMap.get(t.id);
        const key = dateKey(entry?.start_at ?? t.end_at, serverTz);
        result.push({
          type: 'task',
          task: t,
          scheduleStart: entry?.start_at,
          scheduleEnd: entry?.end_at,
          isDone: t.status === 'completed' || t.status === 'skipped',
          dateKey: key,
        });
      }
    }

    if (pending.length > 0) {
      result.push({ type: 'separator', label: 'pending' });
      for (const t of pending) {
        result.push({
          type: 'task',
          task: t,
          isDone: t.status === 'completed' || t.status === 'skipped',
          dateKey: 'pending',
        });
      }
    }

    let lastDate = '';
    // When searching, render all matching tasks individually — parallel
    // grouping is based on the full task list, so a search-filtered guest
    // could be invisible if its host doesn't match the query.
    const skipGrouping = searching;
    for (const t of upcoming) {
      // Skip guests that are part of a parallel group — they're rendered
      // inside the group item alongside their host.
      if (!skipGrouping && groupedGuestIds.has(t.id)) continue;
      const entry = scheduleMap.get(t.id);
      const key = dateKey(entry?.start_at ?? t.end_at, serverTz);
      if (key !== lastDate) {
        result.push({ type: 'separator', label: dateLabel(key, serverTz) });
        lastDate = key;
      }
      // If this task is a host with overlapping guests, render a group
      // (but not when searching — see skipGrouping above).
      const groupGuests = !skipGrouping ? parallelGroups.get(t.id) : undefined;
      if (groupGuests && groupGuests.length > 0) {
        result.push({
          type: 'parallelGroup',
          host: t,
          guests: groupGuests,
          hostScheduleStart: entry?.start_at,
          hostScheduleEnd: entry?.end_at,
          guestScheduleStarts: groupGuests.map(
            (g) => scheduleMap.get(g.id)?.start_at,
          ),
          guestScheduleEnds: groupGuests.map(
            (g) => scheduleMap.get(g.id)?.end_at,
          ),
          dateKey: key,
        });
      } else {
        result.push({
          type: 'task',
          task: t,
          scheduleStart: entry?.start_at,
          scheduleEnd: entry?.end_at,
          isDone: t.status === 'completed' || t.status === 'skipped',
          dateKey: key,
        });
      }
    }

    return result;
  }, [
    tasks,
    scheduleMap,
    searchQuery,
    groupedGuestIds,
    parallelGroups,
    showPast,
    pastWeeks,
    serverTz,
  ]);

  // Whether there are any past tasks to show the toggle (#920)
  // #254: completed/skipped は end_at に関わらず過去扱い。
  const hasPast = useMemo(() => {
    const now = Date.now();
    return tasks.some((t) => {
      if (t.status === 'pending') return false;
      if (t.status === 'in_progress') return false;
      if (t.status === 'scheduled') return false;
      if (t.status === 'completed' || t.status === 'skipped') return true;
      const entry = scheduleMap.get(t.id);
      const end = entry?.end_at ?? t.end_at;
      return new Date(end).getTime() < now;
    });
  }, [tasks, scheduleMap]);

  // Marked dates for calendar overlay (dates that have scheduled tasks)
  const markedDates = useMemo(() => {
    const set = new Set<string>();
    for (const t of tasks) {
      if (t.status === 'pending') continue;
      const entry = scheduleMap.get(t.id);
      const key = dateKey(entry?.start_at ?? t.end_at, serverTz);
      set.add(key);
    }
    return set;
  }, [tasks, scheduleMap, serverTz]);

  // Map dateKey → index in items array (for scroll navigation)
  const dateIndexMap = useMemo(() => {
    const m = new Map<string, number>();
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.type === 'separator' && item.label !== 'pending') {
        // Reconstruct dateKey from the label — but we stored label, not key.
        // Instead, find the first task after this separator to get its dateKey.
        for (let j = i + 1; j < items.length; j++) {
          const next = items[j];
          if (next.type === 'task' || next.type === 'parallelGroup') {
            m.set(next.dateKey, i);
            break;
          }
        }
      }
    }
    return m;
  }, [items]);

  function scrollToDateKey(key: string) {
    const idx = dateIndexMap.get(key);
    if (idx !== undefined && listRef.current) {
      listRef.current.scrollToIndex({ index: idx, animated: true });
    }
  }

  function scrollByDay(direction: -1 | 1) {
    if (!listRef.current) return;
    // Jump to the next/previous day separator relative to the currently
    // visible top item. A "day" boundary is a separator whose label is a
    // date (not 'pending' / '過去' / load-more).
    // Clamp the ref index to the current list length — the ref is updated
    // asynchronously by onViewableItemsChanged, so it can hold a stale
    // index larger than items.length after a search/refresh shrinks the
    // list. Without this guard, items[i] would be undefined and crash.
    const from = Math.min(visibleTopIndexRef.current, items.length - 1);
    if (direction < 0) {
      for (let i = from - 1; i >= 0; i--) {
        const item = items[i];
        if (item.type === 'separator' && isDaySeparator(item)) {
          listRef.current.scrollToIndex({ index: i, animated: true });
          return;
        }
      }
      // No earlier day separator — go to the very top.
      listRef.current.scrollToOffset({ offset: 0, animated: true });
    } else {
      for (let i = from + 1; i < items.length; i++) {
        const item = items[i];
        if (item.type === 'separator' && isDaySeparator(item)) {
          listRef.current.scrollToIndex({ index: i, animated: true });
          return;
        }
      }
      // No later day separator — scroll to the bottom.
      listRef.current.scrollToEnd({ animated: true });
    }
  }

  function scrollByPage(direction: -1 | 1) {
    if (!listRef.current) return;
    const viewport = listLayoutHeightRef.current;
    if (viewport <= 0) return;
    // Scroll by one viewport, keeping a small overlap so the user keeps
    // some context at the edge.
    const delta = viewport * 0.9 * direction;
    const newOffset = Math.max(0, scrollOffsetRef.current + delta);
    listRef.current.scrollToOffset({ offset: newOffset, animated: true });
  }

  function jumpToDate(date: Date) {
    // Construct the date key in the server-configured timezone so it
    // matches the keys in dateIndexMap (which are built via dateKey with
    // serverTz). Falls back to device-local if serverTz is unavailable.
    const key = dateKey(date.toISOString(), serverTz);
    scrollToDateKey(key);
  }

  const toggleSelection = useCallback((id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const markDone = useCallback(async (task: TaskRow) => {
    const currentClient = clientRef.current;
    const currentNotifications = notificationsRef.current;
    if (!currentClient) return;
    // Pending tasks: swipe-right completes directly (pending → completed).
    // After completion, the task enters the 3-state cycle (scheduled →
    // in_progress → completed → scheduled). Undo restores the original
    // pending status.
    // Scheduled/in_progress/completed use the 3-state cycle (#312).
    const isDone = task.status === 'completed' || task.status === 'skipped';
    const isInProgress = task.status === 'in_progress';
    const isPending = task.status === 'pending';
    const prevStatus = task.status;
    let newStatus: TaskStatus;
    let actionLabel: string;
    let errorLabel: string;
    if (isPending) {
      // Pending tasks: 2-state toggle pending ↔ completed
      newStatus = 'completed';
      actionLabel = 'mark done';
      errorLabel = 'タスクの完了に失敗';
    } else if (isInProgress) {
      newStatus = 'completed';
      actionLabel = 'mark done';
      errorLabel = 'タスクの完了に失敗';
    } else if (isDone) {
      newStatus = 'scheduled';
      actionLabel = 'undone';
      errorLabel = 'タスクの未完了に失敗';
    } else {
      newStatus = 'in_progress';
      actionLabel = 'start';
      errorLabel = 'タスクの開始に失敗';
    }
    const operationId = makeProgressOperationId();
    try {
      if (isInProgress || isPending) {
        await currentClient.completeTaskWork(task.id, operationId);
      } else if (isDone) {
        await currentClient.updateTask(task.id, { status: newStatus });
      } else {
        await currentClient.startTaskWork(task.id, operationId);
      }
    } catch (e) {
      showError(e, errorLabel);
      return;
    }
    // Dismiss any delivered notifications for this task (#257).
    dismissTaskNotifications(task.id).catch((e) => logError('通知の消去', e));
    if (prevStatus === 'in_progress') {
      dismissInProgressNotification(task.id).catch((e) =>
        logError('通知の消去', e),
      );
    }
    // Cancel pending scheduled reminders (end-time etc.) when done (#455).
    if (newStatus === 'completed') {
      cancelScheduledTaskNotifications(task.id).catch((e) =>
        logError('通知のキャンセル', e),
      );
    }
    // Cancel pending start-time reminders when the task becomes in_progress
    // so an already-started task does not get a "タスク開始時間" notification.
    if (newStatus === 'in_progress') {
      cancelScheduledStartNotifications(task.id).catch((e) =>
        logError('通知のキャンセル', e),
      );
    }
    // Post in-progress notification when starting via swipe (#312)
    if (newStatus === 'in_progress' && currentNotifications.inProgress) {
      postInProgressNotification(task).catch((e) => logError('通知の投稿', e));
    }
    const originalQuantityDone = task.quantity_done;
    const total = task.quantity_total;

    undoRedo.push({
      description: `${actionLabel}: ${task.title}`,
      undo: async () => {
        const undoClient = clientRef.current;
        if (!undoClient) return;
        const undoNotifications = notificationsRef.current;
        if (newStatus === 'in_progress' && prevStatus === 'scheduled') {
          // undo start: close the work session and return to scheduled.
          await undoClient.pauseTaskWork(task.id, makeProgressOperationId());
          if (undoNotifications.inProgress) {
            dismissInProgressNotification(task.id).catch((e) =>
              logError('通知の消去', e),
            );
          }
        } else if (newStatus === 'completed' && prevStatus === 'in_progress') {
          // undo complete: restore in_progress and previous quantity_done.
          await undoClient.updateTask(task.id, {
            status: 'in_progress',
            quantity_done: originalQuantityDone,
          });
          if (undoNotifications.inProgress) {
            postInProgressNotification(task).catch((e) =>
              logError('通知の投稿', e),
            );
          }
        } else if (
          newStatus === 'scheduled' &&
          (prevStatus === 'completed' || prevStatus === 'skipped')
        ) {
          // undo undone: restore completed/skipped.
          if (prevStatus === 'completed') {
            await undoClient.updateTask(task.id, {
              status: 'completed',
              quantity_done: total ?? originalQuantityDone,
            });
          } else {
            await undoClient.updateTask(task.id, { status: 'skipped' });
          }
          if (undoNotifications.inProgress) {
            dismissInProgressNotification(task.id).catch((e) =>
              logError('通知の消去', e),
            );
          }
        } else {
          await undoClient.updateTask(task.id, {
            status: prevStatus,
            quantity_done: originalQuantityDone,
          });
        }
        await refreshRef.current();
      },
      redo: async () => {
        const redoClient = clientRef.current;
        if (!redoClient) return;
        const redoNotifications = notificationsRef.current;
        if (newStatus === 'in_progress' && prevStatus === 'scheduled') {
          await redoClient.startTaskWork(task.id, makeProgressOperationId());
          if (redoNotifications.inProgress) {
            postInProgressNotification(task).catch((e) =>
              logError('通知の投稿', e),
            );
          }
        } else if (
          newStatus === 'completed' &&
          (prevStatus === 'in_progress' || prevStatus === 'pending')
        ) {
          // redo complete: update status and quantity_done without creating a
          // duplicate progress event.
          await redoClient.updateTask(task.id, {
            status: 'completed',
            quantity_done: total ?? originalQuantityDone,
          });
          if (redoNotifications.inProgress) {
            dismissInProgressNotification(task.id).catch((e) =>
              logError('通知の消去', e),
            );
          }
        } else if (
          newStatus === 'scheduled' &&
          (prevStatus === 'completed' || prevStatus === 'skipped')
        ) {
          await redoClient.updateTask(task.id, { status: 'scheduled' });
        } else {
          await redoClient.updateTask(task.id, { status: newStatus });
        }
        await refreshRef.current();
      },
    });
    await refreshRef.current();
  }, []);

  const markSkipped = useCallback(async (task: TaskRow) => {
    const currentClient = clientRef.current;
    if (!currentClient) return;
    if (task.status === 'skipped' || task.status === 'completed') return;
    const prevStatus = task.status;
    try {
      await currentClient.updateTask(task.id, { status: 'skipped' });
    } catch (e) {
      showError(e, 'タスクのスキップに失敗');
      return;
    }
    dismissTaskNotifications(task.id).catch((e) => logError('通知の消去', e));
    cancelScheduledTaskNotifications(task.id).catch((e) =>
      logError('通知のキャンセル', e),
    );
    if (prevStatus === 'in_progress') {
      dismissInProgressNotification(task.id).catch((e) =>
        logError('通知の消去', e),
      );
    }
    undoRedo.push({
      description: `skip: ${task.title}`,
      undo: async () => {
        const undoClient = clientRef.current;
        if (!undoClient) return;
        await undoClient.updateTask(task.id, { status: prevStatus });
        await refreshRef.current();
      },
      redo: async () => {
        const redoClient = clientRef.current;
        if (!redoClient) return;
        await redoClient.updateTask(task.id, { status: 'skipped' });
        await refreshRef.current();
      },
    });
    await refreshRef.current();
  }, []);

  const inProgressTask = useMemo(
    () => tasks.find((t) => t.status === 'in_progress') || null,
    [tasks],
  );

  const startNextTask = useCallback(async () => {
    const currentClient = clientRef.current;
    if (!currentClient) return;
    const scheduled = tasksRef.current
      .filter((t) => t.status === 'scheduled')
      .sort(
        (a, b) =>
          new Date(a.start_at ?? a.end_at).getTime() -
          new Date(b.start_at ?? b.end_at).getTime(),
      );
    const next =
      scheduled[0] ?? tasksRef.current.find((t) => t.status === 'pending');
    if (!next) return;
    if (next.start_at) {
      const currentTz = serverTzRef.current;
      const taskDate = dateKey(next.start_at, currentTz);
      const today = todayDateKey(currentTz);
      if (taskDate > today) {
        const confirmed = await new Promise<boolean>((resolve) => {
          Alert.alert(
            '明日以降のタスクを開始',
            `「${next.title}」は${futureTaskDateLabel(taskDate, today)}のタスクです。本当に開始しますか？`,
            [
              {
                text: 'キャンセル',
                style: 'cancel',
                onPress: () => resolve(false),
              },
              { text: '開始', onPress: () => resolve(true) },
            ],
            { cancelable: true, onDismiss: () => resolve(false) },
          );
        });
        if (!confirmed) return;
      }
    }
    haptic.medium();
    const operationId = makeProgressOperationId();
    try {
      await currentClient.startTaskWork(next.id, operationId);
      cancelScheduledStartNotifications(next.id).catch((e) =>
        logError('通知のキャンセル', e),
      );
      dismissTaskNotifications(next.id).catch((e) => logError('通知の消去', e));
      const currentNotifications = notificationsRef.current;
      if (currentNotifications.inProgress) {
        postInProgressNotification({ ...next, status: 'in_progress' }).catch(
          (e) => logError('通知の投稿', e),
        );
      }
    } catch (e) {
      showError(e, 'タスクの開始に失敗');
      return;
    }
    await refreshRef.current();
  }, []);

  const pauseInProgress = useCallback(
    async (payload: ProgressPayload) => {
      const currentClient = clientRef.current;
      const task = inProgressTask;
      if (!currentClient || !task) return;
      const recordOperationId = makeProgressOperationId();
      const pauseOperationId = makeProgressOperationId();
      try {
        await recordProgressWithTotal(currentClient, task, payload, {
          operationId: recordOperationId,
        });
        await currentClient.pauseTaskWork(task.id, pauseOperationId);
        dismissInProgressNotification(task.id).catch((e) =>
          logError('通知の消去', e),
        );
      } catch (e) {
        showError(e, '進捗の記録または一時停止に失敗');
        return;
      }
      await refreshRef.current();
    },
    [inProgressTask],
  );

  const recordInProgress = useCallback(
    async (payload: ProgressPayload) => {
      const currentClient = clientRef.current;
      const task = inProgressTask;
      if (!currentClient || !task) return;
      try {
        await recordProgressWithTotal(currentClient, task, payload);
      } catch (e) {
        showError(e, '進捗の記録に失敗');
        return;
      }
      await refreshRef.current();
    },
    [inProgressTask],
  );

  const handleHomeProgressConfirm = useCallback(
    async (payload: ProgressPayload) => {
      await pauseInProgress(payload);
      setProgressSheetVisible(false);
      setProgressTask(null);
    },
    [pauseInProgress],
  );

  const handleHomeRecordOnly = useCallback(
    async (payload: ProgressPayload) => {
      await recordInProgress(payload);
      setProgressSheetVisible(false);
      setProgressTask(null);
    },
    [recordInProgress],
  );

  const openHomeProgressSheet = useCallback(() => {
    const task = inProgressTask;
    if (!task) {
      startNextTask();
      return;
    }
    haptic.light();
    setProgressTask(task);
    setProgressSheetVisible(true);
  }, [inProgressTask, startNextTask]);

  const handleStartDoneTap = useCallback(() => {
    openHomeProgressSheet();
  }, [openHomeProgressSheet]);

  const handleStartDoneSlide = useCallback(() => {
    const task = inProgressTask;
    if (!task) return;
    haptic.medium();
    markDone(task);
  }, [inProgressTask, markDone]);

  const SLIDE_UP_DONE_THRESHOLD = 60;

  const startDoneButtonStyle = useAnimatedStyle(() => ({
    transform: [
      { translateY: startDoneButtonY.value },
      { scale: 1 - 0.08 * startDoneButtonPressed.value },
    ],
  }));

  const startDoneHintStyle = useAnimatedStyle(() => {
    const progress = Math.min(
      1,
      Math.max(0, -startDoneButtonY.value / (SLIDE_UP_DONE_THRESHOLD * 0.7)),
    );
    return {
      opacity: progress,
      transform: [{ scale: 0.8 + progress * 0.2 }],
    };
  });

  const startDoneGesture = useMemo(() => {
    const panGesture = Gesture.Pan()
      .enabled(inProgressTask !== null)
      .activeOffsetY([-10, 10])
      .failOffsetX([-20, 20])
      .onBegin(() => {
        startDoneButtonPressed.value = withTiming(1, { duration: 80 });
      })
      .onUpdate((e) => {
        startDoneButtonY.value = Math.min(0, e.translationY);
      })
      .onEnd((e) => {
        startDoneButtonY.value = withSpring(0);
        if (e.translationY < -SLIDE_UP_DONE_THRESHOLD) {
          runOnJS(handleStartDoneSlide)();
        }
      })
      .onFinalize((_e, success) => {
        startDoneButtonPressed.value = withTiming(0, { duration: 120 });
        if (!success) {
          startDoneButtonY.value = withSpring(0);
        }
      });

    const tapGesture = Gesture.Tap()
      .onBegin(() => {
        startDoneButtonPressed.value = withTiming(1, { duration: 80 });
      })
      .onEnd(() => {
        runOnJS(handleStartDoneTap)();
      })
      .onFinalize(() => {
        startDoneButtonPressed.value = withTiming(0, { duration: 120 });
      });

    return Gesture.Exclusive(panGesture, tapGesture);
  }, [
    handleStartDoneSlide,
    handleStartDoneTap,
    inProgressTask,
    startDoneButtonPressed,
    startDoneButtonY,
  ]);

  const deleteTask = useCallback(async (task: TaskRow) => {
    const currentClient = clientRef.current;
    const currentTasks = tasksRef.current;
    if (!currentClient) return;
    // Remove the deleted task from any other task's depends before deleting,
    // so a deleted host doesn't leave guests with an invalid dependency.
    // Keep the original depends arrays so undo can restore them.
    const dependents: { id: string; original: string[] }[] = [];
    for (const t of currentTasks) {
      if (t.id === task.id) continue;
      const deps = parseDepends(t.depends);
      if (deps.includes(task.id)) {
        dependents.push({ id: t.id, original: deps });
      }
    }
    try {
      for (const d of dependents) {
        await currentClient.updateTask(d.id, {
          depends: d.original.filter((id) => id !== task.id),
        });
      }
    } catch (e) {
      showError(e, '依存関係の更新に失敗');
      return;
    }
    try {
      await currentClient.deleteTask(task.id);
    } catch (e) {
      showError(e, 'タスクの削除に失敗');
      // Best-effort rollback of dependency updates.
      for (const d of dependents) {
        await currentClient
          .updateTask(d.id, { depends: d.original })
          .catch((err) => {
            logError('依存関係の復元に失敗', err);
          });
      }
      return;
    }
    let currentId = task.id;
    undoRedo.push({
      description: `delete: ${task.title}`,
      undo: async () => {
        const undoClient = clientRef.current;
        if (!undoClient) return;
        // Re-create with same fields
        const recreated = await undoClient.createTask({
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
          ical_uid: task.ical_uid,
          habit_id: task.habit_id,
          fixed: task.fixed,
        });
        // CreateTask does not accept `status`; restore it via update.
        if (task.status !== 'pending') {
          await undoClient.updateTask(recreated.id, { status: task.status });
        }
        currentId = recreated.id;
        // Restore dependents to point to the recreated task.
        for (const d of dependents) {
          await undoClient.updateTask(d.id, {
            depends: d.original.map((id) => (id === task.id ? currentId : id)),
          });
        }
        await refreshRef.current();
      },
      redo: async () => {
        const redoClient = clientRef.current;
        if (!redoClient) return;
        await redoClient.deleteTask(currentId);
        for (const d of dependents) {
          await redoClient.updateTask(d.id, {
            depends: d.original.filter((id) => id !== task.id),
          });
        }
        await refreshRef.current();
      },
    });
    await refreshRef.current();
  }, []);

  function rescheduleSelected() {
    if (!client || scheduleOperation) return;
    const pinned = tasks.filter((t) => !selected.has(t.id)).map((t) => t.id);
    const until = new Date();
    until.setDate(until.getDate() + 7);
    startScheduleOperation(
      'reschedule',
      {
        mode: 'range',
        from: new Date().toISOString(),
        until: until.toISOString(),
        pinned,
      },
      'タスクを再スケジュール中',
    );
    setSelected(new Set());
  }

  function rescheduleOthers() {
    if (!client || scheduleOperation) return;
    const pinned = Array.from(selected);
    const until = new Date();
    until.setDate(until.getDate() + 7);
    startScheduleOperation(
      'reschedule',
      {
        mode: 'range',
        from: new Date().toISOString(),
        until: until.toISOString(),
        pinned,
      },
      'タスクを再スケジュール中',
    );
    setSelected(new Set());
  }

  async function deleteSelected() {
    if (!client) return;
    const toDelete = tasks.filter((t) => selected.has(t.id));
    if (toDelete.length === 0) return;
    const deleted: TaskRow[] = [];
    let failed = 0;
    for (const task of toDelete) {
      try {
        await client.deleteTask(task.id);
        deleted.push(task);
      } catch (e) {
        failed++;
        logError(`タスク削除 (${task.title})`, e);
      }
    }
    if (failed > 0) {
      showError(`${failed}件の削除に失敗しました`, 'タスクの削除');
    }
    if (deleted.length === 0) return;

    const message =
      deleted.length === 1
        ? `「${deleted[0].title}」を削除しました`
        : `${deleted.length}件のタスクを削除しました`;
    showUndoToast(message);

    // Track the ids assigned by the server when undo recreates the tasks,
    // so redo deletes the recreated (not the stale original) ids.
    // Push a single grouped undo entry so one undo restores all tasks.
    const currentIds: string[] = [...deleted.map((t) => t.id)];
    // Track which items have been recreated so a retry after partial failure
    // doesn't create duplicates.
    const createdIdx = new Set<number>();
    undoRedo.push({
      description:
        deleted.length === 1
          ? `delete: ${deleted[0].title}`
          : `delete ${deleted.length} tasks`,
      undo: async () => {
        const oldToNew = new Map<string, string>();
        // First pass: create tasks not yet recreated (skip on retry).
        for (let i = 0; i < deleted.length; i++) {
          if (createdIdx.has(i)) {
            // Already recreated on a previous (partial) attempt.
            oldToNew.set(deleted[i].id, currentIds[i]);
            continue;
          }
          const task = deleted[i];
          const recreated = await client.createTask({
            title: task.title,
            description: task.description,
            start_at: task.start_at,
            end_at: task.end_at,
            avg_minutes: task.avg_minutes,
            sigma_minutes: task.sigma_minutes,
            depends: [],
            parallelizable: task.parallelizable,
            allows_parallel: task.allows_parallel,
            abandonability: task.abandonability,
            ical_uid: task.ical_uid,
            habit_id: task.habit_id,
            fixed: task.fixed,
          });
          // CreateTask does not accept `status`; restore it via update.
          if (task.status !== 'pending') {
            await client.updateTask(recreated.id, { status: task.status });
          }
          currentIds[i] = recreated.id;
          oldToNew.set(task.id, recreated.id);
          createdIdx.add(i);
        }
        // Second pass: remap depends to new IDs for deps within the deleted set.
        for (let i = 0; i < deleted.length; i++) {
          const task = deleted[i];
          const origDeps = parseDepends(task.depends);
          if (origDeps.length === 0) continue;
          const newId = oldToNew.get(task.id)!;
          const remapped = origDeps.map((d) => oldToNew.get(d) ?? d);
          await client.updateTask(newId, { depends: remapped });
        }
        await refresh();
      },
      redo: async () => {
        createdIdx.clear();
        for (const id of currentIds) {
          await client.deleteTask(id);
        }
        await refresh();
      },
    });
    setSelected(new Set());
    await refresh();
  }

  function createDependent() {
    const deps = Array.from(selected);
    setSelected(new Set());
    router.push({
      pathname: '/task/add',
      params: { deps: JSON.stringify(deps) },
    });
  }

  async function setStatusSelected(newStatus: TaskStatus) {
    if (!client) return;
    const toUpdate = tasks.filter((t) => selected.has(t.id));
    if (toUpdate.length === 0) return;
    const prevStatuses = new Map(toUpdate.map((t) => [t.id, t.status]));
    const changed: TaskRow[] = [];
    const startedIds: string[] = [];
    let failed = 0;
    for (const task of toUpdate) {
      if (task.status === newStatus) continue;
      try {
        await client.updateTask(task.id, { status: newStatus });
        changed.push(task);
        // Dismiss any delivered notifications for this task (#257).
        dismissTaskNotifications(task.id).catch((e) =>
          logError('通知の消去', e),
        );
        if (task.status === 'in_progress') {
          dismissInProgressNotification(task.id).catch((e) =>
            logError('通知の消去', e),
          );
        }
        if (newStatus === 'in_progress') {
          startedIds.push(task.id);
        }
      } catch (e) {
        failed++;
        logError(`ステータス変更 (${task.title})`, e);
      }
    }
    // Cancel pending start-time reminders for all tasks moved to in_progress
    // in one pass, instead of fetching scheduled notifications per task (#648).
    if (startedIds.length > 0) {
      cancelScheduledStartNotifications(startedIds).catch((e) =>
        logError('通知のキャンセル', e),
      );
    }
    if (failed > 0) {
      showError(
        `${failed}件のステータス変更に失敗しました`,
        'ステータスの一括設定',
      );
    }
    if (changed.length === 0) {
      await refresh();
      return;
    }
    undoRedo.push({
      description:
        changed.length === 1
          ? `set status ${newStatus}: ${changed[0].title}`
          : `set status ${newStatus} on ${changed.length} tasks`,
      undo: async () => {
        const inProgressIds: string[] = [];
        for (const t of changed) {
          const prev = prevStatuses.get(t.id)!;
          await client.updateTask(t.id, { status: prev });
          if (prev === 'in_progress') {
            inProgressIds.push(t.id);
          }
        }
        if (inProgressIds.length > 0) {
          cancelScheduledStartNotifications(inProgressIds).catch((e) =>
            logError('通知のキャンセル', e),
          );
        }
        await refresh();
      },
      redo: async () => {
        const inProgressIds: string[] = [];
        for (const t of changed) {
          await client.updateTask(t.id, { status: newStatus });
          if (newStatus === 'in_progress') {
            inProgressIds.push(t.id);
          }
        }
        if (inProgressIds.length > 0) {
          cancelScheduledStartNotifications(inProgressIds).catch((e) =>
            logError('通知のキャンセル', e),
          );
        }
        await refresh();
      },
    });
    setSelected(new Set());
    await refresh();
  }

  const toggleGroupSelection = useCallback((allIds: string[]) => {
    setSelected((prev) => {
      const next = new Set(prev);
      const allSel = allIds.every((id) => prev.has(id));
      if (allSel) {
        for (const id of allIds) next.delete(id);
      } else {
        for (const id of allIds) next.add(id);
      }
      return next;
    });
  }, []);

  const handleTaskPress = useCallback(
    (task: TaskRow) => {
      if (selectedRef.current.size > 0) {
        toggleSelection(task.id);
      } else {
        router.push(`/task/${task.id}`);
      }
    },
    [toggleSelection, router],
  );

  const handleTaskLongPress = useCallback(
    (task: TaskRow) => {
      toggleSelection(task.id);
    },
    [toggleSelection],
  );

  const handleHostPress = useCallback(
    (host: TaskRow) => {
      if (selectedRef.current.size > 0) {
        const groupGuests = parallelGroupsRef.current.get(host.id) ?? [];
        toggleGroupSelection([host.id, ...groupGuests.map((g) => g.id)]);
      } else {
        router.push(`/task/${host.id}`);
      }
    },
    [toggleGroupSelection, router],
  );

  const handleGuestPress = useCallback(
    (guest: TaskRow) => {
      if (selectedRef.current.size > 0) {
        const allIds = guestToAllIdsRef.current.get(guest.id) ?? [guest.id];
        toggleGroupSelection(allIds);
      } else {
        router.push(`/task/${guest.id}`);
      }
    },
    [toggleGroupSelection, router],
  );

  const handleGroupToggle = useCallback(
    (allIds: string[]) => {
      toggleGroupSelection(allIds);
    },
    [toggleGroupSelection],
  );

  const renderListItem = useCallback(
    ({ item }: { item: ListItem }) => {
      if (item.type === 'separator') {
        // "Load more past" separator is tappable (#206)
        if (item.label.startsWith('過去をさらに読み込む')) {
          return (
            <Pressable
              style={styles.separator}
              onPress={() => {
                haptic.light();
                setPastWeeks((w) => w + 1);
              }}
            >
              <View
                style={[
                  styles.separatorBar,
                  { backgroundColor: colors.separator },
                ]}
              />
              <Text style={[styles.separatorText, { color: colors.brand }]}>
                {item.label}
              </Text>
              <View
                style={[
                  styles.separatorBar,
                  { backgroundColor: colors.separator },
                ]}
              />
            </Pressable>
          );
        }
        return (
          <View style={styles.separator}>
            <View
              style={[
                styles.separatorBar,
                { backgroundColor: colors.separator },
              ]}
            />
            <Text style={[styles.separatorText, { color: colors.gray }]}>
              {item.label}
            </Text>
            <View
              style={[
                styles.separatorBar,
                { backgroundColor: colors.separator },
              ]}
            />
          </View>
        );
      }
      const currentSelected = selectedRef.current;
      if (item.type === 'parallelGroup') {
        const allIds = [item.host.id, ...item.guests.map((g) => g.id)];
        const isSelected = allIds.some((id) => currentSelected.has(id));
        return (
          <ParallelGroupCard
            host={item.host}
            guests={item.guests}
            hostScheduleStart={item.hostScheduleStart}
            hostScheduleEnd={item.hostScheduleEnd}
            guestScheduleStarts={item.guestScheduleStarts}
            guestScheduleEnds={item.guestScheduleEnds}
            selected={isSelected}
            habitDisplayIdMap={habitDisplayIdMapRef.current}
            dependentCountMap={dependentCountMapRef.current}
            onHostPress={handleHostPress}
            onGuestPress={handleGuestPress}
            onToggle={handleGroupToggle}
            onDone={markDone}
            onSkip={markSkipped}
            onDelete={deleteTask}
          />
        );
      }
      const isSelected = currentSelected.has(item.task.id);
      return (
        <TaskCard
          task={item.task}
          scheduleStart={item.scheduleStart}
          scheduleEnd={item.scheduleEnd}
          isDone={item.isDone}
          selected={isSelected}
          habitDisplayId={
            item.task.habit_id
              ? habitDisplayIdMapRef.current.get(item.task.habit_id)
              : undefined
          }
          dependentCount={dependentCountMapRef.current.get(item.task.id)}
          onPress={handleTaskPress}
          onLongPress={handleTaskLongPress}
          onDone={markDone}
          onSkip={markSkipped}
          onDelete={deleteTask}
        />
      );
    },
    [
      handleTaskPress,
      handleTaskLongPress,
      handleHostPress,
      handleGuestPress,
      handleGroupToggle,
      markDone,
      markSkipped,
      deleteTask,
      colors,
      styles,
    ],
  );

  const keyExtractor = useCallback(
    (item: ListItem, index: number) =>
      item.type === 'separator'
        ? `sep-${index}`
        : item.type === 'parallelGroup'
          ? `group-${item.host.id}`
          : `task-${item.task.id}`,
    [],
  );

  const searching = searchQuery.length > 0;
  const listHeader = useMemo(
    () =>
      !searching && hasPast ? (
        <Pressable style={styles.pastToggle} onPress={togglePast}>
          <Reanimated.View style={chevronStyle}>
            <Ionicons name="chevron-down" size={16} color={colors.brand} />
          </Reanimated.View>
          <Text style={styles.pastToggleText}>
            {showPast ? '過去を隠す' : '過去を表示'}
          </Text>
        </Pressable>
      ) : null,
    [hasPast, showPast, chevronStyle, togglePast, searching, styles, colors],
  );

  const handleScroll = useCallback(
    (e: NativeSyntheticEvent<NativeScrollEvent>) => {
      scrollOffsetRef.current = e.nativeEvent.contentOffset.y;
    },
    [],
  );

  const handleListLayout = useCallback((e: LayoutChangeEvent) => {
    listLayoutHeightRef.current = e.nativeEvent.layout.height;
  }, []);

  const handleScrollToIndexFailed = useCallback(
    ({
      index,
      averageItemLength,
    }: {
      index: number;
      averageItemLength: number;
    }) => {
      // Fallback: scroll to approximate offset
      listRef.current?.scrollToOffset({
        offset: index * averageItemLength,
        animated: true,
      });
    },
    [],
  );

  const contentContainerStyle = useMemo(
    () => [styles.listContent, { paddingBottom: 100 + insets.bottom }],
    [insets.bottom, styles],
  );

  if (view === 'graph') {
    return (
      <GraphWrapper
        client={client}
        onBack={() => setView('task')}
        onTaskPress={(taskId) => router.push(`/task/${taskId}`)}
        viewChanger={viewChanger}
        refreshKey={lastCompletedAt}
      />
    );
  }

  if (view === 'habit') {
    return (
      <HabitWrapper
        client={client}
        viewChanger={viewChanger}
        refreshKey={lastCompletedAt}
      />
    );
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      {/* Top bar */}
      <View style={[styles.topBar, { paddingTop: 8 + insets.top }]}>
        <ContextMenu
          hasSelection={selected.size > 0}
          onSettings={() => router.push('/settings')}
          onStats={() => router.push('/stats')}
          onUndo={() =>
            undoRedo
              .undo()
              .then(refresh)
              .catch((e) => showError(e, 'アンドゥに失敗'))
          }
          onRedo={() =>
            undoRedo
              .redo()
              .then(refresh)
              .catch((e) => showError(e, 'リドゥに失敗'))
          }
          onSelectAll={() =>
            setSelected(
              new Set(
                items.flatMap((it) =>
                  it.type === 'task'
                    ? [it.task.id]
                    : it.type === 'parallelGroup'
                      ? [it.host.id, ...it.guests.map((g) => g.id)]
                      : [],
                ),
              ),
            )
          }
          onRescheduleSelected={rescheduleSelected}
          onRescheduleOthers={rescheduleOthers}
          onDeleteSelected={deleteSelected}
          onCreateDependent={createDependent}
          onSetStatusSelected={setStatusSelected}
          onClearSelection={() => setSelected(new Set())}
          operationBusy={scheduleOperation !== null}
        />
        <TaskSearchBar
          value={searchQuery}
          onChangeText={setSearchQuery}
          client={client}
        />
        <Pressable
          style={({ pressed }) => [
            styles.topButton,
            pressed && styles.topButtonPressed,
            scheduleOperation && styles.topButtonDisabled,
          ]}
          disabled={scheduleOperation !== null}
          onPress={() => {
            if (!client || scheduleOperation) return;
            haptic.medium();
            startScheduleOperation('generate', {}, 'タスクをスケジュール中');
          }}
        >
          <Ionicons name="refresh" size={22} color={colors.brand} />
        </Pressable>
      </View>

      {/* Task list */}
      <FlatList
        ref={listRef}
        data={items}
        keyExtractor={keyExtractor}
        renderItem={renderListItem}
        ListHeaderComponent={listHeader}
        refreshControl={
          <RefreshControl refreshing={refreshing} onRefresh={refresh} />
        }
        onScroll={handleScroll}
        onScrollBeginDrag={showNavButtons}
        onScrollEndDrag={scheduleHideNavButtons}
        onMomentumScrollBegin={showNavButtons}
        onMomentumScrollEnd={scheduleHideNavButtons}
        scrollEventThrottle={16}
        onLayout={handleListLayout}
        onViewableItemsChanged={handleViewableItemsChanged}
        viewabilityConfig={VIEWABILITY_CONFIG}
        onScrollToIndexFailed={handleScrollToIndexFailed}
        contentContainerStyle={contentContainerStyle}
        extraData={selected}
        initialNumToRender={12}
        maxToRenderPerBatch={12}
        windowSize={7}
      />

      {/* Bottom bar */}
      <View style={[styles.bottomBar, { paddingBottom: 16 + insets.bottom }]}>
        <GestureDetector gesture={startDoneGesture}>
          <Reanimated.View
            style={[
              styles.startDoneButton,
              startDoneButtonStyle,
              {
                bottom: 16 + insets.bottom,
                backgroundColor: inProgressTask ? colors.red : colors.green,
              },
            ]}
            accessible
            accessibilityRole="button"
            accessibilityLabel={
              inProgressTask
                ? '進行中のタスクを一時停止または進捗を記録'
                : '次のタスクを開始'
            }
            accessibilityHint={
              inProgressTask ? '上にスライドしてタスクを完了' : undefined
            }
          >
            <Ionicons
              name={inProgressTask ? 'pause' : 'play'}
              size={24}
              color={colors.white}
            />
          </Reanimated.View>
        </GestureDetector>
      </View>

      {/* Slide-up-to-done hint for the start/done button */}
      {inProgressTask && (
        <Reanimated.View
          style={[
            styles.startDoneHint,
            { bottom: 72 + insets.bottom, right: 20 },
            startDoneHintStyle,
          ]}
          pointerEvents="none"
        >
          <Ionicons name="checkmark" size={24} color={colors.green} />
        </Reanimated.View>
      )}

      {/* Progress sheet for in-progress pause */}
      {progressTask && (
        <TaskProgressSheet
          visible={progressSheetVisible}
          task={progressTask}
          mode="pause"
          onConfirm={handleHomeProgressConfirm}
          onRecord={handleHomeRecordOnly}
          onCancel={() => {
            setProgressSheetVisible(false);
            setProgressTask(null);
          }}
        />
      )}

      {/* Floating navigation — visible only while scrolling (#308) */}
      <Reanimated.View
        style={[
          { position: 'absolute', top: 0, bottom: 0, left: 0, right: 0 },
          navButtonsStyle,
        ]}
        pointerEvents={navButtonsVisible ? 'box-none' : 'none'}
      >
        <NavigationButtons
          onScrollUpByDay={() => {
            showNavButtons();
            scrollByDay(-1);
            scheduleHideNavButtons();
          }}
          onScrollUpByPage={() => {
            showNavButtons();
            scrollByPage(-1);
            scheduleHideNavButtons();
          }}
          onScrollDownByDay={() => {
            showNavButtons();
            scrollByDay(1);
            scheduleHideNavButtons();
          }}
          onScrollDownByPage={() => {
            showNavButtons();
            scrollByPage(1);
            scheduleHideNavButtons();
          }}
          onJumpToDate={(date) => {
            showNavButtons();
            jumpToDate(date);
            scheduleHideNavButtons();
          }}
          markedDates={markedDates}
        />
      </Reanimated.View>

      {/* View changer */}
      <ViewChanger current={view} onChange={setView} />
    </View>
  );
}

// Placeholder wrappers for graph and habit views within home
function GraphWrapper({
  client,
  onBack,
  viewChanger,
  onTaskPress,
  refreshKey,
}: {
  client: TakusuClient | null;
  onBack: () => void;
  viewChanger: React.ReactNode;
  onTaskPress: (taskId: string) => void;
  refreshKey?: number | null;
}) {
  // Lazy load to avoid circular deps
  const { GraphView } = require('@/src/views/GraphView');
  return (
    <View style={{ flex: 1 }}>
      <GraphView
        client={client}
        onBack={onBack}
        onTaskPress={onTaskPress}
        refreshKey={refreshKey}
      />
      {viewChanger}
    </View>
  );
}

function HabitWrapper({
  client,
  viewChanger,
  refreshKey,
}: {
  client: TakusuClient | null;
  viewChanger: React.ReactNode;
  refreshKey?: number | null;
}) {
  const { HabitView } = require('@/src/views/HabitView');
  return (
    <View style={{ flex: 1 }}>
      <HabitView client={client} refreshKey={refreshKey} />
      {viewChanger}
    </View>
  );
}
