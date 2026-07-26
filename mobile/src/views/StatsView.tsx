// StatsView — task/habit statistics with an Anki-inspired layout
//
// Features:
//   - Period selector (7/30/90 days, all history)
//   - Today's summary
//   - Period summary cards + averages
//   - Daily heatmap (GitHub-style calendar)
//   - Daily stacked bar chart (completed / skipped / remaining)
//   - Future due forecast
//   - Habit breakdown
//
// Stats are computed client-side from listTasks / getSchedule / listHabits.
// The server timezone (from settings) is used for day grouping.

import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  ActivityIndicator,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useIsFocused, useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { IconButton, SegmentedButtons } from 'react-native-paper';
import { useServer } from '@/src/api/ServerProvider';
import { logError, showError } from '@/src/api/errors';
import { parseSchedule } from '@/src/api/types';
import type { HabitRow, ScheduleEntry, TaskRow } from '@/src/api/types';
import { formatDuration } from '@/src/utils/duration';
import { useTheme, habitColorFor, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { dateKey, todayDateKey } from '@/src/utils/dateKey';

type Period = '7' | '30' | '90' | 'all';

const PERIOD_BUTTONS = [
  { value: '7', label: '7日' },
  { value: '30', label: '30日' },
  { value: '90', label: '90日' },
  { value: 'all', label: '全期間' },
];

const WEEKDAY_LABELS = ['月', '火', '水', '木', '金', '土', '日'];
const SHORT_WEEKDAYS = ['日', '月', '火', '水', '木', '金', '土'];

const MS_PER_DAY = 24 * 60 * 60 * 1000;
const BAR_HEIGHT = 80;

function periodKeys(days: number, tz?: string): string[] {
  const keys: string[] = [];
  const now = Date.now();
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now - i * MS_PER_DAY);
    keys.push(dateKey(d.toISOString(), tz));
  }
  return keys;
}

function generateFutureKeys(days: number, tz?: string): string[] {
  const keys: string[] = [];
  const now = Date.now();
  for (let i = 0; i < days; i++) {
    const d = new Date(now + i * MS_PER_DAY);
    keys.push(dateKey(d.toISOString(), tz));
  }
  return keys;
}

function generateDateKeys(
  startKey: string,
  endKey: string,
  tz?: string,
): string[] {
  const start = new Date(`${startKey}T12:00:00Z`);
  const end = new Date(`${endKey}T12:00:00Z`);
  const keys: string[] = [];
  const cur = new Date(start);
  while (cur.getTime() <= end.getTime()) {
    keys.push(dateKey(cur.toISOString(), tz));
    cur.setUTCDate(cur.getUTCDate() + 1);
  }
  return keys;
}

function taskDateKey(
  task: TaskRow,
  scheduleMap: Map<string, ScheduleEntry>,
  tz?: string,
): string | null {
  const endAt = scheduleMap.get(task.id)?.end_at ?? task.end_at;
  if (!endAt) return null;
  return dateKey(endAt, tz);
}

function weekdayIndex(key: string): number {
  const day = new Date(`${key}T12:00:00Z`).getUTCDay();
  return day === 0 ? 6 : day - 1;
}

function formatMonthDay(key: string): string {
  const d = new Date(`${key}T12:00:00Z`);
  return `${d.getUTCMonth() + 1}/${d.getUTCDate()}`;
}

function buildWeeks(keys: string[]): (string | null)[][] {
  const weeks: (string | null)[][] = [];
  let current: (string | null)[] = [null, null, null, null, null, null, null];
  let first = true;
  for (const key of keys) {
    const wd = weekdayIndex(key);
    if (first) {
      for (let i = 0; i < wd; i++) {
        current[i] = null;
      }
      first = false;
    }
    current[wd] = key;
    if (wd === 6) {
      weeks.push(current);
      current = [null, null, null, null, null, null, null];
    }
  }
  if (current.some((k) => k !== null)) {
    weeks.push(current);
  }
  return weeks;
}

interface DayBreakdown {
  completed: number;
  skipped: number;
  inProgress: number;
  scheduled: number;
  pending: number;
  remaining: number;
  total: number;
}

function emptyBreakdown(): DayBreakdown {
  return {
    completed: 0,
    skipped: 0,
    inProgress: 0,
    scheduled: 0,
    pending: 0,
    remaining: 0,
    total: 0,
  };
}

interface StatsData {
  total: number;
  completed: number;
  skipped: number;
  inProgress: number;
  scheduled: number;
  pending: number;
  remaining: number;
  completionRate: number;
  dayKeys: string[];
  dayStatusMap: Map<string, DayBreakdown>;
  maxDayTotal: number;
  daysWithCompleted: number;
  avgCompletedPerDay: number;
  avgCompletedPerStudiedDay: number;
  maxCompletedInDay: number;
  totalActualMinutes: number;
  avgActualMinutesPerCompleted: number;
  todayBreakdown: DayBreakdown;
  futureKeys: string[];
  futureCounts: number[];
  futureMax: number;
  habitCounts: Map<string, number>;
}

function computeStats(
  tasks: TaskRow[],
  scheduleMap: Map<string, ScheduleEntry>,
  habits: HabitRow[],
  serverTz: string | undefined,
  period: Period,
): StatsData {
  const todayKey = todayDateKey(serverTz);

  let startKey: string;
  if (period === 'all') {
    const keys = tasks
      .map((t) => taskDateKey(t, scheduleMap, serverTz))
      .filter((k): k is string => k !== null)
      .sort((a, b) => a.localeCompare(b));
    const minKey = keys[0];
    startKey =
      minKey ??
      dateKey(new Date(Date.now() - 30 * MS_PER_DAY).toISOString(), serverTz);
  } else {
    const days = Number(period);
    startKey = periodKeys(days, serverTz)[0] ?? todayKey;
  }

  const dayKeys = generateDateKeys(startKey, todayKey, serverTz);
  const dayStatusMap = new Map<string, DayBreakdown>();
  for (const k of dayKeys) {
    dayStatusMap.set(k, emptyBreakdown());
  }

  let completed = 0;
  let skipped = 0;
  let inProgress = 0;
  let scheduled = 0;
  let pending = 0;
  let totalActualMinutes = 0;
  const habitCounts = new Map<string, number>();

  for (const t of tasks) {
    const key = taskDateKey(t, scheduleMap, serverTz);
    if (key && key >= startKey && key <= todayKey) {
      const bd = dayStatusMap.get(key) ?? emptyBreakdown();
      bd.total++;
      if (t.habit_id) {
        habitCounts.set(t.habit_id, (habitCounts.get(t.habit_id) ?? 0) + 1);
      }
      switch (t.status) {
        case 'completed':
          completed++;
          bd.completed++;
          totalActualMinutes += t.actual_minutes ?? 0;
          break;
        case 'skipped':
          skipped++;
          bd.skipped++;
          break;
        case 'in_progress':
          inProgress++;
          bd.inProgress++;
          bd.remaining++;
          break;
        case 'scheduled':
          scheduled++;
          bd.scheduled++;
          bd.remaining++;
          break;
        case 'pending':
          pending++;
          bd.pending++;
          bd.remaining++;
          break;
      }
      dayStatusMap.set(key, bd);
    }
  }

  const total = completed + skipped + inProgress + scheduled + pending;
  const remaining = inProgress + scheduled + pending;
  const completionRate = total > 0 ? completed / total : 0;

  let maxDayTotal = 1;
  let maxCompletedInDay = 0;
  let daysWithCompleted = 0;
  for (const bd of dayStatusMap.values()) {
    if (bd.total > maxDayTotal) maxDayTotal = bd.total;
    if (bd.completed > maxCompletedInDay) maxCompletedInDay = bd.completed;
    if (bd.completed > 0) daysWithCompleted++;
  }

  const avgCompletedPerDay =
    dayKeys.length > 0 ? completed / dayKeys.length : 0;
  const avgCompletedPerStudiedDay =
    daysWithCompleted > 0 ? completed / daysWithCompleted : 0;
  const avgActualMinutesPerCompleted =
    completed > 0 ? totalActualMinutes / completed : 0;

  const futureDays = period === 'all' ? 30 : Number(period);
  const futureDayKeys = generateFutureKeys(futureDays, serverTz);
  const futureIndex = new Map(futureDayKeys.map((k, i) => [k, i]));
  const futureCounts = futureDayKeys.map(() => 0);
  for (const t of tasks) {
    if (t.status === 'completed' || t.status === 'skipped') continue;
    const key = taskDateKey(t, scheduleMap, serverTz);
    const idx = key !== null ? futureIndex.get(key) : undefined;
    if (idx !== undefined) {
      futureCounts[idx]++;
    }
  }
  const futureMax = Math.max(1, ...futureCounts);

  return {
    total,
    completed,
    skipped,
    inProgress,
    scheduled,
    pending,
    remaining,
    completionRate,
    dayKeys,
    dayStatusMap,
    maxDayTotal,
    daysWithCompleted,
    avgCompletedPerDay,
    avgCompletedPerStudiedDay,
    maxCompletedInDay,
    totalActualMinutes,
    avgActualMinutesPerCompleted,
    todayBreakdown: dayStatusMap.get(todayKey) ?? emptyBreakdown(),
    futureKeys: futureDayKeys,
    futureCounts,
    futureMax,
    habitCounts,
  };
}

function StatCard({
  label,
  value,
  color,
}: {
  label: string;
  value: string;
  color?: string;
}) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  return (
    <View style={[styles.card, { backgroundColor: colors.surface }]}>
      <Text style={[styles.cardValue, { color: color ?? colors.brand }]}>
        {value}
      </Text>
      <Text style={[styles.cardLabel, { color: colors.gray }]}>{label}</Text>
    </View>
  );
}

function Heatmap({
  dayKeys,
  dayStatusMap,
  maxCount,
}: {
  dayKeys: string[];
  dayStatusMap: Map<string, DayBreakdown>;
  maxCount: number;
}) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const weeks = useMemo(() => buildWeeks(dayKeys), [dayKeys]);

  return (
    <View style={styles.heatmapContainer}>
      <View style={styles.weekdayLabels}>
        {WEEKDAY_LABELS.map((label) => (
          <Text
            key={label}
            style={[styles.weekdayLabel, { color: colors.gray }]}
          >
            {label}
          </Text>
        ))}
      </View>
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        contentContainerStyle={styles.heatmapScroll}
      >
        <View style={styles.heatmapGrid}>
          {weeks.map((week, wi) => (
            <View key={wi} style={styles.weekColumn}>
              {week.map((key, di) => {
                const bd = key ? dayStatusMap.get(key) : undefined;
                const count = bd?.total ?? 0;
                const opacity =
                  count > 0
                    ? 0.25 + 0.75 * Math.min(count / maxCount, 1)
                    : 0.15;
                return (
                  <View
                    key={di}
                    style={[
                      styles.cell,
                      {
                        backgroundColor:
                          count > 0 ? colors.brand : colors.grayLight,
                        opacity,
                      },
                    ]}
                  />
                );
              })}
            </View>
          ))}
        </View>
      </ScrollView>
    </View>
  );
}

function DailyStackedBars({
  dayKeys,
  dayStatusMap,
  maxTotal,
}: {
  dayKeys: string[];
  dayStatusMap: Map<string, DayBreakdown>;
  maxTotal: number;
}) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const scale = maxTotal > 0 ? BAR_HEIGHT / maxTotal : 0;

  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      contentContainerStyle={styles.barScroll}
    >
      <View style={styles.barChart}>
        {dayKeys.map((key) => {
          const bd = dayStatusMap.get(key) ?? emptyBreakdown();
          const completedH = bd.completed * scale;
          const skippedH = bd.skipped * scale;
          const remainingH = bd.remaining * scale;
          return (
            <View key={key} style={styles.barColumn}>
              <View
                style={[
                  styles.barStack,
                  { height: BAR_HEIGHT, justifyContent: 'flex-end' },
                ]}
              >
                {remainingH > 0 && (
                  <View
                    style={{
                      height: remainingH,
                      backgroundColor: colors.brand,
                    }}
                  />
                )}
                {skippedH > 0 && (
                  <View
                    style={{ height: skippedH, backgroundColor: colors.red }}
                  />
                )}
                {completedH > 0 && (
                  <View
                    style={{
                      height: completedH,
                      backgroundColor: colors.green,
                    }}
                  />
                )}
              </View>
              <Text style={[styles.barDate, { color: colors.gray }]}>
                {formatMonthDay(key)}
              </Text>
            </View>
          );
        })}
      </View>
    </ScrollView>
  );
}

function FutureForecast({
  futureKeys,
  futureCounts,
  maxCount,
  todayKey,
}: {
  futureKeys: string[];
  futureCounts: number[];
  maxCount: number;
  todayKey: string;
}) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);

  return (
    <View style={styles.forecastContainer}>
      {futureKeys.map((key, i) => {
        const count = futureCounts[i] ?? 0;
        const isToday = key === todayKey;
        const d = new Date(`${key}T12:00:00Z`);
        const label =
          futureKeys.length <= 7
            ? `${d.getUTCMonth() + 1}/${d.getUTCDate()} (${
                SHORT_WEEKDAYS[d.getUTCDay()]
              })`
            : `${d.getUTCMonth() + 1}/${d.getUTCDate()}`;
        return (
          <View key={key} style={styles.forecastRow}>
            <Text
              style={[
                styles.forecastDate,
                { color: isToday ? colors.brand : colors.black },
              ]}
            >
              {label}
            </Text>
            <View
              style={[
                styles.forecastTrack,
                { backgroundColor: colors.grayLight },
              ]}
            >
              <View
                style={[
                  styles.forecastBar,
                  {
                    width: `${(count / maxCount) * 100}%`,
                    backgroundColor: isToday ? colors.brand : colors.brandLight,
                  },
                ]}
              />
            </View>
            <Text style={[styles.forecastCount, { color: colors.gray }]}>
              {count}
            </Text>
          </View>
        );
      })}
    </View>
  );
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
    topBarCenter: {
      flex: 1,
      alignItems: 'center',
      justifyContent: 'center',
    },
    topBarRight: {
      width: 48,
    },
    title: {
      fontSize: 18,
      fontWeight: '600',
    },
    scroll: {
      flex: 1,
    },
    section: {
      paddingHorizontal: 16,
      paddingVertical: 12,
    },
    todaySection: {
      paddingBottom: 4,
    },
    loader: {
      marginTop: 40,
    },
    cards: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 8,
      paddingHorizontal: 16,
      paddingTop: 8,
    },
    card: {
      flex: 1,
      minWidth: 72,
      borderRadius: 12,
      paddingVertical: 12,
      paddingHorizontal: 8,
      alignItems: 'center',
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 1 },
      shadowOpacity: 0.1,
      shadowRadius: 2,
      elevation: 2,
    },
    cardValue: {
      fontSize: 22,
      fontWeight: '700',
    },
    cardLabel: {
      fontSize: 12,
      marginTop: 4,
    },
    summaryText: {
      fontSize: 13,
      paddingHorizontal: 16,
      paddingTop: 8,
      lineHeight: 20,
    },
    sectionTitle: {
      fontSize: 15,
      fontWeight: '600',
      marginBottom: 8,
    },
    heatmapContainer: {
      flexDirection: 'row',
      alignItems: 'flex-start',
    },
    weekdayLabels: {
      marginRight: 4,
    },
    weekdayLabel: {
      fontSize: 10,
      height: 16,
      lineHeight: 16,
    },
    heatmapScroll: {
      flexDirection: 'row',
    },
    heatmapGrid: {
      flexDirection: 'row',
    },
    weekColumn: {
      flexDirection: 'column',
      marginRight: 2,
    },
    cell: {
      width: 14,
      height: 14,
      borderRadius: 2,
      marginBottom: 2,
    },
    legend: {
      flexDirection: 'row',
      gap: 16,
      marginBottom: 8,
    },
    legendItem: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
    },
    legendDot: {
      width: 10,
      height: 10,
      borderRadius: 5,
    },
    legendText: {
      fontSize: 12,
    },
    barScroll: {
      flexDirection: 'row',
    },
    barChart: {
      flexDirection: 'row',
      alignItems: 'flex-end',
    },
    barColumn: {
      width: 24,
      marginRight: 4,
      alignItems: 'center',
    },
    barStack: {
      width: 18,
      borderRadius: 2,
      overflow: 'hidden',
    },
    barDate: {
      fontSize: 9,
      marginTop: 2,
    },
    forecastContainer: {
      gap: 8,
    },
    forecastRow: {
      flexDirection: 'row',
      alignItems: 'center',
    },
    forecastDate: {
      width: 80,
      fontSize: 12,
    },
    forecastTrack: {
      flex: 1,
      height: 10,
      borderRadius: 5,
      marginHorizontal: 8,
      overflow: 'hidden',
    },
    forecastBar: {
      height: '100%',
      borderRadius: 5,
    },
    forecastCount: {
      width: 28,
      fontSize: 13,
      fontWeight: '600',
      textAlign: 'right',
    },
    habitRow: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingVertical: 10,
      borderBottomWidth: 1,
    },
    habitDot: {
      width: 10,
      height: 10,
      borderRadius: 5,
      marginRight: 10,
    },
    habitTitle: {
      flex: 1,
      fontSize: 15,
    },
    habitCount: {
      fontSize: 15,
      fontWeight: '600',
    },
  });

export function StatsView() {
  const { client } = useServer();
  const router = useRouter();
  const { theme, colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();

  const [tasks, setTasks] = useState<TaskRow[]>([]);
  const [schedule, setSchedule] = useState<ScheduleEntry[]>([]);
  const [habits, setHabits] = useState<HabitRow[]>([]);
  const [serverTz, setServerTz] = useState<string | undefined>();
  const [loading, setLoading] = useState(false);
  const [period, setPeriod] = useState<Period>('30');

  const refresh = useCallback(async () => {
    if (!client) return;
    setLoading(true);
    try {
      const [taskList, sched, habitList, settings] = await Promise.all([
        client.listTasks(),
        client.getSchedule().catch((e) => {
          logError('スケジュール取得', e);
          return null;
        }),
        client.listHabits().catch((e) => {
          logError('Habit取得', e);
          return [] as HabitRow[];
        }),
        client.getSettings().catch((e) => {
          logError('設定取得', e);
          return null;
        }),
      ]);
      setTasks(taskList);
      setSchedule(sched ? parseSchedule(sched.schedule) : []);
      setHabits(habitList);
      setServerTz(settings?.tz);
    } catch (e) {
      showError(e, '統計データの取得に失敗');
    } finally {
      setLoading(false);
    }
  }, [client]);

  const isFocused = useIsFocused();
  useEffect(() => {
    if (client && isFocused) {
      refresh();
    }
  }, [client, isFocused, refresh]);

  const scheduleMap = useMemo(() => {
    const map = new Map<string, ScheduleEntry>();
    for (const e of schedule) map.set(e.task_id, e);
    return map;
  }, [schedule]);

  const stats = useMemo(
    () => computeStats(tasks, scheduleMap, habits, serverTz, period),
    [tasks, scheduleMap, habits, serverTz, period],
  );

  const habitEntries = useMemo(() => {
    return habits
      .map((h) => ({ habit: h, count: stats.habitCounts.get(h.id) ?? 0 }))
      .filter((e) => e.count > 0)
      .sort((a, b) => b.count - a.count)
      .slice(0, 10);
  }, [habits, stats.habitCounts]);

  function handleBack() {
    haptic.light();
    router.back();
  }

  function handlePeriodChange(value: string) {
    haptic.select();
    setPeriod(value as Period);
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View style={[styles.topBar, { paddingTop: 8 + insets.top }]}>
        <IconButton
          icon="chevron-left"
          iconColor={colors.brand}
          size={28}
          onPress={handleBack}
        />
        <View style={styles.topBarCenter}>
          <Text style={[styles.title, { color: colors.black }]}>統計</Text>
        </View>
        <View style={styles.topBarRight} />
      </View>

      <ScrollView
        style={styles.scroll}
        contentContainerStyle={{ paddingBottom: insets.bottom }}
      >
        <View style={styles.section}>
          <SegmentedButtons
            value={period}
            onValueChange={handlePeriodChange}
            buttons={PERIOD_BUTTONS}
            theme={{ colors: { primary: colors.brand } }}
          />
        </View>

        {loading ? (
          <ActivityIndicator
            size="large"
            color={colors.brand}
            style={styles.loader}
          />
        ) : (
          <>
            <View style={[styles.section, styles.todaySection]}>
              <Text style={[styles.sectionTitle, { color: colors.black }]}>
                今日
              </Text>
              <View style={styles.cards}>
                <StatCard
                  label="完了"
                  value={String(stats.todayBreakdown.completed)}
                  color={colors.green}
                />
                <StatCard
                  label="予定"
                  value={String(
                    stats.todayBreakdown.scheduled +
                      stats.todayBreakdown.inProgress,
                  )}
                />
                <StatCard
                  label="残り"
                  value={String(stats.todayBreakdown.remaining)}
                  color={colors.red}
                />
              </View>
            </View>

            <View style={styles.cards}>
              <StatCard
                label="完了"
                value={String(stats.completed)}
                color={colors.green}
              />
              <StatCard
                label="スキップ"
                value={String(stats.skipped)}
                color={colors.red}
              />
              <StatCard label="残タスク" value={String(stats.remaining)} />
              <StatCard
                label="完了率"
                value={`${Math.round(stats.completionRate * 100)}%`}
              />
            </View>

            <View style={styles.cards}>
              <StatCard
                label="実績時間"
                value={formatDuration(stats.totalActualMinutes)}
              />
              {stats.completed > 0 && (
                <StatCard
                  label="平均実績"
                  value={formatDuration(
                    Math.round(stats.avgActualMinutesPerCompleted),
                  )}
                />
              )}
            </View>

            <Text style={[styles.summaryText, { color: colors.gray }]}>
              {stats.total > 0
                ? `期間中 ${stats.completed} タスク完了 ` +
                  `（1日平均 ${stats.avgCompletedPerDay.toFixed(1)} タスク、 ` +
                  `完了した日平均 ${stats.avgCompletedPerStudiedDay.toFixed(
                    1,
                  )} タスク、 ` +
                  `最大 ${stats.maxCompletedInDay} タスク/日）` +
                  (stats.totalActualMinutes > 0
                    ? ` 実績時間 ${formatDuration(stats.totalActualMinutes)}`
                    : '')
                : 'この期間のデータはありません'}
            </Text>

            <View style={styles.section}>
              <Text style={[styles.sectionTitle, { color: colors.black }]}>
                日別タスク数（予定/締切日ベース）
              </Text>
              <Heatmap
                dayKeys={stats.dayKeys}
                dayStatusMap={stats.dayStatusMap}
                maxCount={stats.maxDayTotal}
              />
            </View>

            <View style={styles.section}>
              <Text style={[styles.sectionTitle, { color: colors.black }]}>
                日別内訳
              </Text>
              <View style={styles.legend}>
                <View style={styles.legendItem}>
                  <View
                    style={[
                      styles.legendDot,
                      { backgroundColor: colors.green },
                    ]}
                  />
                  <Text style={[styles.legendText, { color: colors.gray }]}>
                    完了
                  </Text>
                </View>
                <View style={styles.legendItem}>
                  <View
                    style={[styles.legendDot, { backgroundColor: colors.red }]}
                  />
                  <Text style={[styles.legendText, { color: colors.gray }]}>
                    スキップ
                  </Text>
                </View>
                <View style={styles.legendItem}>
                  <View
                    style={[
                      styles.legendDot,
                      { backgroundColor: colors.brand },
                    ]}
                  />
                  <Text style={[styles.legendText, { color: colors.gray }]}>
                    残り
                  </Text>
                </View>
              </View>
              <DailyStackedBars
                dayKeys={stats.dayKeys}
                dayStatusMap={stats.dayStatusMap}
                maxTotal={stats.maxDayTotal}
              />
            </View>

            <View style={styles.section}>
              <Text style={[styles.sectionTitle, { color: colors.black }]}>
                今後の予定
              </Text>
              <FutureForecast
                futureKeys={stats.futureKeys}
                futureCounts={stats.futureCounts}
                maxCount={stats.futureMax}
                todayKey={todayDateKey(serverTz)}
              />
            </View>

            {habitEntries.length > 0 && (
              <View style={styles.section}>
                <Text style={[styles.sectionTitle, { color: colors.black }]}>
                  Habit 別タスク数
                </Text>
                {habitEntries.map((e) => {
                  const displayId = e.habit.display_id;
                  const habitColor =
                    displayId !== undefined
                      ? habitColorFor(displayId, theme)
                      : colors.brand;
                  return (
                    <View
                      key={e.habit.id}
                      style={[
                        styles.habitRow,
                        { borderBottomColor: colors.separator },
                      ]}
                    >
                      <View
                        style={[
                          styles.habitDot,
                          { backgroundColor: habitColor },
                        ]}
                      />
                      <Text
                        style={[styles.habitTitle, { color: colors.black }]}
                      >
                        {e.habit.title}
                      </Text>
                      <Text style={[styles.habitCount, { color: colors.gray }]}>
                        {e.count}
                      </Text>
                    </View>
                  );
                })}
              </View>
            )}
          </>
        )}
      </ScrollView>
    </View>
  );
}
