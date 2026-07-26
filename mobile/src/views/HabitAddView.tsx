// HabitAddView — create a new habit
// Fields: title, recurrence (RRULE), cost (avg, sigma), abandonability

import { useState, useMemo } from 'react';
import {
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { useRouter } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import { Checkbox, SegmentedButtons } from 'react-native-paper';
import { Slider } from '@expo/ui/community/slider';
import { useServer } from '@/src/api/ServerProvider';
import { undoRedo } from '@/src/api/undoRedo';
import { showError } from '@/src/api/errors';
import { useColors, type ColorSet } from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { RruleBuilderModal } from '@/src/components/RruleBuilderModal';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { HabitStepEditor } from '@/src/components/HabitStepEditor';
import { haptic } from '@/src/components/haptics';
import { parseDuration } from '@/src/utils/duration';
import { type StepDraft, saveHabitSteps } from '@/src/utils/habitSteps';
import {
  defaultRule,
  parseRule,
  serializeRule,
  summarizeRule,
} from '@/src/api/rrule';
import type { WindowMode } from '@/src/api/types';
import { WINDOW_MODE_DAY, WINDOW_MODE_PERIOD } from '@/src/api/types';

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
  });

export function HabitAddView() {
  const { client } = useServer();
  const router = useRouter();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();

  const [title, setTitle] = useState('');
  const [recurrence, setRecurrence] = useState(serializeRule(defaultRule()));
  const [showRruleBuilder, setShowRruleBuilder] = useState(false);
  const [startTime, setStartTime] = useState('09:00');
  const [endTime, setEndTime] = useState('10:00');
  const [avgMinutes, setAvgMinutes] = useState('60');
  const [sigmaMinutes, setSigmaMinutes] = useState('');
  const [abandonability, setAbandonability] = useState(0.5);
  const [parallelizable, setParallelizable] = useState(false);
  const [allowsParallel, setAllowsParallel] = useState(false);
  const [fixed, setFixed] = useState(false);
  const [windowMode, setWindowMode] = useState<WindowMode>(WINDOW_MODE_DAY);
  const [stepDrafts, setStepDrafts] = useState<StepDraft[]>([]);
  const [saving, setSaving] = useState(false);
  const [pickerField, setPickerField] = useState<'start' | 'end' | null>(null);

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

  async function create() {
    if (!client || !title || saving) return;
    haptic.medium();
    setSaving(true);
    const avg = parseDuration(avgMinutes) || 60;
    const sigmaRaw = parseDuration(sigmaMinutes);
    // sigma=0/未入力の時は未送信にしてサーバーの auto (avg/5) に任せる
    const sigma = sigmaRaw !== null && sigmaRaw > 0 ? sigmaRaw : undefined;
    let createdHabit: { id: string } | null = null;
    try {
      const habit = await client.createHabit({
        title,
        recurrence,
        start_time: startTime,
        end_time: endTime,
        avg_minutes: avg,
        sigma_minutes: sigma,
        abandonability,
        parallelizable,
        allows_parallel: allowsParallel,
        fixed,
        window_mode: windowMode,
      });
      createdHabit = habit;
      // Save steps if any (#95). Habit body fields are ignored server-side
      // when steps exist, but we still send them for the simple case.
      if (stepDrafts.length > 0) {
        await saveHabitSteps(client, habit.id, stepDrafts);
      }
      let currentId = habit.id;
      undoRedo.push({
        description: `create habit: ${title}`,
        undo: async () => {
          await client.deleteHabit(currentId);
        },
        redo: async () => {
          const recreated = await client.createHabit({
            title,
            recurrence,
            start_time: startTime,
            end_time: endTime,
            avg_minutes: avg,
            sigma_minutes: sigma,
            abandonability,
            parallelizable,
            allows_parallel: allowsParallel,
            fixed,
            window_mode: windowMode,
          });
          currentId = recreated.id;
          if (stepDrafts.length > 0) {
            await saveHabitSteps(client, recreated.id, stepDrafts);
          }
        },
      });
      router.back();
    } catch (e) {
      // If the habit was created but step-save failed, roll back the
      // orphaned habit so the server isn't left in a partial state.
      if (createdHabit) {
        await client.deleteHabit(createdHabit.id).catch(() => {});
      }
      showError(e, 'Habitの追加に失敗');
    } finally {
      setSaving(false);
    }
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View style={[styles.topBar, { paddingTop: 8 + insets.top }]}>
        <Pressable
          style={styles.backButton}
          onPress={() => {
            haptic.light();
            router.back();
          }}
        >
          <Ionicons name="chevron-back" size={28} color={colors.brand} />
        </Pressable>
        <Text style={[styles.title, { color: colors.black }]}>New Habit</Text>
        <View style={{ flex: 1 }} />
        <Pressable
          style={[
            styles.saveButton,
            (!title || saving) && { backgroundColor: colors.grayDark },
          ]}
          onPress={create}
          disabled={!title || saving}
        >
          <Text style={[styles.saveButtonText, { color: colors.white }]}>
            {saving ? '保存中…' : '追加'}
          </Text>
        </Pressable>
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
            placeholder="Habit name"
            placeholderTextColor={colors.grayLight}
          />
        </View>

        <View style={styles.field}>
          <View style={styles.rruleHeader}>
            <Text style={[styles.label, { color: colors.gray }]}>
              周期 (RRULE)
            </Text>
            <Pressable
              style={styles.helpButton}
              onPress={() => {
                haptic.light();
                setShowRruleBuilder(true);
              }}
              hitSlop={8}
            >
              <Ionicons
                name="help-circle-outline"
                size={18}
                color={colors.brand}
              />
            </Pressable>
          </View>
          <Pressable
            style={[
              styles.dateField,
              { borderColor: colors.separator, backgroundColor: colors.white },
            ]}
            onPress={() => {
              haptic.light();
              setShowRruleBuilder(true);
            }}
          >
            <Ionicons name="repeat" size={20} color={colors.brand} />
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
        </View>

        {/* Window mode (#window_mode) */}
        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>
            スケジュール枠
          </Text>
          <SegmentedButtons
            value={windowMode}
            onValueChange={(v) => setWindowMode(v as WindowMode)}
            buttons={[
              { value: WINDOW_MODE_DAY, label: '当日' },
              { value: WINDOW_MODE_PERIOD, label: '期間内どこでも' },
            ]}
            theme={{ colors: { primary: colors.brand } }}
          />
          {windowMode === WINDOW_MODE_PERIOD && (
            <Text style={[styles.hint, { color: colors.grayLight }]}>
              次の周期の直前が締め切りになります
              {stepDrafts.length > 0 && '・全ステップが期間枠を共有します'}
            </Text>
          )}
        </View>

        <View style={styles.row}>
          <View style={[styles.field, { flex: 1 }]}>
            <Text style={[styles.label, { color: colors.gray }]}>開始時刻</Text>
            <Pressable
              style={[
                styles.dateField,
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
              <Ionicons name="time-outline" size={20} color={colors.brand} />
              <Text style={[styles.dateText, { color: colors.black }]}>
                {startTime}
              </Text>
            </Pressable>
          </View>
          <View style={[styles.field, { flex: 1 }]}>
            <Text style={[styles.label, { color: colors.gray }]}>終了時刻</Text>
            <Pressable
              style={[
                styles.dateField,
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
              <Ionicons name="time-outline" size={20} color={colors.brand} />
              <Text style={[styles.dateText, { color: colors.black }]}>
                {endTime}
              </Text>
            </Pressable>
          </View>
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
              <Pressable
                style={[styles.maximizeButton, { borderColor: colors.brand }]}
                onPress={() => {
                  const [sh, sm] = startTime
                    .split(':')
                    .map((n) => parseInt(n, 10) || 0);
                  const [eh, em] = endTime
                    .split(':')
                    .map((n) => parseInt(n, 10) || 0);
                  let diffMin = eh * 60 + em - (sh * 60 + sm);
                  if (diffMin <= 0) diffMin += 24 * 60; // overnight
                  if (diffMin <= 0) return;
                  haptic.light();
                  setAvgMinutes(String(Math.max(1, diffMin)));
                }}
              >
                <Ionicons name="expand" size={16} color={colors.brand} />
              </Pressable>
            </View>
            {(() => {
              const [sh, sm] = startTime
                .split(':')
                .map((n) => parseInt(n, 10) || 0);
              const [eh, em] = endTime
                .split(':')
                .map((n) => parseInt(n, 10) || 0);
              let diffMin = eh * 60 + em - (sh * 60 + sm);
              if (diffMin <= 0) diffMin += 24 * 60;
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

        {/* Steps (#95) */}
        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>ステップ</Text>
          {stepDrafts.length > 0 && (
            <Text style={[styles.hint, { color: colors.grayLight }]}>
              ステップ設定が優先されます (habit 本体の時間帯・コストは無効)
            </Text>
          )}
          <HabitStepEditor
            drafts={stepDrafts}
            onChange={setStepDrafts}
            stepsActive={stepDrafts.length > 0}
          />
        </View>
      </ScrollView>

      <RruleBuilderModal
        visible={showRruleBuilder}
        value={recurrence}
        onConfirm={(json) => {
          setRecurrence(json);
          setShowRruleBuilder(false);
        }}
        onCancel={() => setShowRruleBuilder(false)}
      />

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
    </View>
  );
}
