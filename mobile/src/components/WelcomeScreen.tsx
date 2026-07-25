// WelcomeScreen — shown briefly on cold start after the native splash.
// Fades in over 0.2s, holds for 0.2s, then fades out over 0.2s.

import { useEffect, useRef } from 'react';
import { Animated, Easing, Image, StyleSheet, View } from 'react-native';
import { type AppTheme } from '@/src/theme';

interface WelcomeScreenProps {
  theme: AppTheme;
  backgroundColor: string;
  onFinished: () => void;
}

const FADE_IN_DURATION = 200;
const HOLD_DURATION = 200;
const FADE_OUT_DURATION = 200;

const WELCOME_IMAGES: Record<AppTheme, number> = {
  light: require('../../assets/welcome.png'),
  dark: require('../../assets/welcome-dark.png'),
  catppuccin: require('../../assets/welcome-catppuccin.png'),
  'aura-soft-dark': require('../../assets/welcome-aura-soft-dark.png'),
};

export function WelcomeScreen({
  theme,
  backgroundColor,
  onFinished,
}: WelcomeScreenProps) {
  const fadeAnim = useRef(new Animated.Value(0)).current;

  useEffect(() => {
    const easing = Easing.inOut(Easing.ease);
    const animation = Animated.sequence([
      Animated.timing(fadeAnim, {
        toValue: 1,
        duration: FADE_IN_DURATION,
        easing,
        useNativeDriver: true,
      }),
      Animated.delay(HOLD_DURATION),
      Animated.timing(fadeAnim, {
        toValue: 0,
        duration: FADE_OUT_DURATION,
        easing,
        useNativeDriver: true,
      }),
    ]);

    animation.start(({ finished }) => {
      if (finished) {
        onFinished();
      }
    });

    return () => {
      animation.stop();
    };
  }, [fadeAnim, onFinished]);

  return (
    <View pointerEvents="none" style={[styles.container, { backgroundColor }]}>
      <Animated.View style={[styles.content, { opacity: fadeAnim }]}>
        <Image
          source={WELCOME_IMAGES[theme]}
          style={styles.image}
          resizeMode="contain"
        />
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    ...StyleSheet.absoluteFill,
    alignItems: 'center',
    justifyContent: 'center',
    zIndex: 100,
  },
  content: {
    alignItems: 'center',
    justifyContent: 'center',
  },
  image: {
    width: 280,
    height: 280,
  },
});
