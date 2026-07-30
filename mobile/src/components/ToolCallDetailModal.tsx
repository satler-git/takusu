import { useCallback, useMemo, type ReactNode } from 'react';
import { Modal, Pressable, ScrollView, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { Ionicons } from '@expo/vector-icons';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { formatJson } from '@/src/utils/formatJson';
import type { ToolCallItem } from '@/src/api/agentSessionStore';
import Markdown, {
  type ASTNode,
  type RenderRules,
} from 'react-native-markdown-renderer';
import {
  asString,
  asNumber,
  asBoolean,
  asArray,
  parseDateTime,
  formatInstant,
  isRecord,
} from '@/src/components/ApprovalPanel';
import { formatDuration } from '@/src/utils/duration';
import { makeStyles, DetailRow } from './ToolCallDetailCommon';
import {
  TaskResultView,
  HabitResultView,
} from '@/src/components/ToolResultViews';

const MAX_DEPTH = 3;

function getTime(iso: string): number | null {
  const m = iso.match(
    /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:[+-]\d{2}:\d{2}|Z)(?:\[[^\]]+\])?$/,
  );
  if (!m) return null;
  const [, y, mo, d, h, mi, s] = m;
  return Date.UTC(
    Number(y),
    Number(mo) - 1,
    Number(d),
    Number(h),
    Number(mi),
    Number(s),
  );
}

interface ValueViewProps {
  value: unknown;
  colors: ColorSet;
  depth?: number;
}

function ValueView({ value, colors, depth = 0 }: ValueViewProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  if (value === undefined || value === null) {
    return <Text style={[styles.valueText, { color: colors.gray }]}>-</Text>;
  }
  if (
    typeof value === 'string' ||
    typeof value === 'number' ||
    typeof value === 'boolean'
  ) {
    return (
      <Text style={[styles.valueText, { color: colors.black }]}>
        {String(value)}
      </Text>
    );
  }
  if (depth < MAX_DEPTH) {
    if (isRecord(value)) {
      return <ObjectRows object={value} colors={colors} depth={depth} />;
    }
    if (Array.isArray(value)) {
      return <ArrayRows array={value} colors={colors} depth={depth} />;
    }
  }
  return (
    <Text style={[styles.monoText, { color: colors.black }]}>
      {formatJson(value)}
    </Text>
  );
}

interface ValueRowProps {
  label: string;
  value: ReactNode;
  colors: ColorSet;
}

function ValueRow({ label, value, colors }: ValueRowProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  return (
    <View style={styles.row}>
      <Text style={[styles.rowLabel, { color: colors.gray }]}>{label}</Text>
      <View style={styles.rowValue}>{value}</View>
    </View>
  );
}

interface ObjectRowsProps {
  object: Record<string, unknown>;
  colors: ColorSet;
  depth?: number;
}

function ObjectRows({ object, colors, depth = 0 }: ObjectRowsProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const isNested = depth > 0;
  return (
    <View
      style={[
        styles.sectionBox,
        isNested && styles.nestedSectionBox,
        {
          backgroundColor: colors.surface,
          borderColor: colors.separator,
          marginLeft: isNested ? 12 : 0,
        },
      ]}
    >
      {Object.entries(object).map(([key, value]) => (
        <ValueRow
          key={key}
          label={key}
          value={<ValueView value={value} colors={colors} depth={depth + 1} />}
          colors={colors}
        />
      ))}
    </View>
  );
}

interface ArrayRowsProps {
  array: unknown[];
  colors: ColorSet;
  depth?: number;
}

function ArrayRows({ array, colors, depth = 0 }: ArrayRowsProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const isNested = depth > 0;
  return (
    <View
      style={[
        styles.sectionBox,
        isNested && styles.nestedSectionBox,
        {
          backgroundColor: colors.surface,
          borderColor: colors.separator,
          marginLeft: isNested ? 12 : 0,
        },
      ]}
    >
      {array.map((item, index) => (
        <ValueRow
          key={index}
          label={String(index)}
          value={<ValueView value={item} colors={colors} depth={depth + 1} />}
          colors={colors}
        />
      ))}
    </View>
  );
}

interface AsrArgumentListProps {
  questions: unknown[];
  colors: ColorSet;
}

function AsrArgumentList({ questions, colors }: AsrArgumentListProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const records = questions.filter(isRecord);
  return (
    <View
      style={[
        styles.sectionBox,
        {
          backgroundColor: colors.surface,
          borderColor: colors.separator,
        },
      ]}
    >
      {records.map((item, index) => (
        <View key={index} style={styles.asrItem}>
          <Text style={[styles.asrOriginal, { color: colors.black }]}>
            {String(item.text ?? '')}
          </Text>
          <Text style={[styles.asrPurpose, { color: colors.gray }]}>
            目的: {String(item.for ?? '')}
          </Text>
        </View>
      ))}
    </View>
  );
}

interface ToolCallDetailModalProps {
  visible: boolean;
  call: ToolCallItem | null;
  onClose: () => void;
}

export function ToolCallDetailModal({
  visible,
  call,
  onClose,
}: ToolCallDetailModalProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);

  const { title, status, statusColor, isAsr, args } = useMemo<{
    title: string;
    status: string;
    statusColor: string;
    isAsr: boolean;
    args: unknown;
  }>(() => {
    if (!call) {
      return {
        title: '',
        status: '',
        statusColor: colors.gray,
        isAsr: false,
        args: undefined,
      };
    }
    const asr = call.name === 'correct_asr';
    const done = call.result !== undefined;
    const rejected = call.isRejected ?? false;
    return {
      title: asr ? 'ASR訂正' : call.name,
      status: rejected
        ? '拒否'
        : done
          ? call.isError
            ? 'エラー'
            : '成功'
          : '実行中',
      statusColor: rejected
        ? colors.red
        : done
          ? call.isError
            ? colors.red
            : colors.green
          : colors.gray,
      isAsr: asr,
      args: call.arguments,
    };
  }, [call, colors]);

  const handleCopy = useCallback(async () => {
    if (!call) return;
    const text = formatJson({
      name: call.name,
      arguments: call.arguments,
      result: call.result,
      isError: call.isError,
      isRejected: call.isRejected ?? false,
    });
    if (!text) return;
    try {
      await Clipboard.setStringAsync(text);
      haptic.success();
    } catch {
      haptic.error();
    }
  }, [call]);

  if (!call) return null;

  return (
    <Modal
      visible={visible}
      transparent
      animationType="fade"
      onRequestClose={onClose}
    >
      <View style={styles.overlay}>
        <Pressable style={styles.backdrop} onPress={onClose} />
        <View style={[styles.card, { backgroundColor: colors.white }]}>
          <View style={styles.header}>
            <View
              style={[styles.statusDot, { backgroundColor: statusColor }]}
            />
            <Text style={[styles.title, { color: colors.black }]}>{title}</Text>
            <View style={[styles.badge, { backgroundColor: statusColor }]}>
              {status === '拒否' ? (
                <Ionicons name="close" size={12} color={colors.white} />
              ) : (
                <Text style={[styles.badgeText, { color: colors.white }]}>
                  {status}
                </Text>
              )}
            </View>
          </View>

          <ScrollView
            style={styles.body}
            contentContainerStyle={styles.bodyContent}
          >
            {args !== undefined && (
              <View style={styles.section}>
                <Text style={[styles.sectionTitle, { color: colors.gray }]}>
                  引数
                </Text>
                {isAsr && isRecord(args) && Array.isArray(args.questions) ? (
                  <AsrArgumentList questions={args.questions} colors={colors} />
                ) : isRecord(args) ? (
                  <ObjectRows object={args} colors={colors} depth={0} />
                ) : (
                  <Text style={[styles.monoText, { color: colors.black }]}>
                    {formatJson(args)}
                  </Text>
                )}
              </View>
            )}

            {call.result !== undefined && (
              <View style={styles.section}>
                <Text
                  style={[
                    styles.sectionTitle,
                    { color: call.isError ? colors.red : colors.gray },
                  ]}
                >
                  {call.isError ? 'エラー' : '結果'}
                </Text>
                <ResultContent
                  name={call.name}
                  result={call.result}
                  isRejected={call.isRejected ?? false}
                  colors={colors}
                />
              </View>
            )}
          </ScrollView>

          <View style={styles.footer}>
            <Pressable
              onPress={handleCopy}
              style={[
                styles.copyButton,
                {
                  borderColor: colors.separator,
                  backgroundColor: colors.surface,
                },
              ]}
            >
              <Text style={[styles.copyText, { color: colors.black }]}>
                JSONをコピー
              </Text>
            </Pressable>
            <Pressable
              onPress={onClose}
              style={[styles.closeButton, { backgroundColor: colors.brand }]}
            >
              <Text style={[styles.closeText, { color: colors.white }]}>
                閉じる
              </Text>
            </Pressable>
          </View>
        </View>
      </View>
    </Modal>
  );
}

interface ScheduleEntryItem {
  reference?: string;
  display_id?: string | number;
  title?: string;
  start_at?: string;
  end_at?: string;
}

interface ScheduleEntriesListProps {
  entries: Record<string, unknown>[];
  colors: ColorSet;
}

function ScheduleEntriesList({ entries, colors }: ScheduleEntriesListProps) {
  const grouped = useMemo(() => {
    const map = new Map<string, ScheduleEntryItem[]>();
    const sorted = [...entries].sort((a, b) => {
      const aIso = asString(a.start_at) ?? asString(a.end_at) ?? '';
      const bIso = asString(b.start_at) ?? asString(b.end_at) ?? '';
      const aTime = getTime(aIso);
      const bTime = getTime(bIso);
      if (aTime == null || bTime == null) return 0;
      return aTime - bTime;
    });
    for (const entry of sorted) {
      const start = asString(entry.start_at) ?? asString(entry.end_at);
      const parsed = start ? parseDateTime(start) : null;
      const date = parsed?.date ?? '不明';
      const list = map.get(date) ?? [];
      list.push({
        reference: asString(entry.reference) ?? undefined,
        display_id: entry.display_id as string | number | undefined,
        title: asString(entry.title) ?? undefined,
        start_at: asString(entry.start_at) ?? undefined,
        end_at: asString(entry.end_at) ?? undefined,
      });
      map.set(date, list);
    }
    return Array.from(map.entries());
  }, [entries]);

  return (
    <View style={{ gap: 12 }}>
      {grouped.map(([date, list]) => (
        <ScheduleDayCard
          key={date}
          date={date}
          entries={list}
          colors={colors}
        />
      ))}
    </View>
  );
}

interface ScheduleDayCardProps {
  date: string;
  entries: ScheduleEntryItem[];
  colors: ColorSet;
}

function ScheduleDayCard({ date, entries, colors }: ScheduleDayCardProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeHeader}>
        <View style={[styles.changeBadge, { backgroundColor: colors.gray }]}>
          <Text style={styles.changeBadgeText}>{date}</Text>
        </View>
        <Text style={{ color: colors.gray, fontSize: 13 }}>
          {entries.length}件
        </Text>
      </View>
      <View
        style={[
          styles.whenBlock,
          { backgroundColor: colors.surface, borderColor: colors.separator },
        ]}
      >
        {entries.map((entry, index) => {
          let time = '';
          if (entry.start_at && entry.end_at) {
            const s = parseDateTime(entry.start_at);
            const e = parseDateTime(entry.end_at);
            if (s && e) {
              time =
                s.date === e.date
                  ? `${s.time} 〜 ${e.time}`
                  : `${s.date} ${s.time} 〜 ${e.date} ${e.time}`;
            }
          } else if (entry.start_at) {
            time = parseDateTime(entry.start_at)?.time ?? '';
          } else if (entry.end_at) {
            time = parseDateTime(entry.end_at)?.time ?? '';
          }
          const ref = entry.reference ?? String(entry.display_id ?? '');
          const title = `${ref ? `[${ref}] ` : ''}${entry.title ?? ''}`;
          return (
            <DetailRow
              key={index}
              label={time || '時刻なし'}
              value={title}
              colors={colors}
            />
          );
        })}
      </View>
    </View>
  );
}

interface OverdueCardProps {
  item: Record<string, unknown>;
  colors: ColorSet;
}

function OverdueCard({ item, colors }: OverdueCardProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const title = asString(item.title);
  const ref = asString(item.reference) ?? String(item.display_id ?? '');
  const end = asString(item.end_at);
  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeHeader}>
        <View style={[styles.changeBadge, { backgroundColor: colors.red }]}>
          <Text style={styles.changeBadgeText}>期限超過</Text>
        </View>
        <Text
          style={[styles.changeTarget, { color: colors.black }]}
          numberOfLines={1}
        >
          {ref ? `[${ref}] ` : ''}
          {title ?? ''}
        </Text>
      </View>
      {end && (
        <View
          style={[
            styles.whenBlock,
            { backgroundColor: colors.surface, borderColor: colors.separator },
          ]}
        >
          <DetailRow label="期限" value={formatInstant(end)} colors={colors} />
        </View>
      )}
    </View>
  );
}

interface ReferenceCardProps {
  title: string;
  items: string[];
  colors: ColorSet;
}

function ReferenceCard({ title, items, colors }: ReferenceCardProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const isMove = title === '移動対象';
  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeHeader}>
        <View
          style={[
            styles.changeBadge,
            { backgroundColor: isMove ? colors.brand : colors.gray },
          ]}
        >
          <Text style={styles.changeBadgeText}>{title}</Text>
        </View>
      </View>
      <View
        style={[
          styles.whenBlock,
          { backgroundColor: colors.surface, borderColor: colors.separator },
        ]}
      >
        <DetailRow label="タスク" value={items.join('、 ')} colors={colors} />
      </View>
    </View>
  );
}

interface SleepImpactProps {
  before?: number;
  after?: number;
  colors: ColorSet;
}

function SleepCard({ before, after, colors }: SleepImpactProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const beforeStr = before !== undefined ? formatDuration(before) : '-';
  const afterStr = after !== undefined ? formatDuration(after) : '-';
  const reduced = before !== undefined && after !== undefined && after < before;
  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeHeader}>
        <View style={[styles.changeBadge, { backgroundColor: colors.gray }]}>
          <Text style={styles.changeBadgeText}>睡眠</Text>
        </View>
      </View>
      <View
        style={[
          styles.whenBlock,
          { backgroundColor: colors.surface, borderColor: colors.separator },
        ]}
      >
        <DetailRow label="変更前" value={beforeStr} colors={colors} />
        <DetailRow
          label="変更後"
          value={afterStr}
          colors={colors}
          valueColor={reduced ? colors.red : undefined}
        />
      </View>
    </View>
  );
}

interface WarningListProps {
  warnings: string[];
}

function WarningBox({ warnings }: WarningListProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  return (
    <View style={[styles.warningBox, { borderColor: colors.warning }]}>
      {warnings.map((warning, index) => (
        <Text key={index} style={{ color: colors.warning, fontSize: 13 }}>
          注意: {warning}
        </Text>
      ))}
    </View>
  );
}

interface ScheduleResultViewProps {
  name: string;
  data: Record<string, unknown>;
  colors: ColorSet;
}

function ScheduleResultView({ name, data, colors }: ScheduleResultViewProps) {
  const entries = asArray<Record<string, unknown>>(data.entries);

  if (name === 'get_schedule') {
    const overdue = asArray<Record<string, unknown>>(data.overdue);
    return (
      <View style={{ gap: 12 }}>
        {entries && entries.length > 0 && (
          <ScheduleEntriesList entries={entries} colors={colors} />
        )}
        {overdue && overdue.length > 0 && (
          <View style={{ gap: 12 }}>
            {overdue.map((item, index) => (
              <OverdueCard key={index} item={item} colors={colors} />
            ))}
          </View>
        )}
      </View>
    );
  }

  if (name === 'preview_schedule') {
    const unscheduled = asArray<string>(data.unscheduled_task_ids);
    const displaced = asArray<string>(data.displaced_task_ids);
    const sleepBefore = asNumber(data.sleep_minutes_before);
    const sleepAfter = asNumber(data.sleep_minutes_after);
    const warnings = asArray<string>(data.warnings);

    return (
      <View style={{ gap: 12 }}>
        {entries && entries.length > 0 && (
          <ScheduleEntriesList entries={entries} colors={colors} />
        )}
        {unscheduled && unscheduled.length > 0 && (
          <ReferenceCard
            title="未スケジュール"
            items={unscheduled}
            colors={colors}
          />
        )}
        {displaced && displaced.length > 0 && (
          <ReferenceCard title="移動対象" items={displaced} colors={colors} />
        )}
        {(sleepBefore !== undefined || sleepAfter !== undefined) && (
          <SleepCard before={sleepBefore} after={sleepAfter} colors={colors} />
        )}
        {warnings && warnings.length > 0 && <WarningBox warnings={warnings} />}
      </View>
    );
  }

  return null;
}

interface MemoryResultViewProps {
  data: Record<string, unknown>;
  colors: ColorSet;
}

function MemoryResultView({ data, colors }: MemoryResultViewProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const results = asArray<Record<string, unknown>>(data.results, isRecord);

  return (
    <View style={{ gap: 12 }}>
      {results && results.length > 0 ? (
        results.map((item, index) => (
          <MemoryCard
            key={asString(item.id) ?? index}
            item={item}
            colors={colors}
          />
        ))
      ) : (
        <Text style={[styles.emptyText, { color: colors.gray }]}>
          該当する記憶が見つかりませんでした。
        </Text>
      )}
    </View>
  );
}

interface MemoryCardProps {
  item: Record<string, unknown>;
  colors: ColorSet;
}

function MemoryCard({ item, colors }: MemoryCardProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const kind = asString(item.kind) ?? '';
  const source = asString(item.source) ?? '';
  const key = asString(item.key) ?? '';
  const content = asString(item.content) ?? '';
  const subjectType = asString(item.subject_type);
  const subjectId = asString(item.subject_id);
  const subject = [subjectType, subjectId].filter(Boolean).join(' ');
  const updatedAt = asString(item.updated_at);
  const revision = asNumber(item.revision);
  const id = asString(item.id) ?? '';

  return (
    <View
      style={[
        styles.changeCard,
        { backgroundColor: colors.surfaceTint, borderColor: colors.separator },
      ]}
    >
      <View style={styles.changeHeader}>
        {kind ? (
          <View style={[styles.changeBadge, { backgroundColor: colors.brand }]}>
            <Text style={styles.changeBadgeText}>{kind}</Text>
          </View>
        ) : null}
        {source ? (
          <View style={[styles.changeBadge, { backgroundColor: colors.gray }]}>
            <Text style={styles.changeBadgeText}>{source}</Text>
          </View>
        ) : null}
      </View>
      {key ? <DetailRow label="キー" value={key} colors={colors} /> : null}
      {content ? (
        <DetailRow label="内容" value={content} colors={colors} />
      ) : null}
      {subject ? (
        <DetailRow label="対象" value={subject} colors={colors} />
      ) : null}
      {updatedAt ? (
        <DetailRow
          label="更新"
          value={formatInstant(updatedAt)}
          colors={colors}
        />
      ) : null}
      {id || revision !== undefined ? (
        <Text style={[styles.memoryMeta, { color: colors.gray }]}>
          {id ? `id: ${id}` : ''}
          {id && revision !== undefined ? ' / ' : ''}
          {revision !== undefined ? `revision: ${revision}` : ''}
        </Text>
      ) : null}
    </View>
  );
}

interface SkillResultViewProps {
  data: Record<string, unknown>;
  colors: ColorSet;
}

function SkillResultView({ data, colors }: SkillResultViewProps) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const name = asString(data.name) ?? '';
  const slug = asString(data.slug) ?? '';
  const description = asString(data.description) ?? '';
  const body = asString(data.body) ?? '';
  const builtIn = asBoolean(data.built_in);

  const markdownStyles = useMemo<Record<string, unknown>>(
    () => ({
      body: { color: colors.black, fontSize: 13 },
      paragraph: { marginTop: 0, marginBottom: 8 },
    }),
    [colors.black],
  );
  const markdownRules = useMemo<RenderRules>(
    () => ({
      image: (node: ASTNode) => (
        <Text key={node.key} style={{ color: colors.gray }}>
          {node.content}
        </Text>
      ),
    }),
    [colors.gray],
  );

  return (
    <View style={{ gap: 12 }}>
      <View
        style={[
          styles.changeCard,
          {
            backgroundColor: colors.surfaceTint,
            borderColor: colors.separator,
          },
        ]}
      >
        <View style={styles.changeHeader}>
          <Text style={[styles.skillName, { color: colors.black }]}>
            {name}
          </Text>
          {builtIn ? (
            <View
              style={[styles.changeBadge, { backgroundColor: colors.gray }]}
            >
              <Text style={styles.changeBadgeText}>built-in</Text>
            </View>
          ) : null}
        </View>
        {slug ? (
          <Text style={[styles.skillSlug, { color: colors.gray }]}>{slug}</Text>
        ) : null}
        {description ? (
          <DetailRow label="説明" value={description} colors={colors} />
        ) : null}
      </View>
      {body ? (
        <View
          style={[
            styles.sectionBox,
            { backgroundColor: colors.surface, borderColor: colors.separator },
          ]}
        >
          <Markdown style={markdownStyles} rules={markdownRules}>
            {body}
          </Markdown>
        </View>
      ) : null}
    </View>
  );
}

export function ResultContent({
  name,
  result,
  isRejected,
  colors,
}: {
  name: string;
  result: string;
  isRejected: boolean;
  colors: ColorSet;
}) {
  const styles = useMemo(() => makeStyles(colors), [colors]);
  let parsed: unknown = result;
  try {
    parsed = JSON.parse(result);
  } catch {
    parsed = result;
  }
  if (!isRejected && isRecord(parsed) && name === 'memory_search') {
    return <MemoryResultView data={parsed} colors={colors} />;
  }
  if (!isRejected && isRecord(parsed) && name === 'skills_read') {
    return <SkillResultView data={parsed} colors={colors} />;
  }
  if (
    !isRejected &&
    (Array.isArray(parsed) || isRecord(parsed)) &&
    (name === 'list_tasks' || name === 'get_task')
  ) {
    return <TaskResultView data={parsed} colors={colors} />;
  }
  if (
    !isRejected &&
    (Array.isArray(parsed) || isRecord(parsed)) &&
    (name === 'list_habits' || name === 'get_habit')
  ) {
    return <HabitResultView data={parsed} colors={colors} />;
  }
  if (
    !isRejected &&
    isRecord(parsed) &&
    (name === 'get_schedule' || name === 'preview_schedule')
  ) {
    return <ScheduleResultView name={name} data={parsed} colors={colors} />;
  }
  if (isRecord(parsed)) {
    return <ObjectRows object={parsed} colors={colors} depth={0} />;
  }
  if (Array.isArray(parsed)) {
    return <ArrayRows array={parsed} colors={colors} depth={0} />;
  }
  return (
    <View
      style={[
        styles.sectionBox,
        {
          backgroundColor: colors.surface,
          borderColor: colors.separator,
        },
      ]}
    >
      <ValueView value={parsed} colors={colors} depth={0} />
    </View>
  );
}
