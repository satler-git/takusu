// NavigationButtons — right side floating, vertically arranged
// From top to bottom:
//   ⏫ (scroll up by day — fast)
//   ↑  (scroll up by page)
//   ↓  (scroll down by page)
//   ⏬ (scroll down by day — fast)
//   Calendar button (opens calendar overlay)

import { useState, useMemo } from 'react';
import { Modal, Pressable, StyleSheet, Text, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors } from '@/src/theme';
import { type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

interface NavigationButtonsProps {
  onScrollUpByDay?: () => void;
  onScrollUpByPage?: () => void;
  onScrollDownByDay?: () => void;
  onScrollDownByPage?: () => void;
  onJumpToDate?: (date: Date) => void;
  markedDates?: Set<string>; // YYYY-MM-DD
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      position: 'absolute',
      right: 8,
      top: '40%',
      transform: [{ translateY: -100 }],
      gap: 4,
      zIndex: 10,
    },
    navButton: {
      width: 36,
      height: 36,
      borderRadius: 18,
      alignItems: 'center',
      justifyContent: 'center',
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 1 },
      shadowOpacity: 0.2,
      shadowRadius: 2,
      elevation: 2,
    },
    navButtonPressed: {
      opacity: 0.7,
    },
    overlay: {
      flex: 1,
      backgroundColor: colors.overlay,
      justifyContent: 'center',
      alignItems: 'center',
    },
    calendar: {
      borderRadius: 16,
      padding: 16,
      width: 300,
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 4 },
      shadowOpacity: 0.3,
      shadowRadius: 8,
      elevation: 8,
    },
    calHeader: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      marginBottom: 12,
    },
    calNavButton: {
      width: 44,
      height: 44,
      borderRadius: 22,
      alignItems: 'center',
      justifyContent: 'center',
    },
    calNav: {
      fontSize: 32,
      lineHeight: 36,
    },
    calMonthLabel: {
      fontSize: 16,
      fontWeight: '600',
    },
    calMonthLabelPressable: {
      flex: 1,
      alignItems: 'center',
      justifyContent: 'center',
      paddingVertical: 8,
    },
    calGrid: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 2,
    },
    calWeekday: {
      width: 36,
      textAlign: 'center',
      fontSize: 12,
      fontWeight: '600',
    },
    calDay: {
      width: 36,
      height: 36,
      alignItems: 'center',
      justifyContent: 'center',
      borderRadius: 8,
    },
    calDayText: {
      fontSize: 14,
    },
  });

export function NavigationButtons({
  onScrollUpByDay,
  onScrollUpByPage,
  onScrollDownByDay,
  onScrollDownByPage,
  onJumpToDate,
  markedDates,
}: NavigationButtonsProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);

  function NavButton({
    icon,
    onPress,
    color,
    bgColor,
  }: {
    icon: keyof typeof Ionicons.glyphMap;
    onPress?: () => void;
    color: string;
    bgColor: string;
  }) {
    return (
      <Pressable
        style={({ pressed }) => [
          styles.navButton,
          { backgroundColor: bgColor },
          pressed && styles.navButtonPressed,
        ]}
        onPress={() => {
          if (onPress) {
            haptic.light();
            onPress();
          }
        }}
      >
        <Ionicons name={icon} size={20} color={color} />
      </Pressable>
    );
  }

  const [calendarOpen, setCalendarOpen] = useState(false);
  const [calMonth, setCalMonth] = useState(() => {
    const now = new Date();
    return new Date(now.getFullYear(), now.getMonth(), 1);
  });

  const monthLabel = `${calMonth.getFullYear()}年 ${calMonth.getMonth() + 1}月`;
  const daysInMonth = new Date(
    calMonth.getFullYear(),
    calMonth.getMonth() + 1,
    0,
  ).getDate();
  const firstDay = new Date(
    calMonth.getFullYear(),
    calMonth.getMonth(),
    1,
  ).getDay();
  const today = new Date();

  function prevMonth() {
    haptic.select();
    setCalMonth(new Date(calMonth.getFullYear(), calMonth.getMonth() - 1, 1));
  }
  function nextMonth() {
    haptic.select();
    setCalMonth(new Date(calMonth.getFullYear(), calMonth.getMonth() + 1, 1));
  }

  function jumpToCurrentMonth() {
    haptic.select();
    const now = new Date();
    now.setHours(0, 0, 0, 0);
    now.setDate(1);
    setCalMonth(now);
  }

  function selectDay(day: number) {
    haptic.medium();
    const date = new Date(calMonth.getFullYear(), calMonth.getMonth(), day);
    onJumpToDate?.(date);
    setCalendarOpen(false);
  }

  function dateKey(day: number): string {
    const d = new Date(calMonth.getFullYear(), calMonth.getMonth(), day);
    return `${d.getFullYear()}-${(d.getMonth() + 1)
      .toString()
      .padStart(2, '0')}-${d.getDate().toString().padStart(2, '0')}`;
  }

  return (
    <>
      <View style={styles.container}>
        <NavButton
          icon="arrow-up"
          onPress={onScrollUpByDay}
          color={colors.brand}
          bgColor={colors.surface}
        />
        <NavButton
          icon="chevron-up"
          onPress={onScrollUpByPage}
          color={colors.brand}
          bgColor={colors.surface}
        />
        <NavButton
          icon="chevron-down"
          onPress={onScrollDownByPage}
          color={colors.brand}
          bgColor={colors.surface}
        />
        <NavButton
          icon="arrow-down"
          onPress={onScrollDownByDay}
          color={colors.brand}
          bgColor={colors.surface}
        />
        <NavButton
          icon="calendar"
          onPress={() => {
            haptic.light();
            setCalendarOpen(true);
          }}
          color={colors.brand}
          bgColor={colors.surface}
        />
      </View>

      <Modal
        visible={calendarOpen}
        transparent
        animationType="fade"
        onRequestClose={() => setCalendarOpen(false)}
      >
        <Pressable
          style={styles.overlay}
          onPress={() => setCalendarOpen(false)}
        >
          {/* Inner Pressable stops tap-through so tapping white space inside
              the calendar no longer dismisses it (Issue #30). */}
          <Pressable
            style={[styles.calendar, { backgroundColor: colors.white }]}
            onPress={() => {}}
          >
            <View style={styles.calHeader}>
              <Pressable
                onPress={prevMonth}
                style={styles.calNavButton}
                hitSlop={8}
              >
                <Text style={[styles.calNav, { color: colors.brand }]}>‹</Text>
              </Pressable>
              <Pressable
                onPress={jumpToCurrentMonth}
                style={styles.calMonthLabelPressable}
                hitSlop={8}
              >
                <Text style={[styles.calMonthLabel, { color: colors.black }]}>
                  {monthLabel}
                </Text>
              </Pressable>
              <Pressable
                onPress={nextMonth}
                style={styles.calNavButton}
                hitSlop={8}
              >
                <Text style={[styles.calNav, { color: colors.brand }]}>›</Text>
              </Pressable>
            </View>
            <View style={styles.calGrid}>
              {['日', '月', '火', '水', '木', '金', '土'].map((d) => (
                <Text
                  key={d}
                  style={[styles.calWeekday, { color: colors.gray }]}
                >
                  {d}
                </Text>
              ))}
              {Array.from({ length: firstDay }).map((_, i) => (
                <View key={`empty-${i}`} style={styles.calDay} />
              ))}
              {Array.from({ length: daysInMonth }).map((_, i) => {
                const day = i + 1;
                const marked = markedDates?.has(dateKey(day));
                const isToday =
                  day === today.getDate() &&
                  calMonth.getMonth() === today.getMonth() &&
                  calMonth.getFullYear() === today.getFullYear();
                return (
                  <Pressable
                    key={day}
                    style={[
                      styles.calDay,
                      marked && { backgroundColor: colors.brand },
                      isToday && {
                        borderRadius: 18,
                        borderWidth: 1.5,
                        borderColor: marked ? colors.white : colors.brand,
                      },
                    ]}
                    onPress={() => selectDay(day)}
                  >
                    <Text
                      style={[
                        styles.calDayText,
                        { color: colors.black },
                        marked && { color: colors.white, fontWeight: '600' },
                      ]}
                    >
                      {day}
                    </Text>
                  </Pressable>
                );
              })}
            </View>
          </Pressable>
        </Pressable>
      </Modal>
    </>
  );
}
