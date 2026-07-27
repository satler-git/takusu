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
  type ColorSet,
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

function buildPaperTheme(base: typeof MD3LightTheme, colors: ColorSet) {
  return {
    ...base,
    colors: {
      ...base.colors,
      primary: colors.brand,
      onPrimary: colors.onBrand,
      primaryContainer: colors.surfaceTint,
      onPrimaryContainer: colors.black,
      secondary: colors.gray,
      onSecondary: colors.black,
      secondaryContainer: colors.surface,
      onSecondaryContainer: colors.black,
      tertiary: colors.brandLight,
      onTertiary: colors.black,
      tertiaryContainer: colors.surfaceTint,
      onTertiaryContainer: colors.black,
      surface: colors.surface,
      onSurface: colors.black,
      surfaceVariant: colors.surfaceTint,
      onSurfaceVariant: colors.grayLight,
      background: colors.white,
      onBackground: colors.black,
      outline: colors.separator,
      outlineVariant: colors.grayDark,
      error: colors.red,
      onError: colors.onBrand,
      errorContainer: colors.errorContainer,
      onErrorContainer: colors.black,
      inverseSurface: colors.black,
      inverseOnSurface: colors.white,
      inversePrimary: colors.brandLight,
      shadow: colors.shadow,
      scrim: colors.scrim,
      backdrop: colors.overlay,
    },
  };
}

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
  const {
    theme,
    settingsLoaded,
    client,
    notifications,
    error: serverError,
  } = useServer();
  const MAX_PROCESSED_RESPONSE_IDS = 50;

  const [showWelcome, setShowWelcome] = useState(false);
  const [dismissWelcome, setDismissWelcome] = useState(false);
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

  // Show the welcome overlay once per hour on cold start, and also whenever
  // the app is waiting for the local server to become ready. The overlay stays
  // visible until `client` is set or an error occurs, masking the empty UI
  // during the cold-start health-check wait (#1135).
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
      const isWaiting = !client && !serverError;
      if (shouldShow || isWaiting) {
        setWelcomeTheme(theme);
        setWelcomeBackgroundColor(backgroundColorForTheme(theme));
        setShowWelcome(true);
      }
    }
    checkWelcome().catch(() => {});
  }, [settingsLoaded, theme, client, serverError]);

  // Dismiss the welcome overlay as soon as the server is ready or fails.
  useEffect(() => {
    if (showWelcome && (client || serverError)) {
      setDismissWelcome(true);
    }
  }, [showWelcome, client, serverError]);

  const handleWelcomeFinished = useCallback(() => {
    setShowWelcome(false);
    setDismissWelcome(false);
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
      ? buildPaperTheme(MD3DarkTheme, CATPPUCCIN_COLORS)
      : theme === 'aura-soft-dark'
        ? buildPaperTheme(MD3DarkTheme, AURA_SOFT_DARK_COLORS)
        : isDark
          ? buildPaperTheme(MD3DarkTheme, DARK_COLORS)
          : buildPaperTheme(MD3LightTheme, COLORS);

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
              dismiss={dismissWelcome}
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
