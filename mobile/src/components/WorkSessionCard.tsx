// WorkSessionCard — displays an open work session above the task list.
// Tap opens the progress sheet for this session; swipe toward start completes it;
// swipe toward end reveals a pause button. Multiple sessions can be open at once
// (#1419), so each card independently targets its own session.

import { memo, useEffect, useMemo, useState } from 'react';
import { I18nManager, Pressable, StyleSheet, Text, View } from 'react-native';
import { GestureDetector, Gesture } from 'react-native-gesture-handler';
import Reanimated, {
  useSharedValue,
  useAnimatedStyle,
  useAnimatedReaction,
  runOnJS,
  withSpring,
} from 'react-native-reanimated';
import { Ionicons } from '@expo/vector-icons';
import type { WorkSessionRow } from '@/src/api/types';
import { useTheme, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

interface WorkSessionCardProps {
  session: WorkSessionRow;
  // display_id of the linked task, if any (for the "#N" label).
  taskDisplayId?: number;
  onPress: (session: WorkSessionRow) => void;
  onComplete: (session: WorkSessionRow) => void | Promise<void>;
  onPause: (session: WorkSessionRow) => void | Promise<void>;
}

function formatElapsed(ms: number): string {
  const totalSecs = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSecs / 3600);
  const m = Math.floor((totalSecs % 3600) / 60);
  const s = totalSecs % 60;
  const pad = (n: number) => n.toString().padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
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
      borderStartWidth: 4,
      borderStartColor: colors.brand,
    },
    cardInner: {
      padding: 12,
      gap: 8,
    },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
    },
    taskId: {
      fontSize: 11,
      fontVariant: ['tabular-nums'],
      color: colors.textOnCardSecondary,
    },
    title: {
      flex: 1,
      fontSize: 15,
      fontWeight: '600',
      color: colors.black,
    },
    timer: {
      fontSize: 13,
      fontWeight: '700',
      fontVariant: ['tabular-nums'],
      color: colors.black,
    },
    qty: {
      fontSize: 11,
      fontVariant: ['tabular-nums'],
      color: colors.textOnCardSecondary,
    },
    progressTrack: {
      flex: 1,
      height: 4,
      borderRadius: 999,
      backgroundColor: colors.separator,
      overflow: 'hidden',
    },
    progressFill: {
      height: '100%',
      borderRadius: 999,
      backgroundColor: colors.brand,
    },
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
    actionPanelBg: {
      position: 'absolute',
      left: 0,
      right: 0,
      top: 0,
      bottom: 0,
    },
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
    actionButton: {
      justifyContent: 'center',
      alignItems: 'center',
    },
  });

const ACTION_BUTTON_WIDTH = 72;
const CARD_BORDER_RADIUS = 12;
const PANEL_WIDTH = ACTION_BUTTON_WIDTH + CARD_BORDER_RADIUS;
const REVEAL_THRESHOLD = 80;

function WorkSessionCardImpl({
  session,
  taskDisplayId,
  onPress,
  onComplete,
  onPause,
}: WorkSessionCardProps) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const translateX = useSharedValue(0);
  const hapticFiredDir = useSharedValue(0);
  const actionsRevealedSV = useSharedValue(false);
  const [actionsRevealed, setActionsRevealed] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  const isRTL = I18nManager.isRTL;
  const startSign = isRTL ? -1 : 1;
  const panelOffset = isRTL ? ACTION_BUTTON_WIDTH : -ACTION_BUTTON_WIDTH;

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  useAnimatedReaction(
    () => actionsRevealedSV.value,
    (current, prev) => {
      if (current !== prev) {
        runOnJS(setActionsRevealed)(current);
      }
    },
    [],
  );

  const pan = Gesture.Pan()
    .activeOffsetX([-10, 10])
    .failOffsetY([-10, 10])
    .onUpdate((e) => {
      const base = actionsRevealedSV.value ? panelOffset : 0;
      translateX.value = base + e.translationX;
      if (
        startSign * e.translationX > REVEAL_THRESHOLD &&
        hapticFiredDir.value !== startSign &&
        !actionsRevealedSV.value
      ) {
        hapticFiredDir.value = startSign;
        runOnJS(haptic.light)();
      } else if (
        -startSign * e.translationX > REVEAL_THRESHOLD &&
        hapticFiredDir.value !== -startSign &&
        !actionsRevealedSV.value
      ) {
        hapticFiredDir.value = -startSign;
        runOnJS(haptic.medium)();
      }
    })
    .onEnd((e) => {
      if (actionsRevealedSV.value) {
        if (isRTL ? e.translationX < 20 : e.translationX > -20) {
          actionsRevealedSV.value = false;
          translateX.value = withSpring(0);
        } else {
          translateX.value = withSpring(panelOffset);
        }
      } else if (startSign * e.translationX > REVEAL_THRESHOLD) {
        runOnJS(onComplete)(session);
        translateX.value = withSpring(0);
      } else if (-startSign * e.translationX > REVEAL_THRESHOLD) {
        actionsRevealedSV.value = true;
        translateX.value = withSpring(panelOffset);
      } else {
        translateX.value = withSpring(0);
      }
    })
    .onFinalize((_e, success) => {
      hapticFiredDir.value = 0;
      if (!success) {
        translateX.value = withSpring(
          actionsRevealedSV.value ? panelOffset : 0,
        );
      }
    });

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: translateX.value }],
  }));
  const doneBgStyle = useAnimatedStyle(() => ({
    opacity: Math.min(
      1,
      Math.max(0, (startSign * translateX.value) / REVEAL_THRESHOLD),
    ),
  }));
  const actionPanelBgStyle = useAnimatedStyle(() => ({
    opacity: Math.min(
      1,
      Math.max(0, (-startSign * translateX.value) / REVEAL_THRESHOLD),
    ),
  }));
  // The pause action panel is always rendered behind the card. Hide it
  // completely at rest so the red background doesn't bleed through the
  // rounded right edge. Fade it in as the card slides toward the end.
  const actionPanelStyle = useAnimatedStyle(() => ({
    opacity: Math.min(
      1,
      Math.max(0, (-startSign * translateX.value) / ACTION_BUTTON_WIDTH),
    ),
  }));

  const elapsed = now - new Date(session.started_at).getTime();
  const total = session.quantity_total ?? 0;
  const done = session.quantity_done ?? 0;
  const progress = total > 0 ? Math.min(1, done / total) : 0;

  const handlePress = () => {
    if (actionsRevealed) {
      haptic.light();
      actionsRevealedSV.value = false;
      setActionsRevealed(false);
      translateX.value = withSpring(0);
      return;
    }
    haptic.light();
    onPress(session);
  };

  return (
    // The action panel is absolute and the card is inside a GestureDetector
    // wrapper. With the new architecture's view flattening, the absolute panel
    // can render on top of the card's wrapper child. Keep this view from
    // collapsing so sibling source order (and thus z-order) is respected.
    <View style={styles.container} collapsable={false}>
      <Reanimated.View
        style={[
          styles.actionPanelBg,
          { backgroundColor: colors.red },
          actionPanelBgStyle,
        ]}
        pointerEvents="none"
      />
      <Reanimated.View
        style={[styles.doneBg, { backgroundColor: colors.green }, doneBgStyle]}
        pointerEvents="none"
      >
        <Ionicons name="checkmark" size={28} color={colors.white} />
      </Reanimated.View>
      <Reanimated.View
        style={[styles.actionPanel, { width: PANEL_WIDTH }, actionPanelStyle]}
        pointerEvents={actionsRevealed ? 'auto' : 'none'}
      >
        <Pressable
          style={[
            styles.actionButton,
            {
              width: ACTION_BUTTON_WIDTH + CARD_BORDER_RADIUS,
              paddingStart: CARD_BORDER_RADIUS,
              backgroundColor: colors.red,
            },
          ]}
          onPress={() => {
            haptic.medium();
            actionsRevealedSV.value = false;
            setActionsRevealed(false);
            translateX.value = withSpring(0);
            onPause(session);
          }}
        >
          <Ionicons name="pause" size={24} color={colors.white} />
        </Pressable>
      </Reanimated.View>
      <GestureDetector gesture={pan}>
        <Reanimated.View
          style={[
            styles.card,
            { backgroundColor: colors.surfaceTint },
            animatedStyle,
          ]}
        >
          <Pressable onPress={handlePress}>
            <View style={styles.cardInner}>
              <View style={styles.row}>
                {taskDisplayId !== undefined && (
                  <Text style={styles.taskId}>#{taskDisplayId}</Text>
                )}
                <Text style={styles.title} numberOfLines={1}>
                  {session.title ?? '作業'}
                </Text>
                <Text style={styles.timer}>{formatElapsed(elapsed)}</Text>
              </View>
              <View style={styles.row}>
                {total > 0 && (
                  <Text style={styles.qty}>
                    {done}/{total}
                  </Text>
                )}
                <View style={styles.progressTrack}>
                  <View
                    style={[
                      styles.progressFill,
                      { width: `${progress * 100}%` },
                    ]}
                  />
                </View>
              </View>
            </View>
          </Pressable>
        </Reanimated.View>
      </GestureDetector>
    </View>
  );
}

export const WorkSessionCard = memo(WorkSessionCardImpl);
