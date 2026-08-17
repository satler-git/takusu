// SettingsView — categorized settings
// general: dark/white theme
// worker: endpoint, key
// server restart: bottom of the settings category list
// google calendar: config + OAuth + manual sync
// info: license, version (build number)
//
// Split into two screens (issue #127): a category list and a per-category
// detail screen. The horizontal tab bar overflowed on small screens, making
// some categories (notably "情報") unreachable.

import { useCallback, useEffect, useRef, useState, useMemo } from 'react';
import {
  ActivityIndicator,
  Alert,
  ScrollView,
  StyleSheet,
  Switch,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { Menu } from 'react-native-paper';
import { useRouter } from 'expo-router';
import * as Application from 'expo-application';
import * as FileSystem from 'expo-file-system';
import * as Sharing from 'expo-sharing';
import * as Clipboard from 'expo-clipboard';
import Constants from 'expo-constants';
import {
  GoogleOneTapSignIn,
  isCancelledResponse,
  isErrorWithCode,
  isNoSavedCredentialFoundResponse,
  isSuccessResponse,
  statusCodes,
} from 'react-native-nitro-google-signin';
import { useServer } from '@/src/api/ServerProvider';
import type { GoogleCalSettings, SettingsRow } from '@/src/api/types';
import {
  useColors,
  APP_THEMES,
  type AppTheme,
  type ColorSet,
} from '@/src/theme';
import {
  formatTime,
  minutesToTime,
  timeToMinutes,
} from '@/src/notifications/settings';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { DateTimePickerModal } from '@/src/components/DateTimePickerModal';
import { haptic } from '@/src/components/haptics';
import { PressableScale } from '@/src/components/PressableScale';
import { useTopToast } from '@/src/components/TopToast';
import { showError } from '@/src/api/errors';
import { ExactAlarmPermissionRow } from '@/src/components/ExactAlarmPermissionRow';
import TakusuServerModule from '../../modules/takusu-server/src/TakusuServerModule';
import { AgentSettingsView } from '@/src/views/AgentSettingsView';
import { SkillsSettingsView } from '@/src/views/SkillsSettingsView';
import { useSolverSettings } from '@/src/hooks/useSolverSettings';
import { solverLabel, SOLVER_OPTIONS } from '@/src/utils/settings';

export type SettingsCategory =
  | 'general'
  | 'sleep'
  | 'workload'
  | 'solver'
  | 'notifications'
  | 'agent'
  | 'skills'
  | 'worker'
  | 'google'
  | 'info';

const CATEGORY_LABELS: Record<SettingsCategory, string> = {
  general: '一般',
  sleep: '睡眠',
  workload: '作業負荷',
  solver: 'Solver',
  notifications: '通知',
  agent: 'Agent',
  skills: 'スキル',
  worker: 'Worker',
  google: 'Google Calendar',
  info: '情報',
};

type SettingsCategoryGroup = {
  id: string;
  label: string;
  categories: SettingsCategory[];
};

const CATEGORY_GROUPS: SettingsCategoryGroup[] = [
  {
    id: 'app',
    label: 'アプリ',
    categories: ['general', 'notifications', 'info'],
  },
  {
    id: 'schedule',
    label: 'スケジュール',
    categories: ['sleep', 'workload', 'solver'],
  },
  { id: 'agent', label: 'エージェント', categories: ['agent', 'skills'] },
  { id: 'integrations', label: '連携', categories: ['worker', 'google'] },
];

// Convert stored minutes back to hours for the workload inputs.
// `0` or `null` means "use the default", so the input is left empty.
function formatMinutesToHours(minutes: number | null | undefined): string {
  if (!minutes || minutes <= 0) return '';
  return String(parseFloat((minutes / 60).toFixed(2)));
}

// Parse an hours string into minutes. Empty string or "0" resolves to 0
// (the default sentinel). Returns null for invalid/negative input.
function parseHoursToMinutes(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === '') return 0;
  const n = parseFloat(trimmed);
  if (!isFinite(n) || n < 0) return null;
  return Math.round(n * 60);
}

function themeLabel(t: AppTheme): string {
  switch (t) {
    case 'light':
      return 'Light';
    case 'dark':
      return 'Dark';
    case 'catppuccin':
      return 'Catppuccin';
    case 'aura-soft-dark':
      return 'Aura Soft';
  }
}

// ── Category list screen ──
// Replaces the horizontal tab bar that overflowed on small screens (issue #127).
const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    container: {
      flex: 1,
    },
    topBar: {
      flexDirection: 'row',
      alignItems: 'center',
      paddingHorizontal: 8,
      paddingBottom: 8,
      borderBottomWidth: StyleSheet.hairlineWidth,
    },
    backButton: {
      width: 40,
      height: 40,
      borderRadius: 20,
      alignItems: 'center',
      justifyContent: 'center',
    },
    backButtonText: {
      fontSize: 28,
    },
    title: {
      fontSize: 18,
      fontWeight: '600',
      marginStart: 8,
    },
    body: {
      flex: 1,
    },
    content: {
      padding: 16,
      gap: 16,
    },
    categoryRow: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      paddingVertical: 16,
      paddingHorizontal: 4,
      borderBottomWidth: StyleSheet.hairlineWidth,
    },
    categoryLabel: {
      fontSize: 16,
    },
    groupLabel: {
      fontSize: 13,
      fontWeight: '600',
      marginBottom: 4,
      paddingHorizontal: 4,
    },
    settingRow: {
      flexDirection: 'row',
      justifyContent: 'space-between',
      alignItems: 'center',
      paddingVertical: 8,
    },
    settingLabel: {
      fontSize: 16,
    },
    field: {
      gap: 4,
    },
    label: {
      fontSize: 13,
      fontWeight: '500',
    },
    value: {
      fontSize: 16,
    },
    input: {
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 8,
      fontSize: 16,
    },
    helpText: {
      fontSize: 12,
      marginTop: 2,
    },
    warning: {
      fontSize: 13,
      fontWeight: '500',
    },
    loader: {
      paddingVertical: 16,
    },
    actionButton: {
      paddingHorizontal: 16,
      paddingVertical: 10,
      borderRadius: 8,
      alignItems: 'center',
    },
    actionButtonText: {
      color: colors.onBrand,
      fontSize: 14,
      fontWeight: '600',
    },
    notifGroup: {
      gap: 8,
    },
    timeField: {
      borderWidth: 1,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
      alignItems: 'flex-end',
    },
    timeText: {
      fontSize: 16,
      fontWeight: '500',
      fontVariant: ['tabular-nums'],
    },
    healthResult: {
      fontSize: 13,
      fontFamily: 'monospace',
      paddingHorizontal: 4,
    },
    healthResultRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
    },
    themeDropdown: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      paddingHorizontal: 12,
      paddingVertical: 8,
      borderRadius: 8,
      borderWidth: 1,
      minWidth: 140,
    },
    themeDropdownText: {
      fontSize: 15,
      fontWeight: '500',
    },
  });

export function SettingsCategoryView() {
  const router = useRouter();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const {
    restartServer,
    restarting,
    error: restartError,
    workersUrl,
    workersToken,
  } = useServer();
  const previousRestartErrorRef = useRef<string | null>(restartError);

  useEffect(() => {
    if (restartError && previousRestartErrorRef.current !== restartError) {
      void showError(restartError, 'サーバー再起動失敗');
    }
    previousRestartErrorRef.current = restartError;
  }, [restartError]);

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View
        style={[
          styles.topBar,
          { borderBottomColor: colors.separator, paddingTop: 8 + insets.top },
        ]}
      >
        <PressableScale
          style={styles.backButton}
          onPress={() => {
            haptic.light();
            router.back();
          }}
        >
          <Text style={[styles.backButtonText, { color: colors.brand }]}>
            ‹
          </Text>
        </PressableScale>
        <Text style={[styles.title, { color: colors.black }]}>設定</Text>
      </View>

      <ScrollView
        contentContainerStyle={[
          styles.content,
          { paddingBottom: 16 + insets.bottom },
        ]}
      >
        {CATEGORY_GROUPS.map((group) => (
          <View key={group.id}>
            <Text style={[styles.groupLabel, { color: colors.gray }]}>
              {group.label}
            </Text>
            {group.categories.map((key) => (
              <PressableScale
                key={key}
                style={[
                  styles.categoryRow,
                  { borderBottomColor: colors.separator },
                ]}
                onPress={() => {
                  haptic.select();
                  router.push(`/settings/${key}`);
                }}
              >
                <Text style={[styles.categoryLabel, { color: colors.black }]}>
                  {CATEGORY_LABELS[key]}
                </Text>
                <Ionicons
                  name="chevron-forward"
                  size={20}
                  color={colors.gray}
                />
              </PressableScale>
            ))}
          </View>
        ))}

        <View style={styles.field}>
          <Text style={[styles.label, { color: colors.gray }]}>サーバー</Text>
          <PressableScale
            style={[styles.actionButton, { backgroundColor: colors.brand }]}
            onPress={() => {
              haptic.medium();
              restartServer();
            }}
            disabled={restarting || !workersUrl.trim() || !workersToken.trim()}
          >
            {restarting ? (
              <ActivityIndicator color={colors.onBrand} />
            ) : (
              <Text style={styles.actionButtonText}>サーバーを再起動</Text>
            )}
          </PressableScale>
          {restartError && (
            <Text style={[styles.warning, { color: colors.red }]}>
              {restartError}
            </Text>
          )}
        </View>
      </ScrollView>
    </View>
  );
}

// ── Per-category detail screen ──
export function SettingsDetailView({
  category,
}: {
  category: SettingsCategory;
}) {
  const router = useRouter();
  const {
    client,
    theme,
    setTheme,
    undoSteps,
    setUndoSteps,
    workersUrl: savedUrl,
    workersToken: savedToken,
    setWorkersUrl,
    setWorkersToken,
    notifications,
    setNotifications,
  } = useServer();
  const colors = useColors();
  const styles = useMemo(() => makeStyles(colors), [colors]);
  const insets = useSafeAreaInsets();
  const { showTopToast } = useTopToast();
  const [notifPickerField, setNotifPickerField] = useState<
    'morningBriefing' | null
  >(null);

  // Notification numeric inputs — local text state, committed on blur so
  // the user can clear the field completely while typing (#307).
  const [preStartInput, setPreStartInput] = useState(
    String(notifications.preStartReminderMinutes),
  );
  const [idleHoursInput, setIdleHoursInput] = useState(
    String(notifications.unscheduledIdleHours),
  );

  // Sleep tab state
  const [sleepSettings, setSleepSettings] = useState<SettingsRow | null>(null);
  const [sleepTz, setSleepTz] = useState('');
  const [sleepStart, setSleepStart] = useState('22:00');
  const [sleepEnd, setSleepEnd] = useState('06:00');
  const [sleepLoading, setSleepLoading] = useState(false);
  const [sleepSaving, setSleepSaving] = useState(false);
  const [sleepPickerField, setSleepPickerField] = useState<
    'start' | 'end' | null
  >(null);

  // Workload tab state (#459)
  const [workloadSettings, setWorkloadSettings] = useState<SettingsRow | null>(
    null,
  );
  const [workloadComfortable, setWorkloadComfortable] = useState('');
  const [workloadMaximum, setWorkloadMaximum] = useState('');
  const [workloadLoading, setWorkloadLoading] = useState(false);
  const [workloadSaving, setWorkloadSaving] = useState(false);
  const DEFAULT_COMFORTABLE_HOURS = 8;
  const DEFAULT_MAXIMUM_HOURS = 12;

  // Solver tab state (#789)
  const {
    solverValue,
    setSolverValue,
    timeBudgetInput,
    setTimeBudgetInput,
    seedInput,
    setSeedInput,
    warmStartValue,
    setWarmStartValue,
    loading: solverLoading,
    saving: solverSaving,
    menuVisible: solverMenuVisible,
    setMenuVisible: setSolverMenuVisible,
    dirty: solverDirty,
    loadSolverSettings,
    saveSolverSettings,
  } = useSolverSettings(client);

  // Worker tab state
  const [workerUrl, setWorkerUrl] = useState(savedUrl);
  const [workerKey, setWorkerKey] = useState(savedToken);
  const [workerDirty, setWorkerDirty] = useState(false);

  // Undo steps input — local text state, committed on blur to avoid
  // trimming the undo stack through intermediate values while typing.
  const [undoStepsInput, setUndoStepsInput] = useState(String(undoSteps));

  // Theme dropdown menu
  const [themeMenuVisible, setThemeMenuVisible] = useState(false);

  // Google Calendar state
  const [gcalSettings, setGcalSettings] = useState<GoogleCalSettings | null>(
    null,
  );
  const [gcalEnabled, setGcalEnabled] = useState(false);
  const [gcalCalendarId, setGcalCalendarId] = useState('');
  const [gcalClientId, setGcalClientId] = useState('');
  const [gcalClientSecret, setGcalClientSecret] = useState('');
  const [gcalRefreshToken, setGcalRefreshToken] = useState('');
  const [gcalReminderMinutes, setGcalReminderMinutes] = useState('');
  const [gcalColorId, setGcalColorId] = useState('');
  const [gcalVisibility, setGcalVisibility] = useState('');
  const [gcalTransparency, setGcalTransparency] = useState('');
  const [gcalLoading, setGcalLoading] = useState(false);
  const [oauthLoading, setOauthLoading] = useState(false);
  const [syncLoading, setSyncLoading] = useState(false);
  const [deleteAllLoading, setDeleteAllLoading] = useState(false);

  // Health check state (info tab)
  const [localHealthLoading, setLocalHealthLoading] = useState(false);
  const [localHealthResult, setLocalHealthResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);
  const [workerHealthLoading, setWorkerHealthLoading] = useState(false);
  const [workerHealthResult, setWorkerHealthResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);
  const [logExportLoading, setLogExportLoading] = useState(false);
  const [logCopyLoading, setLogCopyLoading] = useState(false);

  // Sync worker input with saved values when they change
  useEffect(() => {
    setWorkerUrl(savedUrl);
    setWorkerKey(savedToken);
  }, [savedUrl, savedToken]);

  // Keep local undo-steps input in sync with the persisted value
  useEffect(() => {
    setUndoStepsInput(String(undoSteps));
  }, [undoSteps]);

  function commitUndoSteps() {
    const n = parseInt(undoStepsInput, 10);
    if (!isNaN(n) && n > 0) {
      setUndoSteps(n);
    } else {
      // Revert to the current persisted value on invalid input
      setUndoStepsInput(String(undoSteps));
    }
  }

  // Keep notification inputs in sync when the persisted value changes
  useEffect(() => {
    setPreStartInput(String(notifications.preStartReminderMinutes));
  }, [notifications.preStartReminderMinutes]);
  useEffect(() => {
    setIdleHoursInput(String(notifications.unscheduledIdleHours));
  }, [notifications.unscheduledIdleHours]);

  function commitPreStart() {
    const n = parseInt(preStartInput, 10);
    if (!isNaN(n) && n > 0) {
      setNotifications({ ...notifications, preStartReminderMinutes: n });
    } else {
      setPreStartInput(String(notifications.preStartReminderMinutes));
    }
  }

  function commitIdleHours() {
    const n = parseInt(idleHoursInput, 10);
    if (!isNaN(n) && n > 0) {
      setNotifications({ ...notifications, unscheduledIdleHours: n });
    } else {
      setIdleHoursInput(String(notifications.unscheduledIdleHours));
    }
  }

  // Load Google Calendar settings when entering google tab
  const loadGcalSettings = useCallback(async () => {
    if (!client) return;
    setGcalLoading(true);
    try {
      const s = await client.getGcalSettings();
      setGcalSettings(s);
      setGcalEnabled(s.enabled);
      setGcalCalendarId(s.calendar_id);
      setGcalClientId(s.client_id);
      setGcalClientSecret('');
      setGcalReminderMinutes(
        s.reminder_minutes !== null && s.reminder_minutes !== undefined
          ? String(s.reminder_minutes)
          : '',
      );
      setGcalColorId(
        s.color_id !== null && s.color_id !== undefined
          ? String(s.color_id)
          : '',
      );
      setGcalVisibility(s.visibility ?? '');
      setGcalTransparency(s.transparency ?? '');
    } catch {
      // settings may not exist yet
      setGcalSettings(null);
    } finally {
      setGcalLoading(false);
    }
  }, [client]);

  useEffect(() => {
    if (category === 'google') {
      loadGcalSettings();
    }
  }, [category, loadGcalSettings]);

  // Load sleep/planner settings when entering sleep tab
  const loadSleepSettings = useCallback(async () => {
    if (!client) return;
    setSleepLoading(true);
    try {
      const s = await client.getSettings();
      setSleepSettings(s);
      setSleepTz(s.tz);
      setSleepStart(s.sleep_start);
      setSleepEnd(s.sleep_end);
    } catch {
      // fall back to defaults so the user can still set something
      setSleepSettings(null);
    } finally {
      setSleepLoading(false);
    }
  }, [client]);

  useEffect(() => {
    if (category === 'sleep' || category === 'general') {
      loadSleepSettings();
    }
  }, [category, loadSleepSettings]);

  // Load workload settings when entering workload tab
  const loadWorkloadSettings = useCallback(async () => {
    if (!client) return;
    setWorkloadLoading(true);
    try {
      const s = await client.getSettings();
      setWorkloadSettings(s);
      setWorkloadComfortable(formatMinutesToHours(s.comfortable_minutes));
      setWorkloadMaximum(formatMinutesToHours(s.maximum_minutes));
    } catch {
      setWorkloadSettings(null);
    } finally {
      setWorkloadLoading(false);
    }
  }, [client]);

  useEffect(() => {
    if (category === 'workload') {
      loadWorkloadSettings();
    }
  }, [category, loadWorkloadSettings]);

  // Load solver settings only when the solver tab is active (#789).
  useEffect(() => {
    if (category === 'solver') {
      loadSolverSettings();
    }
  }, [category, loadSolverSettings]);
  async function saveWorkerSettings() {
    const url = workerUrl.trim();
    const token = workerKey.trim();
    try {
      await setWorkersUrl(url);
      await setWorkersToken(token);
    } catch (e) {
      void showError(e, 'エラー');
      return;
    }
    if (client) {
      try {
        await client.updateWorkersConfig({ url, token });
        setWorkerDirty(false);
        haptic.success();
      } catch (e) {
        void showError(
          `Worker設定は保存されましたが、サーバーへの反映に失敗しました。再起動してください。 (${
            e instanceof Error ? e.message : String(e)
          })`,
          '保存しました',
        );
      }
    } else {
      setWorkerDirty(false);
      haptic.success();
    }
  }

  async function saveSleepSettings() {
    if (!client) return;
    // Guard against overwriting server values with defaults when the initial
    // load failed (sleepSettings stays null and the form shows defaults).
    if (!sleepSettings) {
      void showError(
        '設定の読み込みに失敗しています。タブを開き直してください',
        'エラー',
      );
      return;
    }
    setSleepSaving(true);
    try {
      const s = await client.updateSettings({
        sleep_start: sleepStart,
        sleep_end: sleepEnd,
      });
      setSleepSettings(s);
      setSleepStart(s.sleep_start);
      setSleepEnd(s.sleep_end);
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setSleepSaving(false);
    }
  }

  async function saveTimezoneSettings() {
    if (!client) return;
    if (!sleepSettings) {
      void showError(
        '設定の読み込みに失敗しています。タブを開き直してください',
        'エラー',
      );
      return;
    }
    setSleepSaving(true);
    try {
      const s = await client.updateSettings({
        tz: sleepTz || undefined,
      });
      setSleepSettings(s);
      setSleepTz(s.tz);
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setSleepSaving(false);
    }
  }

  async function saveWorkloadSettings() {
    if (!client) return;
    if (!workloadSettings) {
      void showError(
        '設定の読み込みに失敗しています。タブを開き直してください',
        'エラー',
      );
      return;
    }
    const comfortable = parseHoursToMinutes(workloadComfortable);
    const maximum = parseHoursToMinutes(workloadMaximum);
    if (comfortable === null || maximum === null) {
      void showError('作業時間は0以上の数値を入力してください');
      return;
    }
    setWorkloadSaving(true);
    try {
      const s = await client.updateSettings({
        comfortable_minutes: comfortable,
        maximum_minutes: maximum,
      });
      setWorkloadSettings(s);
      setWorkloadComfortable(formatMinutesToHours(s.comfortable_minutes));
      setWorkloadMaximum(formatMinutesToHours(s.maximum_minutes));
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setWorkloadSaving(false);
    }
  }

  async function saveGcalSettings() {
    if (!client) return;

    let reminderMinutes: number | undefined;
    const reminderTrimmed = gcalReminderMinutes.trim();
    if (reminderTrimmed !== '') {
      const parsed = parseInt(reminderTrimmed, 10);
      if (String(parsed) !== reminderTrimmed || parsed < 0) {
        void showError(
          'リマインダー時間は0以上の整数を入力してください',
          'エラー',
        );
        return;
      }
      reminderMinutes = parsed;
    }

    let colorId: number | undefined;
    const colorIdTrimmed = gcalColorId.trim();
    if (colorIdTrimmed !== '') {
      const parsed = parseInt(colorIdTrimmed, 10);
      if (String(parsed) !== colorIdTrimmed || parsed < 1 || parsed > 11) {
        void showError('色 ID は 1〜11 の整数を入力してください', 'エラー');
        return;
      }
      colorId = parsed;
    }

    const visibility = gcalVisibility.trim() || undefined;
    if (
      visibility &&
      !['default', 'public', 'private', 'confidential'].includes(visibility)
    ) {
      void showError(
        '公開範囲は default / public / private / confidential のいずれかを入力してください',
        'エラー',
      );
      return;
    }

    const transparency = gcalTransparency.trim() || undefined;
    if (transparency && !['opaque', 'transparent'].includes(transparency)) {
      void showError(
        '予定/空き状態は opaque / transparent のいずれかを入力してください',
        'エラー',
      );
      return;
    }

    try {
      const s = await client.updateGcalSettings({
        enabled: gcalEnabled,
        calendar_id: gcalCalendarId || undefined,
        client_id: gcalClientId || undefined,
        client_secret: gcalClientSecret || undefined,
        reminder_minutes: reminderMinutes ?? null,
        color_id: colorId ?? null,
        visibility: visibility ?? null,
        transparency: transparency ?? null,
      });
      setGcalSettings(s);
      setGcalClientSecret('');
      setGcalReminderMinutes(
        s.reminder_minutes !== null && s.reminder_minutes !== undefined
          ? String(s.reminder_minutes)
          : '',
      );
      setGcalColorId(
        s.color_id !== null && s.color_id !== undefined
          ? String(s.color_id)
          : '',
      );
      setGcalVisibility(s.visibility ?? '');
      setGcalTransparency(s.transparency ?? '');
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    }
  }

  const GOOGLE_CALENDAR_EVENTS_SCOPE =
    'https://www.googleapis.com/auth/calendar.events';

  // Start the native Google sign-in flow to obtain a server auth code,
  // then send it to the backend to exchange for a refresh token (#1057).
  async function startGoogleOAuth() {
    if (!client) return;

    if (!gcalSettings?.client_id || !gcalSettings?.has_client_secret) {
      void showError('Client ID / Client Secretを保存してください');
      return;
    }

    const webClientId = gcalSettings.client_id;

    setOauthLoading(true);
    try {
      GoogleOneTapSignIn.configure({
        webClientId,
        scopes: [GOOGLE_CALENDAR_EVENTS_SCOPE],
        offlineAccess: true,
      });

      await GoogleOneTapSignIn.checkPlayServices();

      let response = await GoogleOneTapSignIn.signIn();
      if (isNoSavedCredentialFoundResponse(response)) {
        response = await GoogleOneTapSignIn.createAccount();
      }

      if (!isSuccessResponse(response)) {
        if (isCancelledResponse(response)) {
          return;
        }
        throw new Error('Google サインインに失敗しました');
      }

      let serverAuthCode = response.data.serverAuthCode;
      if (!serverAuthCode) {
        const authResult = await GoogleOneTapSignIn.requestScopes([
          GOOGLE_CALENDAR_EVENTS_SCOPE,
        ]);
        serverAuthCode = authResult.serverAuthCode;
      }
      if (!serverAuthCode) {
        showTopToast('既に Google Calendar の権限が付与されているようです');
        await loadGcalSettings();
        return;
      }

      await client.oauthCallback(serverAuthCode);
      await loadGcalSettings();
      haptic.success();
    } catch (e) {
      if (
        isErrorWithCode(e) &&
        (e.code === statusCodes.SIGN_IN_CANCELLED ||
          e.code === statusCodes.PLAY_SERVICES_NOT_AVAILABLE)
      ) {
        if (e.code === statusCodes.PLAY_SERVICES_NOT_AVAILABLE) {
          void showError('Google Play サービスが利用できません', 'エラー');
        }
        return;
      }
      void showError(e, 'OAuthエラー');
    } finally {
      setOauthLoading(false);
    }
  }

  // Save a refresh token obtained via the CLI OAuth flow or other means,
  // as a fallback when native sign-in is unavailable (issue #297 / #1057).
  async function saveRefreshToken() {
    if (!client) return;
    if (!gcalRefreshToken.trim()) {
      void showError('Refresh Tokenを入力してください');
      return;
    }
    try {
      const s = await client.updateGcalSettings({
        refresh_token: gcalRefreshToken.trim(),
      });
      setGcalSettings(s);
      setGcalRefreshToken('');
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    }
  }

  async function triggerSync() {
    if (!client) return;
    setSyncLoading(true);
    try {
      await client.triggerSync();
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setSyncLoading(false);
    }
  }

  async function deleteAllGcalEvents() {
    if (!client) return;
    Alert.alert(
      'Google Calendarイベントを削除',
      'マッピングされているGoogle Calendar側のイベントをすべて削除します。よろしいですか？',
      [
        { text: 'キャンセル', style: 'cancel' },
        {
          text: '削除',
          style: 'destructive',
          onPress: async () => {
            setDeleteAllLoading(true);
            try {
              const res = await client.deleteAllGcalEvents();
              const failed = res.failed.length;
              if (failed > 0) {
                void showError(
                  `${res.deleted}件のイベントを削除しました\n${failed}件の削除に失敗しました`,
                  '削除完了（一部失敗）',
                );
              } else {
                showTopToast(`${res.deleted}件のイベントを削除しました`);
              }
            } catch (e) {
              void showError(e, 'エラー');
            } finally {
              setDeleteAllLoading(false);
            }
          },
        },
      ],
    );
  }

  // ── Health checks (info tab) ──

  async function checkLocalHealth() {
    if (!client) return;
    setLocalHealthLoading(true);
    setLocalHealthResult(null);
    try {
      const text = await client.health();
      setLocalHealthResult({ ok: true, message: text });
    } catch (e) {
      setLocalHealthResult({
        ok: false,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setLocalHealthLoading(false);
    }
  }

  async function checkWorkerHealth() {
    if (!client) return;
    setWorkerHealthLoading(true);
    setWorkerHealthResult(null);
    try {
      const { status } = await client.workerHealthCheck();
      setWorkerHealthResult({ ok: true, message: status });
    } catch (e) {
      setWorkerHealthResult({
        ok: false,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setWorkerHealthLoading(false);
    }
  }

  // ── Log export (info tab) ──

  async function exportLogs() {
    setLogExportLoading(true);
    try {
      const lines = await TakusuServerModule.getLogs();
      if (lines.length === 0) {
        showTopToast('エクスポートするログがありません');
        return;
      }
      const content = lines.join('\n') + '\n';
      const filename = `takusu-logs-${new Date().toISOString().replace(/[:.]/g, '-')}.txt`;
      const file = new FileSystem.File(FileSystem.Paths.cache, filename);
      // write() does not auto-create the file, so create it first if missing.
      if (!file.exists) {
        file.create();
      }
      file.write(content);
      if (await Sharing.isAvailableAsync()) {
        await Sharing.shareAsync(file.uri, {
          mimeType: 'text/plain',
          dialogTitle: 'ログをエクスポート',
        });
      } else {
        haptic.success();
      }
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setLogExportLoading(false);
    }
  }

  async function clearLogs() {
    Alert.alert(
      'ログを消去',
      '保存されているログをすべて消去します。よろしいですか？',
      [
        { text: 'キャンセル', style: 'cancel' },
        {
          text: '消去',
          style: 'destructive',
          onPress: async () => {
            try {
              await TakusuServerModule.clearLogs();
              haptic.success();
            } catch (e) {
              void showError(e, 'エラー');
            }
          },
        },
      ],
    );
  }

  async function copyLogs() {
    setLogCopyLoading(true);
    try {
      const lines = await TakusuServerModule.getLogs();
      if (lines.length === 0) {
        showTopToast('コピーするログがありません');
        return;
      }
      const content = lines.join('\n');
      await Clipboard.setStringAsync(content);
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setLogCopyLoading(false);
    }
  }

  const appVersion = Application.nativeApplicationVersion ?? 'unknown';
  const buildVersion = Application.nativeBuildVersion ?? 'unknown';
  const gitCommit = Constants.expoConfig?.extra?.gitCommit ?? 'unknown';
  const gitTag = Constants.expoConfig?.extra?.gitTag ?? 'unknown';

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View
        style={[
          styles.topBar,
          { borderBottomColor: colors.separator, paddingTop: 8 + insets.top },
        ]}
      >
        <PressableScale
          style={styles.backButton}
          onPress={() => {
            haptic.light();
            router.back();
          }}
        >
          <Text style={[styles.backButtonText, { color: colors.brand }]}>
            ‹
          </Text>
        </PressableScale>
        <Text style={[styles.title, { color: colors.black }]}>
          {CATEGORY_LABELS[category]}
        </Text>
      </View>

      <View style={styles.body}>
        <ScrollView
          contentContainerStyle={[
            styles.content,
            { paddingBottom: 16 + insets.bottom },
          ]}
        >
          {category === 'agent' && <AgentSettingsView />}
          {category === 'skills' && <SkillsSettingsView />}
          {category === 'general' && (
            <>
              <View style={styles.settingRow}>
                <Text style={[styles.settingLabel, { color: colors.black }]}>
                  テーマ
                </Text>
                <Menu
                  visible={themeMenuVisible}
                  onDismiss={() => setThemeMenuVisible(false)}
                  anchor={
                    <PressableScale
                      onPress={() => setThemeMenuVisible(true)}
                      style={[
                        styles.themeDropdown,
                        {
                          backgroundColor: colors.surface,
                          borderColor: colors.separator,
                        },
                      ]}
                    >
                      <Text
                        style={[
                          styles.themeDropdownText,
                          { color: colors.black },
                        ]}
                      >
                        {themeLabel(theme)}
                      </Text>
                      <Ionicons
                        name="chevron-down"
                        size={16}
                        color={colors.black}
                      />
                    </PressableScale>
                  }
                >
                  {APP_THEMES.map((t) => (
                    <Menu.Item
                      key={t}
                      title={themeLabel(t)}
                      leadingIcon={theme === t ? 'check' : undefined}
                      onPress={() => {
                        setThemeMenuVisible(false);
                        haptic.select();
                        setTheme(t);
                      }}
                    />
                  ))}
                </Menu>
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  アンドゥ履歴の上限 (ステップ数)
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={undoStepsInput}
                  onChangeText={setUndoStepsInput}
                  onBlur={commitUndoSteps}
                  onSubmitEditing={commitUndoSteps}
                  keyboardType="numeric"
                  placeholder="50"
                  placeholderTextColor={colors.gray}
                />
              </View>

              {sleepLoading ? (
                <ActivityIndicator color={colors.brand} style={styles.loader} />
              ) : (
                <>
                  <View style={styles.field}>
                    <Text style={[styles.label, { color: colors.gray }]}>
                      タイムゾーン
                    </Text>
                    <TextInput
                      style={[
                        styles.input,
                        { borderColor: colors.separator, color: colors.black },
                      ]}
                      value={sleepTz}
                      onChangeText={setSleepTz}
                      placeholder="Asia/Tokyo"
                      placeholderTextColor={colors.gray}
                      autoCapitalize="none"
                      autoCorrect={false}
                    />
                    <PressableScale
                      style={[
                        styles.actionButton,
                        { backgroundColor: colors.surfaceTint },
                      ]}
                      onPress={() => {
                        haptic.light();
                        // Intl is available in Hermes (recent RN) — no native module needed
                        try {
                          const tz =
                            Intl.DateTimeFormat().resolvedOptions().timeZone;
                          if (tz) setSleepTz(tz);
                        } catch {
                          // ignore — device doesn't expose timezone via Intl
                        }
                      }}
                    >
                      <Text
                        style={[
                          styles.actionButtonText,
                          { color: colors.black },
                        ]}
                      >
                        デバイスのタイムゾーンを使用
                      </Text>
                    </PressableScale>
                  </View>

                  <PressableScale
                    style={[
                      styles.actionButton,
                      { backgroundColor: colors.brand },
                    ]}
                    onPress={() => {
                      haptic.medium();
                      saveTimezoneSettings();
                    }}
                    disabled={sleepSaving || !client}
                  >
                    {sleepSaving ? (
                      <ActivityIndicator color={colors.onBrand} />
                    ) : (
                      <Text style={styles.actionButtonText}>設定を保存</Text>
                    )}
                  </PressableScale>
                </>
              )}
            </>
          )}

          {category === 'sleep' && (
            <>
              {sleepLoading ? (
                <ActivityIndicator color={colors.brand} style={styles.loader} />
              ) : (
                <>
                  <View style={styles.notifGroup}>
                    <Text style={[styles.label, { color: colors.gray }]}>
                      就寝時刻
                    </Text>
                    <PressableScale
                      style={[
                        styles.timeField,
                        { borderColor: colors.separator },
                      ]}
                      onPress={() => {
                        haptic.select();
                        setSleepPickerField('start');
                      }}
                    >
                      <Text style={[styles.timeText, { color: colors.black }]}>
                        {sleepStart}
                      </Text>
                    </PressableScale>
                  </View>

                  <View style={styles.notifGroup}>
                    <Text style={[styles.label, { color: colors.gray }]}>
                      起床時刻
                    </Text>
                    <PressableScale
                      style={[
                        styles.timeField,
                        { borderColor: colors.separator },
                      ]}
                      onPress={() => {
                        haptic.select();
                        setSleepPickerField('end');
                      }}
                    >
                      <Text style={[styles.timeText, { color: colors.black }]}>
                        {sleepEnd}
                      </Text>
                    </PressableScale>
                  </View>

                  <PressableScale
                    style={[
                      styles.actionButton,
                      { backgroundColor: colors.brand },
                    ]}
                    onPress={() => {
                      haptic.medium();
                      saveSleepSettings();
                    }}
                    disabled={sleepSaving || !client}
                  >
                    {sleepSaving ? (
                      <ActivityIndicator color={colors.onBrand} />
                    ) : (
                      <Text style={styles.actionButtonText}>設定を保存</Text>
                    )}
                  </PressableScale>
                </>
              )}
            </>
          )}

          {category === 'workload' && (
            <>
              {workloadLoading ? (
                <ActivityIndicator color={colors.brand} style={styles.loader} />
              ) : (
                <>
                  <View style={styles.field}>
                    <Text style={[styles.label, { color: colors.gray }]}>
                      快適な1日の作業時間（時間）
                    </Text>
                    <TextInput
                      style={[
                        styles.input,
                        { borderColor: colors.separator, color: colors.black },
                      ]}
                      value={workloadComfortable}
                      onChangeText={setWorkloadComfortable}
                      keyboardType="numeric"
                      placeholder={String(DEFAULT_COMFORTABLE_HOURS)}
                      placeholderTextColor={colors.gray}
                    />
                  </View>

                  <View style={styles.field}>
                    <Text style={[styles.label, { color: colors.gray }]}>
                      最大の1日の作業時間（時間）
                    </Text>
                    <TextInput
                      style={[
                        styles.input,
                        { borderColor: colors.separator, color: colors.black },
                      ]}
                      value={workloadMaximum}
                      onChangeText={setWorkloadMaximum}
                      keyboardType="numeric"
                      placeholder={String(DEFAULT_MAXIMUM_HOURS)}
                      placeholderTextColor={colors.gray}
                    />
                  </View>

                  <PressableScale
                    style={[
                      styles.actionButton,
                      { backgroundColor: colors.brand },
                    ]}
                    onPress={() => {
                      haptic.medium();
                      saveWorkloadSettings();
                    }}
                    disabled={workloadSaving || !client}
                  >
                    {workloadSaving ? (
                      <ActivityIndicator color={colors.onBrand} />
                    ) : (
                      <Text style={styles.actionButtonText}>設定を保存</Text>
                    )}
                  </PressableScale>
                </>
              )}
            </>
          )}

          {category === 'solver' && (
            <>
              {solverLoading ? (
                <ActivityIndicator color={colors.brand} style={styles.loader} />
              ) : (
                <>
                  <View style={styles.settingRow}>
                    <Text
                      style={[styles.settingLabel, { color: colors.black }]}
                    >
                      Solver
                    </Text>
                    <Menu
                      visible={solverMenuVisible}
                      onDismiss={() => setSolverMenuVisible(false)}
                      anchor={
                        <PressableScale
                          onPress={() => setSolverMenuVisible(true)}
                          style={[
                            styles.themeDropdown,
                            {
                              backgroundColor: colors.surface,
                              borderColor: colors.separator,
                            },
                          ]}
                        >
                          <Text
                            style={[
                              styles.themeDropdownText,
                              { color: colors.black },
                            ]}
                          >
                            {solverLabel(solverValue)}
                          </Text>
                          <Ionicons
                            name="chevron-down"
                            size={16}
                            color={colors.black}
                          />
                        </PressableScale>
                      }
                    >
                      {SOLVER_OPTIONS.map((s) => (
                        <Menu.Item
                          key={s}
                          title={solverLabel(s)}
                          leadingIcon={solverValue === s ? 'check' : undefined}
                          onPress={() => {
                            setSolverMenuVisible(false);
                            haptic.select();
                            setSolverValue(s);
                          }}
                        />
                      ))}
                    </Menu>
                  </View>

                  <View style={styles.field}>
                    <Text style={[styles.label, { color: colors.gray }]}>
                      求解時間の上限（ミリ秒）
                    </Text>
                    <TextInput
                      style={[
                        styles.input,
                        { borderColor: colors.separator, color: colors.black },
                      ]}
                      value={timeBudgetInput}
                      onChangeText={setTimeBudgetInput}
                      keyboardType="numeric"
                      placeholder="0（制限なし）"
                      placeholderTextColor={colors.gray}
                    />
                  </View>

                  <View style={styles.field}>
                    <Text style={[styles.label, { color: colors.gray }]}>
                      乱数シード
                    </Text>
                    <TextInput
                      style={[
                        styles.input,
                        { borderColor: colors.separator, color: colors.black },
                      ]}
                      value={seedInput}
                      onChangeText={setSeedInput}
                      keyboardType="numeric"
                      placeholder="0（デフォルト）"
                      placeholderTextColor={colors.gray}
                    />
                  </View>

                  <View style={styles.settingRow}>
                    <Text
                      style={[styles.settingLabel, { color: colors.black }]}
                    >
                      Warm start
                    </Text>
                    <Switch
                      value={warmStartValue}
                      onValueChange={(v) => {
                        haptic.select();
                        setWarmStartValue(v);
                      }}
                      trackColor={{ true: colors.brand }}
                    />
                  </View>

                  <PressableScale
                    style={[
                      styles.actionButton,
                      { backgroundColor: colors.brand },
                    ]}
                    onPress={() => {
                      haptic.medium();
                      saveSolverSettings();
                    }}
                    disabled={solverSaving || !client || !solverDirty}
                  >
                    {solverSaving ? (
                      <ActivityIndicator color={colors.onBrand} />
                    ) : (
                      <Text style={styles.actionButtonText}>設定を保存</Text>
                    )}
                  </PressableScale>
                </>
              )}
            </>
          )}

          {category === 'notifications' && (
            <>
              {/* Master toggle */}
              <View style={styles.settingRow}>
                <Text style={[styles.settingLabel, { color: colors.black }]}>
                  通知を有効化
                </Text>
                <Switch
                  value={notifications.enabled}
                  onValueChange={(v) => {
                    haptic.select();
                    setNotifications({ ...notifications, enabled: v });
                  }}
                  trackColor={{ true: colors.brand }}
                />
              </View>

              {notifications.enabled && (
                <>
                  <ExactAlarmPermissionRow />

                  {/* Morning briefing */}
                  <View style={styles.notifGroup}>
                    <View style={styles.settingRow}>
                      <Text
                        style={[styles.settingLabel, { color: colors.black }]}
                      >
                        朝のブリーフィング
                      </Text>
                      <Switch
                        value={notifications.morningBriefing}
                        onValueChange={(v) => {
                          haptic.select();
                          setNotifications({
                            ...notifications,
                            morningBriefing: v,
                          });
                        }}
                        trackColor={{ true: colors.brand }}
                      />
                    </View>
                    {notifications.morningBriefing && (
                      <PressableScale
                        style={[
                          styles.timeField,
                          { borderColor: colors.separator },
                        ]}
                        onPress={() => {
                          haptic.select();
                          setNotifPickerField('morningBriefing');
                        }}
                      >
                        <Text
                          style={[styles.timeText, { color: colors.black }]}
                        >
                          {formatTime(notifications.morningBriefingTime)}
                        </Text>
                      </PressableScale>
                    )}
                  </View>

                  {/* Pre-start reminder */}
                  <View style={styles.notifGroup}>
                    <View style={styles.settingRow}>
                      <Text
                        style={[styles.settingLabel, { color: colors.black }]}
                      >
                        開始直前リマインダー
                      </Text>
                      <Switch
                        value={notifications.preStartReminder}
                        onValueChange={(v) => {
                          haptic.select();
                          setNotifications({
                            ...notifications,
                            preStartReminder: v,
                          });
                        }}
                        trackColor={{ true: colors.brand }}
                      />
                    </View>
                    {notifications.preStartReminder && (
                      <View style={styles.field}>
                        <Text style={[styles.label, { color: colors.gray }]}>
                          何分前から通知するか
                        </Text>
                        <TextInput
                          style={[
                            styles.input,
                            {
                              borderColor: colors.separator,
                              color: colors.black,
                            },
                          ]}
                          value={preStartInput}
                          onChangeText={setPreStartInput}
                          onBlur={commitPreStart}
                          onSubmitEditing={commitPreStart}
                          keyboardType="numeric"
                          placeholder="10"
                          placeholderTextColor={colors.gray}
                        />
                      </View>
                    )}
                  </View>

                  {/* Start overdue */}
                  <View style={styles.settingRow}>
                    <Text
                      style={[styles.settingLabel, { color: colors.black }]}
                    >
                      開始時間到着通知
                    </Text>
                    <Switch
                      value={notifications.startOverdue}
                      onValueChange={(v) => {
                        haptic.select();
                        setNotifications({ ...notifications, startOverdue: v });
                      }}
                      trackColor={{ true: colors.brand }}
                    />
                  </View>

                  {/* End time */}
                  <View style={styles.settingRow}>
                    <Text
                      style={[styles.settingLabel, { color: colors.black }]}
                    >
                      タスク終了時間通知
                    </Text>
                    <Switch
                      value={notifications.endTime}
                      onValueChange={(v) => {
                        haptic.select();
                        setNotifications({ ...notifications, endTime: v });
                      }}
                      trackColor={{ true: colors.brand }}
                    />
                  </View>

                  {/* Unscheduled idle */}
                  <View style={styles.notifGroup}>
                    <View style={styles.settingRow}>
                      <Text
                        style={[styles.settingLabel, { color: colors.black }]}
                      >
                        未スケジュール放置通知
                      </Text>
                      <Switch
                        value={notifications.unscheduledIdle}
                        onValueChange={(v) => {
                          haptic.select();
                          setNotifications({
                            ...notifications,
                            unscheduledIdle: v,
                          });
                        }}
                        trackColor={{ true: colors.brand }}
                      />
                    </View>
                    {notifications.unscheduledIdle && (
                      <View style={styles.field}>
                        <Text style={[styles.label, { color: colors.gray }]}>
                          何時間放置で通知 (時間)
                        </Text>
                        <TextInput
                          style={[
                            styles.input,
                            {
                              borderColor: colors.separator,
                              color: colors.black,
                            },
                          ]}
                          value={idleHoursInput}
                          onChangeText={setIdleHoursInput}
                          onBlur={commitIdleHours}
                          onSubmitEditing={commitIdleHours}
                          keyboardType="numeric"
                          placeholder="24"
                          placeholderTextColor={colors.gray}
                        />
                      </View>
                    )}
                  </View>

                  {/* In-progress */}
                  <View style={styles.settingRow}>
                    <Text
                      style={[styles.settingLabel, { color: colors.black }]}
                    >
                      タスク実行中通知
                    </Text>
                    <Switch
                      value={notifications.inProgress}
                      onValueChange={(v) => {
                        haptic.select();
                        setNotifications({ ...notifications, inProgress: v });
                      }}
                      trackColor={{ true: colors.brand }}
                    />
                  </View>
                </>
              )}
            </>
          )}

          {category === 'worker' && (
            <>
              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  エンドポイント
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={workerUrl}
                  onChangeText={(v) => {
                    setWorkerUrl(v);
                    setWorkerDirty(true);
                  }}
                  placeholder="https://your-worker.workers.dev"
                  placeholderTextColor={colors.gray}
                  autoCapitalize="none"
                  autoCorrect={false}
                />
              </View>
              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  Root Token (JWT)
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={workerKey}
                  onChangeText={(v) => {
                    setWorkerKey(v);
                    setWorkerDirty(true);
                  }}
                  placeholder="eyJ..."
                  placeholderTextColor={colors.gray}
                  secureTextEntry
                  autoCapitalize="none"
                  autoCorrect={false}
                />
              </View>

              {workerDirty && (
                <Text style={[styles.warning, { color: colors.red }]}>
                  未保存の変更があります
                </Text>
              )}

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.medium();
                  saveWorkerSettings();
                }}
                disabled={!workerDirty}
              >
                <Text style={styles.actionButtonText}>保存</Text>
              </PressableScale>

              {/* Health checks */}
              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  ヘルスチェック
                </Text>
              </View>

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.light();
                  checkLocalHealth();
                }}
                disabled={localHealthLoading || !client}
              >
                {localHealthLoading ? (
                  <ActivityIndicator color={colors.onBrand} />
                ) : (
                  <Text style={styles.actionButtonText}>ローカルサーバー</Text>
                )}
              </PressableScale>
              {localHealthResult && (
                <View style={styles.healthResultRow}>
                  <Ionicons
                    name={localHealthResult.ok ? 'checkmark' : 'close'}
                    size={16}
                    color={localHealthResult.ok ? colors.black : colors.red}
                  />
                  <Text
                    style={[
                      styles.healthResult,
                      {
                        color: localHealthResult.ok ? colors.black : colors.red,
                      },
                    ]}
                  >
                    {localHealthResult.message}
                  </Text>
                </View>
              )}

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.light();
                  checkWorkerHealth();
                }}
                disabled={workerHealthLoading || !client}
              >
                {workerHealthLoading ? (
                  <ActivityIndicator color={colors.onBrand} />
                ) : (
                  <Text style={styles.actionButtonText}>Worker</Text>
                )}
              </PressableScale>
              {workerHealthResult && (
                <View style={styles.healthResultRow}>
                  <Ionicons
                    name={workerHealthResult.ok ? 'checkmark' : 'close'}
                    size={16}
                    color={workerHealthResult.ok ? colors.black : colors.red}
                  />
                  <Text
                    style={[
                      styles.healthResult,
                      {
                        color: workerHealthResult.ok
                          ? colors.black
                          : colors.red,
                      },
                    ]}
                  >
                    {workerHealthResult.message}
                  </Text>
                </View>
              )}
            </>
          )}

          {category === 'google' && (
            <>
              {gcalLoading && (
                <ActivityIndicator color={colors.brand} style={styles.loader} />
              )}

              <View style={styles.settingRow}>
                <Text style={[styles.settingLabel, { color: colors.black }]}>
                  有効化
                </Text>
                <Switch
                  value={gcalEnabled}
                  onValueChange={(v) => {
                    haptic.select();
                    setGcalEnabled(v);
                  }}
                  trackColor={{ true: colors.brand }}
                />
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  Calendar ID
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalCalendarId}
                  onChangeText={setGcalCalendarId}
                  placeholder="primary"
                  placeholderTextColor={colors.gray}
                  autoCapitalize="none"
                  autoCorrect={false}
                />
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  Client ID
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalClientId}
                  onChangeText={setGcalClientId}
                  placeholder="xxxxx.apps.googleusercontent.com"
                  placeholderTextColor={colors.gray}
                  autoCapitalize="none"
                  autoCorrect={false}
                />
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  Client Secret
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalClientSecret}
                  onChangeText={setGcalClientSecret}
                  placeholder={
                    gcalSettings?.has_client_secret
                      ? '設定済み (入力で上書き)'
                      : 'GOCSPX-...'
                  }
                  placeholderTextColor={colors.gray}
                  secureTextEntry
                  autoCapitalize="none"
                  autoCorrect={false}
                />
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  リマインダー時間（分）
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalReminderMinutes}
                  onChangeText={setGcalReminderMinutes}
                  placeholder="15"
                  placeholderTextColor={colors.gray}
                  keyboardType="numeric"
                  autoCapitalize="none"
                  autoCorrect={false}
                />
                <Text style={[styles.helpText, { color: colors.gray }]}>
                  空欄にすると未設定（Google Calendarのデフォルト）になります
                </Text>
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  色 ID（1〜11）
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalColorId}
                  onChangeText={setGcalColorId}
                  placeholder="5"
                  placeholderTextColor={colors.gray}
                  keyboardType="numeric"
                  autoCapitalize="none"
                  autoCorrect={false}
                />
                <Text style={[styles.helpText, { color: colors.gray }]}>
                  空欄にすると未設定（Google Calendarのデフォルト）になります
                </Text>
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  公開範囲
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalVisibility}
                  onChangeText={setGcalVisibility}
                  placeholder="default"
                  placeholderTextColor={colors.gray}
                  autoCapitalize="none"
                  autoCorrect={false}
                />
                <Text style={[styles.helpText, { color: colors.gray }]}>
                  default / public / private / confidential（空欄で未設定）
                </Text>
              </View>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  予定/空き状態
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalTransparency}
                  onChangeText={setGcalTransparency}
                  placeholder="opaque"
                  placeholderTextColor={colors.gray}
                  autoCapitalize="none"
                  autoCorrect={false}
                />
                <Text style={[styles.helpText, { color: colors.gray }]}>
                  opaque / transparent（空欄で未設定）
                </Text>
              </View>

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.medium();
                  saveGcalSettings();
                }}
              >
                <Text style={styles.actionButtonText}>設定を保存</Text>
              </PressableScale>

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.medium();
                  startGoogleOAuth();
                }}
                disabled={
                  oauthLoading ||
                  !gcalSettings?.client_id ||
                  !gcalSettings?.has_client_secret
                }
              >
                {oauthLoading ? (
                  <ActivityIndicator color={colors.onBrand} />
                ) : (
                  <Text style={styles.actionButtonText}>Googleでログイン</Text>
                )}
              </PressableScale>

              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  Refresh Token
                </Text>
                <TextInput
                  style={[
                    styles.input,
                    { borderColor: colors.separator, color: colors.black },
                  ]}
                  value={gcalRefreshToken}
                  onChangeText={setGcalRefreshToken}
                  placeholder={
                    gcalSettings?.has_refresh_token
                      ? '設定済み (入力で上書き)'
                      : 'CLIでOAuth実行後に貼り付け'
                  }
                  placeholderTextColor={colors.gray}
                  secureTextEntry
                  autoCapitalize="none"
                  autoCorrect={false}
                />
                <Text style={[styles.helpText, { color: colors.gray }]}>
                  ネイティブサインインが使えない場合のフォールバックです。 CLIで
                  `takusu sync login --client-id 〜 --client-secret 〜`
                  を実行して取得したトークンを貼り付けてください
                </Text>
              </View>

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.medium();
                  saveRefreshToken();
                }}
                disabled={!gcalRefreshToken.trim()}
              >
                <Text style={styles.actionButtonText}>Refresh Tokenを保存</Text>
              </PressableScale>

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.medium();
                  triggerSync();
                }}
                disabled={syncLoading || !gcalSettings?.has_refresh_token}
              >
                {syncLoading ? (
                  <ActivityIndicator color={colors.onBrand} />
                ) : (
                  <Text style={styles.actionButtonText}>手動同期</Text>
                )}
              </PressableScale>

              <PressableScale
                style={[
                  styles.actionButton,
                  { backgroundColor: colors.destructive },
                ]}
                onPress={() => {
                  haptic.medium();
                  deleteAllGcalEvents();
                }}
                disabled={
                  deleteAllLoading ||
                  gcalLoading ||
                  !gcalSettings?.has_refresh_token
                }
              >
                {deleteAllLoading ? (
                  <ActivityIndicator color={colors.onBrand} />
                ) : (
                  <Text style={styles.actionButtonText}>
                    Google Calendarイベントを全削除
                  </Text>
                )}
              </PressableScale>
            </>
          )}

          {category === 'info' && (
            <>
              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  バージョン
                </Text>
                <Text style={[styles.value, { color: colors.black }]}>
                  {appVersion} (build {buildVersion})
                </Text>
              </View>
              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>Git</Text>
                <Text style={[styles.value, { color: colors.black }]}>
                  {gitTag} @ {gitCommit}
                </Text>
              </View>
              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>
                  ライセンス
                </Text>
                <PressableScale
                  style={[
                    styles.categoryRow,
                    { borderBottomColor: colors.separator },
                  ]}
                  onPress={() => {
                    haptic.select();
                    router.push('/settings/licenses');
                  }}
                >
                  <Text style={[styles.categoryLabel, { color: colors.black }]}>
                    ライセンス一覧
                  </Text>
                  <Ionicons
                    name="chevron-forward"
                    size={20}
                    color={colors.gray}
                  />
                </PressableScale>
              </View>

              {/* Log export */}
              <View style={styles.field}>
                <Text style={[styles.label, { color: colors.gray }]}>ログ</Text>
              </View>

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.light();
                  exportLogs();
                }}
                disabled={logExportLoading}
              >
                {logExportLoading ? (
                  <ActivityIndicator color={colors.onBrand} />
                ) : (
                  <Text style={styles.actionButtonText}>
                    ログをエクスポート
                  </Text>
                )}
              </PressableScale>

              <PressableScale
                style={[styles.actionButton, { backgroundColor: colors.brand }]}
                onPress={() => {
                  haptic.light();
                  copyLogs();
                }}
                disabled={logCopyLoading || logExportLoading}
              >
                {logCopyLoading ? (
                  <ActivityIndicator color={colors.onBrand} />
                ) : (
                  <Text style={styles.actionButtonText}>ログをコピー</Text>
                )}
              </PressableScale>

              <PressableScale
                style={[
                  styles.actionButton,
                  { backgroundColor: colors.destructive },
                ]}
                onPress={() => {
                  haptic.medium();
                  clearLogs();
                }}
              >
                <Text style={styles.actionButtonText}>ログを消去</Text>
              </PressableScale>
            </>
          )}
        </ScrollView>
      </View>

      {/* Notification time picker modal */}
      {notifPickerField && (
        <DateTimePickerModal
          visible={true}
          mode="time"
          label="通知時刻"
          value={(() => {
            const min = notifications.morningBriefingTime;
            const { hour, minute } = minutesToTime(min);
            const d = new Date();
            d.setHours(hour, minute, 0, 0);
            return d;
          })()}
          onConfirm={(date) => {
            if (!date) {
              setNotifPickerField(null);
              return;
            }
            const minutes = timeToMinutes(date.getHours(), date.getMinutes());
            if (notifPickerField === 'morningBriefing') {
              setNotifications({
                ...notifications,
                morningBriefingTime: minutes,
              });
            }
            setNotifPickerField(null);
          }}
          onCancel={() => setNotifPickerField(null)}
        />
      )}

      {/* Sleep time picker modal */}
      {sleepPickerField && (
        <DateTimePickerModal
          visible={true}
          mode="time"
          label={sleepPickerField === 'start' ? '就寝時刻' : '起床時刻'}
          value={(() => {
            const s = sleepPickerField === 'start' ? sleepStart : sleepEnd;
            const [h, m] = s.split(':').map((n) => parseInt(n, 10) || 0);
            const d = new Date();
            d.setHours(h, m, 0, 0);
            return d;
          })()}
          onConfirm={(date) => {
            if (!date) {
              setSleepPickerField(null);
              return;
            }
            const hh = date.getHours().toString().padStart(2, '0');
            const mm = date.getMinutes().toString().padStart(2, '0');
            const formatted = `${hh}:${mm}`;
            if (sleepPickerField === 'start') {
              setSleepStart(formatted);
            } else {
              setSleepEnd(formatted);
            }
            setSleepPickerField(null);
          }}
          onCancel={() => setSleepPickerField(null)}
        />
      )}
    </View>
  );
}
