// Notification icon color helpers.
//
// The small notification icon is an all-white silhouette (notification_icon.png),
// and Android tints it with the notification's `color`. We tint it with the
// theme's brand color so the notification icon matches the active app icon
// theme (light/dark/catppuccin/aura-soft-dark).

import { loadTheme } from '@/src/api/settingsStore';
import {
  type AppTheme,
  COLORS,
  DARK_COLORS,
  CATPPUCCIN_COLORS,
  AURA_SOFT_DARK_COLORS,
} from '@/src/theme';

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

export async function getNotificationIconColor(
  theme?: AppTheme,
): Promise<string> {
  return notificationColorForTheme(theme ?? (await loadTheme()));
}
