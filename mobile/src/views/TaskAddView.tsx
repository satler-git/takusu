// TaskAddView — create a new task with optional dependencies
// Fields: title, start_at (optional), end_at (required), avg_minutes, sigma_minutes, abandonability, description
// Can add dependency targets (select from existing tasks)

import { useState, useMemo } from 'react';
import {
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { useRouter, useLocalSearchParams } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import { Checkbox } from 'react-native-paper';
import { Slider } from '@expo/ui/community/slider';
import { useServer } from '@/src/api/ServerProvider';
import { undoRedo } from '@/src/api/undoRedo';
import { showError } from '@/src/api/errors';
import type { TaskRow } from '@/src/api/types';
import { useColors, type ColorSet } from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { haptic } from '@/src/components/haptics';
import { PressableScale } from '@/src/components/PressableScale';
import { useTopToast } from '@/src/components/TopToast';
import { formatDate } from '@/src/formatDate';
import { parseDuration } from '@/src/utils/duration';

interface TaskAddViewProps {
  /** Called when the view requests closing (back button / successful save).
   *  When omitted (route usage), falls back to router.back(). */
  onClose?: () => void;
  /** Pre-selected dependency IDs. Takes precedence over the `deps` search param. */
  initialDeps?: string[];
  /** When true the view is embedded inside another container that already
   *  provides top safe-area spacing, so the topBar skips adding `insets.top`
   *  padding. Defaults to false (standalone route). */
  embedded?: boolean;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      flex: 1,
    },
    topBar: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingHorizontal: 8,
      paddingBottom: 8,
    },
    backButton: {
      width: 40,
      height: 40,
      borderRadius: 20,
      alignItems: 'center',
      justifyContent: 'center',
    },
    title: {
      fontSize: 18,
      fontWeight: '600',
      marginLeft: 8,
    },
    saveButton: {
      paddingHorizontal: 16,
      paddingVertical: 8,
      backgroundColor: colors.brand,
      borderRadius: 8,
    },
    saveButtonText: {
      fontSize: 14,
      fontWeight: '600',
    },
    content: {
      padding: 16,
      gap: 16,
    },
    field: {
      gap: 4,
    },
    row: {
      flexDirection: 'row',
      gap: 12,
    },
    rowWithButton: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    inputWithButton: {
      flex: 1,
    },
    maximizeButton: {
      width: 36,
      height: 36,
      borderRadius: 8,
      borderWidth: 1.5,
      alignItems: 'center',
      justifyContent: 'center',
    },
    maximizeButtonDisabled: {
      borderColor: colors.grayLight,
      opacity: 0.4,
    },
    label: {
      fontSize: 13,
      fontWeight: '500',
    },
    hint: {
      fontSize: 11,
      marginTop: 2,
    },
    input: {
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 8,
      fontSize: 16,
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
    clearIcon: {
      padding: 4,
    },
    multiline: {
      minHeight: 80,
    },
    icalInput: {
      minHeight: 120,
      fontFamily: 'monospace',
      fontSize: 12,
    },
    icalHeader: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      paddingVertical: 4,
    },
    icalImportButton: {
      paddingHorizontal: 16,
      paddingVertical: 10,
      backgroundColor: colors.brand,
      borderRadius: 8,
      alignItems: 'center',
      marginTop: 8,
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
    depHeader: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
    },
    addDepButton: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingHorizontal: 12,
      paddingVertical: 4,
      backgroundColor: colors.brand,
      borderRadius: 6,
      gap: 4,
    },
    addDepButtonText: {
      fontSize: 13,
    },
    depItem: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      paddingVertical: 8,
      paddingHorizontal: 12,
      borderRadius: 8,
      marginTop: 4,
    },
    depItemText: {
      fontSize: 14,
    },
    depPicker: {
      position: 'absolute',
      top: 0,
      left: 0,
      right: 0,
      bottom: 0,
      zIndex: 100,
    },
    depPickerHeader: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      padding: 16,
      borderBottomWidth: 1,
    },
    depPickerTitle: {
      fontSize: 18,
      fontWeight: '600',
    },
    depPickerClose: {
      fontSize: 14,
      color: colors.brand,
    },
    depPickerList: {
      flex: 1,
    },
    depPickerItem: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      paddingVertical: 16,
      paddingHorizontal: 16,
      borderBottomWidth: 1,
    },
    depPickerItemId: {
      fontSize: 13,
      fontWeight: '500',
      fontVariant: ['tabular-nums'],
    },
    depPickerItemText: {
      fontSize: 16,
      flex: 1,
    },
    depSearchContainer: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingHorizontal: 16,
      paddingVertical: 10,
      gap: 8,
      borderBottomWidth: 1,
    },
    depSearchInput: {
      flex: 1,
      fontSize: 16,
      paddingVertical: 4,
    },
  });

export function TaskAddView({
  onClose,
  initialDeps: propDeps,
  embedded = false,
}: TaskAddViewProps = {}) {
  const { client } = useServer();
  const router = useRouter();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const { showTopToast } = useTopToast();
  const { deps } = useLocalSearchParams<{ deps?: string }>();

  const initialDeps: string[] = propDeps ?? (deps ? JSON.parse(deps) : []);

  function close() {
    if (onClose) onClose();
    else router.back();
  }

  const [title, setTitle] = useState('');
  const [startAt, setStartAt] = useState<Date | null>(null);
  const [endAt, setEndAt] = useState<Date | null>(null);
  const [avgMinutes, setAvgMinutes] = useState('60');
  const [sigmaMinutes, setSigmaMinutes] = useState('');
  const [abandonability, setAbandonability] = useState(0.5);
  const [parallelizable, setParallelizable] = useState(false);
  const [allowsParallel, setAllowsParallel] = useState(false);
  const [fixed, setFixed] = useState(false);
  const [quantityTotal, setQuantityTotal] = useState('');
  const [quantityUnit, setQuantityUnit] = useState('');
  const [description, setDescription] = useState('');
  const [selectedDeps, setSelectedDeps] = useState<string[]>(initialDeps);
  const [allTasks, setAllTasks] = useState<TaskRow[]>([]);
  const [showDepPicker, setShowDepPicker] = useState(false);
  const [depSearch, setDepSearch] = useState('');
  const [pickerField, setPickerField] = useState<'start' | 'end' | null>(null);
  const [saving, setSaving] = useState(false);
  const [showIcal, setShowIcal] = useState(false);
  const [icalText, setIcalText] = useState('');
  const [importing, setImporting] = useState(false);

  async function loadTasks() {
    if (!client) return;
    setAllTasks(await client.listTasks());
  }

  function toISO(d: Date): string {
    return d.toISOString();
  }

  async function create() {
    if (!client || !title || !endAt || saving) return;
    haptic.medium();
    setSaving(true);
    const avg = parseDuration(avgMinutes) || 60;
    const sigmaRaw = parseDuration(sigmaMinutes);
    // sigma=0/未入力の時は未送信にしてサーバーの auto (avg/5) に任せる
    const sigma = sigmaRaw !== null && sigmaRaw > 0 ? sigmaRaw : undefined;
    const parsedTotal = quantityTotal ? parseInt(quantityTotal, 10) : NaN;
    const quantity_total =
      !Number.isNaN(parsedTotal) && parsedTotal > 0 ? parsedTotal : undefined;
    try {
      const task = await client.createTask({
        title,
        description: description || undefined,
        start_at: startAt ? toISO(startAt) : undefined,
        end_at: toISO(endAt),
        avg_minutes: avg,
        sigma_minutes: sigma,
        depends: selectedDeps.length > 0 ? selectedDeps : undefined,
        abandonability,
        parallelizable,
        allows_parallel: allowsParallel,
        fixed,
        quantity_total,
        quantity_unit: quantityUnit.trim() || undefined,
      });
      undoRedo.push({
        description: `create task: ${title}`,
        undo: async () => {
          await client.deleteTask(task.id);
        },
        redo: async () => {
          await client.createTask({
            title,
            description: description || undefined,
            start_at: startAt ? toISO(startAt) : undefined,
            end_at: toISO(endAt),
            avg_minutes: avg,
            sigma_minutes: sigma,
            depends: selectedDeps.length > 0 ? selectedDeps : undefined,
            abandonability,
            parallelizable,
            allows_parallel: allowsParallel,
            fixed,
            quantity_total,
            quantity_unit: quantityUnit.trim() || undefined,
          });
        },
      });
      close();
    } catch (e) {
      void showError(e, 'タスクの追加に失敗');
    } finally {
      setSaving(false);
    }
  }

  async function importIcal() {
    if (!client || !icalText.trim() || importing) return;
    haptic.medium();
    setImporting(true);
    try {
      const result = await client.importIcal(icalText);
      showTopToast(`${result.imported}件のタスクをインポートしました`);
      setIcalText('');
      setShowIcal(false);
      close();
    } catch (e) {
      void showError(e, 'iCalインポートに失敗');
    } finally {
      setImporting(false);
    }
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View
        style={[styles.topBar, { paddingTop: 8 + (embedded ? 0 : insets.top) }]}
      >
        <PressableScale
          style={styles.backButton}
          onPress={() => {
            haptic.light();
            close();
          }}
        >
          <Ionicons name="chevron-back" size={28} color={colors.brand} />
        </PressableScale>
        <Text style={[styles.title, { color: colors.black }]}>新規タスク</Text>
        <View style={{ flex: 1 }} />
        <PressableScale
          style={[
            styles.saveButton,
            (!title || !endAt || saving) && {
              backgroundColor: colors.grayDark,
            },
          ]}
          onPress={create}
          disabled={!title || !endAt || saving}
        >
          <Text style={[styles.saveButtonText, { color: colors.white }]}>
            {saving ? '保存中…' : '追加'}
          </Text>
        </PressableScale>
      </View>

      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: 40 + insets.bottom },
        ]}
      >
        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>タイトル</Text>
          <TextInput
            style={[
              styles.input,
              { borderColor: colors.separator, color: colors.black },
            ]}
            value={title}
            onChangeText={setTitle}
            placeholder="タスク名"
            placeholderTextColor={colors.grayLight}
          />
        </View>

        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>
            開始日時 (任意)
          </Text>
          <Pressable
            style={[
              styles.dateField,
              { borderColor: colors.separator, backgroundColor: colors.white },
            ]}
            onPress={() => {
              haptic.select();
              setPickerField('start');
            }}
          >
            <Ionicons name="calendar-outline" size={20} color={colors.brand} />
            <Text
              style={[
                styles.dateText,
                { color: startAt ? colors.black : colors.grayLight },
              ]}
            >
              {formatDate(startAt)}
            </Text>
            {startAt && (
              <PressableScale
                style={styles.clearIcon}
                onPress={() => {
                  haptic.light();
                  setStartAt(null);
                }}
              >
                <Ionicons
                  name="close-circle"
                  size={18}
                  color={colors.grayLight}
                />
              </PressableScale>
            )}
          </Pressable>
        </View>

        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>
            期限日時 (必須)
          </Text>
          <Pressable
            style={[
              styles.dateField,
              { borderColor: colors.separator, backgroundColor: colors.white },
            ]}
            onPress={() => {
              haptic.select();
              setPickerField('end');
            }}
          >
            <Ionicons name="calendar-outline" size={20} color={colors.brand} />
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

        <View style={styles.row}>
          <View style={[styles.field, { flex: 1 }]}>
            <Text style={[styles.label, { color: colors.gray }]}>
              avg (1h30m / 90m / 90)
            </Text>
            <View style={styles.rowWithButton}>
              <TextInput
                style={[
                  styles.input,
                  styles.inputWithButton,
                  { borderColor: colors.separator, color: colors.black },
                ]}
                value={avgMinutes}
                onChangeText={setAvgMinutes}
                autoCapitalize="none"
                autoCorrect={false}
              />
              <PressableScale
                style={[
                  styles.maximizeButton,
                  { borderColor: colors.brand },
                  (!startAt || !endAt) && styles.maximizeButtonDisabled,
                ]}
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
              >
                <Ionicons name="expand" size={16} color={colors.brand} />
              </PressableScale>
            </View>
            {startAt &&
              endAt &&
              (() => {
                const diffMin = Math.round(
                  (endAt.getTime() - startAt.getTime()) / 60000,
                );
                return diffMin > 0 ? (
                  <Text style={[styles.hint, { color: colors.grayLight }]}>
                    window: {diffMin}分
                  </Text>
                ) : null;
              })()}
          </View>
          <View style={[styles.field, { flex: 1 }]}>
            <Text style={[styles.label, { color: colors.gray }]}>
              sigma (1h30m / 90m / 90)
            </Text>
            <TextInput
              style={[
                styles.input,
                { borderColor: colors.separator, color: colors.black },
              ]}
              value={sigmaMinutes}
              onChangeText={setSigmaMinutes}
              autoCapitalize="none"
              autoCorrect={false}
              placeholderTextColor={colors.grayLight}
            />
            {(!sigmaMinutes || sigmaMinutes === '0') && (
              <Text style={[styles.hint, { color: colors.grayLight }]}>
                {Math.max(1, Math.round((parseDuration(avgMinutes) || 60) / 5))}
                m (avg/5)
              </Text>
            )}
          </View>
        </View>

        <View style={styles.row}>
          <View style={[styles.field, { flex: 1 }]}>
            <Text style={[styles.label, { color: colors.gray }]}>
              全体数量 (任意)
            </Text>
            <TextInput
              style={[
                styles.input,
                { borderColor: colors.separator, color: colors.black },
              ]}
              value={quantityTotal}
              onChangeText={(text) =>
                setQuantityTotal(text.replace(/[^0-9]/g, ''))
              }
              keyboardType="number-pad"
              placeholder="全体数量"
              placeholderTextColor={colors.grayLight}
            />
          </View>
          <View style={[styles.field, { flex: 1 }]}>
            <Text style={[styles.label, { color: colors.gray }]}>
              単位 (任意)
            </Text>
            <TextInput
              style={[
                styles.input,
                { borderColor: colors.separator, color: colors.black },
              ]}
              value={quantityUnit}
              onChangeText={setQuantityUnit}
              placeholder="ページ、個"
              placeholderTextColor={colors.grayLight}
            />
          </View>
        </View>

        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>
            abandonability: {abandonability.toFixed(2)}
          </Text>
          <Slider
            value={abandonability}
            onValueChange={setAbandonability}
            minimumValue={0}
            maximumValue={1}
            step={0.25}
            minimumTrackTintColor={colors.brand}
          />
        </View>

        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>並列設定</Text>
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
                color={colors.brand}
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
                color={colors.brand}
              />
            </Pressable>
          </View>
        </View>

        <View style={styles.field}>
          <View style={styles.toggleItem}>
            <Text style={[styles.toggleLabel, { color: colors.black }]}>
              時間固定
            </Text>
            <Checkbox
              status={fixed ? 'checked' : 'unchecked'}
              onPress={() => setFixed(!fixed)}
              color={colors.brand}
            />
          </View>
          <Text style={[styles.hint, { color: colors.grayLight }]}>
            開始時刻を固定し、スケジューラの移動を許可しない
          </Text>
        </View>

        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>説明</Text>
          <TextInput
            style={[
              styles.input,
              styles.multiline,
              { borderColor: colors.separator, color: colors.black },
            ]}
            value={description}
            onChangeText={setDescription}
            multiline
            placeholder="説明 (任意)"
            placeholderTextColor={colors.grayLight}
          />
        </View>

        {/* iCal import (foldable) */}
        <View style={styles.field}>
          <Pressable
            style={styles.icalHeader}
            onPress={() => {
              haptic.light();
              setShowIcal(!showIcal);
            }}
          >
            <Ionicons
              name={showIcal ? 'chevron-down' : 'chevron-forward'}
              size={18}
              color={colors.gray}
            />
            <Text style={[styles.label, { color: colors.gray, flex: 1 }]}>
              iCalインポート
            </Text>
          </Pressable>
          {showIcal && (
            <>
              <TextInput
                style={[
                  styles.input,
                  styles.multiline,
                  styles.icalInput,
                  { borderColor: colors.separator, color: colors.black },
                ]}
                value={icalText}
                onChangeText={setIcalText}
                multiline
                placeholder="BEGIN:VCALENDAR... を貼り付け"
                placeholderTextColor={colors.grayLight}
                autoCapitalize="none"
                autoCorrect={false}
              />
              <PressableScale
                style={[
                  styles.icalImportButton,
                  (!icalText.trim() || importing) && {
                    backgroundColor: colors.grayDark,
                  },
                ]}
                onPress={importIcal}
                disabled={!icalText.trim() || importing}
              >
                <Text style={[styles.saveButtonText, { color: colors.white }]}>
                  {importing ? 'インポート中…' : 'iCalをインポート'}
                </Text>
              </PressableScale>
            </>
          )}
        </View>

        {/* Dependencies */}
        <View style={styles.field}>
          <View style={styles.depHeader}>
            <Text style={[styles.label, { color: colors.gray }]}>
              依存先タスク ({selectedDeps.length})
            </Text>
            <PressableScale
              style={styles.addDepButton}
              onPress={() => {
                haptic.light();
                loadTasks();
                setDepSearch('');
                setShowDepPicker(true);
              }}
            >
              <Ionicons name="add" size={16} color={colors.white} />
              <Text style={[styles.addDepButtonText, { color: colors.white }]}>
                追加
              </Text>
            </PressableScale>
          </View>
          {selectedDeps.map((depId) => {
            const depTask = allTasks.find((t) => t.id === depId);
            return (
              <View
                key={depId}
                style={[
                  styles.depItem,
                  { backgroundColor: colors.surfaceTint },
                ]}
              >
                <Text style={[styles.depItemText, { color: colors.black }]}>
                  {depTask
                    ? `#${depTask.display_id} ${depTask.title}`
                    : depId.slice(0, 8)}
                </Text>
                <PressableScale
                  onPress={() => {
                    haptic.light();
                    setSelectedDeps(selectedDeps.filter((d) => d !== depId));
                  }}
                >
                  <Ionicons name="close" size={18} color={colors.red} />
                </PressableScale>
              </View>
            );
          })}
        </View>
      </ScrollView>

      {/* Dep picker overlay */}
      {showDepPicker && (
        <View style={[styles.depPicker, { backgroundColor: colors.white }]}>
          <View
            style={[
              styles.depPickerHeader,
              {
                borderBottomColor: colors.separator,
                paddingTop: 16 + (embedded ? 0 : insets.top),
              },
            ]}
          >
            <Text style={[styles.depPickerTitle, { color: colors.black }]}>
              依存先を選択
            </Text>
            <PressableScale
              onPress={() => {
                haptic.light();
                setShowDepPicker(false);
              }}
            >
              <Text style={styles.depPickerClose}>閉じる</Text>
            </PressableScale>
          </View>
          <View
            style={[
              styles.depSearchContainer,
              { borderBottomColor: colors.separator },
            ]}
          >
            <Ionicons name="search" size={18} color={colors.gray} />
            <TextInput
              style={[styles.depSearchInput, { color: colors.black }]}
              value={depSearch}
              onChangeText={setDepSearch}
              placeholder="タイトルで検索"
              placeholderTextColor={colors.grayLight}
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
          <ScrollView style={styles.depPickerList}>
            {allTasks
              .filter((t) => !selectedDeps.includes(t.id))
              .filter((t) =>
                depSearch.length === 0
                  ? true
                  : t.title.toLowerCase().includes(depSearch.toLowerCase()),
              )
              .map((t) => (
                <Pressable
                  key={t.id}
                  style={[
                    styles.depPickerItem,
                    { borderBottomColor: colors.separator },
                  ]}
                  onPress={() => {
                    haptic.medium();
                    setSelectedDeps([...selectedDeps, t.id]);
                    setShowDepPicker(false);
                  }}
                >
                  <Text
                    style={[styles.depPickerItemId, { color: colors.gray }]}
                  >
                    #{t.display_id}
                  </Text>
                  <Text
                    style={[styles.depPickerItemText, { color: colors.black }]}
                  >
                    {t.title}
                  </Text>
                </Pressable>
              ))}
          </ScrollView>
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
    </View>
  );
}
