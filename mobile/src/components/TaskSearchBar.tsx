// Query-aware task search bar with server-driven completion and token chips.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  FlatList,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useColors } from '@/src/theme';
import type { Completion } from '@/src/api/types';
import { TakusuClient } from '@/src/api/client';
import { PressableScale } from '@/src/components/PressableScale';

interface Token {
  raw: string;
  type: 'op' | 'paren' | 'term';
  start: number;
  end: number;
}

const DEFAULT_QUALIFIERS = [
  'status',
  'title',
  'desc',
  'description',
  'start',
  'end',
  'scheduled-start',
  'scheduled-end',
  'from',
  'until',
  'habit',
  'is',
  'has',
];

const DEFAULT_COMPLETIONS: Completion[] = DEFAULT_QUALIFIERS.map((q) => ({
  label: `${q}:`,
  value: `${q}:`,
}));

const COMPLETION_CACHE_TTL_MS = 30_000;
const COMPLETION_CACHE_MAX_SIZE = 100;

interface CachedCompletions {
  completions: Completion[];
  timestamp: number;
}

function isOp(raw: string): boolean {
  const upper = raw.toUpperCase();
  return upper === 'OR' || upper === 'AND' || upper === 'NOT';
}

function tokenize(query: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < query.length) {
    const c = query[i];
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    if (c === '(' || c === ')') {
      tokens.push({ type: 'paren', raw: c, start: i, end: i + 1 });
      i++;
      continue;
    }
    if (c === '"') {
      const start = i;
      i++;
      while (i < query.length && query[i] !== '"') {
        i++;
      }
      if (i < query.length) {
        i++;
      }
      tokens.push({ type: 'term', raw: query.slice(start, i), start, end: i });
      continue;
    }
    const start = i;
    while (
      i < query.length &&
      !/\s/.test(query[i]) &&
      query[i] !== '(' &&
      query[i] !== ')'
    ) {
      i++;
    }
    const raw = query.slice(start, i);
    if (!raw) continue;

    if (raw === '-') {
      tokens.push({ type: 'op', raw, start, end: i });
      continue;
    }

    if (raw.startsWith('-') && raw.length > 1) {
      tokens.push({ type: 'op', raw: '-', start, end: start + 1 });
      const rest = raw.slice(1);
      tokens.push({
        type: isOp(rest) ? 'op' : 'term',
        raw: rest,
        start: start + 1,
        end: i,
      });
      continue;
    }

    tokens.push({
      type: isOp(raw) ? 'op' : 'term',
      raw,
      start,
      end: i,
    });
  }
  return tokens;
}

function removeTokenAt(query: string, start: number, end: number): string {
  const before = query.slice(0, start).trimEnd();
  const after = query.slice(end).trimStart();
  if (before.length === 0) return after;
  if (after.length === 0) return before;
  return `${before} ${after}`;
}

interface TaskSearchBarProps {
  value: string;
  onChangeText: (text: string) => void;
  client: TakusuClient | null;
}

export function TaskSearchBar({
  value,
  onChangeText,
  client,
}: TaskSearchBarProps) {
  const colors = useColors();
  const [completions, setCompletions] = useState<Completion[]>([]);
  const [focused, setFocused] = useState(false);
  const blurTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const completionCacheRef = useRef<Map<string, CachedCompletions>>(new Map());
  const requestIdRef = useRef(0);

  // Clear pending timers when the component unmounts.
  useEffect(() => {
    return () => {
      if (blurTimeoutRef.current) {
        clearTimeout(blurTimeoutRef.current);
      }
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
      debounceRef.current = null;
    }
    if (!client || value.trim().length === 0) {
      setCompletions(value.trim().length === 0 ? DEFAULT_COMPLETIONS : []);
      return;
    }

    const cache = completionCacheRef.current;
    const now = Date.now();
    const cached = cache.get(value);
    if (cached && now - cached.timestamp <= COMPLETION_CACHE_TTL_MS) {
      // Move the hit to the end to keep the LRU order up to date.
      cache.delete(value);
      cache.set(value, cached);
      setCompletions(cached.completions);
      return;
    }
    if (cached) {
      cache.delete(value);
    }

    const id = ++requestIdRef.current;
    debounceRef.current = setTimeout(() => {
      client
        .completeTaskQuery(value)
        .then((results) => {
          if (requestIdRef.current !== id) {
            return;
          }

          const fetchedAt = Date.now();
          const expiredKeys: string[] = [];
          for (const [key, entry] of cache) {
            if (fetchedAt - entry.timestamp > COMPLETION_CACHE_TTL_MS) {
              expiredKeys.push(key);
            }
          }
          for (const key of expiredKeys) {
            cache.delete(key);
          }
          if (cache.size >= COMPLETION_CACHE_MAX_SIZE) {
            const oldest = cache.keys().next().value as string | undefined;
            if (oldest !== undefined && oldest !== value) {
              cache.delete(oldest);
            }
          }
          // Delete and re-set to move the entry to the most-recent position.
          cache.delete(value);
          cache.set(value, { completions: results, timestamp: fetchedAt });
          setCompletions(results);
        })
        .catch(() => {
          if (requestIdRef.current === id) {
            setCompletions([]);
          }
        });
    }, 250);
    return () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
    };
  }, [value, client]);

  const chips = useMemo(() => tokenize(value), [value]);

  const handleSelect = useCallback(
    (item: Completion) => {
      onChangeText(item.value);
      if (blurTimeoutRef.current) {
        clearTimeout(blurTimeoutRef.current);
      }
      setFocused(true);
    },
    [onChangeText],
  );

  const handleRemove = useCallback(
    (start: number, end: number) => {
      onChangeText(removeTokenAt(value, start, end));
    },
    [onChangeText, value],
  );

  const handleClear = useCallback(() => {
    onChangeText('');
  }, [onChangeText]);

  return (
    <View style={styles.container}>
      <View
        style={[
          styles.inputRow,
          {
            borderColor: colors.separator,
            backgroundColor: colors.surface,
          },
        ]}
      >
        <TextInput
          style={[styles.input, { color: colors.black }]}
          value={value}
          onChangeText={onChangeText}
          onFocus={() => {
            if (blurTimeoutRef.current) {
              clearTimeout(blurTimeoutRef.current);
            }
            setFocused(true);
          }}
          onBlur={() => {
            blurTimeoutRef.current = setTimeout(() => setFocused(false), 150);
          }}
          placeholder="検索..."
          placeholderTextColor={colors.grayLight}
          autoCapitalize="none"
          autoCorrect={false}
        />
        {value.length > 0 && (
          <PressableScale onPress={handleClear} style={styles.clear}>
            <Ionicons name="close-circle" size={18} color={colors.grayLight} />
          </PressableScale>
        )}
      </View>
      {chips.length > 0 && (
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          style={styles.chipsRow}
          contentContainerStyle={styles.chipsContent}
        >
          {chips.map((t, index) => (
            <Pressable
              key={`${t.type}-${t.start}-${index}`}
              onPress={() => handleRemove(t.start, t.end)}
              style={[
                styles.chip,
                {
                  backgroundColor: colors.surfaceTint,
                  borderColor: colors.separator,
                },
              ]}
            >
              <Text style={[styles.chipText, { color: colors.black }]}>
                {t.raw}
              </Text>
              <Ionicons name="close" size={12} color={colors.grayLight} />
            </Pressable>
          ))}
        </ScrollView>
      )}
      {focused && completions.length > 0 && (
        <View
          style={[
            styles.dropdown,
            {
              backgroundColor: colors.surface,
              borderColor: colors.separator,
            },
          ]}
        >
          <FlatList
            data={completions}
            keyboardShouldPersistTaps="handled"
            keyExtractor={(item, index) => `${item.value}-${index}`}
            renderItem={({ item }) => (
              <PressableScale
                onPress={() => handleSelect(item)}
                style={styles.completionItem}
              >
                <Text style={[styles.completionText, { color: colors.black }]}>
                  {item.label}
                </Text>
              </PressableScale>
            )}
            ItemSeparatorComponent={() => (
              <View
                style={[
                  styles.separator,
                  { backgroundColor: colors.separator },
                ]}
              />
            )}
          />
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    position: 'relative',
  },
  inputRow: {
    flexDirection: 'row',
    alignItems: 'center',
    height: 40,
    borderRadius: 12,
    borderWidth: 1,
    paddingHorizontal: 12,
  },
  input: {
    flex: 1,
    fontSize: 15,
    paddingVertical: 0,
  },
  clear: {
    padding: 4,
  },
  chipsRow: {
    marginTop: 6,
    maxHeight: 34,
  },
  chipsContent: {
    gap: 6,
    paddingRight: 8,
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 8,
    paddingVertical: 4,
    borderRadius: 12,
    borderWidth: 1,
  },
  chipText: {
    fontSize: 12,
  },
  dropdown: {
    position: 'absolute',
    top: 46,
    left: 0,
    right: 0,
    maxHeight: 200,
    borderRadius: 12,
    borderWidth: 1,
    zIndex: 100,
    elevation: 5,
    overflow: 'hidden',
  },
  completionItem: {
    paddingHorizontal: 12,
    paddingVertical: 10,
  },
  completionText: {
    fontSize: 14,
  },
  separator: {
    height: 1,
  },
});
