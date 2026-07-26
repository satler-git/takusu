// RruleBuilderModal — bottom-sheet UI for constructing a RecurrenceRule.
//
// Supports all fields of the server-side takusu_habit::RecurrenceRule:
//   - Frequency (daily / weekly / monthly / yearly)
//   - Interval (every N units)
//   - Weekday chips with nth-weekday selection (all frequencies)
//   - Month chips (all frequencies, collapsed under "詳細" for daily/weekly)
//   - Month-day chips including "月末" (-1) (monthly / yearly)
//   - Occurrence count
//   - Exclusion dates (exdates)
//   - Manual JSON input mode for advanced/unsupported patterns
//
// Emits a JSON-serialized RecurrenceRule string (see src/api/rrule.ts).

import { useEffect, useState, useMemo } from 'react';
import {
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import {
  type Frequency,
  type NWeekday,
  type RecurrenceRule,
  type Weekday,
  FREQUENCIES,
  FREQUENCY_LABELS,
  MONTHS,
  NTH_LABELS,
  WEEKDAYS,
  WEEKDAY_LABELS,
  defaultRule,
  parseRule,
  serializeRule,
  summarizeRule,
  validateRule,
} from '@/src/api/rrule';

/** Split an array into rows of `size` elements for grid layout. */
function chunk<T>(arr: T[], size: number): T[][] {
  const rows: T[][] = [];
  for (let i = 0; i < arr.length; i += size) {
    rows.push(arr.slice(i, i + size));
  }
  return rows;
}

// 1..31 positive days plus -1 (月末) as the last entry; rendered in the grid
const MONTH_DAYS = [...Array.from({ length: 31 }, (_, i) => i + 1), -1];
const MONTH_DAY_ROWS = chunk(MONTH_DAYS, 7);

// Months laid out 6 per row (12 months → 2 rows)
const MONTH_ROWS = chunk(MONTHS, 6);

// nth values available in the picker: 1..5 and -1 (最終)
const NTH_OPTIONS: (number | null)[] = [null, 1, 2, 3, 4, 5, -1];

function nthLabel(n: number | null): string {
  if (n === null) return '毎回';
  return NTH_LABELS[n] ?? String(n);
}

/** Format "YYYY-MM-DD" for display. */
function formatExdate(s: string): string {
  return s; // already YYYY-MM-DD; simple display
}

/** Convert JS Date → "YYYY-MM-DD" string. */
function dateToString(d: Date): string {
  const y = d.getFullYear();
  const m = (d.getMonth() + 1).toString().padStart(2, '0');
  const day = d.getDate().toString().padStart(2, '0');
  return `${y}-${m}-${day}`;
}

interface RruleBuilderModalProps {
  visible: boolean;
  value: string; // current JSON recurrence string
  onConfirm: (json: string) => void;
  onCancel: () => void;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      backgroundColor: colors.overlay,
      justifyContent: 'flex-end',
    },
    sheet: {
      borderTopLeftRadius: 20,
      borderTopRightRadius: 20,
      padding: 20,
      maxHeight: '90%',
    },
    header: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      marginBottom: 12,
    },
    headerLeft: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 4,
    },
    headerRight: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    title: {
      fontSize: 18,
      fontWeight: '600',
    },
    iconButton: {
      padding: 2,
    },
    helpBox: {
      borderRadius: 10,
      padding: 12,
      marginBottom: 12,
    },
    helpText: {
      fontSize: 13,
      lineHeight: 20,
    },
    body: {
      flexGrow: 0,
      flexShrink: 1,
    },
    section: {
      marginTop: 12,
    },
    sectionLabel: {
      fontSize: 13,
      fontWeight: '500',
      marginBottom: 8,
    },
    segmented: {
      flexDirection: 'row',
      gap: 8,
    },
    segment: {
      flex: 1,
      paddingVertical: 10,
      borderRadius: 10,
      borderWidth: 1,
      alignItems: 'center',
    },
    segmentText: {
      fontSize: 14,
      fontWeight: '500',
    },
    hiddenNotice: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 10,
      paddingVertical: 8,
      marginTop: 10,
    },
    hiddenNoticeText: {
      flex: 1,
      fontSize: 12,
    },
    hiddenNoticeClear: {
      paddingVertical: 2,
      paddingHorizontal: 6,
    },
    hiddenNoticeClearText: {
      fontSize: 12,
      fontWeight: '600',
    },
    stepper: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 16,
    },
    stepBtn: {
      width: 40,
      height: 40,
      borderRadius: 20,
      borderWidth: 1,
      alignItems: 'center',
      justifyContent: 'center',
    },
    stepValue: {
      fontSize: 20,
      fontWeight: '600',
      minWidth: 40,
      textAlign: 'center',
      fontVariant: ['tabular-nums'],
    },
    // Weekday chips: 7 across, evenly spaced
    weekdayChips: {
      flexDirection: 'row',
      gap: 6,
    },
    weekdayChip: {
      flex: 1,
      paddingVertical: 8,
      borderRadius: 8,
      borderWidth: 1,
      alignItems: 'center',
      gap: 2,
    },
    chipSubText: {
      fontSize: 9,
      fontWeight: '600',
    },
    // nth picker
    nthPicker: {
      borderWidth: 1,
      borderRadius: 10,
      padding: 10,
      marginTop: 8,
    },
    nthPickerLabel: {
      fontSize: 12,
      marginBottom: 8,
    },
    nthOptions: {
      flexDirection: 'row',
      gap: 6,
    },
    nthChip: {
      paddingHorizontal: 12,
      paddingVertical: 8,
      borderRadius: 8,
      borderWidth: 1,
      alignItems: 'center',
    },
    nthActions: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      marginTop: 8,
    },
    nthClose: {
      alignSelf: 'flex-start',
    },
    nthCloseText: {
      fontSize: 13,
    },
    nthRemove: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 4,
    },
    nthRemoveText: {
      fontSize: 13,
    },
    // Generic chips
    chips: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 8,
    },
    // Month chips: 6 per row
    monthRow: {
      flexDirection: 'row',
      gap: 6,
      marginBottom: 6,
    },
    monthChip: {
      flex: 1,
      paddingVertical: 8,
      borderRadius: 8,
      borderWidth: 1,
      alignItems: 'center',
    },
    chip: {
      paddingHorizontal: 14,
      paddingVertical: 8,
      borderRadius: 8,
      borderWidth: 1,
      alignItems: 'center',
    },
    chipText: {
      fontSize: 14,
      fontWeight: '500',
    },
    // Month-day chips: 7 per row
    dayRow: {
      flexDirection: 'row',
      gap: 6,
      marginBottom: 6,
    },
    dayChip: {
      flex: 1,
      paddingVertical: 8,
      borderRadius: 8,
      borderWidth: 1,
      alignItems: 'center',
    },
    dayChipPlaceholder: {
      flex: 1,
    },
    // Exdates
    exdateHeader: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      marginBottom: 8,
    },
    addExdateBtn: {
      flexDirection: 'row',
      alignItems: 'center',
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 10,
      paddingVertical: 5,
      gap: 4,
    },
    addExdateBtnText: {
      fontSize: 13,
      fontWeight: '500',
    },
    exdateRow: {
      flexDirection: 'row',
      alignItems: 'center',
      borderRadius: 8,
      paddingHorizontal: 10,
      paddingVertical: 8,
      marginBottom: 6,
      gap: 8,
    },
    exdateText: {
      flex: 1,
      fontSize: 14,
    },
    emptyNote: {
      fontSize: 13,
      fontStyle: 'italic',
    },
    // Advanced toggle (collapsible month filter)
    advancedToggle: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 4,
    },
    advancedToggleText: {
      fontSize: 13,
    },
    // Count input
    input: {
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 8,
      fontSize: 16,
    },
    // Manual JSON input
    manualInput: {
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 13,
      minHeight: 140,
      fontFamily: 'monospace',
      textAlignVertical: 'top',
    },
    errorText: {
      fontSize: 12,
      marginTop: 4,
    },
    // Summary bar
    summary: {
      borderRadius: 10,
      padding: 12,
      marginTop: 16,
    },
    summaryLabel: {
      fontSize: 12,
      fontWeight: '500',
      marginBottom: 4,
    },
    summaryText: {
      fontSize: 15,
      fontWeight: '600',
    },
    // Action row
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
      backgroundColor: colors.brand,
      alignItems: 'center',
    },
    confirmText: {
      fontSize: 15,
      fontWeight: '600',
    },
  });

export function RruleBuilderModal({
  visible,
  value,
  onConfirm,
  onCancel,
}: RruleBuilderModalProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();

  // ---- state ----
  const [rule, setRule] = useState<RecurrenceRule>(defaultRule());
  const [showHelp, setShowHelp] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false); // month filter collapse

  // Manual JSON mode
  const [manualMode, setManualMode] = useState(false);
  const [manualText, setManualText] = useState('');
  const [manualError, setManualError] = useState<string | null>(null);

  // nth-weekday editor: which weekday is being edited (null = none)
  const [editingNthWeekday, setEditingNthWeekday] = useState<Weekday | null>(
    null,
  );

  // exdate picker
  const [showExdatePicker, setShowExdatePicker] = useState(false);

  useEffect(() => {
    if (visible) {
      const r = parseRule(value);
      setRule(r);
      setManualText(serializeRule(r));
      setManualError(null);
      setManualMode(false);
      setShowHelp(false);
      setShowAdvanced(false);
      setEditingNthWeekday(null);
    }
  }, [visible, value]);

  function update(patch: Partial<RecurrenceRule>) {
    setRule((r) => {
      const next = { ...r, ...patch };
      setManualText(serializeRule(next));
      return next;
    });
  }

  // ---- weekday helpers ----

  /** Returns the current NWeekday entry for a weekday, or null if not selected. */
  function weekdayEntry(wd: Weekday): NWeekday | null {
    return rule.by_day.find((d) => d.weekday === wd) ?? null;
  }

  function toggleWeekday(wd: Weekday) {
    haptic.select();
    const has = weekdayEntry(wd);
    if (has) {
      // Remove
      update({ by_day: rule.by_day.filter((d) => d.weekday !== wd) });
      if (editingNthWeekday === wd) setEditingNthWeekday(null);
    } else {
      // Add with n=null (every occurrence), then sort by canonical weekday
      // order (sun..sat) so the summary is independent of tap order (#302).
      update({
        by_day: [...rule.by_day, { n: null, weekday: wd }].sort(
          (a, b) => WEEKDAYS.indexOf(a.weekday) - WEEKDAYS.indexOf(b.weekday),
        ),
      });
      // Open nth editor for non-daily frequencies
      if (rule.freq !== 'daily') setEditingNthWeekday(wd);
    }
  }

  function setNth(wd: Weekday, n: number | null) {
    haptic.select();
    update({
      by_day: rule.by_day.map((d) => (d.weekday === wd ? { ...d, n } : d)),
    });
  }

  // ---- month-day helpers ----

  function toggleMonthDay(day: number) {
    haptic.select();
    const has = rule.by_month_day.includes(day);
    update({
      by_month_day: has
        ? rule.by_month_day.filter((d) => d !== day)
        : [...rule.by_month_day, day].sort((a, b) => a - b),
    });
  }

  function toggleLastDay() {
    haptic.select();
    const has = rule.by_month_day.includes(-1);
    update({
      by_month_day: has
        ? rule.by_month_day.filter((d) => d !== -1)
        : [...rule.by_month_day, -1].sort((a, b) => a - b),
    });
  }

  // ---- month helpers ----

  function toggleMonth(m: number) {
    haptic.select();
    const has = rule.by_month.includes(m);
    update({
      by_month: has
        ? rule.by_month.filter((x) => x !== m)
        : [...rule.by_month, m].sort((a, b) => a - b),
    });
  }

  /** Render the month chips as a 6-per-row grid. */
  function renderMonthGrid() {
    return MONTH_ROWS.map((row, ri) => (
      <View key={ri} style={styles.monthRow}>
        {row.map((m) => {
          const on = rule.by_month.includes(m);
          return (
            <Pressable
              key={m}
              style={[
                styles.monthChip,
                {
                  borderColor: on ? colors.brand : colors.separator,
                },
                on && { backgroundColor: colors.brand },
              ]}
              onPress={() => toggleMonth(m)}
            >
              <Text
                style={[
                  styles.chipText,
                  { color: on ? colors.white : colors.black },
                ]}
              >
                {m}月
              </Text>
            </Pressable>
          );
        })}
      </View>
    ));
  }

  // ---- exdate helpers ----

  function addExdate(d: Date | null) {
    setShowExdatePicker(false);
    if (!d) return;
    const s = dateToString(d);
    if (rule.exdates.includes(s)) return;
    update({ exdates: [...rule.exdates, s].sort() });
  }

  function removeExdate(s: string) {
    haptic.select();
    update({ exdates: rule.exdates.filter((x) => x !== s) });
  }

  // ---- manual mode helpers ----

  function switchToManual() {
    haptic.light();
    setManualText(serializeRule(rule));
    setManualError(null);
    setManualMode(true);
  }

  function switchToBuilder() {
    haptic.light();
    try {
      const parsed = JSON.parse(manualText);
      const r: RecurrenceRule = {
        freq: parsed.freq ?? 'daily',
        interval: parsed.interval ?? 1,
        by_day: parsed.by_day ?? [],
        by_month: parsed.by_month ?? [],
        by_month_day: parsed.by_month_day ?? [],
        count: parsed.count ?? null,
        exdates: parsed.exdates ?? [],
      };
      const err = validateRule(r);
      if (err) {
        setManualError(err);
        return;
      }
      setRule(r);
      setManualError(null);
      setManualMode(false);
    } catch {
      setManualError('JSON のパースに失敗しました');
    }
  }

  function handleConfirm() {
    haptic.medium();
    if (manualMode) {
      try {
        const parsed = JSON.parse(manualText);
        const r: RecurrenceRule = {
          freq: parsed.freq ?? 'daily',
          interval: parsed.interval ?? 1,
          by_day: parsed.by_day ?? [],
          by_month: parsed.by_month ?? [],
          by_month_day: parsed.by_month_day ?? [],
          count: parsed.count ?? null,
          exdates: parsed.exdates ?? [],
        };
        const err = validateRule(r);
        if (err) {
          setManualError(err);
          return;
        }
        onConfirm(serializeRule(r));
      } catch {
        setManualError('JSON のパースに失敗しました');
      }
    } else {
      onConfirm(serializeRule(rule));
    }
  }

  const unit =
    rule.freq === 'daily'
      ? '日'
      : rule.freq === 'weekly'
        ? '週'
        : rule.freq === 'monthly'
          ? '月'
          : '年';

  // Show month-day grid for monthly and yearly
  const showMonthDays = rule.freq === 'monthly' || rule.freq === 'yearly';
  // Show advanced (month filter) inline for monthly/yearly; collapsible for daily/weekly
  const alwaysShowMonths = rule.freq === 'yearly';

  // Detect fields that are hidden in the builder but still carry values.
  // The engine evaluates by_day / by_month / by_month_day for all frequencies,
  // so these are valid — but the user may not notice them when the UI is hidden.
  const hiddenMonthDays = !showMonthDays && rule.by_month_day.length > 0;
  const hiddenMonths =
    !alwaysShowMonths && !showAdvanced && rule.by_month.length > 0;
  const hiddenFields: string[] = [];
  if (hiddenMonthDays)
    hiddenFields.push(`実行日 ${rule.by_month_day.length}件`);
  if (hiddenMonths) hiddenFields.push(`月 ${rule.by_month.length}件`);
  const hasHiddenFields = hiddenFields.length > 0;

  return (
    <>
      <Modal
        visible={visible}
        transparent
        animationType="slide"
        onRequestClose={onCancel}
      >
        <View style={styles.overlay}>
          <Pressable
            style={StyleSheet.absoluteFill}
            onPress={() => {
              haptic.light();
              onCancel();
            }}
          />
          <View
            style={[
              styles.sheet,
              {
                backgroundColor: colors.white,
                paddingBottom: 32 + insets.bottom,
              },
            ]}
          >
            {/* Header */}
            <View style={styles.header}>
              <View style={styles.headerLeft}>
                <Text style={[styles.title, { color: colors.black }]}>
                  周期 (RRULE)
                </Text>
                <Pressable
                  style={styles.iconButton}
                  onPress={() => {
                    haptic.light();
                    setShowHelp((v) => !v);
                  }}
                  hitSlop={8}
                >
                  <Ionicons
                    name={showHelp ? 'help-circle' : 'help-circle-outline'}
                    size={20}
                    color={colors.brand}
                  />
                </Pressable>
              </View>
              <View style={styles.headerRight}>
                <Pressable
                  style={styles.iconButton}
                  onPress={manualMode ? switchToBuilder : switchToManual}
                  hitSlop={8}
                >
                  <Ionicons
                    name={
                      manualMode ? 'construct-outline' : 'code-slash-outline'
                    }
                    size={20}
                    color={colors.brand}
                  />
                </Pressable>
                <Pressable
                  onPress={() => {
                    haptic.light();
                    onCancel();
                  }}
                  hitSlop={8}
                >
                  <Ionicons name="close" size={24} color={colors.gray} />
                </Pressable>
              </View>
            </View>

            {/* Help box */}
            {showHelp && (
              <View
                style={[
                  styles.helpBox,
                  { backgroundColor: colors.surfaceTint },
                ]}
              >
                <Text style={[styles.helpText, { color: colors.grayDark }]}>
                  RRULEは繰り返しルールの標準形式です。{'\n'}
                  ・頻度: 毎日・毎週・毎月・毎年{'\n'}
                  ・間隔: N日/N週/Nヶ月/N年ごと{'\n'}
                  ・曜日: 実行する曜日。「第N」でN番目の曜日のみ{'\n'}
                  ・日付: 実行日 (例: 1日・15日・月末){'\n'}
                  ・月: 実行する月 (全頻度で使用可){'\n'}
                  ・回数: 繰り返す回数 (未設定なら無限){'\n'}
                  ・除外日: 特定の日をスキップ{'\n'}
                  ・手動入力(右上 {'</>'}): 生JSONで細かく設定
                </Text>
              </View>
            )}

            <ScrollView
              style={styles.body}
              showsVerticalScrollIndicator={false}
            >
              {manualMode ? (
                /* ---- Manual JSON mode ---- */
                <View style={styles.section}>
                  <Text style={[styles.sectionLabel, { color: colors.gray }]}>
                    JSON (手動入力)
                  </Text>
                  <TextInput
                    style={[
                      styles.manualInput,
                      {
                        borderColor: manualError
                          ? colors.red
                          : colors.separator,
                        color: colors.black,
                      },
                    ]}
                    value={manualText}
                    onChangeText={(t) => {
                      setManualText(t);
                      setManualError(null);
                    }}
                    multiline
                    autoCapitalize="none"
                    autoCorrect={false}
                    placeholder='{"freq":"daily","interval":1,...}'
                    placeholderTextColor={colors.grayLight}
                  />
                  {manualError && (
                    <Text style={[styles.errorText, { color: colors.red }]}>
                      {manualError}
                    </Text>
                  )}
                  <Pressable
                    style={[styles.advancedToggle, { marginTop: 8 }]}
                    onPress={switchToBuilder}
                  >
                    <Ionicons
                      name="construct-outline"
                      size={14}
                      color={colors.brand}
                    />
                    <Text
                      style={[
                        styles.advancedToggleText,
                        { color: colors.brand },
                      ]}
                    >
                      ビルダーに戻る (バリデーション後)
                    </Text>
                  </Pressable>
                </View>
              ) : (
                /* ---- Builder mode ---- */
                <>
                  {/* Frequency */}
                  <Text style={[styles.sectionLabel, { color: colors.gray }]}>
                    頻度
                  </Text>
                  <View style={styles.segmented}>
                    {FREQUENCIES.map((f) => (
                      <Pressable
                        key={f}
                        style={[
                          styles.segment,
                          {
                            borderColor:
                              rule.freq === f ? colors.brand : colors.separator,
                          },
                          rule.freq === f && { backgroundColor: colors.brand },
                        ]}
                        onPress={() => {
                          if (rule.freq !== f) haptic.select();
                          // Reset nth-weekday editor when frequency changes
                          setEditingNthWeekday(null);
                          update({ freq: f as Frequency });
                        }}
                      >
                        <Text
                          style={[
                            styles.segmentText,
                            {
                              color:
                                rule.freq === f ? colors.white : colors.black,
                            },
                          ]}
                        >
                          {FREQUENCY_LABELS[f]}
                        </Text>
                      </Pressable>
                    ))}
                  </View>

                  {/* Hidden-fields notice */}
                  {hasHiddenFields && (
                    <View
                      style={[
                        styles.hiddenNotice,
                        {
                          backgroundColor: colors.warningBg,
                          borderColor: colors.warningBorder,
                        },
                      ]}
                    >
                      <Ionicons
                        name="information-circle-outline"
                        size={14}
                        color={colors.warningIcon}
                      />
                      <Text
                        style={[
                          styles.hiddenNoticeText,
                          { color: colors.grayDark },
                        ]}
                      >
                        非表示フィールドに選択が残っています:{' '}
                        {hiddenFields.join('・')} (プレビューで確認)
                      </Text>
                      <Pressable
                        style={styles.hiddenNoticeClear}
                        onPress={() => {
                          haptic.select();
                          const patch: Partial<RecurrenceRule> = {};
                          if (hiddenMonthDays) patch.by_month_day = [];
                          if (hiddenMonths) patch.by_month = [];
                          update(patch);
                        }}
                      >
                        <Text
                          style={[
                            styles.hiddenNoticeClearText,
                            { color: colors.brand },
                          ]}
                        >
                          クリア
                        </Text>
                      </Pressable>
                    </View>
                  )}

                  {/* Interval */}
                  <Text
                    style={[
                      styles.sectionLabel,
                      { color: colors.gray, marginTop: 16 },
                    ]}
                  >
                    間隔 ({unit}ごと)
                  </Text>
                  <View style={styles.stepper}>
                    <Pressable
                      style={[
                        styles.stepBtn,
                        { borderColor: colors.separator },
                      ]}
                      onPress={() => {
                        haptic.select();
                        update({ interval: Math.max(1, rule.interval - 1) });
                      }}
                    >
                      <Ionicons name="remove" size={20} color={colors.brand} />
                    </Pressable>
                    <Text style={[styles.stepValue, { color: colors.black }]}>
                      {rule.interval}
                    </Text>
                    <Pressable
                      style={[
                        styles.stepBtn,
                        { borderColor: colors.separator },
                      ]}
                      onPress={() => {
                        haptic.select();
                        update({ interval: rule.interval + 1 });
                      }}
                    >
                      <Ionicons name="add" size={20} color={colors.brand} />
                    </Pressable>
                  </View>

                  {/* Weekday chips (all frequencies) */}
                  <View style={styles.section}>
                    <Text style={[styles.sectionLabel, { color: colors.gray }]}>
                      曜日
                      {rule.freq !== 'daily' && rule.freq !== 'weekly'
                        ? ' (オプション)'
                        : ''}
                    </Text>
                    <View style={styles.weekdayChips}>
                      {WEEKDAYS.map((wd) => {
                        const entry = weekdayEntry(wd);
                        const on = entry !== null;
                        const isEditing = editingNthWeekday === wd;
                        return (
                          <Pressable
                            key={wd}
                            style={[
                              styles.weekdayChip,
                              {
                                borderColor: on
                                  ? colors.brand
                                  : colors.separator,
                              },
                              on && { backgroundColor: colors.brand },
                            ]}
                            onPress={() => {
                              // For monthly/yearly: tapping a selected weekday opens the nth editor.
                              // For weekly: if an nth value is set, open the editor to allow re-editing;
                              //   if n is null (every week), tap toggles the weekday off directly.
                              // For daily: tap always toggles.
                              const pressedEntry = on ? weekdayEntry(wd) : null;
                              const hasNth =
                                pressedEntry !== null &&
                                pressedEntry.n !== null;
                              if (
                                on &&
                                (rule.freq === 'monthly' ||
                                  rule.freq === 'yearly' ||
                                  hasNth)
                              ) {
                                haptic.light();
                                setEditingNthWeekday(isEditing ? null : wd);
                              } else {
                                toggleWeekday(wd);
                              }
                            }}
                          >
                            <Text
                              style={[
                                styles.chipText,
                                { color: on ? colors.white : colors.black },
                              ]}
                            >
                              {WEEKDAY_LABELS[wd]}
                            </Text>
                            {on && entry.n !== null && (
                              <Text
                                style={[
                                  styles.chipSubText,
                                  { color: colors.white },
                                ]}
                              >
                                {NTH_LABELS[entry.n] ?? String(entry.n)}
                              </Text>
                            )}
                          </Pressable>
                        );
                      })}
                    </View>

                    {/* nth weekday picker (shown when editingNthWeekday is set and freq allows it) */}
                    {editingNthWeekday !== null &&
                      weekdayEntry(editingNthWeekday) && (
                        <View
                          style={[
                            styles.nthPicker,
                            { borderColor: colors.separator },
                          ]}
                        >
                          <Text
                            style={[
                              styles.nthPickerLabel,
                              { color: colors.gray },
                            ]}
                          >
                            {WEEKDAY_LABELS[editingNthWeekday]}曜日: 何番目?
                          </Text>
                          <ScrollView
                            horizontal
                            showsHorizontalScrollIndicator={false}
                          >
                            <View style={styles.nthOptions}>
                              {NTH_OPTIONS.map((n) => {
                                const current =
                                  weekdayEntry(editingNthWeekday)?.n ?? null;
                                const selected = current === n;
                                return (
                                  <Pressable
                                    key={n === null ? 'every' : n}
                                    style={[
                                      styles.nthChip,
                                      {
                                        borderColor: selected
                                          ? colors.brand
                                          : colors.separator,
                                      },
                                      selected && {
                                        backgroundColor: colors.brand,
                                      },
                                    ]}
                                    onPress={() => setNth(editingNthWeekday, n)}
                                  >
                                    <Text
                                      style={[
                                        styles.chipText,
                                        {
                                          color: selected
                                            ? colors.white
                                            : colors.black,
                                        },
                                      ]}
                                    >
                                      {nthLabel(n)}
                                    </Text>
                                  </Pressable>
                                );
                              })}
                            </View>
                          </ScrollView>
                          <View style={styles.nthActions}>
                            <Pressable
                              style={styles.nthClose}
                              onPress={() => {
                                haptic.light();
                                setEditingNthWeekday(null);
                              }}
                            >
                              <Text
                                style={[
                                  styles.nthCloseText,
                                  { color: colors.brand },
                                ]}
                              >
                                閉じる
                              </Text>
                            </Pressable>
                            <Pressable
                              style={styles.nthRemove}
                              onPress={() => {
                                haptic.select();
                                update({
                                  by_day: rule.by_day.filter(
                                    (d) => d.weekday !== editingNthWeekday,
                                  ),
                                });
                                setEditingNthWeekday(null);
                              }}
                            >
                              <Ionicons
                                name="trash-outline"
                                size={14}
                                color={colors.red}
                              />
                              <Text
                                style={[
                                  styles.nthRemoveText,
                                  { color: colors.red },
                                ]}
                              >
                                この曜日を削除
                              </Text>
                            </Pressable>
                          </View>
                        </View>
                      )}
                  </View>

                  {/* Month chips */}
                  {alwaysShowMonths ? (
                    // Always visible for yearly
                    <View style={styles.section}>
                      <Text
                        style={[styles.sectionLabel, { color: colors.gray }]}
                      >
                        月
                      </Text>
                      {renderMonthGrid()}
                    </View>
                  ) : (
                    // Collapsible for daily / weekly / monthly
                    <View style={styles.section}>
                      <Pressable
                        style={styles.advancedToggle}
                        onPress={() => {
                          haptic.light();
                          setShowAdvanced((v) => !v);
                        }}
                      >
                        <Ionicons
                          name={
                            showAdvanced ? 'chevron-down' : 'chevron-forward'
                          }
                          size={14}
                          color={colors.brand}
                        />
                        <Text
                          style={[
                            styles.advancedToggleText,
                            { color: colors.brand },
                          ]}
                        >
                          月フィルタ (詳細)
                          {rule.by_month.length > 0
                            ? ` · ${rule.by_month.length}件選択中`
                            : ''}
                        </Text>
                      </Pressable>
                      {showAdvanced && (
                        <View style={{ marginTop: 8 }}>
                          {renderMonthGrid()}
                        </View>
                      )}
                    </View>
                  )}

                  {/* Month-day chips (monthly / yearly) */}
                  {showMonthDays && (
                    <View style={styles.section}>
                      <Text
                        style={[styles.sectionLabel, { color: colors.gray }]}
                      >
                        実行日
                      </Text>
                      {/* 1..31 grid with 月末 (-1) as the last chip */}
                      {MONTH_DAY_ROWS.map((row, ri) => (
                        <View key={ri} style={styles.dayRow}>
                          {row.map((d) => {
                            const isLastDay = d === -1;
                            const on = rule.by_month_day.includes(d);
                            return (
                              <Pressable
                                key={isLastDay ? 'last' : d}
                                style={[
                                  styles.dayChip,
                                  {
                                    borderColor: on
                                      ? colors.brand
                                      : colors.separator,
                                  },
                                  on && { backgroundColor: colors.brand },
                                ]}
                                onPress={() =>
                                  isLastDay
                                    ? toggleLastDay()
                                    : toggleMonthDay(d)
                                }
                              >
                                <Text
                                  style={[
                                    styles.chipText,
                                    { color: on ? colors.white : colors.black },
                                  ]}
                                >
                                  {isLastDay ? '月末' : d}
                                </Text>
                              </Pressable>
                            );
                          })}
                          {Array.from({ length: 7 - row.length }, (_, i) => (
                            <View
                              key={`pad-${i}`}
                              style={styles.dayChipPlaceholder}
                            />
                          ))}
                        </View>
                      ))}
                    </View>
                  )}

                  {/* Count */}
                  <View style={styles.section}>
                    <Text style={[styles.sectionLabel, { color: colors.gray }]}>
                      回数 (任意・空欄で無限)
                    </Text>
                    <TextInput
                      style={[
                        styles.input,
                        { borderColor: colors.separator, color: colors.black },
                      ]}
                      value={rule.count === null ? '' : String(rule.count)}
                      onChangeText={(t) => {
                        const n = parseInt(t, 10);
                        update({
                          count:
                            Number.isNaN(n) || t === '' ? null : Math.max(1, n),
                        });
                      }}
                      keyboardType="numeric"
                      placeholder="例: 10"
                      placeholderTextColor={colors.grayLight}
                    />
                  </View>

                  {/* Exdates */}
                  <View style={styles.section}>
                    <View style={styles.exdateHeader}>
                      <Text
                        style={[
                          styles.sectionLabel,
                          { color: colors.gray, marginBottom: 0 },
                        ]}
                      >
                        除外日
                      </Text>
                      <Pressable
                        style={[
                          styles.addExdateBtn,
                          { borderColor: colors.brand },
                        ]}
                        onPress={() => {
                          haptic.light();
                          setShowExdatePicker(true);
                        }}
                      >
                        <Ionicons name="add" size={16} color={colors.brand} />
                        <Text
                          style={[
                            styles.addExdateBtnText,
                            { color: colors.brand },
                          ]}
                        >
                          追加
                        </Text>
                      </Pressable>
                    </View>
                    {rule.exdates.length === 0 ? (
                      <Text
                        style={[styles.emptyNote, { color: colors.grayLight }]}
                      >
                        除外する日がある場合は追加してください
                      </Text>
                    ) : (
                      rule.exdates.map((ex) => (
                        <View
                          key={ex}
                          style={[
                            styles.exdateRow,
                            { backgroundColor: colors.surfaceTint },
                          ]}
                        >
                          <Ionicons
                            name="calendar-clear-outline"
                            size={16}
                            color={colors.brand}
                          />
                          <Text
                            style={[styles.exdateText, { color: colors.black }]}
                          >
                            {formatExdate(ex)}
                          </Text>
                          <Pressable
                            onPress={() => removeExdate(ex)}
                            hitSlop={8}
                          >
                            <Ionicons
                              name="close-circle"
                              size={18}
                              color={colors.gray}
                            />
                          </Pressable>
                        </View>
                      ))
                    )}
                  </View>
                </>
              )}
            </ScrollView>

            {/* Summary */}
            {!manualMode && (
              <View
                style={[
                  styles.summary,
                  { backgroundColor: colors.surfaceTint },
                ]}
              >
                <Text style={[styles.summaryLabel, { color: colors.gray }]}>
                  プレビュー
                </Text>
                <Text style={[styles.summaryText, { color: colors.black }]}>
                  {summarizeRule(rule)}
                </Text>
              </View>
            )}

            {manualError && manualMode && (
              <Text
                style={[
                  styles.errorText,
                  { color: colors.red, marginBottom: 4 },
                ]}
              >
                {manualError}
              </Text>
            )}

            <View style={styles.actionRow}>
              <Pressable
                style={[styles.cancelButton, { borderColor: colors.separator }]}
                onPress={() => {
                  haptic.light();
                  onCancel();
                }}
              >
                <Text style={[styles.cancelText, { color: colors.grayDark }]}>
                  キャンセル
                </Text>
              </Pressable>
              <Pressable style={styles.confirmButton} onPress={handleConfirm}>
                <Text style={[styles.confirmText, { color: colors.white }]}>
                  設定
                </Text>
              </Pressable>
            </View>
          </View>
        </View>
      </Modal>

      {/* Exdate date picker — rendered outside the main Modal to avoid nesting issues */}
      <DateTimePickerModal
        visible={showExdatePicker}
        value={null}
        mode="date"
        label="除外日を選択"
        onConfirm={addExdate}
        onCancel={() => setShowExdatePicker(false)}
      />
    </>
  );
}
