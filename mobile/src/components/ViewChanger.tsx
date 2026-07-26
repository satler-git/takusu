import { useMemo } from 'react';
// ViewChanger — left side bottom, vertical buttons to switch between views
// habit / task / graph

import { Pressable, StyleSheet, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useColors, type ColorSet } from '@/src/theme';
import { haptic } from '@/src/components/haptics';

export type ViewType = 'task' | 'graph' | 'habit';

interface ViewChangerProps {
  current: ViewType;
  onChange: (view: ViewType) => void;
}

const ICONS: Record<ViewType, keyof typeof Ionicons.glyphMap> = {
  task: 'list-outline',
  graph: 'git-branch-outline',
  habit: 'repeat-outline',
};

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      position: 'absolute',
      left: 8,
      bottom: 80,
      gap: 4,
      zIndex: 10,
    },
    button: {
      width: 40,
      height: 40,
      borderRadius: 12,
      alignItems: 'center',
      justifyContent: 'center',
      shadowColor: colors.shadow,
      shadowOffset: { width: 0, height: 1 },
      shadowOpacity: 0.2,
      shadowRadius: 2,
      elevation: 2,
    },
    buttonActive: {
      backgroundColor: colors.brand,
    },
    labelHidden: {
      display: 'none',
    },
  });

export function ViewChanger({ current, onChange }: ViewChangerProps) {
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const views: ViewType[] = ['task', 'graph', 'habit'];
  return (
    <View style={[styles.container, { bottom: 80 + insets.bottom }]}>
      {views.map((v) => (
        <Pressable
          key={v}
          style={({ pressed }) => [
            styles.button,
            { backgroundColor: colors.surface },
            current === v && styles.buttonActive,
            pressed && { opacity: 0.7 },
          ]}
          onPress={() => {
            if (current !== v) haptic.select();
            onChange(v);
          }}
        >
          <Ionicons
            name={ICONS[v]}
            size={18}
            color={current === v ? colors.white : colors.brand}
          />
        </Pressable>
      ))}
    </View>
  );
}
