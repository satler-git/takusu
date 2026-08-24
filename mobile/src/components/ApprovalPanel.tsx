import { useEffect, useMemo, useState } from 'react';
import {
  ActivityIndicator,
  Pressable,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  View,
  useWindowDimensions,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import type {
  ApprovalRequest,
  ProposedChange,
  ProposalDecision,
} from '@/src/api/agentTypes';
import { AgentClient } from '@/src/api/agentClient';
import type { PermissionsMap } from '@/src/api/settingsStore';
import type { TaskStatus, WindowMode } from '@/src/api/types';
import type { ColorSet } from '@/src/theme';
import {
  getPermissionTitle,
  resolvePermission,
} from '@/src/components/PermissionsEditor';
import { HabitPreviewModal } from '@/src/components/HabitPreviewModal';
import { haptic } from '@/src/components/haptics';
import { formatDuration } from '@/src/utils/duration';

const WEEKDAYS = ['日', '月', '火', '水', '木', '金', '土'];

function formatInferredValue(value: unknown): string {
  if (value === null || value === undefined) {
    return '-';
  }
  if (typeof value === 'string') {
    return value;
  }
  return JSON.stringify(value);
}

function inferredFieldLabel(field: string): string {
  switch (field) {
    case 'title':
      return 'タイトル';
    case 'quantity_total':
      return '数量';
    case 'quantity_unit':
      return '単位';
    case 'avg_minutes':
      return '見積もり時間';
    case 'sigma_minutes':
      return '標準偏差';
    case 'end_at':
      return '期限';
    case 'start_at':
      return '開始時間';
    default:
      return field;
  }
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    panel: {
      margin: 12,
      borderWidth: 1,
      borderRadius: 12,
      padding: 12,
      gap: 12,
    },
    panelHeader: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'flex-start',
      gap: 12,
    },
    panelHeaderText: { flex: 1 },
    panelBodyContent: {
      flexGrow: 1,
    },
    panelBodyInner: { gap: 12 },
    title: { fontWeight: '700', fontSize: 16 },
    why: { fontSize: 13, lineHeight: 18, marginTop: 4 },
    summary: {
      fontSize: 12,
      borderRadius: 8,
      padding: 6,
      paddingHorizontal: 10,
    },
    changeList: { gap: 10 },
    changeCard: {
      borderWidth: 1,
      borderRadius: 10,
      padding: 10,
      gap: 8,
    },
    changeRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      flexWrap: 'wrap',
    },
    badge: {
      paddingHorizontal: 8,
      paddingVertical: 3,
      borderRadius: 12,
    },
    badgeText: {
      color: colors.onBrand,
      fontSize: 11,
      fontWeight: '700',
    },
    fixedBadge: {
      paddingHorizontal: 8,
      paddingVertical: 3,
      borderRadius: 12,
      marginStart: 'auto',
    },
    changeTarget: {
      fontWeight: '700',
      fontSize: 14,
      flexShrink: 1,
    },
    whenBlock: {
      borderWidth: 1,
      borderRadius: 8,
      padding: 8,
      paddingHorizontal: 10,
      gap: 4,
    },
    whenRow: {
      flexDirection: 'row',
      alignItems: 'baseline',
      gap: 8,
    },
    whenLabel: {
      minWidth: 56,
      fontSize: 13,
    },
    whenValue: { fontSize: 13, fontWeight: '600', flex: 1 },
    strikethrough: { textDecorationLine: 'line-through' },
    stepList: {
      borderWidth: 1,
      borderRadius: 8,
      padding: 8,
      paddingHorizontal: 10,
      gap: 6,
    },
    stepItem: {
      gap: 4,
    },
    stepMain: {
      flexDirection: 'row',
      alignItems: 'center',
      flexWrap: 'wrap',
      gap: 8,
    },
    stepDetails: {
      paddingStart: 28,
      gap: 2,
    },
    stepNumber: {
      width: 20,
      height: 20,
      borderRadius: 10,
      textAlign: 'center',
      fontSize: 11,
      fontWeight: '700',
      lineHeight: 20,
    },
    stepTitle: { fontSize: 13, fontWeight: '600', flex: 1 },
    stepFixedBadge: {
      paddingHorizontal: 5,
      paddingVertical: 1,
      borderRadius: 8,
    },
    stepFixedText: {
      color: colors.onBrand,
      fontSize: 10,
      fontWeight: '700',
    },
    stepMeta: { fontSize: 12 },
    stepDeps: { fontSize: 11 },
    changeDesc: { fontSize: 13 },
    previewButton: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 6,
      borderWidth: 1,
      borderRadius: 8,
      paddingVertical: 8,
      paddingHorizontal: 12,
      alignSelf: 'flex-start',
    },
    previewButtonText: { fontSize: 13, fontWeight: '600' },
    warningBox: {
      borderWidth: 1,
      borderRadius: 8,
      padding: 10,
      gap: 4,
    },
    inferredBox: {
      borderWidth: 1,
      borderRadius: 8,
      padding: 10,
      gap: 4,
    },
    inferredTitle: {
      fontSize: 12,
      fontWeight: '700',
    },
    inferredText: {
      fontSize: 12,
      lineHeight: 18,
    },
    actions: { flexDirection: 'row', gap: 12 },
    pager: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 12,
    },
    pagerButton: {
      width: 32,
      height: 32,
      borderRadius: 16,
      borderWidth: 1,
      alignItems: 'center',
      justifyContent: 'center',
    },
    pagerDots: { flexDirection: 'row', gap: 6 },
    dot: { width: 8, height: 8, borderRadius: 4 },
    deny: {
      flex: 1,
      padding: 12,
      borderRadius: 8,
      borderWidth: 1,
      alignItems: 'center',
    },
    denyText: { fontWeight: '700' },
    approve: {
      flex: 1,
      padding: 12,
      borderRadius: 8,
      alignItems: 'center',
    },
    approveText: { fontWeight: '700' },
    permissionSectionFolded: {
      backgroundColor: 'transparent',
      borderColor: 'transparent',
    },
    permissionSectionExpanded: {
      padding: 10,
      borderWidth: 1,
      borderRadius: 12,
      gap: 8,
    },
    permissionHeader: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: 8,
      paddingHorizontal: 10,
      borderWidth: 1,
      borderRadius: 10,
      minHeight: 34,
    },
    permissionHeaderMain: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      flex: 1,
    },
    permissionHeaderTitle: {
      fontWeight: '700',
      fontSize: 14,
    },
    permissionMaster: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: 8,
      paddingHorizontal: 10,
      borderWidth: 1,
      borderRadius: 10,
      minHeight: 40,
    },
    permissionMasterTitle: {
      fontWeight: '700',
      fontSize: 14,
    },
    permissionList: { gap: 6 },
    permissionRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      padding: 6,
      paddingHorizontal: 8,
      borderWidth: 1,
      borderRadius: 8,
      minHeight: 32,
    },
    permissionRowTitle: {
      fontWeight: '600',
      fontSize: 13,
      flex: 1,
    },
    permissionPersist: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      padding: 4,
    },
    permissionPersistText: {
      fontSize: 12,
    },
  });

export function asString(value: unknown): string | null {
  if (typeof value === 'string') return value;
  return null;
}

export function asNumber(value: unknown): number | undefined {
  if (typeof value === 'number') return value;
  return undefined;
}

export function asBoolean(value: unknown): boolean | undefined {
  if (typeof value === 'boolean') return value;
  return undefined;
}

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function asArray<T>(
  value: unknown,
  guard?: (x: unknown) => x is T,
): T[] | undefined {
  if (!Array.isArray(value)) return undefined;
  if (guard && !value.every(guard)) return undefined;
  return value as T[];
}

export function asStringArray(value: unknown): string[] {
  return asArray(value, isString) ?? [];
}

export function parseDateTime(iso: string): {
  date: string;
  time: string;
} | null {
  const m = iso.match(
    /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:[+-]\d{2}:\d{2}|Z)(?:\[[^\]]+\])?$/,
  );
  if (!m) return null;
  const [, y, mo, d, h, mi] = m;
  const date = new Date(Date.UTC(Number(y), Number(mo) - 1, Number(d)));
  const weekday = WEEKDAYS[date.getUTCDay()];
  return {
    date: `${Number(mo)}/${Number(d)} (${weekday})`,
    time: `${h}:${mi}`,
  };
}

export function formatInstant(iso: string): string {
  const parsed = parseDateTime(iso);
  if (parsed) return `${parsed.date} ${parsed.time}`;
  return iso;
}

export function formatDateTimeRange(start: string, end: string): string | null {
  const s = parseDateTime(start);
  const e = parseDateTime(end);
  if (!s || !e) return null;
  if (s.date === e.date) {
    return `${s.date} ${s.time} 〜 ${e.time}`;
  }
  return `${s.date} ${s.time} 〜 ${e.date} ${e.time}`;
}

function formatTimeRange(start: string, end: string): string {
  return `${start} 〜 ${end}`;
}

export function formatRecurrence(rrule: string): string {
  const freq = rrule.match(/FREQ=([^;]+)/i)?.[1]?.toUpperCase();
  const map: Record<string, string> = {
    DAILY: '毎日',
    WEEKLY: '毎週',
    MONTHLY: '毎月',
    YEARLY: '毎年',
  };
  return map[freq ?? ''] || rrule;
}

function getTargetType(change: ProposedChange): string {
  const label = change.target_label;
  const first = label.split(' ')[0];
  return first || 'task';
}

function getTargetName(change: ProposedChange): string {
  const parts = change.target_label.split(' ');
  if (parts.length <= 1) return '';
  return parts.slice(1).join(' ');
}

function getOperationBadge(operation: string): {
  label: string;
  color: 'success' | 'brand' | 'error' | 'muted';
} {
  switch (operation) {
    case 'create':
      return { label: '作成', color: 'success' };
    case 'update':
      return { label: '更新', color: 'brand' };
    case 'delete':
      return { label: '削除', color: 'error' };
    case 'generate':
      return { label: '生成', color: 'muted' };
    case 'reschedule':
      return { label: '再調整', color: 'muted' };
    case 'move':
      return { label: '移動', color: 'brand' };
    case 'start':
      return { label: '開始', color: 'brand' };
    case 'pause':
      return { label: '一時停止', color: 'muted' };
    case 'progress':
      return { label: '進捗', color: 'brand' };
    case 'complete':
      return { label: '完了', color: 'success' };
    case 'split':
      return { label: '分割', color: 'brand' };
    default:
      return { label: operation, color: 'muted' };
  }
}

function hexToRgba(hex: string, alpha: number): string {
  const sanitized = hex.replace('#', '');
  const full =
    sanitized.length === 3
      ? sanitized
          .split('')
          .map((c) => c + c)
          .join('')
      : sanitized;
  const r = parseInt(full.slice(0, 2), 16);
  const g = parseInt(full.slice(2, 4), 16);
  const b = parseInt(full.slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

function getPermissionKey(change: ProposedChange): string {
  return `${getTargetType(change)}:${change.operation}`;
}

interface DateTimeDiffProps {
  before: string;
  after: string;
  colors: ColorSet;
}

function DateTimeDiff({ before, after, colors }: DateTimeDiffProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const b = parseDateTime(before);
  const a = parseDateTime(after);
  if (b && a && b.date === a.date) {
    return (
      <Text style={{ color: colors.black, flex: 1 }}>
        {b.date}{' '}
        <Text style={[styles.strikethrough, { color: colors.gray }]}>
          {b.time}
        </Text>{' '}
        → {a.time}
      </Text>
    );
  }
  return (
    <Text style={{ color: colors.black, flex: 1 }}>
      <Text style={[styles.strikethrough, { color: colors.gray }]}>
        {formatInstant(before)}
      </Text>{' '}
      → {formatInstant(after)}
    </Text>
  );
}

interface WhenRowProps {
  label: string;
  before?: string;
  after?: string;
  value?: string;
  colors: ColorSet;
}

function WhenRow({ label, before, after, value, colors }: WhenRowProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  return (
    <View style={styles.whenRow}>
      <Text style={[styles.whenLabel, { color: colors.gray }]}>{label}</Text>
      {before !== undefined && after !== undefined ? (
        <DateTimeDiff before={before} after={after} colors={colors} />
      ) : (
        <Text style={[styles.whenValue, { color: colors.black }]}>
          {value ?? ''}
        </Text>
      )}
    </View>
  );
}

function parseDependsOn(value: unknown): (string | number)[] {
  if (Array.isArray(value)) return value as (string | number)[];
  if (typeof value === 'string') {
    try {
      const parsed = JSON.parse(value);
      if (Array.isArray(parsed)) return parsed as (string | number)[];
    } catch {
      // fall through
    }
  }
  return [];
}

function parseStringArray(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value.map((v) => asString(v)).filter((s): s is string => s !== null);
  }
  if (typeof value === 'string') {
    try {
      const parsed = JSON.parse(value);
      if (Array.isArray(parsed)) {
        return parsed
          .map((v: unknown) => asString(v))
          .filter((s: string | null): s is string => s !== null);
      }
    } catch {
      // fall through
    }
  }
  return [];
}

function diffStringArrays(
  before: string[],
  after: string[],
): { added: string[]; removed: string[] } {
  const added = after.filter((v) => !before.includes(v));
  const removed = before.filter((v) => !after.includes(v));
  return { added, removed };
}

function boolText(value: boolean, trueText: string, falseText: string): string {
  return value ? trueText : falseText;
}

function quantityText(
  done: number | undefined,
  total: number | undefined,
  unit: string | undefined,
): string {
  if (total !== undefined) {
    return `${done ?? 0}/${total}${unit ? ` ${unit}` : ''}`;
  }
  if (done !== undefined) {
    return `${done}${unit ? ` ${unit}` : ''}`;
  }
  if (unit) {
    return unit;
  }
  return '';
}

export const STATUS_LABELS: Record<TaskStatus, string> = {
  pending: '未スケジュール',
  scheduled: 'スケジュール済',
  in_progress: '進行中',
  completed: '完了',
  skipped: 'スキップ',
};

const WINDOW_MODE_LABELS: Record<WindowMode, string> = {
  day: '当日',
  period: '期間内どこでも',
};

interface DependsDiffRowProps {
  label: string;
  before: string[];
  after: string[];
  colors: ColorSet;
}

function DependsDiffRow({ label, before, after, colors }: DependsDiffRowProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const { added, removed } = diffStringArrays(before, after);
  if (added.length === 0 && removed.length === 0) return null;
  return (
    <View style={styles.whenRow}>
      <Text style={[styles.whenLabel, { color: colors.gray }]}>{label}</Text>
      <Text style={[styles.whenValue, { color: colors.black }]}>
        {added.length > 0 && `追加: ${added.join('、 ')}`}
        {added.length > 0 && removed.length > 0 && ' / '}
        {removed.length > 0 && (
          <Text style={[styles.strikethrough, { color: colors.gray }]}>
            削除: {removed.join('、 ')}
          </Text>
        )}
      </Text>
    </View>
  );
}

function pushTextDiffRow(
  rows: React.ReactNode[],
  before: Record<string, unknown>,
  after: Record<string, unknown>,
  key: string,
  label: string,
  colors: ColorSet,
  transform: (v: string) => string = (v) => v,
): void {
  if (!(key in after)) return;
  const afterRaw = asString(after[key]);
  const beforeRaw = asString(before[key]);
  const afterStr = transform(afterRaw ?? '');
  const beforeStr = beforeRaw !== null ? transform(beforeRaw) : undefined;
  if (afterStr === '' && (beforeStr === undefined || beforeStr === '')) return;
  if (beforeStr !== undefined && afterStr === beforeStr) return;
  if (beforeStr !== undefined) {
    rows.push(
      <WhenRow
        key={key}
        label={label}
        before={beforeStr}
        after={afterStr}
        colors={colors}
      />,
    );
  } else {
    rows.push(
      <WhenRow key={key} label={label} value={afterStr} colors={colors} />,
    );
  }
}

function pushBoolDiffRow(
  rows: React.ReactNode[],
  before: Record<string, unknown>,
  after: Record<string, unknown>,
  key: string,
  label: string,
  trueText: string,
  falseText: string,
  colors: ColorSet,
): void {
  if (!(key in after)) return;
  const afterVal = asBoolean(after[key]);
  if (afterVal === undefined) return;
  const beforeVal = asBoolean(before[key]);
  const afterStr = boolText(afterVal, trueText, falseText);
  const beforeStr =
    beforeVal !== undefined
      ? boolText(beforeVal, trueText, falseText)
      : undefined;
  if (beforeStr !== undefined && afterStr === beforeStr) return;
  if (beforeStr !== undefined) {
    rows.push(
      <WhenRow
        key={key}
        label={label}
        before={beforeStr}
        after={afterStr}
        colors={colors}
      />,
    );
  } else {
    rows.push(
      <WhenRow key={key} label={label} value={afterStr} colors={colors} />,
    );
  }
}

function pushNumberDiffRow(
  rows: React.ReactNode[],
  before: Record<string, unknown>,
  after: Record<string, unknown>,
  key: string,
  label: string,
  colors: ColorSet,
  format: (v: number) => string = (v) => String(v),
): void {
  if (!(key in after)) return;
  const afterVal = asNumber(after[key]);
  if (afterVal === undefined) return;
  const beforeVal = asNumber(before[key]);
  const afterStr = format(afterVal);
  const beforeStr = beforeVal !== undefined ? format(beforeVal) : undefined;
  if (beforeStr !== undefined && afterStr === beforeStr) return;
  if (beforeStr !== undefined) {
    rows.push(
      <WhenRow
        key={key}
        label={label}
        before={beforeStr}
        after={afterStr}
        colors={colors}
      />,
    );
  } else {
    rows.push(
      <WhenRow key={key} label={label} value={afterStr} colors={colors} />,
    );
  }
}

function pushTaskExtraRows(
  rows: React.ReactNode[],
  before: Record<string, unknown>,
  after: Record<string, unknown>,
  colors: ColorSet,
): void {
  if ('title' in before) {
    pushTextDiffRow(rows, before, after, 'title', 'タイトル', colors);
  }
  pushTextDiffRow(rows, before, after, 'description', '説明', colors);
  pushTextDiffRow(
    rows,
    before,
    after,
    'status',
    'ステータス',
    colors,
    (v) => STATUS_LABELS[v as TaskStatus] ?? v,
  );

  const beforeDeps = parseStringArray(before.depends);
  const afterDeps = parseStringArray(after.depends);
  const { added, removed } = diffStringArrays(beforeDeps, afterDeps);
  if (added.length > 0 || removed.length > 0) {
    rows.push(
      <DependsDiffRow
        key="depends"
        label="依存タスク"
        before={beforeDeps}
        after={afterDeps}
        colors={colors}
      />,
    );
  }

  pushBoolDiffRow(
    rows,
    before,
    after,
    'parallelizable',
    '並列実行可能',
    '可',
    '不可',
    colors,
  );
  pushBoolDiffRow(
    rows,
    before,
    after,
    'allows_parallel',
    '並列受け入れ',
    '可',
    '不可',
    colors,
  );
  pushBoolDiffRow(
    rows,
    before,
    after,
    'fixed',
    '時間固定',
    '固定',
    '解除',
    colors,
  );

  pushNumberDiffRow(
    rows,
    before,
    after,
    'abandonability',
    '諦めやすさ',
    colors,
    (v) => v.toFixed(2),
  );
  pushNumberDiffRow(
    rows,
    before,
    after,
    'sigma_minutes',
    '標準偏差',
    colors,
    (v) => `±${formatDuration(v)}`,
  );

  if (
    'quantity_total' in after ||
    'quantity_done' in after ||
    'quantity_unit' in after
  ) {
    const beforeDone = asNumber(before.quantity_done);
    const beforeTotal = asNumber(before.quantity_total);
    const beforeUnit = asString(before.quantity_unit) ?? undefined;
    const afterDone = asNumber(
      'quantity_done' in after ? after.quantity_done : before.quantity_done,
    );
    const afterTotal = asNumber(
      'quantity_total' in after ? after.quantity_total : before.quantity_total,
    );
    const afterUnit =
      asString(
        'quantity_unit' in after ? after.quantity_unit : before.quantity_unit,
      ) ?? undefined;
    const beforeStr = quantityText(beforeDone, beforeTotal, beforeUnit);
    const afterStr = quantityText(afterDone, afterTotal, afterUnit);
    if (beforeStr !== afterStr) {
      if (beforeStr) {
        rows.push(
          <WhenRow
            key="quantity"
            label="数量"
            before={beforeStr}
            after={afterStr}
            colors={colors}
          />,
        );
      } else {
        rows.push(
          <WhenRow
            key="quantity"
            label="数量"
            value={afterStr}
            colors={colors}
          />,
        );
      }
    }
  }
}

function pushHabitExtraRows(
  rows: React.ReactNode[],
  before: Record<string, unknown>,
  after: Record<string, unknown>,
  colors: ColorSet,
): void {
  if ('title' in before) {
    pushTextDiffRow(rows, before, after, 'title', 'タイトル', colors);
  }
  pushTextDiffRow(rows, before, after, 'description', '説明', colors);
  pushBoolDiffRow(
    rows,
    before,
    after,
    'parallelizable',
    '並列実行可能',
    '可',
    '不可',
    colors,
  );
  pushBoolDiffRow(
    rows,
    before,
    after,
    'allows_parallel',
    '並列受け入れ',
    '可',
    '不可',
    colors,
  );
  pushBoolDiffRow(
    rows,
    before,
    after,
    'fixed',
    '時間固定',
    '固定',
    '解除',
    colors,
  );
  pushBoolDiffRow(
    rows,
    before,
    after,
    'active',
    '有効',
    '有効',
    '無効',
    colors,
  );
  pushTextDiffRow(
    rows,
    before,
    after,
    'window_mode',
    'スケジュール枠',
    colors,
    (v) => WINDOW_MODE_LABELS[v as WindowMode] ?? v,
  );
  pushNumberDiffRow(
    rows,
    before,
    after,
    'abandonability',
    '諦めやすさ',
    colors,
    (v) => v.toFixed(2),
  );
  pushNumberDiffRow(
    rows,
    before,
    after,
    'sigma_minutes',
    '標準偏差',
    colors,
    (v) => `±${formatDuration(v)}`,
  );
}

function resolveStepRef(
  ref: string | number,
  steps: Record<string, unknown>[],
): { index: number; title: string } | null {
  let idx = -1;
  if (typeof ref === 'number') {
    // refs are 1-indexed display numbers.
    idx = ref - 1;
  } else {
    // Numeric strings are also 1-indexed display numbers.
    const num = Number(ref);
    if (!Number.isNaN(num) && String(num) === ref.trim()) {
      idx = num - 1;
    } else {
      idx = steps.findIndex((s) => s.id === ref || s.tempId === ref);
    }
  }
  const target = steps[idx];
  if (target) {
    return {
      index: idx,
      title: asString(target.title) ?? '',
    };
  }
  return null;
}

interface StepListProps {
  steps: unknown[];
  colors: ColorSet;
}

function StepList({ steps, colors }: StepListProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const stepRecords = steps.map((s) => (s ?? {}) as Record<string, unknown>);
  return (
    <View
      style={[
        styles.stepList,
        { backgroundColor: colors.surface, borderColor: colors.separator },
      ]}
    >
      {stepRecords.map((step, index) => {
        const title = asString(step.title) ?? '';
        const start = asString(step.start_time);
        const end = asString(step.end_time);
        const avg = asNumber(step.avg_minutes);
        const sigma = asNumber(step.sigma_minutes);
        const fixed = asBoolean(step.fixed) ?? false;
        const description = asString(step.description) ?? '';
        let time =
          start && end
            ? formatTimeRange(start, end)
            : avg !== undefined
              ? formatDuration(avg)
              : '';
        if (time.length > 0 && sigma !== undefined && sigma > 0) {
          time += ` · ±${formatDuration(sigma)}`;
        }
        const deps = parseDependsOn(step.depends_on);
        const depTexts = deps
          .map((ref) => resolveStepRef(ref, stepRecords))
          .filter((r): r is { index: number; title: string } => r !== null)
          .map((r) => `${r.index + 1}. ${r.title}`);

        return (
          <View key={index} style={styles.stepItem}>
            <View style={styles.stepMain}>
              <Text
                style={[
                  styles.stepNumber,
                  { backgroundColor: colors.surfaceTint, color: colors.black },
                ]}
              >
                {index + 1}
              </Text>
              <Text style={[styles.stepTitle, { color: colors.black }]}>
                {title}
              </Text>
              {fixed && (
                <View
                  style={[
                    styles.stepFixedBadge,
                    { backgroundColor: colors.red },
                  ]}
                >
                  <Text style={styles.stepFixedText}>固定</Text>
                </View>
              )}
            </View>
            <View style={styles.stepDetails}>
              {time.length > 0 && (
                <Text style={[styles.stepMeta, { color: colors.gray }]}>
                  {time}
                </Text>
              )}
              {description.length > 0 && (
                <Text style={[styles.stepMeta, { color: colors.gray }]}>
                  {description}
                </Text>
              )}
              {depTexts.length > 0 && (
                <Text style={[styles.stepDeps, { color: colors.gray }]}>
                  前提タスク: {depTexts.join('、 ')}
                </Text>
              )}
            </View>
          </View>
        );
      })}
    </View>
  );
}

interface TaskChangeRowsProps {
  after: Record<string, unknown>;
  before: Record<string, unknown>;
  colors: ColorSet;
  isFixed: boolean;
  isUpdate: boolean;
}

function TaskChangeRows({
  after,
  before,
  colors,
  isFixed,
  isUpdate,
}: TaskChangeRowsProps): React.ReactNode[] {
  const rows: React.ReactNode[] = [];

  if (!isUpdate) {
    const start = asString(after.start_at ?? before.start_at);
    const end = asString(after.end_at ?? before.end_at);
    const avg = asNumber(after.avg_minutes ?? before.avg_minutes);

    if (isFixed && start && end) {
      const range = formatDateTimeRange(start, end);
      if (range) {
        rows.push(
          <WhenRow
            key="range"
            label="固定予定"
            value={range}
            colors={colors}
          />,
        );
      }
    } else if (end) {
      rows.push(
        <WhenRow
          key="end"
          label="期限"
          value={formatInstant(end)}
          colors={colors}
        />,
      );
      if (avg !== undefined) {
        rows.push(
          <WhenRow
            key="avg"
            label="所要"
            value={formatDuration(avg)}
            colors={colors}
          />,
        );
      }
    } else if (start) {
      rows.push(
        <WhenRow
          key="start"
          label="開始"
          value={formatInstant(start)}
          colors={colors}
        />,
      );
    }

    pushTaskExtraRows(rows, before, after, colors);
  } else {
    const afterEnd = asString(after.end_at);
    const beforeEnd = asString(before.end_at);
    const afterStart = asString(after.start_at);
    const beforeStart = asString(before.start_at);
    const afterAvg = asNumber(after.avg_minutes);
    const beforeAvg = asNumber(before.avg_minutes);

    const startChanged =
      afterStart !== null && beforeStart !== null && afterStart !== beforeStart;
    const endChanged =
      afterEnd !== null && beforeEnd !== null && afterEnd !== beforeEnd;
    const startAdded = afterStart !== null && beforeStart === null;
    const endAdded = afterEnd !== null && beforeEnd === null;

    if (
      isFixed &&
      startChanged &&
      endChanged &&
      afterStart &&
      afterEnd &&
      beforeStart &&
      beforeEnd
    ) {
      const beforeRange = formatDateTimeRange(beforeStart, beforeEnd);
      const afterRange = formatDateTimeRange(afterStart, afterEnd);
      if (beforeRange && afterRange) {
        rows.push(
          <WhenRow
            key="range-diff"
            label="固定予定"
            before={beforeRange}
            after={afterRange}
            colors={colors}
          />,
        );
      }
    } else {
      if (startChanged) {
        rows.push(
          <WhenRow
            key="start-diff"
            label="開始"
            before={beforeStart}
            after={afterStart}
            colors={colors}
          />,
        );
      }
      if (endChanged) {
        rows.push(
          <WhenRow
            key="end-diff"
            label="期限"
            before={beforeEnd}
            after={afterEnd}
            colors={colors}
          />,
        );
      }
    }

    if (startAdded && afterStart) {
      const end = afterEnd ?? beforeEnd;
      if (end) {
        const range = formatDateTimeRange(afterStart, end);
        if (range) {
          rows.push(
            <WhenRow
              key="range"
              label="固定予定"
              value={range}
              colors={colors}
            />,
          );
        }
      } else {
        rows.push(
          <WhenRow
            key="start"
            label="開始"
            value={formatInstant(afterStart)}
            colors={colors}
          />,
        );
      }
    } else if (endAdded && afterEnd) {
      rows.push(
        <WhenRow
          key="end"
          label="期限"
          value={formatInstant(afterEnd)}
          colors={colors}
        />,
      );
    }

    if (
      afterAvg !== undefined &&
      beforeAvg !== undefined &&
      afterAvg !== beforeAvg
    ) {
      rows.push(
        <WhenRow
          key="avg-diff"
          label="所要"
          before={formatDuration(beforeAvg)}
          after={formatDuration(afterAvg)}
          colors={colors}
        />,
      );
    } else if (afterAvg !== undefined && beforeAvg === undefined) {
      rows.push(
        <WhenRow
          key="avg"
          label="所要"
          value={formatDuration(afterAvg)}
          colors={colors}
        />,
      );
    }

    pushTaskExtraRows(rows, before, after, colors);
  }

  return rows;
}

interface MoveChangeRowsProps {
  after: Record<string, unknown>;
  before: Record<string, unknown>;
  colors: ColorSet;
}

function MoveChangeRows({
  after,
  before,
  colors,
}: MoveChangeRowsProps): React.ReactNode[] {
  const rows: React.ReactNode[] = [];
  const afterStart = asString(after.start_at);
  const afterEnd = asString(after.end_at);
  const beforeStart = asString(before.schedule_start_at);
  const beforeEnd = asString(before.schedule_end_at);

  if (afterStart) {
    const beforeValue =
      beforeStart && beforeEnd
        ? (formatDateTimeRange(beforeStart, beforeEnd) ?? undefined)
        : beforeStart
          ? formatInstant(beforeStart)
          : undefined;
    const afterValue = afterEnd
      ? (formatDateTimeRange(afterStart, afterEnd) ?? formatInstant(afterStart))
      : formatInstant(afterStart);

    if (beforeValue) {
      rows.push(
        <WhenRow
          key="move-range"
          label="予定"
          before={beforeValue}
          after={afterValue}
          colors={colors}
        />,
      );
    } else {
      rows.push(
        <WhenRow
          key="move-after"
          label="移動先"
          value={afterValue}
          colors={colors}
        />,
      );
    }
  }

  return rows;
}

interface HabitChangeRowsProps {
  after: Record<string, unknown>;
  before: Record<string, unknown>;
  colors: ColorSet;
  isUpdate: boolean;
}

function HabitChangeRows({
  after,
  before,
  colors,
  isUpdate,
}: HabitChangeRowsProps): React.ReactNode[] {
  const rows: React.ReactNode[] = [];

  if (!isUpdate) {
    const startTime = asString(after.start_time ?? before.start_time);
    const endTime = asString(after.end_time ?? before.end_time);
    const recurrence = asString(after.recurrence ?? before.recurrence);
    const avg = asNumber(after.avg_minutes ?? before.avg_minutes);

    if (startTime && endTime) {
      rows.push(
        <WhenRow
          key="range"
          label="時間帯"
          value={formatTimeRange(startTime, endTime)}
          colors={colors}
        />,
      );
    }
    if (recurrence) {
      rows.push(
        <WhenRow
          key="recurrence"
          label="繰り返し"
          value={formatRecurrence(recurrence)}
          colors={colors}
        />,
      );
    }
    if (avg !== undefined) {
      rows.push(
        <WhenRow
          key="avg"
          label="所要"
          value={formatDuration(avg)}
          colors={colors}
        />,
      );
    }

    pushHabitExtraRows(rows, before, after, colors);
  } else {
    const afterStart = asString(after.start_time);
    const beforeStart = asString(before.start_time);
    const afterEnd = asString(after.end_time);
    const beforeEnd = asString(before.end_time);
    const afterRecurrence = asString(after.recurrence);
    const beforeRecurrence = asString(before.recurrence);
    const afterAvg = asNumber(after.avg_minutes);
    const beforeAvg = asNumber(before.avg_minutes);

    const startChanged =
      afterStart !== null && beforeStart !== null && afterStart !== beforeStart;
    const endChanged =
      afterEnd !== null && beforeEnd !== null && afterEnd !== beforeEnd;
    const startAdded = afterStart !== null && beforeStart === null;
    const endAdded = afterEnd !== null && beforeEnd === null;

    if (
      startChanged &&
      endChanged &&
      afterStart &&
      afterEnd &&
      beforeStart &&
      beforeEnd
    ) {
      rows.push(
        <WhenRow
          key="range-diff"
          label="時間帯"
          before={formatTimeRange(beforeStart, beforeEnd)}
          after={formatTimeRange(afterStart, afterEnd)}
          colors={colors}
        />,
      );
    } else {
      if (startChanged) {
        rows.push(
          <WhenRow
            key="start-diff"
            label="開始"
            before={beforeStart}
            after={afterStart}
            colors={colors}
          />,
        );
      }
      if (endChanged) {
        rows.push(
          <WhenRow
            key="end-diff"
            label="終了"
            before={beforeEnd}
            after={afterEnd}
            colors={colors}
          />,
        );
      }
    }

    if (startAdded && afterStart) {
      const end = afterEnd ?? beforeEnd;
      if (end) {
        rows.push(
          <WhenRow
            key="range"
            label="時間帯"
            value={formatTimeRange(afterStart, end)}
            colors={colors}
          />,
        );
      } else {
        rows.push(
          <WhenRow
            key="start"
            label="開始"
            value={afterStart}
            colors={colors}
          />,
        );
      }
    } else if (endAdded && afterEnd) {
      rows.push(
        <WhenRow key="end" label="終了" value={afterEnd} colors={colors} />,
      );
    }

    if (
      afterRecurrence &&
      beforeRecurrence &&
      afterRecurrence !== beforeRecurrence
    ) {
      rows.push(
        <WhenRow
          key="recurrence-diff"
          label="繰り返し"
          before={formatRecurrence(beforeRecurrence)}
          after={formatRecurrence(afterRecurrence)}
          colors={colors}
        />,
      );
    } else if (afterRecurrence && !beforeRecurrence) {
      rows.push(
        <WhenRow
          key="recurrence"
          label="繰り返し"
          value={formatRecurrence(afterRecurrence)}
          colors={colors}
        />,
      );
    }

    if (
      afterAvg !== undefined &&
      beforeAvg !== undefined &&
      afterAvg !== beforeAvg
    ) {
      rows.push(
        <WhenRow
          key="avg-diff"
          label="所要"
          before={formatDuration(beforeAvg)}
          after={formatDuration(afterAvg)}
          colors={colors}
        />,
      );
    } else if (afterAvg !== undefined && beforeAvg === undefined) {
      rows.push(
        <WhenRow
          key="avg"
          label="所要"
          value={formatDuration(afterAvg)}
          colors={colors}
        />,
      );
    }

    pushHabitExtraRows(rows, before, after, colors);
  }

  return rows;
}

interface ScheduleChangeRowsProps {
  after: Record<string, unknown>;
  before: Record<string, unknown>;
  colors: ColorSet;
}

function ScheduleChangeRows({
  after,
  before,
  colors,
}: ScheduleChangeRowsProps): React.ReactNode[] {
  const rows: React.ReactNode[] = [];
  const from = asString(after.from ?? before.from);
  const until = asString(after.until ?? before.until);
  const taskIds = asArray<string>(after.task_ids ?? before.task_ids);
  if (from && until) {
    const range = formatDateTimeRange(from, until);
    if (range) {
      rows.push(
        <WhenRow key="range" label="範囲" value={range} colors={colors} />,
      );
    }
  }
  if (taskIds && taskIds.length > 0) {
    rows.push(
      <WhenRow
        key="tasks"
        label="対象"
        value={`${taskIds.length} 件`}
        colors={colors}
      />,
    );
  }
  return rows;
}

interface ChangeCardProps {
  change: ProposedChange;
  client?: AgentClient;
  colors: ColorSet;
}

function ChangeCard({ change, client, colors }: ChangeCardProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const [previewVisible, setPreviewVisible] = useState(false);
  const targetType = getTargetType(change);
  const targetName = getTargetName(change);
  const op = getOperationBadge(change.operation);

  const after = (change.after ?? {}) as Record<string, unknown>;
  const before = (change.before ?? {}) as Record<string, unknown>;

  const previewHabitData = useMemo<Record<string, unknown>>(() => {
    const mergedAfter = (change.after ?? {}) as Record<string, unknown>;
    const mergedBefore = (change.before ?? {}) as Record<string, unknown>;
    if (change.operation === 'update') {
      return { ...mergedBefore, ...mergedAfter };
    }
    return mergedAfter;
  }, [change.after, change.before, change.operation]);

  const stepsArray = asArray<Record<string, unknown>>(
    after.steps ?? before.steps,
    isRecord,
  );

  const isFixed = asBoolean(after.fixed) ?? asBoolean(before.fixed) ?? false;
  const isUpdate = change.operation === 'update';
  const isHabit = targetType === 'habit' && change.operation !== 'delete';

  const rows: React.ReactNode[] =
    change.operation === 'move'
      ? MoveChangeRows({ after, before, colors })
      : targetType === 'task'
        ? TaskChangeRows({ after, before, colors, isFixed, isUpdate })
        : targetType === 'habit'
          ? HabitChangeRows({ after, before, colors, isUpdate })
          : targetType === 'schedule'
            ? ScheduleChangeRows({ after, before, colors })
            : [];

  const badgeColor =
    op.color === 'success'
      ? colors.green
      : op.color === 'brand'
        ? colors.brand
        : op.color === 'error'
          ? colors.red
          : colors.gray;

  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeRow}>
        <View style={[styles.badge, { backgroundColor: badgeColor }]}>
          <Text style={styles.badgeText}>{op.label}</Text>
        </View>
        <Text style={[styles.changeTarget, { color: colors.black }]}>
          {targetName}
        </Text>
        {isFixed && (
          <View style={[styles.fixedBadge, { backgroundColor: colors.red }]}>
            <Text style={styles.badgeText}>固定</Text>
          </View>
        )}
      </View>
      {rows.length > 0 && (
        <View
          style={[
            styles.whenBlock,
            { backgroundColor: colors.surface, borderColor: colors.separator },
          ]}
        >
          {rows}
        </View>
      )}
      {stepsArray && stepsArray.length > 0 && (
        <StepList steps={stepsArray} colors={colors} />
      )}
      {isHabit && (
        <>
          <Pressable
            onPress={() => {
              haptic.light();
              setPreviewVisible(true);
            }}
            style={[styles.previewButton, { borderColor: colors.brand }]}
          >
            <Ionicons name="eye-outline" size={16} color={colors.brand} />
            <Text style={[styles.previewButtonText, { color: colors.brand }]}>
              プレビューを表示
            </Text>
          </Pressable>
          <HabitPreviewModal
            visible={previewVisible}
            onClose={() => setPreviewVisible(false)}
            client={client}
            habit={previewHabitData}
            title={targetName}
          />
        </>
      )}
      {change.description.length > 0 && (
        <Text style={[styles.changeDesc, { color: colors.gray }]}>
          {change.description}
        </Text>
      )}
    </View>
  );
}

interface PermissionSectionValue {
  granted: PermissionsMap;
  persist: boolean;
}

interface PermissionSectionProps {
  approvalId: string;
  changes: ProposedChange[];
  colors: ColorSet;
  onChange: (value: PermissionSectionValue) => void;
  permissions?: PermissionsMap;
  value: PermissionSectionValue;
}

interface VisiblePermission {
  key: string;
  title: string;
  danger: boolean;
}

function PermissionSection({
  approvalId,
  changes,
  colors,
  onChange,
  permissions,
  value,
}: PermissionSectionProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    setExpanded(false);
  }, [approvalId]);

  const visiblePermissions = useMemo(() => {
    const seen = new Set<string>();
    const result: VisiblePermission[] = [];
    for (const change of changes) {
      const key = getPermissionKey(change);
      if (seen.has(key)) continue;
      seen.add(key);
      if (resolvePermission(key, permissions)) continue;
      result.push({
        key,
        title: getPermissionTitle(key),
        danger: change.operation === 'delete',
      });
    }
    return result;
  }, [changes, permissions]);

  useEffect(() => {
    const visibleKeys = new Set(visiblePermissions.map((p) => p.key));
    let changed = false;
    const filtered: PermissionsMap = {};
    for (const [key, val] of Object.entries(value.granted)) {
      if (visibleKeys.has(key)) {
        filtered[key] = val;
      } else {
        changed = true;
      }
    }
    if (changed) {
      onChange({ ...value, granted: filtered });
    }
  }, [visiblePermissions, value, onChange]);

  if (visiblePermissions.length === 0) return null;

  const enabledCount = visiblePermissions.filter(
    (p) => value.granted[p.key],
  ).length;
  const allOn = enabledCount === visiblePermissions.length;

  function toggleAll() {
    const next: PermissionsMap = { ...value.granted };
    for (const p of visiblePermissions) {
      next[p.key] = !allOn;
    }
    onChange({ ...value, granted: next });
  }

  function toggleOne(key: string) {
    onChange({
      ...value,
      granted: { ...value.granted, [key]: !value.granted[key] },
    });
  }

  function togglePersist() {
    onChange({ ...value, persist: !value.persist });
  }

  const header = (
    <Pressable
      onPress={() => setExpanded(!expanded)}
      style={[
        styles.permissionHeader,
        { backgroundColor: colors.surface, borderColor: colors.separator },
      ]}
    >
      <View style={styles.permissionHeaderMain}>
        <Ionicons
          name={expanded ? 'chevron-down' : 'chevron-forward'}
          size={16}
          color={colors.brand}
        />
        <Text style={[styles.permissionHeaderTitle, { color: colors.black }]}>
          権限
        </Text>
        <Text style={{ color: colors.gray }}>
          {enabledCount}/{visiblePermissions.length}
        </Text>
      </View>
    </Pressable>
  );

  if (!expanded) {
    return <View style={styles.permissionSectionFolded}>{header}</View>;
  }

  return (
    <View
      style={[
        styles.permissionSectionExpanded,
        { backgroundColor: colors.surface, borderColor: colors.separator },
      ]}
    >
      {header}
      <Pressable
        onPress={toggleAll}
        style={[
          styles.permissionMaster,
          {
            backgroundColor: allOn
              ? hexToRgba(colors.brand, 0.12)
              : colors.surfaceTint,
            borderColor: allOn ? colors.brand : colors.separator,
          },
        ]}
      >
        <Text style={[styles.permissionMasterTitle, { color: colors.black }]}>
          すべて許可
        </Text>
        <Switch
          value={allOn}
          onValueChange={toggleAll}
          accessibilityLabel="すべて許可"
          trackColor={{ false: colors.grayLight, true: colors.brand }}
        />
      </Pressable>
      <View style={styles.permissionList}>
        {visiblePermissions.map((p) => {
          const on = !!value.granted[p.key];
          return (
            <Pressable
              key={p.key}
              onPress={() => toggleOne(p.key)}
              style={[
                styles.permissionRow,
                {
                  backgroundColor: on
                    ? p.danger
                      ? hexToRgba(colors.red, 0.12)
                      : hexToRgba(colors.brand, 0.12)
                    : p.danger
                      ? hexToRgba(colors.red, 0.05)
                      : colors.surfaceTint,
                  borderColor: p.danger
                    ? colors.red
                    : on
                      ? colors.brand
                      : colors.separator,
                },
              ]}
            >
              <Text
                style={[styles.permissionRowTitle, { color: colors.black }]}
              >
                {p.title}
              </Text>
              <Switch
                value={on}
                onValueChange={() => toggleOne(p.key)}
                accessibilityLabel={p.title}
                trackColor={{
                  false: colors.grayLight,
                  true: p.danger ? colors.red : colors.brand,
                }}
              />
            </Pressable>
          );
        })}
      </View>
      <Pressable onPress={togglePersist} style={styles.permissionPersist}>
        <Switch
          value={value.persist}
          onValueChange={togglePersist}
          accessibilityLabel="永続"
          trackColor={{ false: colors.grayLight, true: colors.brand }}
        />
        <Text style={[styles.permissionPersistText, { color: colors.gray }]}>
          永続（Provider 設定にも保存）
        </Text>
      </Pressable>
    </View>
  );
}

interface ApprovalPanelProps {
  approval: ApprovalRequest;
  busy: boolean;
  client?: AgentClient;
  colors: ColorSet;
  onResolve: (
    decisions: ProposalDecision[],
    grantedPermissions: PermissionsMap,
    persistToProvider: boolean,
  ) => void;
  permissions?: PermissionsMap;
}

interface ProposalGroup {
  proposal_id: string;
  changes: ProposedChange[];
}

export function ApprovalPanel({
  approval,
  busy,
  client,
  colors,
  onResolve,
  permissions,
}: ApprovalPanelProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const { height: windowHeight } = useWindowDimensions();
  const [permissionValue, setPermissionValue] =
    useState<PermissionSectionValue>({
      granted: {},
      persist: false,
    });
  const [currentIndex, setCurrentIndex] = useState(0);
  const [decisions, setDecisions] = useState<Record<string, boolean>>({});

  const groups: ProposalGroup[] = useMemo(() => {
    const map = new Map<string, ProposedChange[]>();
    for (const change of approval.changes) {
      const id = change.proposal_id;
      if (!map.has(id)) map.set(id, []);
      map.get(id)!.push(change);
    }
    return Array.from(map.entries()).map(([proposal_id, changes]) => ({
      proposal_id,
      changes,
    }));
  }, [approval]);

  const total = groups.length;
  const currentGroup = groups[currentIndex];

  useEffect(() => {
    setPermissionValue({ granted: {}, persist: false });
    setCurrentIndex(0);
    setDecisions({});
  }, [approval.id]);

  function allDecided(finalDecisions: Record<string, boolean>) {
    return groups.every((group) => group.proposal_id in finalDecisions);
  }

  function firstUndecidedIndex(finalDecisions: Record<string, boolean>) {
    return groups.findIndex((group) => !(group.proposal_id in finalDecisions));
  }

  function submit(finalDecisions: Record<string, boolean>) {
    if (!allDecided(finalDecisions)) {
      const next = firstUndecidedIndex(finalDecisions);
      if (next >= 0) setCurrentIndex(next);
      return;
    }
    const proposalDecisions: ProposalDecision[] = groups.map((group) => ({
      proposal_id: group.proposal_id,
      approve: finalDecisions[group.proposal_id],
    }));
    onResolve(
      proposalDecisions,
      permissionValue.granted,
      permissionValue.persist,
    );
  }

  function handleDecision(approve: boolean) {
    if (busy || !currentGroup) return;
    haptic.light();
    const nextDecisions = {
      ...decisions,
      [currentGroup.proposal_id]: approve,
    };
    setDecisions(nextDecisions);
    if (allDecided(nextDecisions)) {
      submit(nextDecisions);
    } else {
      const next = firstUndecidedIndex(nextDecisions);
      if (next >= 0) setCurrentIndex(next);
    }
  }

  function goTo(index: number) {
    if (index >= 0 && index < total) {
      setCurrentIndex(index);
    }
  }

  return (
    <View
      style={[
        styles.panel,
        { backgroundColor: colors.surface, borderColor: colors.separator },
      ]}
    >
      <View style={styles.panelHeader}>
        <View style={styles.panelHeaderText}>
          <Text style={[styles.title, { color: colors.black }]}>
            以下の変更を承認しますか？
          </Text>
          {approval.why.length > 0 && (
            <Text style={[styles.why, { color: colors.gray }]}>
              {approval.why}
            </Text>
          )}
        </View>

        <Text
          style={[
            styles.summary,
            { color: colors.gray, backgroundColor: colors.surfaceTint },
          ]}
        >
          {currentIndex + 1} / {total}
        </Text>
      </View>

      <ScrollView
        style={{ maxHeight: windowHeight * 0.6 }}
        contentContainerStyle={styles.panelBodyContent}
        keyboardShouldPersistTaps="handled"
      >
        <View style={styles.panelBodyInner}>
          <View style={styles.changeList}>
            {currentGroup?.changes.map((change, i) => (
              <ChangeCard
                key={`${change.proposal_id}-${i}`}
                change={change}
                client={client}
                colors={colors}
              />
            ))}
          </View>

          <PermissionSection
            approvalId={approval.id}
            changes={approval.changes}
            colors={colors}
            onChange={setPermissionValue}
            permissions={permissions}
            value={permissionValue}
          />

          {approval.warnings.length > 0 && (
            <View style={[styles.warningBox, { borderColor: colors.warning }]}>
              {approval.warnings.map((warning) => (
                <Text key={warning} style={{ color: colors.warning }}>
                  注意: {warning}
                </Text>
              ))}
            </View>
          )}

          {approval.inferred_fields.length > 0 && (
            <View
              style={[
                styles.inferredBox,
                { borderColor: colors.gray, backgroundColor: colors.surfaceTint },
              ]}
            >
              <Text style={[styles.inferredTitle, { color: colors.gray }]}>
                推定した項目
              </Text>
              {approval.inferred_fields.map((field, index) => (
                <Text
                  key={`inferred-${index}`}
                  style={[styles.inferredText, { color: colors.gray }]}
                >
                  {inferredFieldLabel(field.field)}: {formatInferredValue(field.value)} —{' '}
                  {field.reason}
                </Text>
              ))}
            </View>
          )}
        </View>
      </ScrollView>

      <View style={styles.actions}>
        <Pressable
          disabled={busy}
          onPress={() => handleDecision(false)}
          style={[styles.deny, { borderColor: colors.red }]}
        >
          <Text style={[styles.denyText, { color: colors.red }]}>拒否</Text>
        </Pressable>
        <Pressable
          disabled={busy}
          onPress={() => handleDecision(true)}
          style={[styles.approve, { backgroundColor: colors.brand }]}
        >
          {busy ? (
            <ActivityIndicator color={colors.white} />
          ) : (
            <Text style={[styles.approveText, { color: colors.white }]}>
              承認
            </Text>
          )}
        </Pressable>
      </View>

      <View style={styles.pager}>
        <Pressable
          onPress={() => goTo(currentIndex - 1)}
          disabled={currentIndex === 0 || busy}
          style={[
            styles.pagerButton,
            {
              borderColor: colors.separator,
              backgroundColor: colors.surfaceTint,
            },
          ]}
        >
          <Text style={{ color: colors.black, fontSize: 18 }}>&#8249;</Text>
        </Pressable>
        <View style={styles.pagerDots}>
          {groups.map((group, i) => {
            const decided = decisions[group.proposal_id];
            let dotColor = colors.separator;
            if (i === currentIndex) dotColor = colors.brand;
            else if (decided === true) dotColor = colors.green;
            else if (decided === false) dotColor = colors.red;
            return (
              <Pressable
                key={group.proposal_id}
                onPress={() => goTo(i)}
                style={[styles.dot, { backgroundColor: dotColor }]}
              />
            );
          })}
        </View>
        <Pressable
          onPress={() => goTo(currentIndex + 1)}
          disabled={currentIndex === total - 1 || busy}
          style={[
            styles.pagerButton,
            {
              borderColor: colors.separator,
              backgroundColor: colors.surfaceTint,
            },
          ]}
        >
          <Text style={{ color: colors.black, fontSize: 18 }}>&#8250;</Text>
        </Pressable>
      </View>
    </View>
  );
}
