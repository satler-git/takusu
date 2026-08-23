import AsyncStorage from '@react-native-async-storage/async-storage';
import type { ApprovalRequest, IntakeState } from './agentTypes';
import type { PermissionsMap } from './settingsStore';

const HISTORY_KEY = 'takusu.agent.sessionHistory';
const SNAPSHOT_KEY = (id: string) => `takusu.agent.sessionSnapshot.${id}`;

export interface ToolCallItem {
  name: string;
  callId?: string;
  arguments?: unknown;
  result?: string;
  isError?: boolean;
  isRejected?: boolean;
}

export type MessageSegment =
  | { type: 'thinking'; text: string }
  | { type: 'text'; text: string }
  | { type: 'toolCall'; callIndex: number };

export interface Message {
  id: string;
  role: 'user' | 'assistant';
  text: string;
  thinking?: string;
  toolCalls?: ToolCallItem[];
  state?: 'thinking' | 'tool_call' | 'answering' | 'done';
  collapsed?: boolean;
  segments?: MessageSegment[];
  collapsedGroups?: boolean[];
}

export interface AgentSessionSnapshot {
  messages: Message[];
  approval: ApprovalRequest | null;
  permissions?: PermissionsMap;
  intakeState?: IntakeState | null;
}

export interface AgentSessionHistory {
  ids: string[];
  activeIndex: number;
}

export async function loadSessionHistory(): Promise<AgentSessionHistory> {
  const raw = await AsyncStorage.getItem(HISTORY_KEY);
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as unknown;
      if (
        parsed &&
        typeof parsed === 'object' &&
        Array.isArray((parsed as AgentSessionHistory).ids) &&
        (parsed as AgentSessionHistory).ids.every(
          (id) => typeof id === 'string',
        ) &&
        typeof (parsed as AgentSessionHistory).activeIndex === 'number'
      ) {
        return parsed as AgentSessionHistory;
      }
    } catch {
      // fallthrough
    }
  }
  return { ids: [], activeIndex: 0 };
}

export async function saveSessionHistory(
  history: AgentSessionHistory,
): Promise<void> {
  await AsyncStorage.setItem(HISTORY_KEY, JSON.stringify(history));
}

export async function deleteSessionHistory(): Promise<void> {
  await AsyncStorage.removeItem(HISTORY_KEY);
}

export async function loadSessionSnapshot(
  id: string,
): Promise<AgentSessionSnapshot | null> {
  const raw = await AsyncStorage.getItem(SNAPSHOT_KEY(id));
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (
      parsed &&
      typeof parsed === 'object' &&
      Array.isArray((parsed as AgentSessionSnapshot).messages) &&
      ((parsed as AgentSessionSnapshot).approval === null ||
        typeof (parsed as AgentSessionSnapshot).approval === 'object')
    ) {
      return parsed as AgentSessionSnapshot;
    }
  } catch {
    // fallthrough
  }
  return null;
}

export async function saveSessionSnapshot(
  id: string,
  snapshot: AgentSessionSnapshot,
): Promise<void> {
  await AsyncStorage.setItem(SNAPSHOT_KEY(id), JSON.stringify(snapshot));
}

export async function deleteSessionSnapshot(id: string): Promise<void> {
  await AsyncStorage.removeItem(SNAPSHOT_KEY(id));
}
