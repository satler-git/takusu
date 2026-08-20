import { memo, useMemo, type ComponentProps } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { PressableScale } from '@/src/components/PressableScale';
import { haptic } from '@/src/components/haptics';
import { useTheme, type ColorSet } from '@/src/theme';
import type { TaskCard, WorkState } from '@/src/api/agentTypes';

interface CurrentTaskCardProps {
  card: TaskCard;
  loading?: boolean;
  onStart: () => void;
  onPause: () => void;
  onProgress: () => void;
  onComplete: () => void;
  onDelay: () => void;
  onConsult: () => void;
}

function formatTimeWindow(startAt?: string, endAt?: string): string {
  if (!startAt && !endAt) return '予定なし';
  const format = (s: string) =>
    new Date(s).toLocaleTimeString('ja-JP', {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    });
  if (startAt && endAt) return `${format(startAt)} – ${format(endAt)}`;
  return startAt ? format(startAt) : endAt ? format(endAt) : '';
}

function stateLabel(state: WorkState): string {
  switch (state) {
    case 'not_started':
      return '未着手';
    case 'in_progress':
      return '作業中';
    case 'overdue':
      return '遅延';
  }
}

function authorityLabel(authority: 'candidate' | 'today_covered'): string {
  return authority === 'today_covered' ? '今やること' : '候補';
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      marginHorizontal: 12,
      marginVertical: 8,
      padding: 12,
      borderRadius: 12,
      backgroundColor: colors.surface,
      borderWidth: 1,
      borderColor: colors.cardOutline,
      gap: 10,
    },
    header: {
      flexDirection: 'row',
      alignItems: 'flex-start',
      gap: 8,
    },
    meta: {
      flex: 1,
      gap: 4,
    },
    reference: {
      fontSize: 12,
      fontVariant: ['tabular-nums'],
      color: colors.textOnCardSecondary,
    },
    title: {
      fontSize: 16,
      fontWeight: '600',
      color: colors.black,
    },
    time: {
      fontSize: 13,
      color: colors.textOnCard,
    },
    badges: {
      flexDirection: 'row',
      gap: 6,
    },
    badge: {
      paddingHorizontal: 8,
      paddingVertical: 3,
      borderRadius: 999,
      backgroundColor: colors.surfaceTint,
    },
    badgeText: {
      fontSize: 11,
      fontWeight: '600',
      color: colors.textOnCardSecondary,
    },
    actions: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 8,
    },
    action: {
      paddingHorizontal: 12,
      paddingVertical: 7,
      borderRadius: 8,
      backgroundColor: colors.surfaceTint,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 5,
      // Do not let the row compress the label out of view on narrow screens;
      // the actions wrap instead (#phase1).
      flexShrink: 0,
    },
    primaryAction: {
      backgroundColor: colors.brand,
    },
    disabledAction: {
      opacity: 0.4,
    },
    actionText: {
      fontSize: 13,
      fontWeight: '600',
      color: colors.textOnCard,
    },
    primaryActionText: {
      color: colors.onBrand,
    },
  });

function CurrentTaskCardImpl({
  card,
  loading,
  onStart,
  onPause,
  onProgress,
  onComplete,
  onDelay,
  onConsult,
}: CurrentTaskCardProps) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);

  const isInProgress = card.work_state === 'in_progress';

  const handleStartPause = () => {
    haptic.light();
    if (isInProgress) {
      onPause();
    } else {
      onStart();
    }
  };

  const actionButton = (
    label: string,
    icon: ComponentProps<typeof Ionicons>['name'] | null,
    onPress: () => void,
    {
      primary = false,
      isLoading = false,
      disabled = false,
    }: {
      primary?: boolean;
      isLoading?: boolean;
      disabled?: boolean;
    } = {},
  ) => (
    <PressableScale
      key={label}
      style={[
        styles.action,
        primary && styles.primaryAction,
        disabled && styles.disabledAction,
      ]}
      onPress={onPress}
      disabled={disabled || isLoading}
      accessibilityRole="button"
      accessibilityLabel={label}
    >
      {icon && (
        <Ionicons
          name={icon}
          size={14}
          color={primary ? colors.onBrand : colors.textOnCard}
        />
      )}
      <Text
        style={[styles.actionText, primary && styles.primaryActionText]}
        numberOfLines={1}
        ellipsizeMode="tail"
      >
        {label}
      </Text>
      {isLoading && (
        <ActivityIndicator
          size="small"
          color={primary ? colors.onBrand : colors.brand}
        />
      )}
    </PressableScale>
  );

  return (
    <View style={styles.container}>
      <View style={styles.header}>
        <View style={styles.meta}>
          <Text style={styles.reference}>
            {card.reference} · {formatTimeWindow(card.start_at, card.end_at)}
          </Text>
          <Text style={styles.title}>{card.title}</Text>
        </View>
        <View style={styles.badges}>
          <View style={styles.badge}>
            <Text style={styles.badgeText}>{stateLabel(card.work_state)}</Text>
          </View>
          <View style={styles.badge}>
            <Text style={styles.badgeText}>
              {authorityLabel(card.authority)}
            </Text>
          </View>
        </View>
      </View>
      <View style={styles.actions}>
        {actionButton(
          isInProgress ? '一時停止' : '着手',
          isInProgress ? 'pause' : 'play',
          handleStartPause,
          { primary: true, isLoading: loading },
        )}
        {actionButton(
          '進捗',
          'bar-chart-outline',
          () => {
            haptic.light();
            onProgress();
          },
          { disabled: !isInProgress },
        )}
        {actionButton(
          '完了',
          'checkmark',
          () => {
            haptic.light();
            onComplete();
          },
          { disabled: !isInProgress },
        )}
        {actionButton('延期', 'time-outline', () => {
          haptic.light();
          onDelay();
        })}
        {actionButton('相談', 'chatbubble-outline', () => {
          haptic.light();
          onConsult();
        })}
      </View>
    </View>
  );
}

export const CurrentTaskCard = memo(CurrentTaskCardImpl);
