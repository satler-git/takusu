import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactElement,
} from 'react';
import {
  ActivityIndicator,
  AppState,
  FlatList,
  Keyboard,
  KeyboardAvoidingView,
  type LayoutChangeEvent,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  useWindowDimensions,
  View,
} from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { Ionicons } from '@expo/vector-icons';
import { useRouter, useFocusEffect } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Reanimated, {
  cancelAnimation,
  Easing,
  runOnJS,
  useAnimatedStyle,
  useSharedValue,
  withRepeat,
  withTiming,
} from 'react-native-reanimated';
import Markdown, {
  MarkdownIt,
  renderRules,
  type ASTNode,
  type MarkdownStyles,
  type RenderRules,
} from 'react-native-markdown-renderer';
import type { NativeScrollEvent, NativeSyntheticEvent } from 'react-native';
import { DEFAULT_PORT, useServer } from '@/src/api/ServerProvider';
import { showError } from '@/src/api/errors';
import { useTopToast } from '@/src/components/TopToast';
import { useVoice } from '@/src/api/VoiceContext';
import { markdownToSpeech } from '@/src/utils/markdownToSpeech';
import {
  ensureAudioConfigured,
  voiceBridge,
  type VoiceResult,
} from '@/src/utils/voice';
import TakusuAudioModule from '../../modules/takusu-server/src/TakusuAudioModule';
import {
  AGENT_SESSION_HISTORY_DEFAULT,
  loadSettings,
  loadTtsMuted,
  saveAgentProviders,
  saveTtsMuted,
} from '@/src/api/settingsStore';
import {
  AgentClient,
  AgentApiError,
  AbortError,
  type AgentTurnResult,
} from '@/src/api/agentClient';
import type {
  AgentStreamEvent,
  ApprovalRequest,
  UserInputQuestion,
  UserInputAnswer,
} from '@/src/api/agentTypes';
import {
  deleteSessionSnapshot,
  loadSessionHistory,
  loadSessionSnapshot,
  type AgentSessionSnapshot,
  type Message,
  type MessageSegment,
  type ToolCallItem,
  saveSessionHistory,
  saveSessionSnapshot,
} from '@/src/api/agentSessionStore';
import { formatJson } from '@/src/utils/formatJson';
import { getTurnIndex } from '@/src/utils/getTurnIndex';
import { ApprovalPanel } from '@/src/components/ApprovalPanel';
import { ComposerRecordButton } from '@/src/components/ComposerRecordButton';
import { DeleteConfirmButton } from '@/src/components/DeleteConfirmButton';
import { EditMessageModal } from '@/src/components/EditMessageModal';
import { MessageContextMenu } from '@/src/components/MessageContextMenu';
import { SessionPermissionsModal } from '@/src/components/SessionPermissionsModal';
import { ToolCallDetailModal } from '@/src/components/ToolCallDetailModal';
import { haptic } from '@/src/components/haptics';
import { BRAND_COLOR, COLORS, useColors, type ColorSet } from '@/src/theme';
import type { PermissionsMap } from '@/src/api/settingsStore';

function newId(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function markRejectedToolCalls(message: Message): Message {
  if (!message.toolCalls || message.toolCalls.length === 0) return message;
  const updated = message.toolCalls.map((call) => {
    if (call.isRejected || call.result === undefined) return call;
    try {
      const parsed = JSON.parse(call.result) as unknown;
      if (
        parsed &&
        typeof parsed === 'object' &&
        !Array.isArray(parsed) &&
        (parsed as Record<string, unknown>).approval_required === true
      ) {
        return { ...call, isRejected: true };
      }
    } catch {
      // ignore non-JSON results
    }
    return call;
  });
  return { ...message, toolCalls: updated };
}

function appendSegment(
  segments: MessageSegment[],
  segment: MessageSegment,
): MessageSegment[] {
  const last = segments[segments.length - 1];
  if (last && last.type === 'text' && segment.type === 'text') {
    return [
      ...segments.slice(0, -1),
      { ...last, text: last.text + segment.text },
    ];
  }
  if (last && last.type === 'thinking' && segment.type === 'thinking') {
    return [
      ...segments.slice(0, -1),
      { ...last, text: last.text + segment.text },
    ];
  }
  return [...segments, segment];
}

function appendTextSegment(
  segments: MessageSegment[],
  text: string,
): MessageSegment[] {
  return appendSegment(segments, { type: 'text', text });
}

function finalizeTextSegment(
  segments: MessageSegment[],
  text: string,
): MessageSegment[] {
  const last = segments[segments.length - 1];
  if (last && last.type === 'text') {
    return [...segments.slice(0, -1), { ...last, text }];
  }
  return [...segments, { type: 'text', text }];
}

function buildFallbackSegments(message: Message): MessageSegment[] {
  const segments: MessageSegment[] = [];
  if (message.thinking && message.thinking.length > 0) {
    segments.push({ type: 'thinking', text: message.thinking });
  }
  message.toolCalls?.forEach((_, index) => {
    segments.push({ type: 'toolCall', callIndex: index });
  });
  if (message.text.length > 0) {
    segments.push({ type: 'text', text: message.text });
  }
  return segments;
}

function getCollapsed(message: Message, groupIndex: number): boolean {
  if (message.collapsedGroups && message.collapsedGroups.length > groupIndex) {
    return message.collapsedGroups[groupIndex] ?? true;
  }
  return message.collapsed ?? true;
}

function updateCollapsedGroup(message: Message, groupIndex: number): boolean[] {
  const next = [...(message.collapsedGroups ?? [])];
  while (next.length <= groupIndex) {
    next.push(true);
  }
  next[groupIndex] = !getCollapsed(message, groupIndex);
  return next;
}

type ContextStage =
  | { type: 'thinking'; text: string; active: boolean }
  | { type: 'toolCall'; callIndex: number };

interface TextItem {
  type: 'text';
  text: string;
}

interface ContextItem {
  type: 'context';
  stages: ContextStage[];
  groupIndex: number;
}

type AssistantItem = TextItem | ContextItem;

function buildAssistantItems(
  message: Message,
  isLatest: boolean,
): AssistantItem[] {
  const segments = message.segments ?? buildFallbackSegments(message);
  const items: AssistantItem[] = [];
  let context: ContextItem | null = null;
  let text: TextItem | null = null;
  let groupCount = 0;
  for (const segment of segments) {
    if (segment.type === 'text') {
      if (context) {
        items.push(context);
        context = null;
      }
      if (text) {
        text.text += segment.text;
      } else {
        text = { type: 'text', text: segment.text };
      }
      continue;
    }
    if (text) {
      items.push(text);
      text = null;
    }
    if (!context) {
      context = { type: 'context', stages: [], groupIndex: groupCount++ };
    }
    if (segment.type === 'thinking') {
      context.stages.push({
        type: 'thinking',
        text: segment.text,
        active: false,
      });
    } else {
      context.stages.push({ type: 'toolCall', callIndex: segment.callIndex });
    }
  }
  if (text) items.push(text);
  if (context) items.push(context);

  if (message.state === 'thinking' && isLatest) {
    let activated = false;
    for (let i = items.length - 1; i >= 0 && !activated; i--) {
      const it = items[i];
      if (it.type === 'context') {
        for (let j = it.stages.length - 1; j >= 0; j--) {
          const stage = it.stages[j];
          if (stage.type === 'thinking') {
            stage.active = true;
            activated = true;
            break;
          }
        }
      }
    }
  }

  return items;
}

interface ToolNameChipProps {
  call: ToolCallItem;
  colors: ColorSet;
  onPress?: (call: ToolCallItem) => void;
}

function ToolNameChip({ call, colors, onPress }: ToolNameChipProps) {
  const rejected = call.isRejected ?? false;
  const chipStyle = [
    styles.toolChip,
    {
      backgroundColor: colors.surface,
      borderColor: colors.separator,
    },
  ];
  const dotColor =
    call.result === undefined
      ? colors.gray
      : call.isError || rejected
        ? colors.red
        : colors.green;
  const children = (
    <>
      <View style={[styles.toolChipDot, { backgroundColor: dotColor }]} />
      <Text style={[styles.toolChipText, { color: colors.black }]}>
        {call.name}
      </Text>
      {rejected && <Ionicons name="close" size={10} color={colors.red} />}
    </>
  );
  if (!onPress) {
    return <View style={chipStyle}>{children}</View>;
  }
  return (
    <Pressable
      style={chipStyle}
      accessibilityRole="button"
      accessibilityLabel={call.name}
      accessibilityHint="ツールの詳細を開く"
      onPress={(event) => {
        event.stopPropagation();
        onPress(call);
      }}
    >
      {children}
    </Pressable>
  );
}

interface ToolCallCardProps {
  call: ToolCallItem;
  colors: ColorSet;
  isLatest: boolean;
}

function ToolCallCard({ call, colors, isLatest }: ToolCallCardProps) {
  const isAsr = call.name === 'correct_asr';
  const rejected = call.isRejected ?? false;
  const asrCount = isAsr
    ? ((call.arguments as { questions?: unknown[] } | undefined)?.questions
        ?.length ?? 0)
    : 0;
  const [expanded, setExpanded] = useState(isLatest);
  useEffect(() => {
    setExpanded(isLatest);
  }, [isLatest]);
  const toggleExpanded = useCallback(() => setExpanded((value) => !value), []);
  return (
    <View
      style={[
        styles.toolCall,
        {
          backgroundColor: colors.surface,
          borderColor: colors.separator,
        },
      ]}
    >
      <Pressable
        style={styles.toolCallHeader}
        onPress={toggleExpanded}
        accessibilityRole="button"
        accessibilityState={{ expanded }}
        accessibilityLabel={isAsr ? 'ASR訂正' : call.name}
        accessibilityHint={
          expanded ? 'raw JSON を折りたたむ' : 'raw JSON を展開する'
        }
      >
        <View style={styles.toolCallHeaderLeft}>
          <View
            style={[
              styles.toolStatus,
              {
                backgroundColor:
                  call.isError || rejected ? colors.red : colors.green,
              },
            ]}
          />
          <Text style={{ color: colors.black, fontWeight: '700' }}>
            {isAsr ? 'ASR訂正' : call.name}
          </Text>
          {rejected && (
            <View
              style={[styles.rejectedBadge, { backgroundColor: colors.red }]}
            >
              <Text style={styles.rejectedBadgeText}>拒否</Text>
            </View>
          )}
        </View>
        <Ionicons
          name={expanded ? 'chevron-up' : 'chevron-down'}
          size={14}
          color={colors.gray}
        />
      </Pressable>
      {expanded && (
        <>
          {!isAsr && call.arguments !== undefined && (
            <Text style={[styles.toolArgs, { color: colors.gray }]}>
              {formatJson(call.arguments)}
            </Text>
          )}
          {isAsr && call.arguments !== undefined && (
            <Text style={[styles.toolArgs, { color: colors.gray }]}>
              {asrCount} 件の認識テキストを確認
            </Text>
          )}
          {call.result !== undefined && (
            <Text
              style={{
                color: call.isError || rejected ? colors.red : colors.green,
              }}
            >
              {isAsr
                ? call.result
                : call.isError
                  ? `エラー: ${formatJson(call.result)}`
                  : formatJson(call.result)}
            </Text>
          )}
        </>
      )}
    </View>
  );
}

function PulsingDot({ color, size = 6 }: { color: string; size?: number }) {
  const scale = useSharedValue(1);
  const opacity = useSharedValue(1);

  useEffect(() => {
    scale.value = withRepeat(
      withTiming(0.85, { duration: 700, easing: Easing.inOut(Easing.ease) }),
      -1,
      true,
    );
    opacity.value = withRepeat(
      withTiming(0.5, { duration: 700, easing: Easing.inOut(Easing.ease) }),
      -1,
      true,
    );
    return () => {
      cancelAnimation(scale);
      cancelAnimation(opacity);
    };
  }, [scale, opacity]);

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [{ scale: scale.value }],
    opacity: opacity.value,
  }));

  return (
    <Reanimated.View
      style={[
        {
          width: size,
          height: size,
          borderRadius: size / 2,
          backgroundColor: color,
        },
        animatedStyle,
      ]}
    />
  );
}

function BouncingDots({ color, size = 4 }: { color: string; size?: number }) {
  const progress = useSharedValue(0);

  useEffect(() => {
    progress.value = withRepeat(
      withTiming(1, { duration: 600, easing: Easing.linear }),
      -1,
      false,
    );
    return () => {
      cancelAnimation(progress);
    };
  }, [progress]);

  const dot1Style = useAnimatedStyle(() => {
    const t = progress.value % 1;
    return {
      transform: [{ translateY: -Math.abs(Math.sin(t * Math.PI)) * 4 }],
    };
  });
  const dot2Style = useAnimatedStyle(() => {
    const t = (progress.value + 1 / 3) % 1;
    return {
      transform: [{ translateY: -Math.abs(Math.sin(t * Math.PI)) * 4 }],
    };
  });
  const dot3Style = useAnimatedStyle(() => {
    const t = (progress.value + 2 / 3) % 1;
    return {
      transform: [{ translateY: -Math.abs(Math.sin(t * Math.PI)) * 4 }],
    };
  });

  const dotStyle = {
    width: size,
    height: size,
    borderRadius: size / 2,
    backgroundColor: color,
  };

  return (
    <View style={{ flexDirection: 'row', gap: 3, alignItems: 'center' }}>
      <Reanimated.View style={[dotStyle, dot1Style]} />
      <Reanimated.View style={[dotStyle, dot2Style]} />
      <Reanimated.View style={[dotStyle, dot3Style]} />
    </View>
  );
}

interface ThinkingChipProps {
  active: boolean;
  colors: ColorSet;
}

function ThinkingChip({ active, colors }: ThinkingChipProps) {
  return (
    <View
      style={[
        styles.toolChip,
        {
          backgroundColor: colors.surface,
          borderColor: active ? colors.brand : colors.separator,
        },
      ]}
    >
      {active ? (
        <PulsingDot color={colors.brand} size={6} />
      ) : (
        <View style={[styles.toolChipDot, { backgroundColor: colors.gray }]} />
      )}
      <Text
        style={[
          styles.toolChipText,
          { color: active ? colors.brand : colors.black },
        ]}
      >
        考え中
      </Text>
    </View>
  );
}

interface ThinkingCardProps {
  text: string;
  active: boolean;
  colors: ColorSet;
}

function ThinkingCard({ text, active, colors }: ThinkingCardProps) {
  return (
    <View
      style={[
        styles.thinkingCard,
        {
          backgroundColor: colors.surface,
          borderColor: colors.separator,
        },
      ]}
    >
      <View style={styles.thinkingHeader}>
        <Text
          style={[
            styles.thinkingLabel,
            { color: active ? colors.brand : colors.gray },
          ]}
        >
          考え中
        </Text>
        {active && <BouncingDots color={colors.brand} size={4} />}
      </View>
      <Text style={[styles.thinkingText, { color: colors.gray }]}>{text}</Text>
    </View>
  );
}

interface ContextGroupProps {
  context: ContextItem;
  message: Message;
  colors: ColorSet;
  availableHeight: number;
  onToggle: () => void;
  onToolPress?: (call: ToolCallItem) => void;
}

function ContextGroup({
  context,
  message,
  colors,
  availableHeight,
  onToggle,
  onToolPress,
}: ContextGroupProps) {
  const collapsed = getCollapsed(message, context.groupIndex);
  const [isTall, setIsTall] = useState(false);
  const bodyHeightRef = useRef(0);

  const hasThinking = context.stages.some((s) => s.type === 'thinking');
  const hasActiveThinking = context.stages.some(
    (s) => s.type === 'thinking' && s.active,
  );
  const toolStages = context.stages.filter(
    (s): s is { type: 'toolCall'; callIndex: number } => s.type === 'toolCall',
  );

  const maxToolChips = 3 - (hasThinking ? 1 : 0);
  const visibleToolChips = Math.min(maxToolChips, toolStages.length);
  const overflowCount = toolStages.length - visibleToolChips;
  const startToolIndex = toolStages.length - visibleToolChips;

  const chips: ReactElement[] = [];
  if (hasThinking) {
    chips.push(
      <ThinkingChip
        key="thinking"
        active={hasActiveThinking}
        colors={colors}
      />,
    );
  }
  if (overflowCount > 0) {
    chips.push(
      <View
        key="overflow"
        style={[
          styles.toolChip,
          {
            backgroundColor: colors.surface,
            borderColor: colors.separator,
          },
        ]}
      >
        <Text style={[styles.toolChipText, { color: colors.gray }]}>
          {overflowCount}+
        </Text>
      </View>,
    );
  }
  for (let i = startToolIndex; i < toolStages.length; i++) {
    const call = message.toolCalls?.[toolStages[i].callIndex];
    if (call) {
      chips.push(
        <ToolNameChip
          key={`tool-${toolStages[i].callIndex}`}
          call={call}
          colors={colors}
          onPress={onToolPress}
        />,
      );
    }
  }

  const updateTall = useCallback(() => {
    if (availableHeight <= 0) return;
    setIsTall(bodyHeightRef.current > availableHeight * 0.6);
  }, [availableHeight]);

  useEffect(() => {
    updateTall();
  }, [updateTall]);

  const handleBodyLayout = (event: LayoutChangeEvent) => {
    bodyHeightRef.current = event.nativeEvent.layout.height;
    updateTall();
  };

  const latestCallIndex = (message.toolCalls?.length ?? 0) - 1;

  return (
    <View
      style={[
        styles.contextHeader,
        {
          backgroundColor: colors.surfaceTint,
          borderColor: colors.separator,
        },
      ]}
    >
      <Pressable
        onPress={onToggle}
        accessibilityRole="button"
        accessibilityState={{ expanded: !collapsed }}
        accessibilityLabel="コンテキスト"
        accessibilityHint={
          collapsed ? 'コンテキストを展開する' : 'コンテキストを折りたたむ'
        }
      >
        <View style={styles.contextHeaderInner}>
          <Ionicons
            name={collapsed ? 'chevron-forward' : 'chevron-down'}
            size={14}
            color={colors.gray}
          />
          <View style={styles.toolChips}>{chips}</View>
        </View>
      </Pressable>
      {!collapsed && (
        <View style={styles.contextBody} onLayout={handleBodyLayout}>
          {context.stages.map((stage, idx) => {
            if (stage.type === 'thinking') {
              return (
                <ThinkingCard
                  key={`s-${idx}`}
                  text={stage.text}
                  active={stage.active}
                  colors={colors}
                />
              );
            }
            const call = message.toolCalls?.[stage.callIndex];
            return call ? (
              <ToolCallCard
                key={`s-${idx}`}
                call={call}
                colors={colors}
                isLatest={stage.callIndex === latestCallIndex}
              />
            ) : null;
          })}
          {isTall && (
            <Pressable onPress={onToggle}>
              <View
                style={[
                  styles.contextFooter,
                  { borderTopColor: colors.separator },
                ]}
              >
                <Ionicons name="chevron-up" size={14} color={colors.gray} />
                <Text
                  style={[styles.contextFooterText, { color: colors.gray }]}
                >
                  畳む
                </Text>
              </View>
            </Pressable>
          )}
        </View>
      )}
    </View>
  );
}

interface AssistantMessageProps {
  item: Message;
  colors: ColorSet;
  markdownStyles: Partial<MarkdownStyles>;
  markdownIt: MarkdownIt;
  markdownRules: RenderRules;
  availableHeight: number;
  isLatest: boolean;
  onToggleGroupCollapsed: (messageId: string, groupIndex: number) => void;
  onLongPress: (message: Message, position: { x: number; y: number }) => void;
  onToolCallPress?: (call: ToolCallItem) => void;
}

const AssistantMessage = memo(function AssistantMessageImpl({
  item,
  colors,
  markdownStyles,
  markdownIt,
  markdownRules,
  availableHeight,
  isLatest,
  onToggleGroupCollapsed,
  onLongPress,
  onToolCallPress,
}: AssistantMessageProps) {
  const items = useMemo(
    () => buildAssistantItems(item, isLatest),
    [item, isLatest],
  );
  const handleLongPress = (event: {
    nativeEvent: { pageX?: number; pageY?: number };
  }) => {
    onLongPress(item, {
      x: event.nativeEvent.pageX ?? 0,
      y: event.nativeEvent.pageY ?? 0,
    });
  };

  return (
    <Pressable
      style={[
        styles.bubble,
        styles.assistantBubble,
        { backgroundColor: colors.separator, gap: 8 },
      ]}
      onLongPress={handleLongPress}
    >
      {items.map((it, idx) => {
        if (it.type === 'text') {
          return (
            <View key={`t-${idx}`}>
              {item.state === 'done' || item.state === undefined ? (
                <Markdown
                  style={markdownStyles}
                  markdownit={markdownIt}
                  rules={markdownRules}
                >
                  {it.text}
                </Markdown>
              ) : (
                <Text style={{ color: colors.black }}>{it.text}</Text>
              )}
            </View>
          );
        }
        return (
          <ContextGroup
            key={`c-${idx}`}
            context={it}
            message={item}
            colors={colors}
            availableHeight={availableHeight}
            onToggle={() => onToggleGroupCollapsed(item.id, it.groupIndex)}
            onToolPress={onToolCallPress}
          />
        );
      })}
      {item.state !== 'done' &&
        item.text.length === 0 &&
        items.length === 0 && (
          <View style={styles.loadingIndicator}>
            <ActivityIndicator color={BRAND_COLOR} />
          </View>
        )}
    </Pressable>
  );
});

interface UserMessageProps {
  message: Message;
  onLongPress: (message: Message, position: { x: number; y: number }) => void;
}

const UserMessage = memo(function UserMessageImpl({
  message,
  onLongPress,
}: UserMessageProps) {
  const handleLongPress = (event: {
    nativeEvent: { pageX?: number; pageY?: number };
  }) => {
    onLongPress(message, {
      x: event.nativeEvent.pageX ?? 0,
      y: event.nativeEvent.pageY ?? 0,
    });
  };

  return (
    <Pressable
      style={[
        styles.bubble,
        styles.userBubble,
        { backgroundColor: BRAND_COLOR },
      ]}
      onLongPress={handleLongPress}
    >
      <Text style={{ color: COLORS.white }}>{message.text}</Text>
    </Pressable>
  );
});

const SWIPE_THRESHOLD = 40;
const SCROLL_THRESHOLD = 40;

export function AgentView() {
  const router = useRouter();
  const colors = useColors();
  const markdownStyles = useMemo<Partial<MarkdownStyles>>(
    () => ({
      text: { color: colors.black },
      link: { color: BRAND_COLOR },
      codeBlock: {
        backgroundColor: colors.separator,
        padding: 8,
        borderRadius: 8,
      },
      codeInline: {
        backgroundColor: colors.separator,
        paddingHorizontal: 4,
        paddingVertical: 2,
        borderRadius: 4,
      },
      listUnorderedItemIcon: {
        color: colors.gray,
        fontSize: 14,
        lineHeight: 24,
      },
      listOrderedItemIcon: {
        color: colors.gray,
        fontSize: 14,
        lineHeight: 24,
      },
    }),
    [colors],
  );
  const markdownIt = useMemo(() => new MarkdownIt({ typographer: false }), []);
  const markdownRules = useMemo<RenderRules>(
    () => ({
      image: (node: ASTNode) => (
        <Text key={node.key} style={{ color: colors.gray }}>
          {node.content}
        </Text>
      ),
      list_item: (node, children, parent, styles) => {
        if (parent.some((p) => p.type === 'bullet_list')) {
          return (
            <View key={node.key} style={styles.listUnorderedItem as any}>
              <Text style={styles.listUnorderedItemIcon as any}>
                {'\u2022'}
              </Text>
              <View style={styles.listItem as any}>{children}</View>
            </View>
          );
        }
        return renderRules.list_item(node, children, parent, styles);
      },
    }),
    [colors.gray],
  );
  const insets = useSafeAreaInsets();
  const { width } = useWindowDimensions();
  const { workersToken, ready, pushAgentConfig } = useServer();
  const { pendingSessionId, setPendingSessionId } = useVoice();
  const { showTopToast } = useTopToast();
  const [pendingResult, setPendingResult] = useState<VoiceResult | null>(null);
  const sendTextRef = useRef(sendText);
  const client = useMemo(
    () => new AgentClient(`http://127.0.0.1:${DEFAULT_PORT}`, workersToken),
    [workersToken],
  );

  const [messages, setMessages] = useState<Message[]>([]);
  const [text, setText] = useState('');
  const [sessionIds, setSessionIds] = useState<string[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const [sessionHistoryCount, setSessionHistoryCount] = useState(
    AGENT_SESSION_HISTORY_DEFAULT,
  );
  const [approval, setApproval] = useState<ApprovalRequest | null>(null);
  const [busy, setBusy] = useState(false);
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [audioReady, setAudioReady] = useState(false);
  const [isMuted, setIsMuted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [keyboardVisible, setKeyboardVisible] = useState(Keyboard.isVisible());
  const [inputHeight, setInputHeight] = useState(44);
  const [messagesHeight, setMessagesHeight] = useState(0);
  const [historyReady, setHistoryReady] = useState(false);
  const [isSwitching, setIsSwitching] = useState(false);
  const [userInput, setUserInput] = useState<{
    sessionId: string;
    callId: string;
    questions: UserInputQuestion[];
    values: string[];
  } | null>(null);
  const [contextMenu, setContextMenu] = useState<{
    visible: boolean;
    messageId: string;
    position: { x: number; y: number };
  }>({ visible: false, messageId: '', position: { x: 0, y: 0 } });
  const [editModal, setEditModal] = useState<{
    visible: boolean;
    messageId: string;
    text: string;
  }>({ visible: false, messageId: '', text: '' });
  const [permissionsModal, setPermissionsModal] = useState(false);
  const [toolDetail, setToolDetail] = useState<{
    visible: boolean;
    call: ToolCallItem | null;
  }>({ visible: false, call: null });
  const [sessionPermissions, setSessionPermissions] = useState<PermissionsMap>(
    {},
  );

  const sessionIdsRef = useRef<string[]>([]);
  const activeIndexRef = useRef(0);
  const sessionIdRef = useRef<string | null>(null);
  const sessionHistoryCountRef = useRef(AGENT_SESSION_HISTORY_DEFAULT);
  const isSwitchingRef = useRef(false);
  const streamAbortRef = useRef<AbortController | null>(null);
  const backgroundAbortedRef = useRef(false);
  const flatListRef = useRef<FlatList<Message>>(null);
  const autoScrollRef = useRef(true);
  const sessionJustLoadedRef = useRef(false);
  const sessionLoadTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const skipSnapshotSaveRef = useRef(false);
  const lastPendingSessionIdRef = useRef<string | null>(null);
  const sessionPermissionsRef = useRef<PermissionsMap>({});
  const isMutedRef = useRef(false);
  const ttsQueueRef = useRef<string[]>([]);
  const ttsProcessingRef = useRef(false);
  const ttsBlockReceivedRef = useRef(false);
  const ttsPromiseRef = useRef<Promise<void>>(Promise.resolve());
  const viewOffset = useSharedValue(0);
  const animatedStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: viewOffset.value }],
  }));

  useEffect(() => {
    sessionIdsRef.current = sessionIds;
  }, [sessionIds]);
  useEffect(() => {
    activeIndexRef.current = activeIndex;
  }, [activeIndex]);
  useEffect(() => {
    sessionHistoryCountRef.current = sessionHistoryCount;
  }, [sessionHistoryCount]);
  useEffect(() => {
    isSwitchingRef.current = isSwitching;
  }, [isSwitching]);
  useEffect(() => {
    sessionPermissionsRef.current = sessionPermissions;
  }, [sessionPermissions]);
  useEffect(() => {
    isMutedRef.current = isMuted;
  }, [isMuted]);

  useEffect(() => {
    const subscription = AppState.addEventListener('change', (next) => {
      if (next === 'background' && streamAbortRef.current) {
        backgroundAbortedRef.current = true;
        streamAbortRef.current.abort();
      }
    });
    return () => subscription.remove();
  }, []);

  useEffect(() => {
    return () => {
      if (sessionLoadTimerRef.current)
        clearTimeout(sessionLoadTimerRef.current);
    };
  }, []);

  function markSessionJustLoaded() {
    sessionJustLoadedRef.current = true;
    if (sessionLoadTimerRef.current) clearTimeout(sessionLoadTimerRef.current);
    sessionLoadTimerRef.current = setTimeout(() => {
      sessionJustLoadedRef.current = false;
      sessionLoadTimerRef.current = null;
      flatListRef.current?.scrollToEnd({ animated: false });
    }, 500);
  }

  function clearSessionJustLoaded() {
    if (sessionLoadTimerRef.current) {
      clearTimeout(sessionLoadTimerRef.current);
      sessionLoadTimerRef.current = null;
    }
    sessionJustLoadedRef.current = false;
  }

  useEffect(() => {
    const showEvent = 'keyboardDidShow';
    const hideEvent = 'keyboardDidHide';
    const showSub = Keyboard.addListener(showEvent, () =>
      setKeyboardVisible(true),
    );
    const hideSub = Keyboard.addListener(hideEvent, () =>
      setKeyboardVisible(false),
    );
    return () => {
      showSub.remove();
      hideSub.remove();
    };
  }, []);

  const activateSessionId = useCallback(
    async (id: string, isNew = false) => {
      const previousActiveId = sessionIdRef.current;
      let ids = sessionIdsRef.current;
      const existingIndex = ids.indexOf(id);
      if (isNew && existingIndex !== -1) {
        ids = ids.filter((s) => s !== id);
      }
      if (existingIndex === -1 || isNew) {
        ids = [...ids, id];
        const max = sessionHistoryCountRef.current;
        const { ids: trimmed, removed } = trimSessionIds(
          ids,
          max,
          previousActiveId,
        );
        ids = trimmed;
        await Promise.all(removed.map((r) => deleteSessionSnapshot(r))).catch(
          () => {},
        );
      }
      const index = ids.indexOf(id);
      const snapshot = await loadSessionSnapshot(id);
      const perms = snapshot?.permissions ?? {};
      setSessionPermissions(perms);
      sessionPermissionsRef.current = perms;
      autoScrollRef.current = true;
      markSessionJustLoaded();
      setMessages(snapshot?.messages ?? []);
      setApproval(snapshot?.approval ?? null);
      sessionIdRef.current = id;
      setText('');
      setSessionIds(ids);
      sessionIdsRef.current = ids;
      setActiveIndex(index);
      activeIndexRef.current = index;
      if (!isNew && Object.keys(perms).length > 0) {
        client.updateSessionSettings(id, perms).catch(() => {});
      }
      await saveSessionHistory({ ids, activeIndex: index });
    },
    [client],
  );

  function trimSessionIds(
    ids: string[],
    max: number,
    activeId: string | null,
  ): { ids: string[]; index: number; removed: string[] } {
    if (ids.length <= max) {
      const found = activeId ? ids.indexOf(activeId) : -1;
      return {
        ids,
        index: found !== -1 ? found : Math.max(0, ids.length - 1),
        removed: [],
      };
    }
    if (!activeId || ids.indexOf(activeId) === -1) {
      const trimmed = ids.slice(-max);
      return {
        ids: trimmed,
        index: trimmed.length - 1,
        removed: ids.slice(0, -max),
      };
    }
    const keep = new Set<string>();
    keep.add(activeId);
    let i = ids.length - 1;
    while (keep.size < max && i >= 0) {
      keep.add(ids[i]);
      i--;
    }
    const trimmed = ids.filter((id) => keep.has(id));
    return {
      ids: trimmed,
      index: trimmed.indexOf(activeId),
      removed: ids.filter((id) => !keep.has(id)),
    };
  }

  function isEmptySnapshot(snapshot: AgentSessionSnapshot | null): boolean {
    if (!snapshot) return false;
    if (snapshot.messages.length > 0) return false;
    if (snapshot.approval !== null) return false;
    if (Object.keys(snapshot.permissions ?? {}).length > 0) return false;
    return true;
  }

  async function findEmptySession(
    ids: string[],
    preferredId?: string | null,
  ): Promise<string | null> {
    if (ids.length === 0) return null;
    const snapshots = await Promise.all(
      ids.map((id) => loadSessionSnapshot(id)),
    );
    const preferredIndex = preferredId ? ids.indexOf(preferredId) : -1;
    if (
      preferredIndex !== -1 &&
      preferredId != null &&
      isEmptySnapshot(snapshots[preferredIndex])
    ) {
      return preferredId;
    }
    for (let i = snapshots.length - 1; i >= 0; i--) {
      if (i === preferredIndex) continue;
      if (isEmptySnapshot(snapshots[i])) return ids[i];
    }
    return null;
  }

  async function activateNewSession(id: string) {
    const previousActiveId = sessionIdRef.current;
    let ids = sessionIdsRef.current;
    const existingIndex = ids.indexOf(id);
    if (existingIndex !== -1) {
      ids = ids.filter((s) => s !== id);
    }
    ids = [...ids, id];
    const max = sessionHistoryCountRef.current;
    const { ids: trimmed, removed } = trimSessionIds(
      ids,
      max,
      previousActiveId,
    );
    ids = trimmed;
    await Promise.all(removed.map((r) => deleteSessionSnapshot(r))).catch(
      () => {},
    );
    const index = ids.indexOf(id);
    sessionIdRef.current = id;
    setSessionIds(ids);
    sessionIdsRef.current = ids;
    setActiveIndex(index);
    activeIndexRef.current = index;
    await saveSessionHistory({ ids, activeIndex: index });
  }

  async function ensureSession(): Promise<string> {
    if (sessionIdRef.current) return sessionIdRef.current;
    if (messages.length === 0 && approval === null) {
      const emptyId = await findEmptySession(sessionIdsRef.current);
      if (emptyId) {
        await activateNewSession(emptyId);
        const permissions = sessionPermissionsRef.current;
        if (Object.keys(permissions).length > 0) {
          client
            .updateSessionSettings(emptyId, permissions)
            .catch((e: unknown) => {
              console.error('Failed to update session settings:', e);
            });
        }
        return emptyId;
      }
    }
    const created = await client.createSession(sessionPermissionsRef.current);
    await activateNewSession(created);
    return created;
  }

  useEffect(() => {
    let cancelled = false;
    async function init() {
      setError(null);
      try {
        const [history, settings] = await Promise.all([
          loadSessionHistory(),
          loadSettings(),
        ]);
        if (cancelled) return;

        const count = settings.agentSessionHistoryCount;
        setSessionHistoryCount(count);
        sessionHistoryCountRef.current = count;

        let ids = history.ids;
        if (!Array.isArray(ids)) ids = [];

        if (pendingSessionId) {
          if (lastPendingSessionIdRef.current !== pendingSessionId) {
            // FloatingVoiceButton queued a brand-new session.
            // Restore the saved history so the new session is appended to it
            // instead of replacing it.
            let pendingIndex = history.activeIndex;
            if (pendingIndex < 0 || pendingIndex >= ids.length) {
              pendingIndex = Math.max(0, ids.length - 1);
            }
            const previousActiveId = ids[pendingIndex] ?? null;
            sessionIdsRef.current = ids;
            sessionIdRef.current = previousActiveId;
            activeIndexRef.current = pendingIndex;
            lastPendingSessionIdRef.current = pendingSessionId;

            const emptyId = await findEmptySession(ids, previousActiveId);
            if (cancelled) return;

            if (emptyId) {
              const {
                ids: trimmed,
                index,
                removed,
              } = trimSessionIds(ids, count, emptyId);
              if (removed.length > 0) {
                await Promise.all(
                  removed.map((id) => deleteSessionSnapshot(id)),
                ).catch(() => {});
              }
              if (cancelled) return;

              sessionIdsRef.current = trimmed;
              sessionIdRef.current = emptyId;
              activeIndexRef.current = index;
              setPendingSessionId(null);
              // If a concrete session id was queued (legacy), it is no longer needed.
              if (!pendingSessionId.startsWith('__new__')) {
                client.deleteSession(pendingSessionId).catch((e: unknown) => {
                  console.error('Failed to delete unused pending session:', e);
                });
              }
              await activateSessionId(emptyId, false);
              if (cancelled) return;
            } else {
              const created = pendingSessionId.startsWith('__new__')
                ? await client.createSession({})
                : pendingSessionId;
              if (cancelled) return;
              await activateSessionId(created, true);
              if (cancelled) return;
              setPendingSessionId(null);
            }
          }
          if (!cancelled) setHistoryReady(true);
          return;
        }

        if (lastPendingSessionIdRef.current !== null) {
          // We consumed the queued new session in a previous effect run.
          // `pendingSessionId` is already null (FloatingVoiceButton cleared it),
          // so `pendingSessionId == null` here. Without this guard we would fall
          // through to the normal history-load path and overwrite the just-created
          // session with the previous active session. The ref is intentionally not
          // reset to null; it is a marker that a queued session was already handled.
          if (!cancelled) setHistoryReady(true);
          return;
        }

        let index = history.activeIndex;
        if (index < 0 || index >= ids.length) {
          index = Math.max(0, ids.length - 1);
        }

        const activeId = ids[index] ?? null;
        const {
          ids: trimmed,
          index: trimmedIndex,
          removed,
        } = trimSessionIds(ids, count, activeId);
        if (cancelled) return;

        if (removed.length > 0) {
          await Promise.all(
            removed.map((id) => deleteSessionSnapshot(id)),
          ).catch(() => {});
        }
        if (cancelled) return;

        sessionIdsRef.current = trimmed;
        setSessionIds(trimmed);
        setActiveIndex(trimmedIndex);
        activeIndexRef.current = trimmedIndex;
        if (cancelled) return;

        if (trimmed.length === 0) {
          const created = await client.createSession();
          if (cancelled) return;
          await activateSessionId(created, true);
          if (cancelled) return;
        } else {
          const newActiveId = trimmed[trimmedIndex];
          await activateSessionId(newActiveId, false);
          if (cancelled) return;
          try {
            const pending = await client.getApproval(newActiveId);
            if (cancelled) return;
            setApproval(pending);
          } catch (e) {
            if (e instanceof AgentApiError && e.status === 404) {
              // The server session no longer exists, but keep the local snapshot
              // so the history remains visible and swipable. Clear any stale
              // pending approval because it can no longer be resolved.
              setApproval(null);
            } else {
              throw e;
            }
          }
        }

        if (!cancelled) setHistoryReady(true);
      } catch (e: unknown) {
        if (!cancelled) {
          void showError(e, 'エラー');
        }
      }
    }

    if (ready && workersToken) init();

    return () => {
      cancelled = true;
    };
  }, [
    client,
    ready,
    workersToken,
    pendingSessionId,
    setPendingSessionId,
    activateSessionId,
  ]);

  useEffect(() => {
    if (!historyReady) return;
    const max = sessionHistoryCountRef.current;
    const activeId = sessionIdRef.current;
    const currentIds = sessionIdsRef.current;
    const { removed } = trimSessionIds(currentIds, max, activeId);
    if (removed.length === 0) return;

    let cancelled = false;
    async function trimHistory() {
      const deletedIds: string[] = [];
      for (const id of removed) {
        if (cancelled || sessionIdRef.current === id) continue;
        try {
          await deleteSessionSnapshot(id);
          deletedIds.push(id);
        } catch (e) {
          console.error(`Failed to delete session snapshot ${id}:`, e);
        }
      }
      if (cancelled) return;

      // If the user switched sessions or a newer effect run updated the refs,
      // do not overwrite the newer state with this stale trim result.
      if (
        sessionIdsRef.current !== currentIds ||
        sessionIdRef.current !== activeId
      ) {
        return;
      }

      if (deletedIds.length === 0) return;

      const keptIds = currentIds.filter((id) => !deletedIds.includes(id));
      const newIndex =
        activeId != null
          ? keptIds.indexOf(activeId)
          : Math.max(0, keptIds.length - 1);
      if (activeId != null && newIndex === -1) return;

      sessionIdsRef.current = keptIds;
      setSessionIds(keptIds);
      if (newIndex !== activeIndexRef.current) {
        setActiveIndex(newIndex);
        activeIndexRef.current = newIndex;
      }
      if (cancelled) return;

      try {
        await saveSessionHistory({
          ids: keptIds,
          activeIndex: newIndex,
        });
      } catch (e) {
        if (!cancelled) {
          console.error('Failed to save session history:', e);
        }
      }
    }

    trimHistory();

    return () => {
      cancelled = true;
    };
  }, [sessionHistoryCount, historyReady]);

  useFocusEffect(
    useCallback(() => {
      loadSettings()
        .then((settings) => {
          const count = settings.agentSessionHistoryCount;
          setSessionHistoryCount(count);
          sessionHistoryCountRef.current = count;
        })
        .catch(() => {});
    }, []),
  );

  useFocusEffect(
    useCallback(() => {
      loadTtsMuted()
        .then((muted) => {
          setIsMuted(muted);
          isMutedRef.current = muted;
        })
        .catch(() => {});
    }, []),
  );

  useFocusEffect(
    useCallback(() => {
      if (!ready || !workersToken) return;
      let cancelled = false;
      ensureAudioConfigured().then(
        () => {
          if (cancelled) return;
          setAudioReady(true);
          setError(null);
        },
        (e: unknown) => {
          if (cancelled) return;
          setAudioReady(false);
          void showError(e, '音声モデルを準備してください');
        },
      );
      return () => {
        cancelled = true;
      };
    }, [ready, workersToken]),
  );

  useEffect(() => {
    if (!sessionIdRef.current || busy || skipSnapshotSaveRef.current) return;
    saveSessionSnapshot(sessionIdRef.current, {
      messages,
      approval,
      permissions: sessionPermissionsRef.current,
    }).catch(() => {});
  }, [messages, approval, busy]);

  useEffect(() => {
    return voiceBridge.subscribe((r) => {
      if (r) setPendingResult(r);
    });
  }, []);

  useEffect(() => {
    if (!pendingResult) return;
    const { transcript, sendNow } = pendingResult;
    if (sendNow) {
      sendTextRef.current(transcript);
    } else {
      setText((t) => {
        const prefix = t && !t.endsWith(' ') ? `${t} ` : t;
        return `${prefix}${transcript}`;
      });
    }
    voiceBridge.consume();
    setPendingResult(null);
  }, [pendingResult]);

  async function sendText(value: string) {
    if (!value.trim() || busy || isSwitchingRef.current || !historyReady)
      return;
    setError(null);
    backgroundAbortedRef.current = false;
    autoScrollRef.current = true;
    setApproval(null);
    setUserInput(null);
    const trimmed = value.trim();
    setMessages((current) => [
      ...current,
      { id: newId('user'), role: 'user', text: trimmed },
    ]);
    setBusy(true);
    try {
      const session = await ensureSession();
      const assistantId = newId('assistant');
      const initialTurnId = newId('turn');
      const retryTurnId = newId('turn');
      setMessages((current) => [
        ...current,
        {
          id: assistantId,
          role: 'assistant',
          text: '',
          thinking: '',
          toolCalls: [],
          state: 'thinking',
          segments: [],
          collapsedGroups: [],
        },
      ]);
      await runAssistantStream(
        assistantId,
        (sessionId, onEvent, signal, attempt) =>
          client.runTurnStream(
            sessionId,
            trimmed,
            attempt === 0 ? initialTurnId : retryTurnId,
            onEvent,
            signal,
          ),
        session,
      );
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      void showError(message, 'エラー');
      setBusy(false);
    }
  }

  async function editTurn(turnIndex: number, userId: string, newText: string) {
    if (!newText.trim() || busy || isSwitchingRef.current || !historyReady)
      return;
    setError(null);
    backgroundAbortedRef.current = false;
    autoScrollRef.current = true;
    setApproval(null);
    setUserInput(null);
    const trimmed = newText.trim();
    setBusy(true);
    try {
      const session = await ensureSession();
      const assistantId = newId('assistant');
      const initialTurnId = newId('turn');
      const retryTurnId = newId('turn');
      let userIndex = -1;
      setMessages((current) => {
        const index = current.findIndex((m) => m.id === userId);
        if (index === -1) return current;
        userIndex = index;
        return [
          ...current.slice(0, index),
          { ...current[index], text: trimmed },
          {
            id: assistantId,
            role: 'assistant',
            text: '',
            thinking: '',
            toolCalls: [],
            state: 'thinking',
            segments: [],
            collapsedGroups: [],
          },
        ];
      });
      if (userIndex === -1) {
        setBusy(false);
        return;
      }
      await runAssistantStream(
        assistantId,
        (sessionId, onEvent, signal, attempt) => {
          if (attempt === 0) {
            return client.editTurnStream(
              sessionId,
              turnIndex,
              trimmed,
              initialTurnId,
              onEvent,
              signal,
            );
          }
          return client.runTurnStream(
            sessionId,
            trimmed,
            retryTurnId,
            onEvent,
            signal,
          );
        },
        session,
      );
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      void showError(message, 'エラー');
      setBusy(false);
    }
  }

  async function runAssistantStream(
    assistantId: string,
    apiCall: (
      sessionId: string,
      onEvent: (event: AgentStreamEvent) => void,
      signal: AbortSignal,
      attempt: number,
    ) => Promise<AgentTurnResult>,
    initialSessionId: string,
  ) {
    setBusy(true);
    let currentSessionId = initialSessionId;
    ttsQueueRef.current = [];
    ttsProcessingRef.current = false;
    ttsBlockReceivedRef.current = false;
    ttsPromiseRef.current = Promise.resolve();

    const finish = async () => {
      await ttsPromiseRef.current;
      streamAbortRef.current = null;
      setUserInput(null);
      setBusy(false);
      setIsSpeaking(false);
      ttsQueueRef.current = [];
      ttsProcessingRef.current = false;
      ttsBlockReceivedRef.current = false;
      ttsPromiseRef.current = Promise.resolve();
    };

    const handleStreamEvent = (event: AgentStreamEvent) => {
      if (event.type === 'TtsBlock') {
        // TtsBlock values are already filtered by the Rust TtsQueue.
        handleTtsBlock(event.data, false);
        return;
      }

      setMessages((current) => {
        const index = current.findIndex((m) => m.id === assistantId);
        if (index === -1) return current;
        const msg = current[index];
        const next = { ...msg };
        switch (event.type) {
          case 'Thinking':
            next.thinking = (next.thinking ?? '') + event.data;
            next.segments = appendSegment(next.segments ?? [], {
              type: 'thinking',
              text: event.data,
            });
            next.state = 'thinking';
            break;
          case 'ToolCall': {
            if (event.data.name === 'correct_asr' && sessionIdRef.current) {
              const args = event.data.arguments as
                | { questions: UserInputQuestion[] }
                | undefined;
              const questions = args?.questions ?? [];
              if (questions.length > 0) {
                setUserInput({
                  sessionId: sessionIdRef.current,
                  callId: event.data.call_id,
                  questions,
                  values: questions.map((q) => q.text),
                });
              }
            }
            const callIndex = (next.toolCalls ?? []).length;
            next.toolCalls = [
              ...(next.toolCalls ?? []),
              {
                name: event.data.name,
                callId: event.data.call_id,
                arguments: event.data.arguments,
              },
            ];
            next.segments = appendSegment(next.segments ?? [], {
              type: 'toolCall',
              callIndex,
            });
            next.state = 'tool_call';
            break;
          }
          case 'ToolResult': {
            const calls = [...(next.toolCalls ?? [])];
            const match = calls.find((c) => c.callId === event.data.call_id);
            if (match) {
              if (event.data.name === 'correct_asr') {
                match.result = '訂正を反映しました';
                match.isError = false;
              } else {
                match.result = event.data.content;
                match.isError = event.data.is_error;
              }
            }
            next.toolCalls = calls;
            break;
          }
          case 'Text':
            next.text = (next.text ?? '') + event.data;
            next.segments = appendTextSegment(next.segments ?? [], event.data);
            next.state = 'answering';
            break;
          case 'Error':
            next.text = event.data;
            next.segments = finalizeTextSegment(
              next.segments ?? [],
              event.data,
            );
            next.state = 'done';
            break;
          case 'Done':
            next.text = event.data.text;
            next.segments = finalizeTextSegment(
              next.segments ?? [],
              event.data.text,
            );
            next.state = 'done';
            break;
        }
        return [...current.slice(0, index), next, ...current.slice(index + 1)];
      });
    };

    const handleTtsBlock = (block: string, raw = false) => {
      if (!audioReady || !block.trim() || isMutedRef.current) {
        return;
      }
      const ttsText = raw ? markdownToSpeech(block) : block;
      if (!ttsText.trim()) return;
      ttsBlockReceivedRef.current = true;
      ttsQueueRef.current.push(ttsText);
      if (!ttsProcessingRef.current) {
        ttsProcessingRef.current = true;
        setIsSpeaking(true);
        ttsPromiseRef.current = processTtsQueue();
      }
    };

    const processTtsQueue = async () => {
      TakusuAudioModule.clearTtsStop();
      let nextFilePromise: Promise<string> | null = null;
      let nextBlock: string | undefined = ttsQueueRef.current.shift();
      try {
        while (nextBlock || nextFilePromise) {
          if (!nextFilePromise && nextBlock) {
            nextFilePromise = TakusuAudioModule.synthesizeToFile(nextBlock);
          }
          const currentFilePromise = nextFilePromise;
          nextFilePromise = null;
          nextBlock = ttsQueueRef.current.shift();
          if (nextBlock) {
            nextFilePromise = TakusuAudioModule.synthesizeToFile(nextBlock);
          }
          try {
            const path = await currentFilePromise;
            if (!path) continue;
            await TakusuAudioModule.playFile(path);
          } catch (ttsError: unknown) {
            const ttsMessage =
              ttsError instanceof Error ? ttsError.message : String(ttsError);
            const ttsCode =
              ttsError instanceof Error
                ? (ttsError as Error & { code?: string }).code
                : undefined;
            ttsQueueRef.current = [];
            if (
              ttsCode === 'ERR_TTS_INTERRUPTED' ||
              ttsCode === 'ERR_TTS_STOPPED'
            ) {
              console.log('TTS stopped:', ttsMessage);
              break;
            }
            console.error('TTS failed:', ttsMessage);
            void showError(ttsMessage, '音声読み上げに失敗');
            break;
          }
        }
      } finally {
        // Stop speaking immediately; any pre-fetched synthesis that resolves
        // to a path we will not play is deleted in the background.
        TakusuAudioModule.clearTtsStop();
        ttsProcessingRef.current = false;
        setIsSpeaking(false);
        if (nextFilePromise) {
          nextFilePromise
            .then((path) => {
              if (path) {
                TakusuAudioModule.deleteFile(path).catch(() => {});
              }
            })
            .catch(() => {});
        }
      }
    };

    const handleResult = async (result: AgentTurnResult) => {
      setApproval(result.approval_request);
      // If the server did not emit any TtsBlock events (e.g. an older agent
      // version), fall back to synthesizing the full final text once.
      // The final text from an older server is raw, so filter it here.
      if (!ttsBlockReceivedRef.current) {
        handleTtsBlock(result.text, true);
      }
      await ttsPromiseRef.current;
    };

    const handleError = (e: unknown) => {
      const message = e instanceof Error ? e.message : String(e);
      setMessages((current) =>
        current.map((m) =>
          m.id === assistantId
            ? { ...m, text: message, state: 'done', toolCalls: [] }
            : m,
        ),
      );
      if (e instanceof AgentApiError && e.status === 404) {
        setError('Agentセッションが終了しました。もう一度送信してください');
      } else {
        void showError(message, 'エラー');
      }
    };

    for (let attempt = 0; attempt < 2; attempt++) {
      // Reset any TTS state from a previous attempt so retries (e.g. 404)
      // do not carry over queued blocks or the ttsBlockReceived flag.
      ttsQueueRef.current = [];
      ttsProcessingRef.current = false;
      ttsBlockReceivedRef.current = false;
      ttsPromiseRef.current = Promise.resolve();
      const abortController = new AbortController();
      streamAbortRef.current = abortController;
      setUserInput(null);
      try {
        const result = await apiCall(
          currentSessionId,
          handleStreamEvent,
          abortController.signal,
          attempt,
        );
        await handleResult(result);
        await finish();
        return;
      } catch (e: unknown) {
        const isAbort = backgroundAbortedRef.current || e instanceof AbortError;
        if (isAbort) {
          backgroundAbortedRef.current = false;
          setMessages((current) =>
            current.map((m) => {
              const fallback = m.text || '応答が中断されました';
              return m.id === assistantId
                ? {
                    ...m,
                    text: fallback,
                    state: 'done',
                    toolCalls: [],
                    segments: undefined,
                  }
                : m;
            }),
          );
          await finish();
          return;
        }
        if (e instanceof AgentApiError && e.status === 404 && attempt === 0) {
          const lostId = currentSessionId;
          if (lostId) {
            await deleteSessionSnapshot(lostId).catch(() => {});
          }
          const remaining = sessionIdsRef.current.filter((s) => s !== lostId);
          sessionIdsRef.current = remaining;
          setSessionIds(remaining);
          try {
            const emptyId = await findEmptySession(remaining);
            if (emptyId) {
              await activateNewSession(emptyId);
              if (Object.keys(sessionPermissionsRef.current).length > 0) {
                client
                  .updateSessionSettings(emptyId, sessionPermissionsRef.current)
                  .catch(() => {});
              }
              currentSessionId = emptyId;
            } else {
              const created = await client.createSession(
                sessionPermissionsRef.current,
              );
              await activateNewSession(created);
              currentSessionId = created;
            }
          } catch {
            sessionIdRef.current = null;
            handleError(e);
            await finish();
            return;
          }
        } else {
          handleError(e);
          await finish();
          return;
        }
      }
    }
  }
  sendTextRef.current = sendText;

  async function send() {
    const value = text.trim();
    if (!value || busy || isSwitchingRef.current || !historyReady) return;
    setText('');
    setInputHeight(44);
    await sendText(value);
  }

  const appendText = useCallback((transcript: string) => {
    setText((current) => {
      const prefix =
        current && !current.endsWith(' ') ? `${current} ` : current;
      return `${prefix}${transcript.trim()}`;
    });
  }, []);

  const toggleGroupCollapsed = useCallback((id: string, groupIndex: number) => {
    setMessages((current) =>
      current.map((m) =>
        m.id === id
          ? { ...m, collapsedGroups: updateCollapsedGroup(m, groupIndex) }
          : m,
      ),
    );
  }, []);

  const handleMessageLongPress = useCallback(
    (message: Message, position: { x: number; y: number }) => {
      haptic.medium();
      setContextMenu({
        visible: true,
        messageId: message.id,
        position,
      });
    },
    [],
  );

  const contextMenuMessage = useMemo(
    () => messages.find((m) => m.id === contextMenu.messageId) ?? null,
    [messages, contextMenu.messageId],
  );

  const handleCopy = useCallback(async () => {
    if (!contextMenuMessage) return;
    await Clipboard.setStringAsync(contextMenuMessage.text);
    haptic.success();
    setContextMenu((current) => ({ ...current, visible: false }));
  }, [contextMenuMessage]);

  const handleEdit = useCallback(() => {
    if (!contextMenuMessage || busy || isSwitchingRef.current || !historyReady)
      return;
    setEditModal({
      visible: true,
      messageId: contextMenuMessage.id,
      text: contextMenuMessage.text,
    });
    setContextMenu((current) => ({ ...current, visible: false }));
  }, [contextMenuMessage, busy, historyReady]);

  function handleSaveEdit(newText: string) {
    setEditModal({ visible: false, messageId: '', text: '' });
    if (
      !sessionIdRef.current ||
      !newText.trim() ||
      busy ||
      isSwitchingRef.current ||
      !historyReady
    )
      return;
    const index = messages.findIndex((m) => m.id === editModal.messageId);
    if (index === -1 || messages[index].role !== 'user') return;
    const turnIndex = getTurnIndex(messages, index);
    editTurn(turnIndex, editModal.messageId, newText.trim());
  }

  const handleToolPress = useCallback((call: ToolCallItem) => {
    haptic.light();
    setToolDetail({ visible: true, call });
  }, []);

  const closeToolDetail = useCallback(() => {
    setToolDetail({ visible: false, call: null });
  }, []);

  const handleRevert = useCallback(async () => {
    if (
      !contextMenuMessage ||
      !sessionIdRef.current ||
      busy ||
      isSwitchingRef.current ||
      !historyReady
    )
      return;
    streamAbortRef.current?.abort();
    const index = messages.findIndex((m) => m.id === contextMenuMessage.id);
    if (index === -1 || index === messages.length - 1) {
      setContextMenu((current) => ({ ...current, visible: false }));
      return;
    }
    const turnIndex = getTurnIndex(messages, index);
    const afterUser = contextMenuMessage.role === 'user';
    haptic.medium();
    setBusy(true);
    try {
      await client.revertTurn(sessionIdRef.current, turnIndex, afterUser);
      setMessages((current) => current.slice(0, index + 1));
      setApproval(null);
      setUserInput(null);
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setBusy(false);
      setContextMenu((current) => ({ ...current, visible: false }));
    }
  }, [contextMenuMessage, messages, client, historyReady, busy]);

  const handleScroll = useCallback(
    (event: NativeSyntheticEvent<NativeScrollEvent>) => {
      if (sessionJustLoadedRef.current) return;
      const { contentOffset, contentSize, layoutMeasurement } =
        event.nativeEvent;
      const distanceFromBottom =
        contentSize.height - contentOffset.y - layoutMeasurement.height;
      autoScrollRef.current = distanceFromBottom <= SCROLL_THRESHOLD;
    },
    [],
  );

  const handleMessagesContentSizeChange = useCallback(() => {
    if (autoScrollRef.current || sessionJustLoadedRef.current) {
      flatListRef.current?.scrollToEnd({ animated: false });
      clearSessionJustLoaded();
    }
  }, []);

  const keyExtractor = useCallback((item: Message) => item.id, []);

  const handleMessagesLayout = useCallback((event: LayoutChangeEvent) => {
    setMessagesHeight(event.nativeEvent.layout.height);
  }, []);

  const renderItem = useCallback(
    ({ item }: { item: Message }) => {
      if (item.role === 'user') {
        return (
          <UserMessage message={item} onLongPress={handleMessageLongPress} />
        );
      }
      const isLatest = item.id === messages[messages.length - 1]?.id;
      return (
        <AssistantMessage
          item={item}
          colors={colors}
          markdownStyles={markdownStyles}
          markdownIt={markdownIt}
          markdownRules={markdownRules}
          availableHeight={messagesHeight}
          isLatest={isLatest}
          onToggleGroupCollapsed={toggleGroupCollapsed}
          onLongPress={handleMessageLongPress}
          onToolCallPress={handleToolPress}
        />
      );
    },
    [
      messages,
      colors,
      markdownStyles,
      markdownIt,
      markdownRules,
      messagesHeight,
      toggleGroupCollapsed,
      handleMessageLongPress,
      handleToolPress,
    ],
  );

  const listEmpty = useMemo(
    () => (
      <Text style={[styles.empty, { color: colors.gray }]}>
        何を予定しますか？
      </Text>
    ),
    [colors.gray],
  );

  async function resolve(
    approve: boolean,
    grantedPermissions?: PermissionsMap,
    persistToProvider?: boolean,
  ) {
    if (!sessionIdRef.current || !approval || busy || isSwitchingRef.current)
      return;
    setBusy(true);
    setError(null);
    const positiveGranted: PermissionsMap = {};
    if (approve && grantedPermissions) {
      for (const [key, value] of Object.entries(grantedPermissions)) {
        if (value) positiveGranted[key] = true;
      }
    }
    let newSessionPermissions: PermissionsMap | undefined;
    if (Object.keys(positiveGranted).length > 0) {
      newSessionPermissions = {
        ...sessionPermissionsRef.current,
        ...positiveGranted,
      };
    }
    try {
      const result = await client.resolveApproval(
        sessionIdRef.current,
        approval.id,
        approve,
        newId('approval'),
      );
      setApproval(null);
      if (result.approved) {
        showTopToast('変更を適用しました');
      }
      if (!result.approved) {
        setMessages((current) => {
          const next = [...current];
          for (let i = next.length - 1; i >= 0; i--) {
            if (next[i].role === 'assistant') {
              next[i] = markRejectedToolCalls(next[i]);
              break;
            }
          }
          const resolveText = '変更を取り消しました。';
          next.push({
            id: newId('assistant'),
            role: 'assistant',
            text: resolveText,
            segments: [{ type: 'text', text: resolveText }],
            collapsedGroups: [],
          });
          return next;
        });
      }

      if (result.approved && newSessionPermissions) {
        sessionPermissionsRef.current = newSessionPermissions;
        setSessionPermissions(newSessionPermissions);

        await client
          .updateSessionSettings(sessionIdRef.current, newSessionPermissions)
          .catch((e: unknown) => {
            const message = e instanceof Error ? e.message : String(e);
            const errorText = `セッション権限の保存に失敗しました: ${message}`;
            void showError(errorText, '保存失敗');
            console.error('Failed to update session permissions:', e);
          });

        if (persistToProvider) {
          (async () => {
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
            await pushAgentConfig();
          })().catch((e: unknown) => {
            const message = e instanceof Error ? e.message : String(e);
            const errorText = `Provider 設定の保存に失敗しました: ${message}`;
            void showError(errorText, '保存失敗');
            console.error('Failed to persist provider permissions:', e);
          });
        }
      }
    } catch (e: unknown) {
      void showError(e, 'エラー');
    } finally {
      setBusy(false);
    }
  }

  async function submitUserInput(answers: UserInputAnswer[]) {
    if (!userInput) return;
    const { sessionId, callId } = userInput;
    setUserInput(null);
    try {
      await client.submitUserInput(sessionId, callId, answers);
    } catch (e: unknown) {
      const message = e instanceof Error ? e.message : String(e);
      void showError(message, '訂正の送信に失敗');
      streamAbortRef.current?.abort();
    }
  }

  function resetSwitchState() {
    setIsSwitching(false);
    isSwitchingRef.current = false;
  }

  function resetToCenter() {
    if (viewOffset.value === 0) {
      resetSwitchState();
      return;
    }
    viewOffset.value = withTiming(
      0,
      { duration: 200, easing: Easing.out(Easing.exp) },
      (finished) => {
        'worklet';
        if (finished) runOnJS(resetSwitchState)();
      },
    );
  }

  async function switchSession(delta: number) {
    if (isSwitchingRef.current || busy || !historyReady) {
      resetToCenter();
      return;
    }
    const nextIndex = activeIndexRef.current + delta;
    if (nextIndex < 0 || nextIndex >= sessionIdsRef.current.length) {
      resetToCenter();
      return;
    }
    const nextId = sessionIdsRef.current[nextIndex];
    const currentId = sessionIdRef.current;
    isSwitchingRef.current = true;
    setIsSwitching(true);
    try {
      if (currentId) {
        await saveSessionSnapshot(currentId, {
          messages,
          approval,
          permissions: sessionPermissionsRef.current,
        }).catch(() => {});
      }
      const snapshot = (await loadSessionSnapshot(nextId)) ?? {
        messages: [],
        approval: null,
      };
      const perms = snapshot.permissions ?? {};
      setSessionPermissions(perms);
      sessionPermissionsRef.current = perms;
      sessionIdRef.current = nextId;
      setActiveIndex(nextIndex);
      activeIndexRef.current = nextIndex;
      setText('');
      autoScrollRef.current = true;
      markSessionJustLoaded();
      setMessages(snapshot.messages);
      setApproval(snapshot.approval);
      if (Object.keys(perms).length > 0) {
        client.updateSessionSettings(nextId, perms).catch(() => {});
      }
      saveSessionHistory({
        ids: sessionIdsRef.current,
        activeIndex: nextIndex,
      }).catch(() => {});
      resetToCenter();
    } catch (e: unknown) {
      void showError(e, 'エラー');
      resetToCenter();
    }
  }

  async function toggleMuted() {
    const next = !isMuted;
    setIsMuted(next);
    isMutedRef.current = next;
    try {
      await saveTtsMuted(next);
      await TakusuAudioModule.setMuted(next);
    } catch (e: unknown) {
      setIsMuted(!next);
      isMutedRef.current = !next;
      await showError(e, 'ミュート状態の更新に失敗');
    }
  }

  async function startNewSession() {
    if (isSwitchingRef.current || busy || !historyReady) return;
    setError(null);
    isSwitchingRef.current = true;
    setIsSwitching(true);
    const currentId = sessionIdRef.current;

    const emptyId = await findEmptySession(sessionIdsRef.current, currentId);
    if (emptyId) {
      if (emptyId === currentId) {
        if (
          currentId &&
          Object.keys(sessionPermissionsRef.current).length > 0
        ) {
          sessionPermissionsRef.current = {};
          setSessionPermissions({});
          client.updateSessionSettings(currentId, {}).catch((e: unknown) => {
            console.error('Failed to clear session permissions:', e);
          });
        }
        setText('');
        resetToCenter();
        return;
      }
      if (currentId) {
        await saveSessionSnapshot(currentId, {
          messages,
          approval,
          permissions: sessionPermissionsRef.current,
        }).catch(() => {});
      }
      await activateSessionId(emptyId, false);
      resetToCenter();
      return;
    }

    const previousMessages = messages;
    const previousApproval = approval;
    if (currentId) {
      await saveSessionSnapshot(currentId, {
        messages: previousMessages,
        approval: previousApproval,
        permissions: sessionPermissionsRef.current,
      }).catch(() => {});
    }
    skipSnapshotSaveRef.current = true;
    setSessionPermissions({});
    sessionPermissionsRef.current = {};
    setMessages([]);
    setApproval(null);
    try {
      const created = await client.createSession({});
      let ids = [...sessionIdsRef.current, created];
      const max = sessionHistoryCountRef.current;
      const { ids: trimmed, removed } = trimSessionIds(ids, max, currentId);
      ids = trimmed;
      const nextIndex = ids.indexOf(created);
      await Promise.all(removed.map((id) => deleteSessionSnapshot(id))).catch(
        () => {},
      );
      sessionIdsRef.current = ids;
      setSessionIds(ids);
      setActiveIndex(nextIndex);
      activeIndexRef.current = nextIndex;
      sessionIdRef.current = created;
      setText('');
      autoScrollRef.current = true;
      markSessionJustLoaded();
      saveSessionHistory({ ids, activeIndex: nextIndex }).catch(() => {});
      resetToCenter();
    } catch (e: unknown) {
      sessionIdRef.current = currentId;
      setMessages(previousMessages);
      setApproval(previousApproval);
      void showError(e, 'エラー');
      resetToCenter();
    } finally {
      skipSnapshotSaveRef.current = false;
    }
  }

  async function deleteCurrentSession() {
    if (isSwitchingRef.current || busy || !historyReady) return;
    setError(null);
    const idToDelete = sessionIdRef.current;
    if (!idToDelete) return;

    isSwitchingRef.current = true;
    setIsSwitching(true);

    try {
      try {
        await client.deleteSession(idToDelete);
      } catch (e: unknown) {
        if (e instanceof AgentApiError && e.status === 404) {
          // Already deleted on the server; continue to clean up locally.
        } else {
          throw e;
        }
      }

      await deleteSessionSnapshot(idToDelete);

      const ids = sessionIdsRef.current.filter((s) => s !== idToDelete);
      sessionIdsRef.current = ids;
      setSessionIds(ids);

      // Clear the current view so stale content is not shown if activation fails.
      setMessages([]);
      setApproval(null);
      setSessionPermissions({});
      sessionPermissionsRef.current = {};
      sessionIdRef.current = null;

      if (ids.length === 0) {
        setActiveIndex(0);
        activeIndexRef.current = 0;
        const created = await client.createSession({});
        await activateSessionId(created, true);
      } else {
        const nextIndex = Math.min(activeIndexRef.current, ids.length - 1);
        activeIndexRef.current = nextIndex;
        setActiveIndex(nextIndex);
        const nextId = ids[nextIndex];
        await activateSessionId(nextId, false);
      }
    } catch (e: unknown) {
      void showError(e, 'エラー');
    } finally {
      isSwitchingRef.current = false;
      setIsSwitching(false);
    }
  }

  const switcherGesture = Gesture.Pan()
    .minDistance(20)
    .onUpdate((e) => {
      const x = e.translationX;
      viewOffset.value = Math.max(-width, Math.min(width, x));
    })
    .onEnd((e) => {
      if (e.translationX > SWIPE_THRESHOLD) {
        runOnJS(switchSession)(-1);
      } else if (e.translationX < -SWIPE_THRESHOLD) {
        runOnJS(switchSession)(1);
      } else {
        viewOffset.value = withTiming(0, {
          duration: 200,
          easing: Easing.out(Easing.exp),
        });
      }
    });

  return (
    <KeyboardAvoidingView
      style={[styles.container, { backgroundColor: colors.white }]}
      behavior="height"
      enabled={keyboardVisible}
    >
      <Reanimated.View style={[styles.container, animatedStyle]}>
        <View
          style={[
            styles.header,
            {
              borderBottomColor: colors.separator,
              paddingTop: 8 + insets.top,
            },
          ]}
        >
          <Pressable onPress={() => router.back()} style={styles.back}>
            <Text style={[styles.backText, { color: BRAND_COLOR }]}>‹</Text>
          </Pressable>
          <Text style={[styles.title, { color: colors.black }]}>Agent</Text>
          <Pressable
            disabled={busy || isSwitching || !historyReady}
            onPress={toggleMuted}
            style={styles.muteButton}
          >
            <Ionicons
              name={isMuted ? 'volume-mute' : 'volume-high'}
              size={24}
              color={
                busy || isSwitching || !historyReady
                  ? colors.gray
                  : isMuted
                    ? colors.gray
                    : BRAND_COLOR
              }
            />
          </Pressable>
          <Pressable
            disabled={busy || isSwitching || !historyReady}
            onPress={() => setPermissionsModal(true)}
            style={styles.permissionsButton}
          >
            <Ionicons
              name={
                Object.values(sessionPermissions).some(Boolean)
                  ? 'shield-checkmark'
                  : 'shield-outline'
              }
              size={24}
              color={
                busy || isSwitching || !historyReady
                  ? colors.gray
                  : Object.values(sessionPermissions).some(Boolean)
                    ? BRAND_COLOR
                    : colors.gray
              }
            />
          </Pressable>
          <View style={styles.deleteSession}>
            <DeleteConfirmButton
              onConfirm={deleteCurrentSession}
              disabled={
                busy || isSwitching || !historyReady || !sessionIdRef.current
              }
            />
          </View>
          <Pressable
            disabled={busy || isSwitching || !historyReady}
            onPress={startNewSession}
            style={styles.newSession}
          >
            <Ionicons
              name="add"
              size={28}
              color={
                busy || isSwitching || !historyReady ? colors.gray : BRAND_COLOR
              }
            />
          </Pressable>
        </View>
        <FlatList
          ref={flatListRef}
          style={styles.messages}
          contentContainerStyle={styles.messageContent}
          data={messages}
          keyExtractor={keyExtractor}
          onLayout={handleMessagesLayout}
          renderItem={renderItem}
          onScroll={handleScroll}
          scrollEventThrottle={16}
          onContentSizeChange={handleMessagesContentSizeChange}
          ListEmptyComponent={listEmpty}
          initialNumToRender={10}
          maxToRenderPerBatch={10}
          windowSize={5}
        />
        {approval && (
          <ApprovalPanel
            approval={approval}
            busy={busy || isSwitching}
            client={client}
            colors={colors}
            onApprove={(granted, persist) => resolve(true, granted, persist)}
            onDeny={() => resolve(false)}
            permissions={sessionPermissions}
          />
        )}
        {userInput && (
          <View
            style={[
              styles.userInputSheet,
              {
                borderColor: colors.separator,
                backgroundColor: colors.surface,
              },
            ]}
          >
            <Text style={[styles.userInputTitle, { color: colors.black }]}>
              認識結果を確認してください
            </Text>
            <Text style={[styles.userInputSubtitle, { color: colors.gray }]}>
              自明な部分は推測済みです。曖昧な語だけ確認します
            </Text>
            <ScrollView style={styles.userInputScroll}>
              {userInput.questions.map((q, i) => (
                <View
                  key={i}
                  style={[
                    styles.userInputCard,
                    {
                      borderColor: colors.separator,
                      backgroundColor: colors.surfaceTint,
                    },
                  ]}
                >
                  <Text style={[styles.userInputLabel, { color: colors.gray }]}>
                    元の認識テキスト
                  </Text>
                  <Text
                    style={[styles.userInputOriginal, { color: colors.black }]}
                  >
                    {q.text}
                  </Text>
                  <Text style={[styles.userInputLabel, { color: colors.gray }]}>
                    目的
                  </Text>
                  <Text
                    style={[styles.userInputPurpose, { color: colors.black }]}
                  >
                    {q.for}
                  </Text>
                  <TextInput
                    style={[
                      styles.userInputField,
                      {
                        color: colors.black,
                        borderColor: colors.separator,
                        backgroundColor: colors.surface,
                      },
                    ]}
                    value={userInput.values[i]}
                    onChangeText={(value) =>
                      setUserInput((current) =>
                        current
                          ? {
                              ...current,
                              values: current.values.map((v, idx) =>
                                idx === i ? value : v,
                              ),
                            }
                          : null,
                      )
                    }
                    placeholder={q.text}
                    placeholderTextColor={colors.gray}
                    editable={!isSwitching}
                  />
                </View>
              ))}
            </ScrollView>
            <View style={styles.userInputActions}>
              <Pressable
                disabled={isSwitching}
                onPress={() =>
                  submitUserInput(
                    userInput.questions.map((q) => ({ text: q.text })),
                  )
                }
                style={[styles.userInputButton, styles.userInputSecondary]}
              >
                <Text style={styles.userInputSecondaryText}>
                  そのまま続ける
                </Text>
              </Pressable>
              <Pressable
                disabled={isSwitching}
                onPress={() =>
                  submitUserInput(
                    userInput.values.map((value) => ({ text: value })),
                  )
                }
                style={[styles.userInputButton, styles.userInputPrimary]}
              >
                <Text style={styles.userInputPrimaryText}>確定</Text>
              </Pressable>
            </View>
          </View>
        )}
        {error && (
          <Pressable onPress={() => void showError(error, 'Agentエラー')}>
            <Text style={styles.error}>{error}</Text>
          </Pressable>
        )}
        {sessionIds.length > 1 && (
          <GestureDetector gesture={switcherGesture}>
            <View
              style={[
                styles.switcher,
                {
                  borderTopColor: colors.separator,
                  paddingBottom: 8,
                },
              ]}
            >
              <Pressable
                disabled={activeIndex === 0 || isSwitching || busy}
                onPress={() => switchSession(-1)}
                style={styles.switcherButton}
              >
                <Ionicons
                  name="chevron-back"
                  size={20}
                  color={
                    activeIndex > 0 && !isSwitching && !busy
                      ? colors.black
                      : colors.gray
                  }
                />
              </Pressable>
              <View style={styles.dots}>
                {sessionIds.map((id, i) => (
                  <View
                    key={id}
                    style={[
                      styles.dot,
                      {
                        backgroundColor:
                          i === activeIndex ? BRAND_COLOR : colors.grayLight,
                      },
                    ]}
                  />
                ))}
              </View>
              <Pressable
                disabled={
                  activeIndex >= sessionIds.length - 1 || isSwitching || busy
                }
                onPress={() => switchSession(1)}
                style={styles.switcherButton}
              >
                <Ionicons
                  name="chevron-forward"
                  size={20}
                  color={
                    activeIndex < sessionIds.length - 1 && !isSwitching && !busy
                      ? colors.black
                      : colors.gray
                  }
                />
              </Pressable>
            </View>
          </GestureDetector>
        )}
        <View
          style={[
            styles.composer,
            {
              borderTopColor: colors.separator,
              paddingBottom: 12 + (keyboardVisible ? 0 : insets.bottom),
            },
          ]}
        >
          <TextInput
            style={[
              styles.input,
              {
                color: colors.black,
                borderColor: colors.separator,
                height: inputHeight,
              },
            ]}
            value={text}
            onChangeText={setText}
            placeholder="メッセージ"
            placeholderTextColor={colors.gray}
            editable={(!busy || isSpeaking) && !isSwitching && historyReady}
            multiline
            textAlignVertical="top"
            onContentSizeChange={(e) => {
              const h = e.nativeEvent.contentSize.height;
              setInputHeight(Math.max(44, Math.min(120, h)));
            }}
          />
          <ComposerRecordButton
            audioReady={audioReady}
            historyReady={historyReady}
            busy={busy || isSwitching}
            onAppend={appendText}
          />
          <Pressable
            disabled={busy || isSwitching || !historyReady || !text.trim()}
            onPress={send}
            style={styles.send}
          >
            {busy ? (
              <ActivityIndicator color={COLORS.white} />
            ) : (
              <Text style={styles.sendText}>送信</Text>
            )}
          </Pressable>
        </View>

        <MessageContextMenu
          visible={contextMenu.visible}
          position={contextMenu.position}
          canEdit={!busy && contextMenuMessage?.role === 'user'}
          canRevert={
            !busy &&
            !!contextMenuMessage &&
            messages.findIndex((m) => m.id === contextMenuMessage.id) <
              messages.length - 1
          }
          onClose={() =>
            setContextMenu((current) => ({ ...current, visible: false }))
          }
          onCopy={handleCopy}
          onEdit={handleEdit}
          onRevert={handleRevert}
        />

        <EditMessageModal
          visible={editModal.visible}
          text={editModal.text}
          onClose={() =>
            setEditModal({ visible: false, messageId: '', text: '' })
          }
          onSave={handleSaveEdit}
        />

        <SessionPermissionsModal
          visible={permissionsModal}
          permissions={sessionPermissions}
          onChange={(permissions) => {
            setSessionPermissions(permissions);
            sessionPermissionsRef.current = permissions;
            const id = sessionIdRef.current;
            if (id) {
              client.updateSessionSettings(id, permissions).catch(() => {});
              saveSessionSnapshot(id, {
                messages,
                approval,
                permissions,
              }).catch(() => {});
            }
          }}
          onClose={() => setPermissionsModal(false)}
        />

        <ToolCallDetailModal
          visible={toolDetail.visible}
          call={toolDetail.call}
          onClose={closeToolDetail}
        />
      </Reanimated.View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1 },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    borderBottomWidth: 1,
    paddingBottom: 10,
  },
  back: { width: 56, alignItems: 'center' },
  backText: { fontSize: 40, lineHeight: 40 },
  title: { flex: 1, fontSize: 20, fontWeight: '700' },
  muteButton: {
    width: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  permissionsButton: {
    width: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  deleteSession: {
    width: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  newSession: { width: 56, alignItems: 'center', justifyContent: 'center' },
  messages: { flex: 1 },
  messageContent: {
    padding: 16,
    gap: 10,
    flexGrow: 1,
    justifyContent: 'flex-end',
  },
  empty: { textAlign: 'center', marginBottom: 20 },
  bubble: { maxWidth: '85%', padding: 12, borderRadius: 14 },
  userBubble: { alignSelf: 'flex-end' },
  assistantBubble: { alignSelf: 'flex-start' },
  contextHeader: {
    borderRadius: 10,
    borderWidth: 1,
    overflow: 'hidden',
  },
  contextHeaderInner: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    padding: 8,
  },
  toolChips: {
    flexGrow: 1,
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 6,
  },
  toolChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 12,
    borderWidth: 1,
  },
  toolChipDot: { width: 6, height: 6, borderRadius: 3 },
  toolChipText: { fontSize: 11 },
  contextBody: { gap: 6 },
  loadingIndicator: { alignItems: 'center', minWidth: 120 },
  thinkingCard: {
    borderRadius: 8,
    padding: 10,
    borderWidth: 1,
    gap: 4,
  },
  thinkingHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  thinkingLabel: { fontSize: 11, fontWeight: '700' },
  thinkingText: { fontSize: 12, fontStyle: 'italic' },
  contextFooter: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 4,
    paddingTop: 8,
    marginTop: 8,
    borderTopWidth: 1,
  },
  contextFooterText: { fontSize: 11 },
  toolCall: {
    borderRadius: 10,
    padding: 10,
    borderWidth: 1,
    gap: 4,
  },
  toolCallHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 4,
  },
  toolCallHeaderLeft: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
  },
  toolStatus: { width: 8, height: 8, borderRadius: 4 },
  rejectedBadge: {
    paddingHorizontal: 6,
    paddingVertical: 2,
    borderRadius: 8,
  },
  rejectedBadgeText: {
    color: COLORS.white,
    fontSize: 10,
    fontWeight: '700',
  },
  toolArgs: { fontSize: 11, fontFamily: 'monospace' },
  userInputSheet: {
    margin: 12,
    padding: 12,
    borderWidth: 1,
    borderRadius: 16,
    gap: 10,
    maxHeight: '60%',
  },
  userInputTitle: { fontWeight: '700', fontSize: 16 },
  userInputSubtitle: { fontSize: 12 },
  userInputScroll: { maxHeight: 320 },
  userInputCard: {
    borderWidth: 1,
    borderRadius: 10,
    padding: 10,
    marginBottom: 10,
    gap: 4,
  },
  userInputLabel: { fontSize: 11, fontWeight: '600' },
  userInputOriginal: { fontSize: 15, fontWeight: '700' },
  userInputPurpose: { fontSize: 13, marginBottom: 6 },
  userInputField: {
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 8,
    fontSize: 14,
  },
  userInputActions: { flexDirection: 'row', gap: 8, marginTop: 4 },
  userInputButton: {
    flex: 1,
    padding: 12,
    borderRadius: 8,
    alignItems: 'center',
  },
  userInputSecondary: { borderWidth: 1, borderColor: '#B33A3A' },
  userInputSecondaryText: { color: '#B33A3A', fontWeight: '700' },
  userInputPrimary: { backgroundColor: BRAND_COLOR },
  userInputPrimaryText: { color: COLORS.white, fontWeight: '700' },
  error: { color: '#B33A3A', paddingHorizontal: 16, paddingBottom: 8 },
  switcher: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 12,
    paddingTop: 8,
    borderTopWidth: 1,
  },
  switcherButton: {
    width: 56,
    height: 32,
    alignItems: 'center',
    justifyContent: 'center',
  },
  dots: { flexDirection: 'row', gap: 8, alignItems: 'center' },
  dot: { width: 8, height: 8, borderRadius: 4 },
  composer: {
    flexDirection: 'row',
    gap: 8,
    paddingHorizontal: 12,
    paddingTop: 12,
    borderTopWidth: 1,
  },
  input: {
    flex: 1,
    minHeight: 44,
    borderWidth: 1,
    borderRadius: 10,
    paddingHorizontal: 12,
  },
  send: {
    minWidth: 64,
    borderRadius: 10,
    backgroundColor: BRAND_COLOR,
    alignItems: 'center',
    justifyContent: 'center',
  },
  sendText: { color: COLORS.white, fontWeight: '700' },
});

export default AgentView;
