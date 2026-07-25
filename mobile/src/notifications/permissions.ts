// Notification permissions.

import * as Notifications from 'expo-notifications';
import { setupNotificationChannels } from './channels';

export async function ensureNotificationPermissions(): Promise<boolean> {
  // Channels must be created before requesting permissions on Android 13+
  await setupNotificationChannels();

  const { status: existing } = await Notifications.getPermissionsAsync();
  if (existing === 'granted') return true;

  const { status } = await Notifications.requestPermissionsAsync();
  return status === 'granted';
}
