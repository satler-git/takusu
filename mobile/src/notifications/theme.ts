// Notification icon color helpers.
//
// The small notification icon is an all-white silhouette (notification_icon.png),
// and Android tints it with the notification's `color`. We tint it with the
// theme's brand color so the notification icon matches the active app icon
// theme (light/dark/catppuccin/aura-soft-dark).

import AsyncStorage from '@react-native-async-storage/async-storage';
import {
  APP_THEMES,
  type AppTheme,
  COLORS,
  DARK_COLORS,
  CATPPUCCIN_COLORS,
  AURA_SOFT_DARK_COLORS,
} from '@/src/theme';

const THEME_KEY = 'takusu.theme';

export function notificationColorForTheme(theme: AppTheme): string {
  switch (theme) {
    case 'dark':
      return DARK_COLORS.brandLight;
    case 'catppuccin':
      return CATPPUCCIN_COLORS.brandLight;
    case 'aura-soft-dark':
      return AURA_SOFT_DARK_COLORS.brandLight;
    default:
      return COLORS.brand;
  }
}

export async function getNotificationIconColor(): Promise<string> {
  const stored = await AsyncStorage.getItem(THEME_KEY);
  const theme =
    stored && APP_THEMES.includes(stored as AppTheme)
      ? (stored as AppTheme)
      : 'light';
  return notificationColorForTheme(theme);
}
