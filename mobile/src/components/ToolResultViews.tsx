import { useMemo } from 'react';
import { View, Text } from 'react-native';
import { type ColorSet } from '@/src/theme';
import {
  isRecord,
  asString,
  asNumber,
  asBoolean,
  asArray,
  formatInstant,
  formatDateTimeRange,
  formatRecurrence,
  STATUS_LABELS,
} from '@/src/components/ApprovalPanel';
import { formatDuration } from '@/src/utils/duration';
import { makeStyles, DetailRow } from './ToolCallDetailCommon';

function taskStatusColor(status: string, colors: ColorSet): string {
  switch (status) {
    case 'completed':
      return colors.green;
    case 'in_progress':
    case 'scheduled':
      return colors.brand;
    case 'overdue':
      return colors.red;
    case 'skipped':
    case 'pending':
    default:
      return colors.gray;
  }
}

const taskStatusLabels: Record<string, string> = {
  ...STATUS_LABELS,
  overdue: '遅延',
};

interface TaskCardProps {
  task: Record<string, unknown>;
  colors: ColorSet;
}

function TaskCard({ task, colors }: TaskCardProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const reference = asString(task.reference);
  const title = asString(task.title);
  const status = asString(task.status);
  const startAt = asString(task.start_at);
  const endAt = asString(task.end_at);
  const avg = asNumber(task.avg_minutes);
  const sigma = asNumber(task.sigma_minutes);
  const quantityTotal = asNumber(task.quantity_total);
  const quantityDone = asNumber(task.quantity_done);
  const quantityUnit = asString(task.quantity_unit);
  const depends = asArray<string>(task.depends);
  const description = asString(task.description);

  const statusLabel = status ? (taskStatusLabels[status] ?? status) : null;
  const statusColor = status ? taskStatusColor(status, colors) : colors.gray;
  const time =
    startAt && endAt
      ? formatDateTimeRange(startAt, endAt)
      : endAt
        ? formatInstant(endAt)
        : startAt
          ? formatInstant(startAt)
          : null;
  const duration =
    avg !== undefined
      ? `${formatDuration(avg)}${
          sigma !== undefined ? ` (σ ${formatDuration(sigma)})` : ''
        }`
      : null;
  const quantity =
    quantityTotal !== undefined
      ? `${quantityDone ?? 0}/${quantityTotal}${
          quantityUnit ? ` ${quantityUnit}` : ''
        }`
      : null;

  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeHeader}>
        {status ? (
          <View style={[styles.changeBadge, { backgroundColor: statusColor }]}>
            <Text style={styles.changeBadgeText}>{statusLabel}</Text>
          </View>
        ) : null}
        <Text
          style={[styles.changeTarget, { color: colors.black }]}
          numberOfLines={1}
        >
          {reference ? `[${reference}] ` : ''}
          {title ?? ''}
        </Text>
      </View>
      <View
        style={[
          styles.whenBlock,
          { backgroundColor: colors.surface, borderColor: colors.separator },
        ]}
      >
        {time ? <DetailRow label="予定" value={time} colors={colors} /> : null}
        {duration ? (
          <DetailRow label="見積" value={duration} colors={colors} />
        ) : null}
        {quantity ? (
          <DetailRow label="進捗" value={quantity} colors={colors} />
        ) : null}
        {depends && depends.length > 0 ? (
          <DetailRow label="依存" value={depends.join('、')} colors={colors} />
        ) : null}
      </View>
      {description ? (
        <Text
          style={{ fontSize: 13, color: colors.black, marginTop: 4 }}
          numberOfLines={2}
        >
          {description}
        </Text>
      ) : null}
    </View>
  );
}

interface TaskResultViewProps {
  data: unknown;
  colors: ColorSet;
}

export function TaskResultView({ data, colors }: TaskResultViewProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const tasks = useMemo<Record<string, unknown>[]>(() => {
    if (Array.isArray(data)) {
      return data.filter(isRecord);
    }
    if (isRecord(data)) {
      return asArray<Record<string, unknown>>(data.tasks, isRecord) ?? [];
    }
    return [];
  }, [data]);
  const dependencies = useMemo<Record<string, unknown>[]>(() => {
    if (isRecord(data)) {
      return (
        asArray<Record<string, unknown>>(data.dependencies, isRecord) ?? []
      );
    }
    return [];
  }, [data]);
  const missingDependencies = useMemo<string[]>(() => {
    if (isRecord(data)) {
      return asArray<string>(data.missing_dependencies) ?? [];
    }
    return [];
  }, [data]);

  if (
    tasks.length === 0 &&
    dependencies.length === 0 &&
    missingDependencies.length === 0
  ) {
    return (
      <Text style={[styles.emptyText, { color: colors.gray }]}>
        該当するタスクが見つかりませんでした。
      </Text>
    );
  }

  return (
    <View style={{ gap: 12 }}>
      {tasks.map((task, index) => (
        <TaskCard
          key={asString(task.reference) ?? index}
          task={task}
          colors={colors}
        />
      ))}
      {dependencies.length > 0 && (
        <View style={styles.section}>
          <Text style={[styles.sectionTitle, { color: colors.gray }]}>
            依存タスク
          </Text>
          <View style={{ gap: 12 }}>
            {dependencies.map((task, index) => (
              <TaskCard
                key={asString(task.reference) ?? `dep-${index}`}
                task={task}
                colors={colors}
              />
            ))}
          </View>
        </View>
      )}
      {missingDependencies.length > 0 && (
        <View style={styles.section}>
          <Text style={[styles.sectionTitle, { color: colors.red }]}>
            未解決の依存
          </Text>
          {missingDependencies.map((ref, index) => (
            <DetailRow
              key={index}
              label={String(index + 1)}
              value={ref}
              colors={colors}
            />
          ))}
        </View>
      )}
    </View>
  );
}

interface HabitStepRowProps {
  step: Record<string, unknown>;
  colors: ColorSet;
}

function HabitStepRow({ step, colors }: HabitStepRowProps) {
  const position = asNumber(step.position);
  const title = asString(step.title);
  const start = asString(step.start_time);
  const end = asString(step.end_time);
  const avg = asNumber(step.avg_minutes);
  const label = `${position !== undefined ? `${position}. ` : ''}${title ?? ''}`;
  const time = start && end ? `${start} 〜 ${end}` : (start ?? end ?? null);
  const value = [time, avg !== undefined ? formatDuration(avg) : null]
    .filter(Boolean)
    .join(' / ');

  return <DetailRow label={label} value={value || '-'} colors={colors} />;
}

interface HabitCardProps {
  habit: Record<string, unknown>;
  colors: ColorSet;
}

function HabitCard({ habit, colors }: HabitCardProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const reference = asString(habit.reference);
  const title = asString(habit.title);
  const recurrence = asString(habit.recurrence);
  const active = asBoolean(habit.active);
  const fixed = asBoolean(habit.fixed);
  const description = asString(habit.description);
  const steps = asArray<Record<string, unknown>>(habit.steps, isRecord);

  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeHeader}>
        <View style={[styles.changeBadge, { backgroundColor: colors.gray }]}>
          <Text style={styles.changeBadgeText}>習慣</Text>
        </View>
        <Text
          style={[styles.changeTarget, { color: colors.black }]}
          numberOfLines={1}
        >
          {reference ? `[${reference}] ` : ''}
          {title ?? ''}
        </Text>
      </View>
      {recurrence ? (
        <DetailRow
          label="繰返"
          value={formatRecurrence(recurrence)}
          colors={colors}
        />
      ) : null}
      <View style={styles.detailRow}>
        {active === true ? (
          <Text style={{ fontSize: 12, color: colors.green }}>有効</Text>
        ) : active === false ? (
          <Text style={{ fontSize: 12, color: colors.gray }}>無効</Text>
        ) : null}
        {fixed === true ? (
          <Text style={{ fontSize: 12, color: colors.brand }}>固定</Text>
        ) : null}
      </View>
      {description ? (
        <Text
          style={{ fontSize: 13, color: colors.black, marginTop: 4 }}
          numberOfLines={2}
        >
          {description}
        </Text>
      ) : null}
      {steps && steps.length > 0 ? (
        <View
          style={[
            styles.whenBlock,
            { backgroundColor: colors.surface, borderColor: colors.separator },
          ]}
        >
          {steps.map((step, index) => {
            const position = asNumber(step.position);
            return (
              <HabitStepRow
                key={position !== undefined ? String(position) : index}
                step={step}
                colors={colors}
              />
            );
          })}
        </View>
      ) : null}
    </View>
  );
}

interface HabitResultViewProps {
  data: unknown;
  colors: ColorSet;
}

export function HabitResultView({ data, colors }: HabitResultViewProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const habits = useMemo<Record<string, unknown>[]>(() => {
    if (Array.isArray(data)) {
      return data.filter(isRecord);
    }
    if (isRecord(data)) {
      return asArray<Record<string, unknown>>(data.habits, isRecord) ?? [];
    }
    return [];
  }, [data]);

  if (habits.length === 0) {
    return (
      <Text style={[styles.emptyText, { color: colors.gray }]}>
        該当する習慣が見つかりませんでした。
      </Text>
    );
  }

  return (
    <View style={{ gap: 12 }}>
      {habits.map((habit, index) => (
        <HabitCard
          key={asString(habit.reference) ?? index}
          habit={habit}
          colors={colors}
        />
      ))}
    </View>
  );
}
