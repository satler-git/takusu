// DateTimePickerModal — modal wrapper around @react-native-community/datetimepicker
// Allows picking a date and time (or date-only) via the native Android picker.
// On Android, the native picker shows as a dialog; we wrap it in a Modal for
// consistent UX and provide a "clear" button for optional fields.

import { useEffect, useState, useMemo } from 'react';
import {
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import DateTimePicker, {
  type DateTimePickerEvent,
} from '@react-native-community/datetimepicker';
import { Ionicons } from '@expo/vector-icons';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useTheme, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

interface DateTimePickerModalProps {
  visible: boolean;
  value: Date | null;
  mode: 'date' | 'datetime' | 'time';
  label: string;
  onConfirm: (date: Date | null) => void;
  onCancel: () => void;
  optional?: boolean;
  minimumDate?: Date;
  /** Quick-pick shortcut chips shown above the field rows.
   *  Each entry sets tempDate to the result of `compute()` when tapped.
   *  When omitted, no shortcut row is rendered. */
  shortcuts?: Array<{
    label: string;
    compute: (current: Date | null) => Date;
  }>;
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
      paddingBottom: 32,
    },
    header: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      marginBottom: 16,
    },
    title: {
      fontSize: 18,
      fontWeight: '600',
    },
    shortcutsRow: {
      marginBottom: 12,
    },
    shortcutsContent: {
      gap: 8,
      paddingEnd: 20,
    },
    shortcutChip: {
      paddingHorizontal: 12,
      paddingVertical: 6,
      borderRadius: 14,
      borderWidth: 1,
    },
    shortcutText: {
      fontSize: 13,
      fontWeight: '500',
    },
    fieldRow: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingVertical: 14,
      paddingHorizontal: 12,
      borderRadius: 10,
      marginBottom: 8,
      gap: 8,
    },
    fieldLabel: {
      fontSize: 15,
      flex: 1,
    },
    fieldValue: {
      fontSize: 15,
      fontWeight: '500',
    },
    clearButton: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      paddingVertical: 10,
      alignSelf: 'flex-start',
    },
    clearText: {
      fontSize: 14,
    },
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

export function DateTimePickerModal({
  visible,
  value,
  mode,
  label,
  onConfirm,
  onCancel,
  optional,
  minimumDate,
  shortcuts,
}: DateTimePickerModalProps) {
  const [tempDate, setTempDate] = useState<Date>(value ?? new Date());
  const [pickerMode, setPickerMode] = useState<'date' | 'time'>(
    mode === 'time' ? 'time' : 'date',
  );
  const [showPicker, setShowPicker] = useState(false);
  const insets = useSafeAreaInsets();
  const { dark, colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);

  // Sync tempDate with value prop when modal opens
  useEffect(() => {
    if (visible) {
      setTempDate(value ?? new Date());
    }
  }, [visible, value]);

  function openPicker(pMode: 'date' | 'time') {
    haptic.select();
    setPickerMode(pMode);
    setShowPicker(true);
  }

  function handlePickerChange(event: DateTimePickerEvent, selected?: Date) {
    setShowPicker(false);
    if (event.type === 'set' && selected) {
      setTempDate(selected);
    }
  }

  function formatDate(d: Date): string {
    return `${d.getFullYear()}/${(d.getMonth() + 1).toString().padStart(2, '0')}/${d.getDate().toString().padStart(2, '0')}`;
  }

  function formatTime(d: Date): string {
    return `${d.getHours().toString().padStart(2, '0')}:${d.getMinutes().toString().padStart(2, '0')}`;
  }

  function formatDisplay(d: Date | null): string {
    if (!d) return '未設定';
    if (mode === 'datetime') {
      return `${formatDate(d)} ${formatTime(d)}`;
    }
    if (mode === 'time') {
      return formatTime(d);
    }
    return formatDate(d);
  }

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={onCancel}
    >
      <Pressable style={styles.overlay} onPress={onCancel}>
        <Pressable
          style={[
            styles.sheet,
            {
              backgroundColor: colors.white,
              paddingBottom: 32 + insets.bottom,
            },
          ]}
          onPress={(e) => e.stopPropagation()}
        >
          <View style={styles.header}>
            <Text style={[styles.title, { color: colors.black }]}>{label}</Text>
            <Pressable
              onPress={() => {
                haptic.light();
                onCancel();
              }}
            >
              <Ionicons name="close" size={24} color={colors.gray} />
            </Pressable>
          </View>

          {shortcuts && shortcuts.length > 0 && (
            <ScrollView
              horizontal
              showsHorizontalScrollIndicator={false}
              style={styles.shortcutsRow}
              contentContainerStyle={styles.shortcutsContent}
            >
              {shortcuts.map((sc, i) => {
                // Check minimumDate at render time so we can visually
                // disable chips that would produce an invalid date. The
                // actual guard is also applied at tap time (below) to avoid
                // any stale-closure drift.
                const preview = sc.compute(tempDate);
                const wouldViolate =
                  minimumDate !== undefined && preview < minimumDate;
                return (
                  <Pressable
                    key={`${sc.label}-${i}`}
                    disabled={wouldViolate}
                    style={[
                      styles.shortcutChip,
                      {
                        borderColor: wouldViolate
                          ? colors.grayLight
                          : colors.separator,
                      },
                    ]}
                    onPress={() => {
                      const candidate = sc.compute(tempDate);
                      if (
                        minimumDate !== undefined &&
                        candidate < minimumDate
                      ) {
                        return;
                      }
                      haptic.light();
                      setTempDate(candidate);
                    }}
                  >
                    <Text
                      style={[
                        styles.shortcutText,
                        {
                          color: wouldViolate ? colors.grayLight : colors.brand,
                        },
                      ]}
                    >
                      {sc.label}
                    </Text>
                  </Pressable>
                );
              })}
            </ScrollView>
          )}

          {mode === 'time' ? (
            <Pressable
              style={[styles.fieldRow, { backgroundColor: colors.surfaceTint }]}
              onPress={() => openPicker('time')}
            >
              <Ionicons name="time-outline" size={20} color={colors.brand} />
              <Text style={[styles.fieldLabel, { color: colors.grayDark }]}>
                時刻
              </Text>
              <Text style={[styles.fieldValue, { color: colors.black }]}>
                {formatDisplay(tempDate)}
              </Text>
              <Ionicons
                name="chevron-forward"
                size={18}
                color={colors.grayLight}
              />
            </Pressable>
          ) : (
            <>
              <Pressable
                style={[
                  styles.fieldRow,
                  { backgroundColor: colors.surfaceTint },
                ]}
                onPress={() => openPicker('date')}
              >
                <Ionicons
                  name="calendar-outline"
                  size={20}
                  color={colors.brand}
                />
                <Text style={[styles.fieldLabel, { color: colors.grayDark }]}>
                  日付
                </Text>
                <Text style={[styles.fieldValue, { color: colors.black }]}>
                  {formatDate(tempDate)}
                </Text>
                <Ionicons
                  name="chevron-forward"
                  size={18}
                  color={colors.grayLight}
                />
              </Pressable>

              {mode === 'datetime' && (
                <Pressable
                  style={[
                    styles.fieldRow,
                    { backgroundColor: colors.surfaceTint },
                  ]}
                  onPress={() => openPicker('time')}
                  disabled={!tempDate}
                >
                  <Ionicons
                    name="time-outline"
                    size={20}
                    color={tempDate ? colors.brand : colors.grayLight}
                  />
                  <Text
                    style={[
                      styles.fieldLabel,
                      { color: colors.grayDark },
                      !tempDate && { color: colors.grayLight },
                    ]}
                  >
                    時間
                  </Text>
                  <Text style={[styles.fieldValue, { color: colors.black }]}>
                    {tempDate ? formatTime(tempDate) : '—'}
                  </Text>
                  <Ionicons
                    name="chevron-forward"
                    size={18}
                    color={tempDate ? colors.gray : colors.grayLight}
                  />
                </Pressable>
              )}
            </>
          )}

          {optional && (
            <Pressable
              style={styles.clearButton}
              onPress={() => {
                haptic.light();
                onConfirm(null);
              }}
            >
              <Ionicons name="trash-outline" size={16} color={colors.red} />
              <Text style={[styles.clearText, { color: colors.red }]}>
                クリア
              </Text>
            </Pressable>
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
            <Pressable
              style={styles.confirmButton}
              onPress={() => {
                haptic.medium();
                onConfirm(tempDate);
              }}
            >
              <Text style={[styles.confirmText, { color: colors.white }]}>
                設定
              </Text>
            </Pressable>
          </View>

          {showPicker && (
            <DateTimePicker
              value={tempDate}
              mode={pickerMode}
              display="default"
              onChange={handlePickerChange}
              minimumDate={minimumDate}
              timeZoneName={undefined}
              themeVariant={dark ? 'dark' : 'light'}
            />
          )}
        </Pressable>
      </Pressable>
    </Modal>
  );
}
