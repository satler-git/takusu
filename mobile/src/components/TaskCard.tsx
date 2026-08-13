// TaskCard component — displays a single task in the list
// Left: start/end time, Center: title, Right-bottom: cost (avg, sigma)
// Background color based on abandonability
// Swipe toward start cycles: start → complete → revert (#312)
// Swipe toward end past the threshold reveals skip + delete action buttons (#1044)
// Done tasks: strikethrough + gray

import { memo, useCallback, useEffect, useMemo, useState } from 'react';
import {
  I18nManager,
  Pressable,
  StyleSheet,
  Text,
  View,
  type ViewStyle,
} from 'react-native';
import { GestureDetector, Gesture } from 'react-native-gesture-handler';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  useAnimatedReaction,
  runOnJS,
  withSpring,
} from 'react-native-reanimated';
import { Ionicons } from '@expo/vector-icons';
import { PressableScale } from '@/src/components/PressableScale';
import { CrossFadeIcon } from '@/src/components/CrossFadeIcon';
import type { TaskRow } from '@/src/api/types';
import { parseDepends } from '@/src/api/types';
import { taskCardColor, useTheme, type ColorSet, useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import { formatDuration } from '@/src/utils/duration';

interface TaskCardProps {
  task: TaskRow;
  scheduleStart?: string;
  scheduleEnd?: string;
  isDone: boolean;
  onPress: (task: TaskRow) => void;
  onDone?: (task: TaskRow) => void | Promise<void>;
  onStart?: (task: TaskRow) => void | Promise<void>;
  onComplete?: (task: TaskRow) => void | Promise<void>;
  onSkip?: (task: TaskRow) => void | Promise<void>;
  onDelete?: (task: TaskRow) => void | Promise<void>;
  onLongPress?: (task: TaskRow) => void;
  selected?: boolean;
  // Habit display_id for habit-based coloring (#309). Undefined when the
  // task has no habit or the habit map is unavailable.
  habitDisplayId?: number;
  // Number of tasks that depend on this task (reverse dependencies).
  dependentCount?: number;
  // Optional override for the outer container (e.g. to remove margins in a group).
  containerStyle?: ViewStyle;
}

function formatTime(iso?: string): string {
  if (!iso) return '--:--';
  const d = new Date(iso);
  return `${d.getHours().toString().padStart(2, '0')}:${d
    .getMinutes()
    .toString()
    .padStart(2, '0')}`;
}

// Format a deadline hint "〜M/D" when the task's deadline (end_at) falls on
// a different day than the scheduled start — i.e. a multi-day window
// (period-mode habits, #window_mode). Returns '' for same-day tasks.
function deadlineHint(task: TaskRow, scheduleStart?: string): string {
  if (!task.end_at || !scheduleStart) return '';
  const start = new Date(scheduleStart);
  const end = new Date(task.end_at);
  if (
    start.getFullYear() === end.getFullYear() &&
    start.getMonth() === end.getMonth() &&
    start.getDate() === end.getDate()
  ) {
    return '';
  }
  return `〜${end.getMonth() + 1}/${end.getDate()}`;
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      marginHorizontal: 12,
      marginVertical: 4,
      position: 'relative',
      overflow: 'hidden',
      borderRadius: 12,
    },
    card: {
      borderRadius: 12,
      minHeight: 72,
    },
    cardInner: {
      flexDirection: 'row',
      padding: 12,
      borderRadius: 12,
      minHeight: 72,
      alignItems: 'center',
      gap: 12,
      borderWidth: 2,
      borderColor: 'transparent',
    },
    cardSelected: {
      borderColor: colors.brand,
    },
    // Slide-right done preview background (#170)
    doneBg: {
      position: 'absolute',
      top: 0,
      left: 0,
      right: 0,
      bottom: 0,
      borderRadius: 12,
      justifyContent: 'center',
      alignItems: 'flex-start',
      paddingStart: 20,
    },
    // #1170: full-width background behind the action panel so over-sliding
    // left never reveals a gap between the card edge and the panel.
    actionPanelBg: {
      position: 'absolute',
      left: 0,
      right: 0,
      top: 0,
      bottom: 0,
    },
    // #1044: revealed skip/delete action panel behind the card
    actionPanel: {
      position: 'absolute',
      end: 0,
      top: 0,
      bottom: 0,
      flexDirection: 'row',
      borderTopEndRadius: 12,
      borderBottomEndRadius: 12,
      overflow: 'hidden',
    },
    // Revealed start/done action panel for scheduled tasks.
    startActionPanel: {
      position: 'absolute',
      start: 0,
      top: 0,
      bottom: 0,
      flexDirection: 'row',
      borderTopStartRadius: 12,
      borderBottomStartRadius: 12,
      overflow: 'hidden',
    },
    actionButton: {
      justifyContent: 'center',
      alignItems: 'center',
    },
    pressed: {
      opacity: 0.8,
    },
    times: {
      width: 48,
      alignItems: 'center',
      gap: 4,
    },
    timeText: {
      fontSize: 12,
      fontVariant: ['tabular-nums'],
    },
    titleContainer: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    title: {
      fontSize: 16,
      fontWeight: '500',
      flex: 1,
    },
    taskId: {
      fontSize: 11,
      fontVariant: ['tabular-nums'],
    },
    meta: {
      alignSelf: 'stretch',
      justifyContent: 'center',
      alignItems: 'flex-end',
      gap: 1,
    },
    metaText: {
      fontSize: 11,
      fontVariant: ['tabular-nums'],
    },
    deadlineHint: {
      fontSize: 10,
      fontVariant: ['tabular-nums'],
      textAlign: 'right',
      marginTop: 1,
    },
  });

function TaskCardImpl({
  task,
  scheduleStart,
  scheduleEnd,
  isDone,
  onPress,
  onDone,
  onStart,
  onComplete,
  onSkip,
  onDelete,
  onLongPress,
  selected,
  habitDisplayId,
  dependentCount,
  containerStyle,
}: TaskCardProps) {
  const translateX = useSharedValue(0);
  // Track which direction the haptic last fired for (0=none, 1=right, -1=left)
  // so reversing swipe direction mid-gesture re-fires the haptic (#313).
  const hapticFiredDir = useSharedValue(0);
  const { theme, colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  // #1044/#393: swipe left reveals an action panel with skip + delete buttons.
  // Use a SharedValue for the UI-thread worklet logic (avoids stale React
  // state in gesture callbacks) and mirror to React state for rendering.
  const actionsRevealedSV = useSharedValue(false);
  const [actionsRevealed, setActionsRevealed] = useState(false);
  const startActionsRevealedSV = useSharedValue(false);
  const [startActionsRevealed, setStartActionsRevealed] = useState(false);

  // Keep React state in sync with the UI-thread shared value so
  // pointerEvents updates as soon as the panel is revealed or hidden.
  useAnimatedReaction(
    () => actionsRevealedSV.value,
    (current, prev) => {
      if (current !== prev) {
        runOnJS(setActionsRevealed)(current);
      }
    },
    [],
  );
  useAnimatedReaction(
    () => startActionsRevealedSV.value,
    (current, prev) => {
      if (current !== prev) {
        runOnJS(setStartActionsRevealed)(current);
      }
    },
    [],
  );

  // Width of each action button and the full revealed panel.
  const ACTION_BUTTON_WIDTH = 72;
  const ACTION_PANEL_WIDTH =
    (onDelete ? ACTION_BUTTON_WIDTH : 0) +
    (onSkip && !isDone ? ACTION_BUTTON_WIDTH : 0);
  const REVEAL_THRESHOLD = 80;
  const START_ACTION_REVEAL_THRESHOLD = REVEAL_THRESHOLD * 2;
  const startActionsAvailable =
    task.status === 'scheduled' && Boolean(onStart && onComplete);
  const START_ACTION_PANEL_WIDTH = startActionsAvailable
    ? ACTION_BUTTON_WIDTH * 2
    : 0;

  // The task card's rounded end corners expose a small concave notch next
  // to the leading swipe action while the card is slid open. Widen the
  // action panel and extend the leading button 12dp under the card so its
  // background fills that notch (issue #1097).
  const CARD_BORDER_RADIUS = 12;
  const PANEL_WIDTH = ACTION_PANEL_WIDTH + CARD_BORDER_RADIUS;
  const START_PANEL_WIDTH = START_ACTION_PANEL_WIDTH + CARD_BORDER_RADIUS;
  const skipVisible = onSkip && !isDone;
  const isRTL = I18nManager.isRTL;
  const startSign = isRTL ? -1 : 1;
  const panelOffset = isRTL ? ACTION_PANEL_WIDTH : -ACTION_PANEL_WIDTH;
  const startPanelOffset = startSign * START_ACTION_PANEL_WIDTH;
  const startActionStyle = {
    width: ACTION_BUTTON_WIDTH + CARD_BORDER_RADIUS,
    paddingStart: CARD_BORDER_RADIUS,
  };
  const endActionStyle = { width: ACTION_BUTTON_WIDTH };
  const startPanelActionStyle = { width: ACTION_BUTTON_WIDTH };
  const startPanelDoneStyle = {
    width: ACTION_BUTTON_WIDTH + CARD_BORDER_RADIUS,
    paddingEnd: CARD_BORDER_RADIUS,
  };

  useEffect(() => {
    if (startActionsAvailable) return;
    startActionsRevealedSV.value = false;
    setStartActionsRevealed(false);
    translateX.value = withSpring(0);
  }, [startActionsAvailable, startActionsRevealedSV, translateX]);

  // Single pan gesture handles swipe toward start (done) and swipe toward end
  // (actions). Using Gesture.Race with two separate pans was unreliable for
  // the end-direction swipe (#230): Race resolution between gestures with
  // activeOffsetX in opposite directions can fail to activate. A single
  // gesture with bidirectional activeOffsetX avoids the issue entirely.
  const pan = Gesture.Pan()
    // testID allows threshold behavior to be covered without a device.
    .withTestId(`task-card-pan-${task.id}`)
    .activeOffsetX([-10, 10])
    .failOffsetY([-10, 10])
    .onUpdate((e) => {
      // If already revealed, start from the revealed position.
      const base = actionsRevealedSV.value
        ? panelOffset
        : startActionsRevealedSV.value
          ? startPanelOffset
          : 0;
      translateX.value = base + e.translationX;
      // Fire haptic when crossing the action threshold mid-slide (#313).
      // Suppress haptics when actions are revealed — no action will fire
      // regardless of swipe direction.
      if (
        startSign * e.translationX > REVEAL_THRESHOLD &&
        onDone &&
        hapticFiredDir.value !== startSign &&
        !actionsRevealedSV.value &&
        !startActionsRevealedSV.value
      ) {
        hapticFiredDir.value = startSign;
        runOnJS(haptic.light)();
      } else if (
        -startSign * e.translationX > REVEAL_THRESHOLD &&
        ACTION_PANEL_WIDTH > 0 &&
        hapticFiredDir.value !== -startSign &&
        !actionsRevealedSV.value &&
        !startActionsRevealedSV.value
      ) {
        hapticFiredDir.value = -startSign;
        runOnJS(haptic.medium)();
      }
    })
    .onEnd((e) => {
      if (actionsRevealedSV.value) {
        // When actions are revealed, swipe back toward the start to hide.
        // Don't trigger onDone even if the swipe passes the threshold.
        if (isRTL ? e.translationX < 20 : e.translationX > -20) {
          actionsRevealedSV.value = false;
          translateX.value = withSpring(0);
        } else {
          translateX.value = withSpring(panelOffset);
        }
      } else if (startActionsRevealedSV.value) {
        // When the start actions are revealed, swipe toward the end to hide.
        if (isRTL ? e.translationX > -20 : e.translationX < 20) {
          startActionsRevealedSV.value = false;
          translateX.value = withSpring(0);
        } else {
          translateX.value = withSpring(startPanelOffset);
        }
      } else if (
        startActionsAvailable &&
        startSign * e.translationX > START_ACTION_REVEAL_THRESHOLD
      ) {
        // Keep the existing threshold behavior for normal slides, but reveal
        // explicit Start/Done actions when a scheduled task is over-slid.
        startActionsRevealedSV.value = true;
        translateX.value = withSpring(startPanelOffset);
      } else if (startSign * e.translationX > REVEAL_THRESHOLD && onDone) {
        runOnJS(onDone)(task);
        translateX.value = withSpring(0);
      } else if (
        -startSign * e.translationX > REVEAL_THRESHOLD &&
        ACTION_PANEL_WIDTH > 0
      ) {
        // Reveal the skip/delete action panel.
        actionsRevealedSV.value = true;
        translateX.value = withSpring(panelOffset);
      } else {
        translateX.value = withSpring(0);
      }
    })
    // onFinalize fires for both END and CANCELLED terminal states, ensuring
    // hapticFiredDir is always reset even if the gesture is interrupted.
    // Only snap to resting position when the gesture was cancelled (not
    // when it ended normally — onEnd already handles that) to avoid
    // restarting the spring animation.
    .onFinalize((_e, success) => {
      hapticFiredDir.value = 0;
      if (!success) {
        translateX.value = withSpring(
          actionsRevealedSV.value
            ? panelOffset
            : startActionsRevealedSV.value
              ? startPanelOffset
              : 0,
        );
      }
    });

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: translateX.value }],
  }));

  // Background preview opacity for swipe toward start (done) action (#170).
  const doneBgStyle = useAnimatedStyle(() => ({
    opacity: Math.min(
      1,
      Math.max(0, (startSign * translateX.value) / REVEAL_THRESHOLD),
    ),
  }));

  // #1170: action panel background only visible while sliding toward end.
  // Keeping it transparent during the start-swipe done transition prevents
  // the skip/delete color from bleeding through the partial-opacity doneBg.
  const actionPanelBgStyle = useAnimatedStyle(() => ({
    opacity: Math.min(
      1,
      Math.max(0, (-startSign * translateX.value) / REVEAL_THRESHOLD),
    ),
  }));

  const bgColor = taskCardColor(
    task.abandonability,
    task.habit_id,
    habitDisplayId,
    theme,
  );
  const deps = parseDepends(task.depends);

  // Slide-right background preview: icon and color depend on what the
  // next state in the cycle will be (#312).
  // pending → completed (checkmark, green)
  // scheduled → in_progress (play, blue), in_progress → completed (check, green),
  // completed → scheduled (refresh, red)
  const isPending = task.status === 'pending';
  const isInProgress = task.status === 'in_progress';
  const doneIcon = isDone
    ? 'refresh'
    : isPending
      ? 'checkmark'
      : isInProgress
        ? 'checkmark'
        : 'play';
  const doneColor = isDone
    ? colors.red
    : isPending
      ? colors.green
      : isInProgress
        ? colors.green
        : colors.brand;

  const handlePress = () => {
    if (actionsRevealed || startActionsRevealed) {
      // Tapping the card when actions are revealed snaps it back.
      haptic.light();
      actionsRevealedSV.value = false;
      startActionsRevealedSV.value = false;
      setActionsRevealed(false);
      setStartActionsRevealed(false);
      translateX.value = withSpring(0);
      return;
    }
    haptic.light();
    onPress(task);
  };
  const handleLongPress = onLongPress
    ? () => {
        if (actionsRevealed || startActionsRevealed) {
          haptic.light();
          actionsRevealedSV.value = false;
          startActionsRevealedSV.value = false;
          setActionsRevealed(false);
          setStartActionsRevealed(false);
          translateX.value = withSpring(0);
          return;
        }
        haptic.medium();
        onLongPress(task);
      }
    : undefined;

  return (
    <View style={[styles.container, containerStyle]}>
      {/* #1170: full-width background behind the action panel so over-sliding
          left never reveals a gap. Opacity animates with translateX so it is
          only visible while sliding left — the right-swipe done transition
          is unaffected. */}
      {ACTION_PANEL_WIDTH > 0 && (
        <Reanimated.View
          style={[
            styles.actionPanelBg,
            { backgroundColor: skipVisible ? colors.gray : colors.red },
            actionPanelBgStyle,
          ]}
          pointerEvents="none"
        />
      )}
      {/* Slide-right done preview background (#170) */}
      <Reanimated.View
        style={[styles.doneBg, { backgroundColor: doneColor }, doneBgStyle]}
        pointerEvents="none"
      >
        <CrossFadeIcon name={doneIcon} size={28} color={colors.white} />
      </Reanimated.View>
      {/* Revealed start/done action panel for scheduled tasks. */}
      {startActionsAvailable && (
        <View
          style={[styles.startActionPanel, { width: START_PANEL_WIDTH }]}
          pointerEvents={startActionsRevealed ? 'auto' : 'none'}
        >
          <PressableScale
            style={[
              styles.actionButton,
              startPanelActionStyle,
              { backgroundColor: colors.brand },
            ]}
            accessibilityRole="button"
            accessibilityLabel="タスクを開始"
            onPress={() => {
              haptic.light();
              startActionsRevealedSV.value = false;
              setStartActionsRevealed(false);
              translateX.value = withSpring(0);
              onStart?.(task);
            }}
          >
            <Ionicons name="play" size={24} color={colors.white} />
          </PressableScale>
          <PressableScale
            style={[
              styles.actionButton,
              startPanelDoneStyle,
              { backgroundColor: colors.green },
            ]}
            accessibilityRole="button"
            accessibilityLabel="タスクを完了"
            onPress={() => {
              haptic.medium();
              startActionsRevealedSV.value = false;
              setStartActionsRevealed(false);
              translateX.value = withSpring(0);
              onComplete?.(task);
            }}
          >
            <Ionicons name="checkmark" size={24} color={colors.white} />
          </PressableScale>
        </View>
      )}
      {/* #1044: revealed skip/delete action panel */}
      {ACTION_PANEL_WIDTH > 0 && (
        <View
          style={[styles.actionPanel, { width: PANEL_WIDTH }]}
          pointerEvents={actionsRevealed ? 'auto' : 'none'}
        >
          {skipVisible && (
            <PressableScale
              style={[
                styles.actionButton,
                startActionStyle,
                { backgroundColor: colors.gray },
              ]}
              onPress={() => {
                haptic.warning();
                actionsRevealedSV.value = false;
                setActionsRevealed(false);
                translateX.value = withSpring(0);
                onSkip(task);
              }}
            >
              <Ionicons
                name="play-skip-forward-outline"
                size={24}
                color={colors.white}
              />
            </PressableScale>
          )}
          {onDelete && (
            <PressableScale
              style={[
                styles.actionButton,
                skipVisible ? endActionStyle : startActionStyle,
                { backgroundColor: colors.red },
              ]}
              onPress={() => {
                haptic.medium();
                actionsRevealedSV.value = false;
                setActionsRevealed(false);
                translateX.value = withSpring(0);
                onDelete(task);
              }}
            >
              <Ionicons name="trash" size={24} color={colors.white} />
            </PressableScale>
          )}
        </View>
      )}
      <GestureDetector gesture={pan}>
        <Reanimated.View
          testID={`task-card-${task.id}`}
          style={[styles.card, { backgroundColor: bgColor }, animatedStyle]}
        >
          <Pressable
            style={({ pressed }) => [
              styles.cardInner,
              pressed && styles.pressed,
              selected && styles.cardSelected,
              isInProgress && {
                borderStartColor: colors.brand,
                borderStartWidth: 4,
              },
            ]}
            onPress={handlePress}
            onLongPress={handleLongPress}
          >
            {/* Left: times */}
            <View style={styles.times}>
              <Text
                style={[styles.timeText, { color: colors.textOnCardSecondary }]}
              >
                {formatTime(scheduleStart)}
              </Text>
              <Text
                style={[styles.timeText, { color: colors.textOnCardSecondary }]}
              >
                {formatTime(scheduleEnd)}
              </Text>
            </View>

            {/* Center: title */}
            <View style={styles.titleContainer}>
              <Text
                style={[styles.taskId, { color: colors.textOnCardSecondary }]}
              >
                {task.habit_id && habitDisplayId !== undefined
                  ? `h${habitDisplayId}#${task.display_id}`
                  : `#${task.display_id}`}
              </Text>
              <Text
                style={[
                  styles.title,
                  { color: colors.textOnCard },
                  isDone && {
                    textDecorationLine: 'line-through',
                    color: colors.textOnCardDone,
                  },
                ]}
                numberOfLines={2}
              >
                {task.title}
              </Text>
            </View>

            {/* Right: deps, dependents, and cost stacked vertically */}
            <View style={styles.meta}>
              {task.quantity_total != null && task.quantity_total > 0 && (
                <Text
                  style={[
                    styles.metaText,
                    { color: colors.textOnCardSecondary },
                  ]}
                >
                  {task.quantity_done}/{task.quantity_total}
                </Text>
              )}
              {deps.length > 0 && (
                <Text
                  style={[
                    styles.metaText,
                    { color: colors.textOnCardSecondary },
                  ]}
                >
                  ↳ {deps.length}
                </Text>
              )}
              {(dependentCount ?? 0) > 0 && (
                <Text
                  style={[
                    styles.metaText,
                    { color: colors.textOnCardSecondary },
                  ]}
                >
                  ↗ {dependentCount}
                </Text>
              )}
              {task.avg_minutes > 0 && (
                <Text
                  style={[
                    styles.metaText,
                    { color: colors.textOnCardSecondary },
                  ]}
                >
                  {`${formatDuration(task.avg_minutes)} ±${formatDuration(
                    task.sigma_minutes,
                  )}`}
                </Text>
              )}
              {(() => {
                const hint = deadlineHint(task, scheduleStart);
                return hint ? (
                  <Text
                    style={[
                      styles.deadlineHint,
                      { color: colors.textOnCardSecondary },
                    ]}
                  >
                    {hint}
                  </Text>
                ) : null;
              })()}
            </View>
          </Pressable>
        </Reanimated.View>
      </GestureDetector>
    </View>
  );
}

export const TaskCard = memo(TaskCardImpl);

// ── ParallelGroupCard ──
// Renders a parallel task group as a rotated "L": host on top, a thin
// vertical rail (same color as the host) extending down, and guests
// indented on the right as normal TaskCards. Each task keeps its own
// 3-state swipe gesture; the rail is static and does not move (#573).

const RAIL_WIDTH = 10;
const OUTLINE_WIDTH = 1;
const INDENT_WIDTH = RAIL_WIDTH + OUTLINE_WIDTH;

interface ParallelGroupCardProps {
  host: TaskRow;
  guests: TaskRow[];
  hostScheduleStart?: string;
  hostScheduleEnd?: string;
  guestScheduleStarts: (string | undefined)[];
  guestScheduleEnds: (string | undefined)[];
  selected?: boolean;
  onHostPress: (host: TaskRow) => void;
  onGuestPress: (guest: TaskRow) => void;
  onToggle: (allIds: string[]) => void;
  onDone?: (task: TaskRow) => void | Promise<void>;
  onStart?: (task: TaskRow) => void | Promise<void>;
  onComplete?: (task: TaskRow) => void | Promise<void>;
  onSkip?: (task: TaskRow) => void | Promise<void>;
  onDelete?: (task: TaskRow) => void | Promise<void>;
  // habit_id → display_id map for habit-based coloring (#309).
  habitDisplayIdMap?: Map<string, number>;
  // task_id → number of tasks that depend on it (reverse dependency count).
  dependentCountMap?: Map<string, number>;
}

function ParallelGroupCardImpl({
  host,
  guests,
  hostScheduleStart,
  hostScheduleEnd,
  guestScheduleStarts,
  guestScheduleEnds,
  selected,
  onHostPress,
  onGuestPress,
  onToggle,
  onDone,
  onStart,
  onComplete,
  onSkip,
  onDelete,
  habitDisplayIdMap,
  dependentCountMap,
}: ParallelGroupCardProps) {
  const colors = useColors();
  const { theme } = useTheme();
  const hostHabitDisplayId = host.habit_id
    ? habitDisplayIdMap?.get(host.habit_id)
    : undefined;
  const hostBgColor = taskCardColor(
    host.abandonability,
    host.habit_id,
    hostHabitDisplayId,
    theme,
  );
  const outlineColor = colors.cardOutline;

  const allIds = useMemo(
    () => [host.id, ...guests.map((g) => g.id)],
    [host.id, guests],
  );

  const handleGroupLongPress = useCallback(
    (_task: TaskRow) => {
      onToggle(allIds);
    },
    [onToggle, allIds],
  );

  return (
    <View
      style={[groupStyles.container, selected && { borderColor: colors.brand }]}
    >
      <View
        style={[
          groupStyles.rail,
          { backgroundColor: hostBgColor, borderEndColor: outlineColor },
        ]}
      />
      <View style={groupStyles.cards}>
        <TaskCard
          task={host}
          scheduleStart={hostScheduleStart}
          scheduleEnd={hostScheduleEnd}
          isDone={host.status === 'completed' || host.status === 'skipped'}
          onPress={onHostPress}
          onDone={onDone}
          onStart={onStart}
          onComplete={onComplete}
          onSkip={onSkip}
          onDelete={onDelete}
          onLongPress={handleGroupLongPress}
          habitDisplayId={hostHabitDisplayId}
          dependentCount={dependentCountMap?.get(host.id)}
          containerStyle={groupStyles.groupCard}
        />
        {guests.map((guest, idx) => {
          const guestHabitDisplayId = guest.habit_id
            ? habitDisplayIdMap?.get(guest.habit_id)
            : undefined;
          return (
            <TaskCard
              key={guest.id}
              task={guest}
              scheduleStart={guestScheduleStarts[idx]}
              scheduleEnd={guestScheduleEnds[idx]}
              isDone={
                guest.status === 'completed' || guest.status === 'skipped'
              }
              onPress={onGuestPress}
              onDone={onDone}
              onStart={onStart}
              onComplete={onComplete}
              onSkip={onSkip}
              onDelete={onDelete}
              onLongPress={handleGroupLongPress}
              habitDisplayId={guestHabitDisplayId}
              dependentCount={dependentCountMap?.get(guest.id)}
              containerStyle={groupStyles.groupCard}
            />
          );
        })}
      </View>
    </View>
  );
}

const groupStyles = StyleSheet.create({
  container: {
    marginHorizontal: 12,
    marginVertical: 4,
    borderTopStartRadius: 6,
    borderTopEndRadius: 12,
    borderBottomStartRadius: 6,
    borderBottomEndRadius: 12,
    overflow: 'hidden',
    borderWidth: 2,
    borderColor: 'transparent',
    position: 'relative',
    minHeight: 72,
  },
  rail: {
    position: 'absolute',
    start: 0,
    top: 0,
    bottom: 0,
    width: INDENT_WIDTH,
    borderTopStartRadius: 4,
    borderTopEndRadius: 4,
    borderBottomStartRadius: 4,
    borderBottomEndRadius: 4,
    borderEndWidth: OUTLINE_WIDTH,
  },
  cards: {
    flexDirection: 'column',
  },
  groupCard: {
    marginHorizontal: 0,
    marginVertical: 0,
    marginStart: INDENT_WIDTH,
  },
});

export const ParallelGroupCard = memo(ParallelGroupCardImpl);
