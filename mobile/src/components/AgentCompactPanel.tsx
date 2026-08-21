import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ActivityIndicator,
  Modal,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { PressableScale } from '@/src/components/PressableScale';
import { haptic } from '@/src/components/haptics';
import { useTheme, type ColorSet } from '@/src/theme';
import { AgentClient, AgentApiError } from '@/src/api/agentClient';
import { showError } from '@/src/api/errors';
import {
  loadSettings,
  saveAgentProviders,
  type PermissionsMap,
} from '@/src/api/settingsStore';
import { ApprovalPanel } from '@/src/components/ApprovalPanel';
import type {
  Action,
  ActionCapability,
  AgentStreamEvent,
  ApprovalRequest,
  CapabilityRequest,
  Presentation,
  ProposalDecision,
  SurfaceSnapshot,
  UserInputQuestion,
} from '@/src/api/agentTypes';

interface QuickActionSpec {
  mode: 'complete' | 'delay' | 'progress';
  taskId: string;
  taskTitle: string;
  taskReference: string;
  onSuccess: () => Promise<void>;
}

interface SurfaceSpec {
  /** Transcribed utterance to send as a turn. */
  transcript?: string;
  /** Optional existing session to resume. */
  sessionId?: string;
  /** Called when the surface turn finishes, whether approved or not. */
  onComplete?: () => void;
}

interface AgentCompactPanelProps {
  visible: boolean;
  onClose: () => void;
  /** Agent client for the active session or quick action. */
  agentClient?: AgentClient | null;
  /** Current surface snapshot, used for the title in surface mode. */
  snapshot?: SurfaceSnapshot | null;
  /** Quick-action mode (Phase 1). Provide `quickAction` or old props. */
  quickAction?: QuickActionSpec;
  /** Surface mode (WI-6). Renders a voice turn transcript and result. */
  surface?: SurfaceSpec;
  // Backwards-compatible Phase 1 quick-action props.
  mode?: 'complete' | 'delay' | 'progress';
  taskId?: string;
  taskTitle?: string;
  taskReference?: string;
  onSuccess?: () => Promise<void>;
}

const SNOOZE_OPTIONS = [
  { minutes: 10, label: '10分後' },
  { minutes: 30, label: '30分後' },
  { minutes: 60, label: '1時間後' },
];

function decodeError(e: unknown): string {
  if (e instanceof AgentApiError) {
    try {
      const parsed = JSON.parse(e.body);
      if (typeof parsed.error === 'string') return parsed.error;
    } catch {
      // fall through
    }
    return `Agent API error ${e.status}`;
  }
  return e instanceof Error ? e.message : String(e);
}

function stateLabel(snapshot: SurfaceSnapshot | null): string {
  if (!snapshot) return '待機中';
  switch (snapshot.state) {
    case 'idle':
      return '待機中';
    case 'listening':
      return '聞いています';
    case 'transcribing':
      return '書き起こし中';
    case 'thinking':
      return '考え中';
    case 'waiting_for_user':
      return '確認待ち';
    case 'waiting_for_approval':
      return '承認待ち';
    case 'speaking':
      return '話しています';
    case 'error':
      return 'エラー';
  }
}

function formatTime(iso?: string): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleTimeString('ja-JP', {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  });
}

function CompactPresentationView({
  presentation,
  onAction,
  onClarify,
}: {
  presentation: Presentation;
  onAction?: (capability: ActionCapability) => void;
  onClarify?: (choice: string) => void;
}) {
  const { colors } = useTheme();
  const styles = useMemo(() => makePresentationStyles(colors), [colors]);

  switch (presentation.type) {
    case 'work_transition':
      return (
        <View style={styles.result}>
          <Text style={styles.resultTitle}>
            {presentation.title} {presentation.reference}
          </Text>
          {presentation.detail ? (
            <Text style={styles.resultDetail}>{presentation.detail}</Text>
          ) : null}
        </View>
      );
    case 'current_task':
      return (
        <View style={styles.result}>
          <Text style={styles.resultTitle}>
            {presentation.title} {presentation.reference}
          </Text>
          <Text style={styles.resultDetail}>
            {formatTime(presentation.start_at)}–
            {formatTime(presentation.end_at)} ·{' '}
            {presentation.work_state === 'in_progress'
              ? '作業中'
              : presentation.work_state === 'overdue'
                ? '遅延'
                : '未着手'}
          </Text>
        </View>
      );
    case 'schedule_summary':
      return (
        <View style={styles.result}>
          {presentation.next ? (
            <Text style={styles.resultDetail}>
              次: {presentation.next.title}{' '}
              {formatTime(presentation.next.start_at)}
            </Text>
          ) : null}
          {presentation.entries && presentation.entries.length > 0 ? (
            <Text style={styles.resultDetail}>
              {presentation.entries.length} 件の予定
            </Text>
          ) : null}
        </View>
      );
    case 'progress_summary':
      return (
        <View style={styles.result}>
          <Text style={styles.resultDetail}>
            完了 {presentation.done} / 進行中 {presentation.in_progress} / 予定{' '}
            {presentation.scheduled}
          </Text>
        </View>
      );
    case 'schedule_alert':
      return (
        <View style={[styles.result, { borderColor: colors.warningBorder }]}>
          <Text style={styles.resultTitle}>
            {presentation.kind === 'conflict'
              ? '競合'
              : presentation.kind === 'overdue'
                ? '期限超過'
                : '生成失敗'}
          </Text>
          <Text style={styles.resultDetail}>{presentation.message}</Text>
        </View>
      );
    case 'check_in': {
      const ActionButton = (action: Action) => (
        <PressableScale
          key={action.id}
          style={[
            styles.action,
            action.kind === 'immediate' ? styles.primaryAction : null,
          ]}
          onPress={() => {
            if (action.kind === 'immediate' && action.capability && onAction) {
              onAction(action.capability);
            }
          }}
          disabled={action.kind !== 'immediate' || !action.capability}
        >
          <Text
            style={[
              styles.actionText,
              action.kind === 'immediate' ? styles.primaryActionText : null,
            ]}
          >
            {action.label}
          </Text>
        </PressableScale>
      );
      return (
        <View style={styles.result}>
          <Text style={styles.resultTitle}>{presentation.question}</Text>
          <Text style={styles.resultDetail}>{presentation.act.title}</Text>
          <View style={styles.actions}>
            {presentation.act.actions.map(ActionButton)}
          </View>
          <Text style={styles.resultDetail}>{presentation.shift.title}</Text>
          <View style={styles.actions}>
            {presentation.shift.actions.map(ActionButton)}
          </View>
        </View>
      );
    }
    case 'clarification':
      // Choices are shown as tappable only when the parent provides a handler.
      // A full follow-up turn from the compact panel is not wired yet.
      return (
        <View style={styles.result}>
          <Text style={styles.resultTitle}>{presentation.message}</Text>
          {presentation.choices ? (
            <View style={styles.actions}>
              {presentation.choices.map((choice, i) => (
                <PressableScale
                  key={i}
                  style={styles.action}
                  disabled={!onClarify}
                  onPress={onClarify ? () => onClarify(choice) : undefined}
                >
                  <Text style={styles.actionText}>{choice}</Text>
                </PressableScale>
              ))}
            </View>
          ) : null}
        </View>
      );
    case 'change_proposal':
      // Approval requests are rendered by the dedicated approval UI.
      return (
        <View style={styles.result}>
          <Text style={styles.resultTitle}>{presentation.why}</Text>
        </View>
      );
    case 'text':
      return (
        <View style={styles.result}>
          <Text style={styles.resultDetail}>{presentation.text}</Text>
        </View>
      );
    default:
      return (
        <View style={styles.result}>
          <Text style={styles.resultDetail}>結果を受け取りました</Text>
        </View>
      );
  }
}

const makePresentationStyles = (colors: ColorSet) =>
  StyleSheet.create({
    result: {
      padding: 12,
      borderRadius: 10,
      backgroundColor: colors.surfaceTint,
      gap: 6,
      borderWidth: 1,
      borderColor: colors.cardOutline,
    },
    resultTitle: {
      fontSize: 15,
      fontWeight: '600',
      color: colors.black,
    },
    resultDetail: {
      fontSize: 13,
      color: colors.textOnCard,
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
      backgroundColor: colors.surface,
    },
    primaryAction: {
      backgroundColor: colors.brand,
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

function AgentCompactPanelImpl({
  visible,
  onClose,
  agentClient,
  snapshot: surfaceSnapshot,
  quickAction: quickActionProp,
  surface: surfaceProp,
  mode: modeProp,
  taskId: taskIdProp,
  taskTitle: taskTitleProp,
  taskReference: taskReferenceProp,
  onSuccess: onSuccessProp,
}: AgentCompactPanelProps) {
  const { colors } = useTheme();
  const styles = useMemo(() => makeStyles(colors), [colors]);

  const quickAction = useMemo<QuickActionSpec | undefined>(() => {
    if (quickActionProp) return quickActionProp;
    if (
      modeProp &&
      taskIdProp &&
      taskTitleProp &&
      taskReferenceProp &&
      onSuccessProp
    ) {
      return {
        mode: modeProp,
        taskId: taskIdProp,
        taskTitle: taskTitleProp,
        taskReference: taskReferenceProp,
        onSuccess: onSuccessProp,
      };
    }
    return undefined;
  }, [
    quickActionProp,
    modeProp,
    taskIdProp,
    taskTitleProp,
    taskReferenceProp,
    onSuccessProp,
  ]);

  const isSurface = surfaceProp !== undefined;

  // ── Quick-action state ─────────────────────────────────────────────────
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<Presentation | null>(null);
  const [quantity, setQuantity] = useState('');
  const [note, setNote] = useState('');

  // ── Surface state ─────────────────────────────────────────────────────
  const [surfaceBusy, setSurfaceBusy] = useState(false);
  const [transcript, setTranscript] = useState('');
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [turnText, setTurnText] = useState('');
  const [turnError, setTurnError] = useState<string | null>(null);
  const [turnPresentation, setTurnPresentation] = useState<Presentation | null>(
    null,
  );
  const [approval, setApproval] = useState<ApprovalRequest | null>(null);
  const [approvalBusy, setApprovalBusy] = useState(false);
  const [sessionPermissions, setSessionPermissions] = useState<PermissionsMap>(
    {},
  );

  const abortRef = useRef<AbortController | null>(null);
  const pendingSessionRef = useRef<string | null>(null);

  useEffect(() => {
    if (!visible) {
      // Reset quick-action form state on close.
      setResult(null);
      setQuantity('');
      setNote('');

      // Abort an ongoing surface turn when the panel closes.
      if (abortRef.current) {
        abortRef.current.abort();
        abortRef.current = null;
      }
      // Ensure the surface busy flag and pending session cache are cleared so
      // the cleanup effect downstream can also reset its state.
      setSurfaceBusy(false);
      pendingSessionRef.current = null;
      return;
    }

    if (!isSurface || !agentClient) return;

    const spec = surfaceProp ?? {};
    if (!spec.transcript && !spec.sessionId) {
      // No transcript: the panel was opened from a surface command; just show
      // the current state.
      setTranscript('');
      return;
    }

    setTranscript(spec.transcript ?? '');
    setTurnText('');
    setTurnError(null);
    setTurnPresentation(null);
    setApproval(null);
    setSurfaceBusy(true);

    const abort = new AbortController();
    abortRef.current = abort;

    const start = async () => {
      try {
        let sid = spec.sessionId ?? pendingSessionRef.current ?? null;
        if (!sid) {
          sid = await agentClient.createSession();
          pendingSessionRef.current = sid;
        }
        setSessionId(sid);

        const text = spec.transcript ?? '';
        if (!text.trim()) {
          setSurfaceBusy(false);
          return;
        }

        await agentClient.runTurnStream(
          sid,
          text,
          `compact-${Date.now()}`,
          (event: AgentStreamEvent) => {
            if (abort.signal.aborted) return;
            if (event.type === 'TtsBlock') {
              // TTS is handled by the server-side queue; the compact panel
              // only shows text and state.
              return;
            }
            switch (event.type) {
              case 'Thinking':
                setTurnText((current) => current + event.data);
                break;
              case 'Text':
                setTurnText((current) => current + event.data);
                break;
              case 'ToolCall':
                if (event.data.name === 'correct_asr') {
                  const args = event.data.arguments as
                    | { questions: UserInputQuestion[] }
                    | undefined;
                  if (args?.questions && args.questions.length > 0) {
                    setTurnText(
                      (current) =>
                        current +
                        '\n' +
                        args.questions.map((q) => q.text).join('\n'),
                    );
                  }
                }
                break;
              case 'ToolResult':
                // Surface turn tool results are intentionally terse; the final
                // presentation carries the user-facing summary.
                break;
              case 'Error':
                setTurnError(event.data);
                setSurfaceBusy(false);
                break;
              case 'Done':
                setTurnText(event.data.text);
                setTurnPresentation(
                  event.data.presentation ?? {
                    type: 'text',
                    text: event.data.text,
                  },
                );
                setApproval(event.data.approval_request ?? null);
                setSurfaceBusy(false);
                spec.onComplete?.();
                break;
            }
          },
          abort.signal,
        );
      } catch (e) {
        if (abort.signal.aborted) return;
        const message = e instanceof Error ? e.message : String(e);
        setTurnError(message);
        setSurfaceBusy(false);
      }
    };

    void start();

    return () => {
      setSurfaceBusy(false);
      pendingSessionRef.current = null;
      abort.abort();
      if (abortRef.current === abort) {
        abortRef.current = null;
      }
    };
  }, [visible, isSurface, surfaceProp, agentClient]);

  // Reset the pending session cache when the voice turn ends or is closed.
  useEffect(() => {
    if (!visible && !surfaceBusy) {
      pendingSessionRef.current = null;
    }
  }, [visible, surfaceBusy]);

  const handleQuickAction = useCallback(
    async (request: CapabilityRequest) => {
      if (!agentClient || loading) return;
      setLoading(true);
      try {
        const presentation = await agentClient.quickAction(request);
        setResult(presentation);
        await quickAction?.onSuccess();
      } catch (e) {
        showError(e, decodeError(e));
      } finally {
        setLoading(false);
      }
    },
    [agentClient, loading, quickAction],
  );

  const handleComplete = useCallback(() => {
    if (!quickAction) return;
    haptic.light();
    handleQuickAction({
      task_id: quickAction.taskId,
      action: 'complete',
      device_id: 'mobile',
      input_path: 'screen_capability',
    });
  }, [handleQuickAction, quickAction]);

  const handleDelay = useCallback(
    (minutes: number) => {
      if (!quickAction) return;
      haptic.light();
      handleQuickAction({
        task_id: quickAction.taskId,
        action: 'delay',
        device_id: 'mobile',
        input_path: 'screen_capability',
        snooze_minutes: minutes,
      });
    },
    [handleQuickAction, quickAction],
  );

  const handleProgress = useCallback(() => {
    if (!quickAction) return;
    haptic.light();
    const n = Number(quantity);
    if (
      !Number.isFinite(n) ||
      !Number.isInteger(n) ||
      n <= 0 ||
      n > Number.MAX_SAFE_INTEGER
    ) {
      showError(
        new Error(
          '1 以上 ' + Number.MAX_SAFE_INTEGER + ' 以下の整数を入力してください',
        ),
        '進捗の記録に失敗',
      );
      return;
    }
    handleQuickAction({
      task_id: quickAction.taskId,
      action: 'progress',
      device_id: 'mobile',
      input_path: 'screen_capability',
      quantity_done: n,
      note: note.trim() || undefined,
    });
  }, [handleQuickAction, quickAction, quantity, note]);

  const handleClose = useCallback(() => {
    setResult(null);
    setQuantity('');
    setNote('');
    setTurnText('');
    setTurnError(null);
    setTurnPresentation(null);
    setApproval(null);
    onClose();
  }, [onClose]);

  const handleApprove = useCallback(
    async (
      approve: boolean,
      decisions: ProposalDecision[] = [],
      grantedPermissions?: PermissionsMap,
      persistToProvider?: boolean,
    ) => {
      if (!agentClient || !sessionId || !approval || approvalBusy) return;
      setApprovalBusy(true);
      try {
        const resolution = await agentClient.resolveApproval(
          sessionId,
          approval.id,
          approve,
          `compact-approval-${Date.now()}`,
          decisions,
        );

        const positiveGranted: PermissionsMap = {};
        if (approve && grantedPermissions) {
          for (const [key, value] of Object.entries(grantedPermissions)) {
            if (value) positiveGranted[key] = true;
          }
        }
        let newSessionPermissions: PermissionsMap | undefined;
        if (Object.keys(positiveGranted).length > 0) {
          newSessionPermissions = { ...sessionPermissions, ...positiveGranted };
        }

        if (resolution.approved && newSessionPermissions) {
          setSessionPermissions(newSessionPermissions);
          await agentClient
            .updateSessionSettings(sessionId, newSessionPermissions)
            .catch((e: unknown) => {
              const message = e instanceof Error ? e.message : String(e);
              const errorText = `セッション権限の保存に失敗しました: ${message}`;
              void showError(errorText, '保存失敗');
              console.error('Failed to update session permissions:', e);
            });

          if (persistToProvider) {
            (async () => {
              try {
                const settings = await loadSettings();
                const active = settings.llmModels.find(
                  (m) => m.id === settings.activeLlmModel,
                );
                if (!active) return;
                const updatedModels = settings.llmModels.map((m) =>
                  m.id === active.id
                    ? {
                        ...m,
                        permissions: {
                          ...(m.permissions ?? {}),
                          ...positiveGranted,
                        },
                      }
                    : m,
                );
                await saveAgentProviders(
                  settings.llmProviders,
                  updatedModels,
                  settings.activeLlmModel,
                  settings.ttsProviders,
                  settings.activeTtsProvider,
                );
              } catch (e: unknown) {
                const message = e instanceof Error ? e.message : String(e);
                const errorText = `プロバイダ設定の保存に失敗しました: ${message}`;
                void showError(errorText, '保存失敗');
                console.error('Failed to persist provider permissions:', e);
              }
            })();
          }
        }

        setApproval(null);
        setTurnPresentation(null);
        setTurnText(approve ? '承認しました' : '拒否しました');
      } catch (e) {
        showError(e, decodeError(e));
      } finally {
        setApprovalBusy(false);
      }
    },
    [agentClient, sessionId, approval, approvalBusy, sessionPermissions],
  );

  const handlePresentationAction = useCallback(
    async (capability: ActionCapability) => {
      if (!agentClient) return;
      try {
        const presentation = await agentClient.authorizeAction(capability);
        if (isSurface) {
          setTurnPresentation(presentation);
        } else {
          setResult(presentation);
          await quickAction?.onSuccess();
        }
      } catch (e) {
        showError(e, decodeError(e));
      }
    },
    [agentClient, isSurface, quickAction],
  );

  const title = useMemo(() => {
    if (isSurface) {
      return stateLabel(surfaceSnapshot ?? null);
    }
    switch (quickAction?.mode) {
      case 'complete':
        return '完了';
      case 'delay':
        return '延期';
      case 'progress':
        return '進捗';
      default:
        return '';
    }
  }, [isSurface, surfaceSnapshot, quickAction?.mode]);

  const reference = useMemo(() => {
    if (isSurface) {
      return transcript ? `「${transcript}」` : '';
    }
    return quickAction?.taskReference ?? '';
  }, [isSurface, transcript, quickAction?.taskReference]);

  const renderQuickActionBody = () => {
    if (result) {
      return (
        <View style={styles.body}>
          <CompactPresentationView
            presentation={result}
            onAction={handlePresentationAction}
          />
        </View>
      );
    }

    if (!quickAction) return null;

    switch (quickAction.mode) {
      case 'complete':
        return (
          <View style={styles.body}>
            <Text style={styles.prompt}>
              「{quickAction.taskTitle}」を完了しますか？
            </Text>
            <PressableScale
              style={[styles.action, styles.primaryAction]}
              onPress={handleComplete}
              disabled={loading}
            >
              <Text style={[styles.actionText, styles.primaryActionText]}>
                完了
              </Text>
            </PressableScale>
          </View>
        );
      case 'delay':
        return (
          <View style={styles.body}>
            <Text style={styles.prompt}>
              「{quickAction.taskTitle}」をいつまで延期しますか？
            </Text>
            <View style={styles.actions}>
              {SNOOZE_OPTIONS.map((opt) => (
                <PressableScale
                  key={opt.minutes}
                  style={styles.action}
                  onPress={() => handleDelay(opt.minutes)}
                  disabled={loading}
                >
                  <Text style={styles.actionText}>{opt.label}</Text>
                </PressableScale>
              ))}
            </View>
          </View>
        );
      case 'progress':
        return (
          <View style={styles.body}>
            <Text style={styles.prompt}>
              「{quickAction.taskTitle}」の進捗を入力
            </Text>
            <TextInput
              style={styles.input}
              value={quantity}
              onChangeText={setQuantity}
              keyboardType="number-pad"
              placeholder="完了数"
              placeholderTextColor={colors.textOnCardSecondary}
            />
            <TextInput
              style={styles.input}
              value={note}
              onChangeText={setNote}
              placeholder="メモ（任意）"
              placeholderTextColor={colors.textOnCardSecondary}
            />
            <PressableScale
              style={[styles.action, styles.primaryAction]}
              onPress={handleProgress}
              disabled={loading}
            >
              <Text style={[styles.actionText, styles.primaryActionText]}>
                記録
              </Text>
            </PressableScale>
          </View>
        );
    }
  };

  const renderSurfaceBody = () => {
    if (turnError) {
      return (
        <View style={styles.body}>
          <View style={styles.result}>
            <Text style={styles.resultTitle}>エラー</Text>
            <Text style={styles.resultDetail}>{turnError}</Text>
          </View>
        </View>
      );
    }

    if (approval && sessionId) {
      return (
        <View style={styles.body}>
          <ApprovalPanel
            approval={approval}
            busy={approvalBusy}
            client={agentClient ?? undefined}
            colors={colors}
            onResolve={(decisions, granted, persist) => {
              handleApprove(true, decisions, granted, persist);
            }}
            permissions={sessionPermissions}
          />
          <PressableScale
            style={[styles.action, { backgroundColor: colors.surface }]}
            onPress={() => handleApprove(false)}
            disabled={approvalBusy}
          >
            <Text style={styles.actionText}>拒否</Text>
          </PressableScale>
        </View>
      );
    }

    if (turnPresentation) {
      return (
        <View style={styles.body}>
          <CompactPresentationView
            presentation={turnPresentation}
            onAction={handlePresentationAction}
          />
        </View>
      );
    }

    return (
      <View style={styles.body}>
        {transcript ? (
          <View style={styles.result}>
            <Text style={styles.resultDetail}>{transcript}</Text>
          </View>
        ) : null}
        {turnText ? (
          <View style={styles.result}>
            <Text style={styles.resultDetail}>{turnText}</Text>
          </View>
        ) : null}
        {surfaceBusy ? (
          <ActivityIndicator color={colors.brand} />
        ) : (
          <Text style={styles.prompt}>
            {transcript || turnText
              ? 'タップして話しかけるか、長押しで音声入力'
              : 'タップしてエージェントを開く'}
          </Text>
        )}
      </View>
    );
  };

  return (
    <Modal
      visible={visible}
      transparent
      animationType="slide"
      onRequestClose={handleClose}
    >
      <Pressable style={styles.overlay} onPress={handleClose}>
        <View style={styles.sheet}>
          <View style={styles.header}>
            <View>
              <Text style={styles.headerTitle}>{title}</Text>
              <Text style={styles.reference}>{reference}</Text>
            </View>
            <PressableScale onPress={handleClose}>
              <Ionicons name="close" size={24} color={colors.textOnCard} />
            </PressableScale>
          </View>
          {isSurface ? renderSurfaceBody() : renderQuickActionBody()}
        </View>
      </Pressable>
    </Modal>
  );
}

const makeStyles = (colors: ColorSet) =>
  StyleSheet.create({
    overlay: {
      flex: 1,
      justifyContent: 'flex-end',
      backgroundColor: colors.overlay,
    },
    sheet: {
      backgroundColor: colors.surface,
      borderTopStartRadius: 16,
      borderTopEndRadius: 16,
      padding: 16,
      paddingBottom: 32,
      gap: 16,
    },
    header: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
    },
    headerTitle: {
      fontSize: 16,
      fontWeight: '700',
      color: colors.black,
    },
    reference: {
      fontSize: 12,
      color: colors.textOnCardSecondary,
    },
    body: {
      gap: 12,
    },
    prompt: {
      fontSize: 14,
      color: colors.textOnCard,
    },
    actions: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      gap: 8,
    },
    action: {
      paddingHorizontal: 16,
      paddingVertical: 10,
      borderRadius: 10,
      backgroundColor: colors.surfaceTint,
    },
    primaryAction: {
      backgroundColor: colors.brand,
    },
    actionText: {
      fontSize: 14,
      fontWeight: '600',
      color: colors.textOnCard,
    },
    primaryActionText: {
      color: colors.onBrand,
    },
    input: {
      borderWidth: 1,
      borderColor: colors.cardOutline,
      borderRadius: 8,
      padding: 10,
      color: colors.black,
      backgroundColor: colors.surfaceTint,
      fontSize: 14,
    },
    result: {
      padding: 12,
      borderRadius: 10,
      backgroundColor: colors.surfaceTint,
      gap: 6,
    },
    resultTitle: {
      fontSize: 15,
      fontWeight: '600',
      color: colors.black,
    },
    resultDetail: {
      fontSize: 13,
      color: colors.textOnCard,
    },
  });

export const AgentCompactPanel = memo(AgentCompactPanelImpl);
