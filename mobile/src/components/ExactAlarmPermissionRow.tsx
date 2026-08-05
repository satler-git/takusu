import { useEffect, useState } from 'react';
import { AppState, Platform, StyleSheet, Text, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors } from '@/src/theme';
import { PressableScale } from '@/src/components/PressableScale';
import { haptic } from '@/src/components/haptics';
import TakusuAlarmsModule from '@/modules/takusu-alarms/src/TakusuAlarmsModule';

export function ExactAlarmPermissionRow() {
  const colors = useColors();
  const [granted, setGranted] = useState<boolean | null>(null);

  useEffect(() => {
    const module = TakusuAlarmsModule;
    if (Platform.OS !== 'android' || module == null) {
      return;
    }

    let mounted = true;

    async function check() {
      if (module == null) return;
      try {
        const ok = await module.canScheduleExactAlarms();
        if (mounted) setGranted(ok);
      } catch {
        if (mounted) setGranted(false);
      }
    }

    check();

    const subscription = AppState.addEventListener('change', (nextState) => {
      if (nextState === 'active') {
        check();
      }
    });

    return () => {
      mounted = false;
      subscription.remove();
    };
  }, []);

  const module = TakusuAlarmsModule;
  if (Platform.OS !== 'android' || module == null) {
    return null;
  }

  if (granted === null || granted) {
    return null;
  }

  async function openSettings() {
    if (module == null) return;
    haptic.select();
    try {
      await module.requestExactAlarmPermission();
    } catch {
      // The user may need to open system settings manually.
    }
  }

  return (
    <View
      style={[
        styles.container,
        {
          backgroundColor: colors.warningBg,
          borderColor: colors.warningBorder,
        },
      ]}
    >
      <Ionicons name="warning" size={20} color={colors.warningIcon} />
      <View style={styles.text}>
        <Text style={[styles.title, { color: colors.textOnCard }]}>
          正確な時刻で通知する
        </Text>
        <Text
          style={[styles.description, { color: colors.textOnCardSecondary }]}
        >
          タスク開始前リマインダーなどを時間通りに受け取るには、システム設定の「アラームとリマインダー」を有効にしてください。
        </Text>
      </View>
      <PressableScale
        onPress={openSettings}
        style={[styles.button, { backgroundColor: colors.brand }]}
      >
        <Text style={[styles.buttonText, { color: colors.onBrand }]}>
          設定を開く
        </Text>
      </PressableScale>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 10,
    padding: 12,
    borderWidth: 1,
    borderRadius: 12,
  },
  text: { flex: 1, gap: 2 },
  title: { fontSize: 14, fontWeight: '700' },
  description: { fontSize: 12, lineHeight: 16 },
  button: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 8,
  },
  buttonText: { fontSize: 13, fontWeight: '600' },
});
