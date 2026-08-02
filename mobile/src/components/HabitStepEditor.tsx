// HabitStepEditor — accordion-based editor for a habit's steps (#95).
//
// Shared by HabitDetailView (edit mode) and HabitAddView. Renders one
// expandable card per step with the full field set (title, time window,
// cost, parallel flags, abandonability, fixed, depends), plus reorder
// (up/down) and delete controls and an "+ add step" button.
//
// Controlled: the parent owns the `drafts` array and persists it on save
// via `saveHabitSteps`. Depends references use `tempId` so new (unsaved)
// steps can be referenced before the server assigns real ids.

import { useState, useMemo } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { Checkbox, TextInput as PaperTextInput } from 'react-native-paper';
import { Slider } from '@expo/ui/community/slider';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { DeleteConfirmButton } from '@/src/components/DeleteConfirmButton';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { parseDuration, formatDuration } from '@/src/utils/duration';
import { type StepDraft, newStepDraft, hasCycle } from '@/src/utils/habitSteps';

interface HabitStepEditorProps {
  drafts: StepDraft[];
  onChange: (drafts: StepDraft[]) => void;
  // When true, the habit body's time/cost fields are shown as overridden
  // (rendered by the parent; this prop is currently unused but reserved
  // for future hint text).
  stepsActive: boolean;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      gap: 8,
    },
    stepCard: {
      borderRadius: 10,
      borderWidth: 1,
      padding: 10,
    },
    stepHeader: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
    },
    foldButton: {
      padding: 2,
    },
    stepHeaderTap: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    stepIndex: {
      fontSize: 16,
      fontWeight: '700',
      minWidth: 20,
    },
    stepTitle: {
      flex: 1,
      fontSize: 15,
      fontWeight: '500',
    },
    stepTime: {
      fontSize: 12,
      fontVariant: ['tabular-nums'],
    },
    stepHeaderActions: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
    },
    stepBody: {
      marginTop: 8,
      gap: 10,
    },
    row: {
      flexDirection: 'row',
      gap: 10,
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
    timeField: {
      flex: 1,
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 10,
      paddingVertical: 8,
      gap: 2,
    },
    timeFieldLabel: {
      fontSize: 11,
      fontWeight: '500',
    },
    timeFieldValue: {
      fontSize: 15,
    },
    hint: {
      fontSize: 11,
      marginTop: 2,
    },
    label: {
      fontSize: 13,
      fontWeight: '500',
    },
    sliderRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
    },
    slider: {
      flex: 1,
    },
    sliderValue: {
      fontSize: 13,
      fontVariant: ['tabular-nums'],
    },
    toggleRow: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 16,
    },
    toggleItem: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
    },
    toggleLabel: {
      fontSize: 13,
    },
    depSection: {
      gap: 4,
    },
    depItem: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      paddingVertical: 2,
    },
    depLabel: {
      fontSize: 14,
    },
    addButton: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'center',
      gap: 6,
      borderWidth: 1,
      borderStyle: 'dashed',
      borderRadius: 10,
      paddingVertical: 10,
    },
    addButtonText: {
      color: colors.brand,
      fontSize: 14,
      fontWeight: '500',
    },
  });

export function HabitStepEditor({ drafts, onChange }: HabitStepEditorProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [pickerField, setPickerField] = useState<{
    tempId: string;
    field: 'start' | 'end';
  } | null>(null);

  function toggle(tempId: string) {
    haptic.light();
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(tempId)) next.delete(tempId);
      else next.add(tempId);
      return next;
    });
  }

  function update(tempId: string, patch: Partial<StepDraft>) {
    onChange(drafts.map((d) => (d.tempId === tempId ? { ...d, ...patch } : d)));
  }

  function addStep() {
    haptic.medium();
    const pos = drafts.length;
    const draft = newStepDraft(pos);
    onChange([...drafts, draft]);
    setExpanded((prev) => new Set(prev).add(draft.tempId));
  }

  function moveStep(tempId: string, dir: -1 | 1) {
    haptic.light();
    const idx = drafts.findIndex((d) => d.tempId === tempId);
    if (idx < 0) return;
    const target = idx + dir;
    if (target < 0 || target >= drafts.length) return;
    const next = [...drafts];
    const [moved] = next.splice(idx, 1);
    next.splice(target, 0, moved!);
    // Reassign positions to match the new order.
    onChange(next.map((d, i) => ({ ...d, position: i })));
  }

  function deleteStep(tempId: string) {
    const draft = drafts.find((d) => d.tempId === tempId);
    if (!draft) return;
    haptic.medium();
    const filtered = drafts
      .filter((d) => d.tempId !== tempId)
      .map((d, i) => ({ ...d, position: i }));
    // Remove deleted tempId from any depends_on.
    const cleaned = filtered.map((d) => ({
      ...d,
      depends_on: d.depends_on.filter((t) => t !== tempId),
    }));
    onChange(cleaned);
    setExpanded((prev) => {
      const next = new Set(prev);
      next.delete(tempId);
      return next;
    });
  }

  function toggleDep(tempId: string, depTempId: string) {
    const draft = drafts.find((d) => d.tempId === tempId);
    if (!draft) return;
    const has = draft.depends_on.includes(depTempId);
    if (has) {
      update(tempId, {
        depends_on: draft.depends_on.filter((t) => t !== depTempId),
      });
      return;
    }
    // Would adding this dep create a cycle? Temporarily add and check.
    const trial = drafts.map((d) =>
      d.tempId === tempId
        ? { ...d, depends_on: [...d.depends_on, depTempId] }
        : d,
    );
    if (hasCycle(trial)) {
      haptic.medium();
      return; // silently reject — the checkbox stays unchecked
    }
    haptic.light();
    update(tempId, { depends_on: [...draft.depends_on, depTempId] });
  }

  function timeStringToDate(s: string): Date {
    const [h, m] = s.split(':').map((n) => parseInt(n, 10) || 0);
    const d = new Date();
    d.setHours(h, m, 0, 0);
    return d;
  }
  function dateToTimeString(d: Date): string {
    return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;
  }
  function windowMinutes(start: string, end: string): number {
    const s = timeStringToDate(start);
    const e = timeStringToDate(end);
    let diffMs = e.getTime() - s.getTime();
    if (diffMs <= 0) diffMs += 24 * 60 * 60 * 1000;
    return Math.round(diffMs / 60000);
  }

  return (
    <View style={styles.container}>
      {drafts.map((d, idx) => {
        const isOpen = expanded.has(d.tempId);
        const depLabels = d.depends_on
          .map((t) => drafts.find((x) => x.tempId === t))
          .filter(Boolean)
          .map((x) => stepLabel(drafts.indexOf(x!), x!));
        const windowMin = windowMinutes(d.start_time, d.end_time);
        const canMaximize = windowMin > 0;
        return (
          <View
            key={d.tempId}
            style={[
              styles.stepCard,
              {
                backgroundColor: colors.surface,
                borderColor: colors.separator,
              },
            ]}
          >
            <View style={styles.stepHeader}>
              {/* Fold toggle — leftmost in the card (#390) */}
              <Pressable
                onPress={() => toggle(d.tempId)}
                hitSlop={8}
                style={styles.foldButton}
              >
                <Ionicons
                  name={isOpen ? 'chevron-up' : 'chevron-down'}
                  size={20}
                  color={colors.gray}
                />
              </Pressable>
              <Pressable
                style={styles.stepHeaderTap}
                onPress={() => toggle(d.tempId)}
              >
                <Text style={[styles.stepIndex, { color: colors.brand }]}>
                  {idx + 1}
                </Text>
                <Text
                  style={[styles.stepTitle, { color: colors.black }]}
                  numberOfLines={1}
                >
                  {d.title || '(無題)'}
                </Text>
                <Text style={[styles.stepTime, { color: colors.gray }]}>
                  {d.start_time}-{d.end_time}
                </Text>
              </Pressable>
              <View style={styles.stepHeaderActions}>
                <Pressable
                  onPress={() => moveStep(d.tempId, -1)}
                  disabled={idx === 0}
                  hitSlop={8}
                >
                  <Ionicons
                    name="chevron-up"
                    size={20}
                    color={idx === 0 ? colors.grayLight : colors.brand}
                  />
                </Pressable>
                <Pressable
                  onPress={() => moveStep(d.tempId, 1)}
                  disabled={idx === drafts.length - 1}
                  hitSlop={8}
                >
                  <Ionicons
                    name="chevron-down"
                    size={20}
                    color={
                      idx === drafts.length - 1
                        ? colors.grayLight
                        : colors.brand
                    }
                  />
                </Pressable>
                <DeleteConfirmButton
                  onConfirm={() => deleteStep(d.tempId)}
                  size={34}
                  iconSize={20}
                  hitSlop={8}
                />
              </View>
            </View>

            {isOpen && (
              <View style={styles.stepBody}>
                {/* Title */}
                <PaperTextInput
                  mode="outlined"
                  value={d.title}
                  onChangeText={(v) => update(d.tempId, { title: v })}
                  label="タイトル"
                  outlineColor={colors.separator}
                  activeOutlineColor={colors.brand}
                  dense
                />

                {/* Time window */}
                <View style={styles.row}>
                  <Pressable
                    style={[
                      styles.timeField,
                      { borderColor: colors.separator },
                    ]}
                    onPress={() => {
                      haptic.select();
                      setPickerField({ tempId: d.tempId, field: 'start' });
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
                      {d.start_time}
                    </Text>
                  </Pressable>
                  <Pressable
                    style={[
                      styles.timeField,
                      { borderColor: colors.separator },
                    ]}
                    onPress={() => {
                      haptic.select();
                      setPickerField({ tempId: d.tempId, field: 'end' });
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
                      {d.end_time}
                    </Text>
                  </Pressable>
                </View>

                {/* Cost */}
                <View style={styles.row}>
                  <View style={{ flex: 1 }}>
                    <View style={styles.rowWithButton}>
                      <PaperTextInput
                        mode="outlined"
                        label="avg"
                        value={String(d.avg_minutes)}
                        onChangeText={(v) => {
                          const parsed = parseDuration(v);
                          if (parsed !== null && parsed > 0)
                            update(d.tempId, { avg_minutes: parsed });
                          else if (v === '')
                            update(d.tempId, { avg_minutes: 0 });
                        }}
                        outlineColor={colors.separator}
                        activeOutlineColor={colors.brand}
                        style={styles.inputWithButton}
                        dense
                      />
                      <Pressable
                        style={[
                          styles.maximizeButton,
                          { borderColor: colors.brand },
                          !canMaximize && styles.maximizeButtonDisabled,
                        ]}
                        onPress={() => {
                          if (!canMaximize) return;
                          haptic.light();
                          update(d.tempId, { avg_minutes: windowMin });
                        }}
                        disabled={!canMaximize}
                      >
                        <Ionicons
                          name="expand"
                          size={16}
                          color={colors.brand}
                        />
                      </Pressable>
                    </View>
                    {canMaximize && (
                      <Text style={[styles.hint, { color: colors.grayLight }]}>
                        window: {windowMin}分
                      </Text>
                    )}
                  </View>
                  <PaperTextInput
                    mode="outlined"
                    label="sigma"
                    value={d.sigma_minutes > 0 ? String(d.sigma_minutes) : ''}
                    onChangeText={(v) => {
                      const parsed = parseDuration(v);
                      update(d.tempId, {
                        sigma_minutes: parsed !== null ? parsed : 0,
                      });
                    }}
                    outlineColor={colors.separator}
                    activeOutlineColor={colors.brand}
                    style={{ flex: 1 }}
                    dense
                  />
                </View>
                {d.sigma_minutes === 0 && (
                  <Text style={[styles.hint, { color: colors.grayLight }]}>
                    sigma: {Math.max(1, Math.round(d.avg_minutes / 5))}m (avg/5)
                  </Text>
                )}

                {/* Abandonability */}
                <View style={styles.sliderRow}>
                  <Text style={[styles.label, { color: colors.gray }]}>
                    abandonability
                  </Text>
                  <Slider
                    value={d.abandonability}
                    onValueChange={(v) =>
                      update(d.tempId, { abandonability: v })
                    }
                    minimumValue={0}
                    maximumValue={1}
                    step={0.25}
                    minimumTrackTintColor={colors.brand}
                    style={styles.slider}
                  />
                  <Text style={[styles.sliderValue, { color: colors.brand }]}>
                    {d.abandonability.toFixed(2)}
                  </Text>
                </View>

                {/* Parallel + fixed */}
                <View style={styles.toggleRow}>
                  <Pressable
                    style={styles.toggleItem}
                    onPress={() =>
                      update(d.tempId, { parallelizable: !d.parallelizable })
                    }
                  >
                    <Text style={[styles.toggleLabel, { color: colors.black }]}>
                      並列実行可能
                    </Text>
                    <Checkbox
                      status={d.parallelizable ? 'checked' : 'unchecked'}
                      onPress={() =>
                        update(d.tempId, { parallelizable: !d.parallelizable })
                      }
                      color={colors.brand}
                    />
                  </Pressable>
                  <Pressable
                    style={styles.toggleItem}
                    onPress={() =>
                      update(d.tempId, { allows_parallel: !d.allows_parallel })
                    }
                  >
                    <Text style={[styles.toggleLabel, { color: colors.black }]}>
                      並列受け入れ
                    </Text>
                    <Checkbox
                      status={d.allows_parallel ? 'checked' : 'unchecked'}
                      onPress={() =>
                        update(d.tempId, {
                          allows_parallel: !d.allows_parallel,
                        })
                      }
                      color={colors.brand}
                    />
                  </Pressable>
                  <Pressable
                    style={styles.toggleItem}
                    onPress={() => update(d.tempId, { fixed: !d.fixed })}
                  >
                    <Text style={[styles.toggleLabel, { color: colors.black }]}>
                      時間固定
                    </Text>
                    <Checkbox
                      status={d.fixed ? 'checked' : 'unchecked'}
                      onPress={() => update(d.tempId, { fixed: !d.fixed })}
                      color={colors.brand}
                    />
                  </Pressable>
                </View>

                {/* Depends */}
                <View style={styles.depSection}>
                  <Text style={[styles.label, { color: colors.gray }]}>
                    依存 (DAG)
                  </Text>
                  {drafts.length <= 1 ? (
                    <Text style={[styles.hint, { color: colors.grayLight }]}>
                      他のステップを追加すると依存を設定できます
                    </Text>
                  ) : (
                    drafts.map((other) => {
                      if (other.tempId === d.tempId) return null;
                      const checked = d.depends_on.includes(other.tempId);
                      // Would checking this create a cycle?
                      const wouldCycle = (() => {
                        if (checked) return false;
                        const trial = drafts.map((x) =>
                          x.tempId === d.tempId
                            ? {
                                ...x,
                                depends_on: [...x.depends_on, other.tempId],
                              }
                            : x,
                        );
                        return hasCycle(trial);
                      })();
                      return (
                        <Pressable
                          key={other.tempId}
                          style={[
                            styles.depItem,
                            wouldCycle && { opacity: 0.4 },
                          ]}
                          disabled={wouldCycle}
                          onPress={() => toggleDep(d.tempId, other.tempId)}
                        >
                          <Checkbox
                            status={checked ? 'checked' : 'unchecked'}
                            disabled={wouldCycle}
                            color={colors.brand}
                          />
                          <Text
                            style={[styles.depLabel, { color: colors.black }]}
                          >
                            {stepLabel(drafts.indexOf(other), other)}
                          </Text>
                        </Pressable>
                      );
                    })
                  )}
                  {depLabels.length > 0 && (
                    <Text style={[styles.hint, { color: colors.grayLight }]}>
                      依存先: {depLabels.join(', ')}
                    </Text>
                  )}
                </View>
              </View>
            )}
          </View>
        );
      })}

      <Pressable
        style={[styles.addButton, { borderColor: colors.brand }]}
        onPress={addStep}
      >
        <Ionicons name="add" size={20} color={colors.brand} />
        <Text style={styles.addButtonText}>ステップを追加</Text>
      </Pressable>

      <DateTimePickerModal
        visible={pickerField !== null}
        mode="time"
        label={pickerField?.field === 'start' ? '開始時刻' : '終了時刻'}
        value={
          pickerField
            ? timeStringToDate(
                drafts.find((d) => d.tempId === pickerField.tempId)?.[
                  pickerField.field === 'start' ? 'start_time' : 'end_time'
                ] ?? '09:00',
              )
            : new Date()
        }
        onConfirm={(date) => {
          if (date && pickerField) {
            const s = dateToTimeString(date);
            update(pickerField.tempId, {
              [pickerField.field === 'start' ? 'start_time' : 'end_time']: s,
            } as Partial<StepDraft>);
          }
          setPickerField(null);
        }}
        onCancel={() => setPickerField(null)}
      />
    </View>
  );
}

function stepLabel(idx: number, d: StepDraft): string {
  return `${idx + 1}. ${d.title || '(無題)'}`;
}

// Re-export for callers that need to format a step's cost.
export { formatDuration };
