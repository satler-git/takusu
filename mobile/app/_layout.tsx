import { isRunningInExpoGo } from 'expo';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { GestureHandlerRootView } from 'react-native-gesture-handler';
import { PaperProvider, MD3DarkTheme, MD3LightTheme } from 'react-native-paper';
import { SafeAreaProvider } from 'react-native-safe-area-context';
import { useEffect, useRef, useCallback, useState } from 'react';
import * as Notifications from 'expo-notifications';
import { router } from 'expo-router';
import * as Sentry from '@sentry/react-native';
import { ServerProvider, useServer } from '@/src/api/ServerProvider';
import { VoiceProvider } from '@/src/api/VoiceContext';
import { setRecordingChangeListener } from '@/src/utils/voice';
import { FloatingVoiceButton } from '@/src/components/FloatingVoiceButton';
import { installGlobalErrorHandler } from '@/src/api/installGlobalErrorHandler';
import {
  ThemeProvider,
  COLORS,
  DARK_COLORS,
  CATPPUCCIN_COLORS,
  AURA_SOFT_DARK_COLORS,
  type AppTheme,
} from '@/src/theme';
import { UndoRedoToast } from '@/src/components/UndoRedoToast';
import { WelcomeScreen } from '@/src/components/WelcomeScreen';
import { haptic } from '@/src/components/haptics';
import {
  loadWelcomeShownAt,
  saveWelcomeShownAt,
} from '@/src/api/settingsStore';
import { TopToastProvider } from '@/src/components/TopToast';
import {
  setupNotificationCategories,
  ensureNotificationPermissions,
} from '@/src/notifications';
import { handleActionButtonResponse } from '@/src/notifications/actionHandler';
import {
  registerNotificationBackgroundTask,
  unregisterNotificationBackgroundTask,
} from '@/src/notifications/backgroundTask';

// Foreground notification handler — show notifications while app is open
Notifications.setNotificationHandler({
  handleNotification: async () => ({
    shouldPlaySound: false,
    shouldSetBadge: false,
    shouldShowBanner: true,
    shouldShowList: true,
  }),
});

// Allowlist of valid route prefixes for notification deep links.
// '/' is treated as exact match only — using startsWith('/') would match
// every absolute path and defeat the allowlist purpose.
const VALID_ROUTE_PREFIXES = ['/task/', '/habit/', '/settings'];

function isValidRoute(url: string): boolean {
  return (
    url === '/' || VALID_ROUTE_PREFIXES.some((prefix) => url.startsWith(prefix))
  );
}

function redirect(notification: Notifications.Notification) {
  const url = notification.request.content.data?.url;
  if (typeof url === 'string' && url && isValidRoute(url)) {
    router.push(url);
  }
}

if (process.env.EXPO_PUBLIC_SENTRY_DSN) {
  Sentry.init({
    dsn: process.env.EXPO_PUBLIC_SENTRY_DSN,
    environment: __DEV__ ? 'development' : 'production',
    debug: __DEV__,
    tracesSampleRate: 1.0,
    enableNativeFramesTracking: !isRunningInExpoGo(),
    integrations: [
      Sentry.expoRouterIntegration({
        enableTimeToInitialDisplay: !isRunningInExpoGo(),
      }),
    ],
  });
}

function backgroundColorForTheme(theme: AppTheme): string {
  return theme === 'catppuccin'
    ? CATPPUCCIN_COLORS.white
    : theme === 'aura-soft-dark'
      ? AURA_SOFT_DARK_COLORS.white
      : theme === 'dark'
        ? DARK_COLORS.white
        : COLORS.white;
}

function ThemedApp() {
  const { theme, settingsLoaded, client, notifications } = useServer();
  const MAX_PROCESSED_RESPONSE_IDS = 50;

  const [showWelcome, setShowWelcome] = useState(false);
  const [welcomeTheme, setWelcomeTheme] = useState<AppTheme | null>(null);
  const [welcomeBackgroundColor, setWelcomeBackgroundColor] = useState<
    string | null
  >(null);

  // Queue of notification action responses that arrived before `client` was
  // ready (server still starting on cold launch). Drained once `client` is set.
  const pendingActions = useRef<Notifications.NotificationResponse[]>([]);
  // Track ids queued while waiting for `client` so we don't enqueue duplicates.
  const pendingResponseIds = useRef(new Set<string>());
  // Deduplicate notification responses; the same response may be reported
  // through multiple channels (cold-start value + listener event).
  const processedResponseIds = useRef(new Set<string>());
  const processedResponseOrder = useRef<string[]>([]);
  const lastNotificationResponse = Notifications.useLastNotificationResponse();

  // Set up notification channels, categories, permissions, action categories,
  // and the background task that handles action buttons on Android.
  useEffect(() => {
    async function setupNotifications() {
      await ensureNotificationPermissions();
      await setupNotificationCategories();
      await registerNotificationBackgroundTask();
    }
    setupNotifications();

    return () => {
      unregisterNotificationBackgroundTask().catch(() => {
        // ignore cleanup errors
      });
    };
  }, []);

  // Show the welcome overlay once per hour on cold start.
  // Wait for settings to load so the theme is finalized before capturing it.
  const hasCheckedWelcome = useRef(false);
  useEffect(() => {
    if (!settingsLoaded || hasCheckedWelcome.current) return;
    hasCheckedWelcome.current = true;

    async function checkWelcome() {
      const WELCOME_COOLDOWN_MS = 60 * 60 * 1000; // 1 hour
      const lastShown = await loadWelcomeShownAt();
      const now = Date.now();
      const shouldShow =
        lastShown === null || now - lastShown > WELCOME_COOLDOWN_MS;
      if (shouldShow) {
        setWelcomeTheme(theme);
        setWelcomeBackgroundColor(backgroundColorForTheme(theme));
      }
      setShowWelcome(shouldShow);
    }
    checkWelcome().catch(() => {
      setShowWelcome(false);
    });
  }, [settingsLoaded, theme]);

  const handleWelcomeFinished = useCallback(() => {
    setShowWelcome(false);
    saveWelcomeShownAt(Date.now()).catch(() => {
      // ignore storage errors
    });
  }, []);

  const processResponse = useCallback(
    async (response: Notifications.NotificationResponse) => {
      const id = response.notification.request.identifier;
      if (!id) return;
      if (
        pendingResponseIds.current.has(id) ||
        processedResponseIds.current.has(id)
      ) {
        return;
      }

      function markProcessed() {
        if (processedResponseIds.current.has(id)) return;
        processedResponseIds.current.add(id);
        processedResponseOrder.current.push(id);
        if (
          processedResponseOrder.current.length > MAX_PROCESSED_RESPONSE_IDS
        ) {
          const oldest = processedResponseOrder.current.shift()!;
          processedResponseIds.current.delete(oldest);
        }
      }

      if (!client) {
        pendingActions.current.push(response);
        pendingResponseIds.current.add(id);
        return;
      }

      const handled = await handleActionButtonResponse(response, {
        client,
        inProgressNotifications: notifications.inProgress,
        haptic,
      });
      if (handled) {
        markProcessed();
      } else {
        redirect(response.notification);
        markProcessed();
      }

      if (
        lastNotificationResponse &&
        lastNotificationResponse.notification.request.identifier === id
      ) {
        try {
          Notifications.clearLastNotificationResponse();
        } catch {
          // ignore missing native method
        }
      }
    },
    [client, notifications.inProgress, lastNotificationResponse],
  );

  // Drain queued action responses once `client` becomes available (#353).
  useEffect(() => {
    if (!client || pendingActions.current.length === 0) return;
    const queued = pendingActions.current;
    pendingActions.current = [];
    for (const response of queued) {
      const id = response.notification.request.identifier;
      if (id) pendingResponseIds.current.delete(id);
      void processResponse(response);
    }
  }, [
    client,
    notifications.inProgress,
    lastNotificationResponse,
    processResponse,
  ]);

  // Handle notification responses (body tap and action buttons) from both
  // cold start and live listener events. On Android, action buttons are now
  // handled by the background task so the app stays closed (#788).
  useEffect(() => {
    if (!lastNotificationResponse) return;
    void processResponse(lastNotificationResponse);
  }, [lastNotificationResponse, processResponse]);

  const isDark = theme !== 'light';
  const stackBg = backgroundColorForTheme(theme);

  const paperTheme =
    theme === 'catppuccin'
      ? {
          ...MD3DarkTheme,
          colors: {
            ...MD3DarkTheme.colors,
            primary: CATPPUCCIN_COLORS.brand,
            onPrimary: CATPPUCCIN_COLORS.onBrand,
            primaryContainer: CATPPUCCIN_COLORS.surfaceTint,
            onPrimaryContainer: CATPPUCCIN_COLORS.black,
            secondary: CATPPUCCIN_COLORS.gray,
            onSecondary: CATPPUCCIN_COLORS.black,
            secondaryContainer: CATPPUCCIN_COLORS.surface,
            onSecondaryContainer: CATPPUCCIN_COLORS.black,
            tertiary: CATPPUCCIN_COLORS.brandLight,
            onTertiary: CATPPUCCIN_COLORS.black,
            tertiaryContainer: CATPPUCCIN_COLORS.surfaceTint,
            onTertiaryContainer: CATPPUCCIN_COLORS.black,
            surface: CATPPUCCIN_COLORS.surface,
            onSurface: CATPPUCCIN_COLORS.black,
            surfaceVariant: CATPPUCCIN_COLORS.surfaceTint,
            onSurfaceVariant: CATPPUCCIN_COLORS.grayLight,
            background: CATPPUCCIN_COLORS.white,
            onBackground: CATPPUCCIN_COLORS.black,
            outline: CATPPUCCIN_COLORS.separator,
            outlineVariant: CATPPUCCIN_COLORS.grayDark,
            error: CATPPUCCIN_COLORS.red,
            onError: CATPPUCCIN_COLORS.onBrand,
            errorContainer: '#4D2A32',
            onErrorContainer: CATPPUCCIN_COLORS.black,
            inverseSurface: CATPPUCCIN_COLORS.black,
            inverseOnSurface: CATPPUCCIN_COLORS.white,
            inversePrimary: CATPPUCCIN_COLORS.brandLight,
            shadow: CATPPUCCIN_COLORS.shadow,
            scrim: CATPPUCCIN_COLORS.scrim,
            backdrop: 'rgba(24,25,38,0.5)',
          },
        }
      : theme === 'aura-soft-dark'
        ? {
            ...MD3DarkTheme,
            colors: {
              ...MD3DarkTheme.colors,
              primary: AURA_SOFT_DARK_COLORS.brand,
              onPrimary: AURA_SOFT_DARK_COLORS.onBrand,
              primaryContainer: AURA_SOFT_DARK_COLORS.surfaceTint,
              onPrimaryContainer: AURA_SOFT_DARK_COLORS.black,
              secondary: AURA_SOFT_DARK_COLORS.gray,
              onSecondary: AURA_SOFT_DARK_COLORS.black,
              secondaryContainer: AURA_SOFT_DARK_COLORS.surface,
              onSecondaryContainer: AURA_SOFT_DARK_COLORS.black,
              tertiary: AURA_SOFT_DARK_COLORS.brandLight,
              onTertiary: AURA_SOFT_DARK_COLORS.black,
              tertiaryContainer: AURA_SOFT_DARK_COLORS.surfaceTint,
              onTertiaryContainer: AURA_SOFT_DARK_COLORS.black,
              surface: AURA_SOFT_DARK_COLORS.surface,
              onSurface: AURA_SOFT_DARK_COLORS.black,
              surfaceVariant: AURA_SOFT_DARK_COLORS.surfaceTint,
              onSurfaceVariant: AURA_SOFT_DARK_COLORS.grayLight,
              background: AURA_SOFT_DARK_COLORS.white,
              onBackground: AURA_SOFT_DARK_COLORS.black,
              outline: AURA_SOFT_DARK_COLORS.separator,
              outlineVariant: AURA_SOFT_DARK_COLORS.grayDark,
              error: AURA_SOFT_DARK_COLORS.red,
              onError: AURA_SOFT_DARK_COLORS.onBrand,
              errorContainer: '#4D2A32',
              onErrorContainer: AURA_SOFT_DARK_COLORS.black,
              inverseSurface: AURA_SOFT_DARK_COLORS.black,
              inverseOnSurface: AURA_SOFT_DARK_COLORS.white,
              inversePrimary: AURA_SOFT_DARK_COLORS.brandLight,
              shadow: AURA_SOFT_DARK_COLORS.shadow,
              scrim: AURA_SOFT_DARK_COLORS.scrim,
              backdrop: 'rgba(20,20,30,0.5)',
            },
          }
        : isDark
          ? MD3DarkTheme
          : MD3LightTheme;

  return (
    <ThemeProvider theme={theme}>
      <PaperProvider theme={paperTheme}>
        <TopToastProvider>
          <StatusBar style={isDark ? 'light' : 'dark'} />
          <Stack
            screenOptions={{
              headerShown: false,
              contentStyle: { backgroundColor: stackBg },
            }}
          >
            <Stack.Screen name="index" />
            <Stack.Screen name="agent" />
            <Stack.Screen name="task/[id]" />
            <Stack.Screen name="task/add" />
            <Stack.Screen name="habit/[id]" />
            <Stack.Screen name="habit/add" />
            <Stack.Screen name="settings" />
            <Stack.Screen name="settings/licenses" />
            <Stack.Screen name="stats" />
            <Stack.Screen name="import-ical" />
          </Stack>
          <UndoRedoToast />
          <FloatingVoiceButton />
          {showWelcome && welcomeTheme !== null && (
            <WelcomeScreen
              theme={welcomeTheme}
              backgroundColor={welcomeBackgroundColor ?? stackBg}
              onFinished={handleWelcomeFinished}
            />
          )}
        </TopToastProvider>
      </PaperProvider>
    </ThemeProvider>
  );
}

function RootLayout() {
  // Forward uncaught JS exceptions and promise rejections to the native log
  // ring buffer so they appear in log exports alongside server logs.
  useEffect(() => {
    installGlobalErrorHandler();
  }, []);

  return (
    <GestureHandlerRootView style={{ flex: 1 }}>
      <SafeAreaProvider>
        <ServerProvider>
          <VoiceProvider onRecordingChange={setRecordingChangeListener}>
            <ThemedApp />
          </VoiceProvider>
        </ServerProvider>
      </SafeAreaProvider>
    </GestureHandlerRootView>
  );
}

export default Sentry.wrap(RootLayout);
