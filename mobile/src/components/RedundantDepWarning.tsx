// RedundantDepWarning — banner + modal for composite (redundant) dependency
// edges (#355).
//
// Shows a warning banner when redundant dependency edges are detected. Tapping
// it opens a modal that lists each redundant edge with its witness path and
// lets the user choose which edge to delete:
//   - the redundant (direct) edge itself, or
//   - one of the edges on the witness path.
//
// The parent owns the actual deletion (calling updateTask / replaceHabitSteps)
// via `onResolve(fromId, toId)` — removing `toId` from `fromId`'s depends list.

import { useState } from 'react';
import {
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { BRAND_COLOR, useColors } from '@/src/theme';
import { haptic } from '@/src/components/haptics';
import type { RedundantDependency } from '@/src/api/types';

interface RedundantDepWarningProps {
  // Redundant edges relevant to the current view (already filtered by parent).
  edges: RedundantDependency[];
  // Resolve a single edge: remove `toId` from the depends list of `fromId`.
  onResolve: (fromId: string, toId: string) => Promise<void>;
  // Human-readable label for a node id (e.g. "#3 レポート" for tasks,
  // "2. 下書き" for steps).
  nodeLabel: (id: string, title: string) => string;
}

export function RedundantDepWarning({
  edges,
  onResolve,
  nodeLabel,
}: RedundantDepWarningProps) {
  const colors = useColors();
  const [visible, setVisible] = useState(false);
  const [resolving, setResolving] = useState(false);

  if (edges.length === 0) return null;

  async function handleResolve(fromId: string, toId: string) {
    if (resolving) return;
    haptic.medium();
    setResolving(true);
    try {
      await onResolve(fromId, toId);
      setVisible(false);
    } catch {
      // Error already shown by the parent's onResolve callback.
    } finally {
      setResolving(false);
    }
  }

  return (
    <>
      <Pressable
        style={[
          styles.banner,
          { backgroundColor: '#F5E5D5', borderColor: colors.red },
        ]}
        onPress={() => {
          haptic.light();
          setVisible(true);
        }}
      >
        <Ionicons name="git-branch" size={18} color={colors.red} />
        <Text style={[styles.bannerText, { color: colors.grayDark }]}>
          冗長な依存が{edges.length}件あります
        </Text>
        <Ionicons name="chevron-forward" size={16} color={colors.gray} />
      </Pressable>

      <Modal
        visible={visible}
        transparent
        animationType="fade"
        onRequestClose={() => setVisible(false)}
      >
        <Pressable
          style={styles.overlay}
          onPress={() => !resolving && setVisible(false)}
        >
          <Pressable
            style={[styles.sheet, { backgroundColor: colors.white }]}
            onPress={(e) => e.stopPropagation()}
          >
            <View style={styles.sheetHeader}>
              <Text style={[styles.sheetTitle, { color: colors.black }]}>
                冗長な依存
              </Text>
              <Pressable
                onPress={() => !resolving && setVisible(false)}
                hitSlop={8}
              >
                <Ionicons name="close" size={22} color={colors.gray} />
              </Pressable>
            </View>

            <ScrollView style={styles.sheetBody}>
              {edges.map((edge, i) => {
                const pathLabel = edge.via
                  .map((n) => nodeLabel(n.id, n.title))
                  .join(' → ');
                // Path edges: via[0]→via[1], via[1]→via[2], ...
                const pathEdges: { from: string; to: string }[] = [];
                for (let j = 0; j < edge.via.length - 1; j++) {
                  pathEdges.push({
                    from: edge.via[j]!.id,
                    to: edge.via[j + 1]!.id,
                  });
                }
                return (
                  <View
                    key={`${edge.from}-${edge.to}-${i}`}
                    style={[
                      styles.edgeCard,
                      {
                        backgroundColor: colors.surface,
                        borderColor: colors.separator,
                      },
                    ]}
                  >
                    <Text style={[styles.edgePath, { color: colors.black }]}>
                      {pathLabel}
                    </Text>
                    <Text style={[styles.edgeDesc, { color: colors.gray }]}>
                      「{nodeLabel(edge.from, edge.from_title)}」→ 「
                      {nodeLabel(edge.to, edge.to_title)}」 は冗長です
                    </Text>

                    <Pressable
                      style={[
                        styles.optionButton,
                        { borderColor: colors.red },
                        resolving && { opacity: 0.5 },
                      ]}
                      disabled={resolving}
                      onPress={() => handleResolve(edge.from, edge.to)}
                    >
                      <Ionicons
                        name="trash-outline"
                        size={16}
                        color={colors.red}
                      />
                      <Text style={[styles.optionText, { color: colors.red }]}>
                        冗長な辺を削除
                      </Text>
                    </Pressable>

                    <Text style={[styles.optionHint, { color: colors.gray }]}>
                      経路上の辺を削除:
                    </Text>
                    {pathEdges.map((pe, j) => (
                      <Pressable
                        key={`${pe.from}-${pe.to}-${j}`}
                        style={[
                          styles.optionButton,
                          { borderColor: BRAND_COLOR },
                          resolving && { opacity: 0.5 },
                        ]}
                        disabled={resolving}
                        onPress={() => handleResolve(pe.from, pe.to)}
                      >
                        <Ionicons
                          name="remove-circle-outline"
                          size={16}
                          color={BRAND_COLOR}
                        />
                        <Text
                          style={[styles.optionText, { color: BRAND_COLOR }]}
                        >
                          {nodeLabel(pe.from, edge.via[j]!.title)}
                          {' → '}
                          {nodeLabel(pe.to, edge.via[j + 1]!.title)}
                        </Text>
                      </Pressable>
                    ))}
                  </View>
                );
              })}
            </ScrollView>
          </Pressable>
        </Pressable>
      </Modal>
    </>
  );
}

const styles = StyleSheet.create({
  banner: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    marginTop: 4,
  },
  bannerText: {
    flex: 1,
    fontSize: 13,
    fontWeight: '500',
  },
  overlay: {
    flex: 1,
    justifyContent: 'center',
    alignItems: 'center',
    backgroundColor: 'rgba(0,0,0,0.5)',
    padding: 20,
  },
  sheet: {
    width: '100%',
    maxWidth: 380,
    maxHeight: '80%',
    borderRadius: 14,
    padding: 16,
  },
  sheetHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    marginBottom: 12,
  },
  sheetTitle: {
    fontSize: 17,
    fontWeight: '700',
  },
  sheetBody: {
    gap: 12,
  },
  edgeCard: {
    borderWidth: 1,
    borderRadius: 10,
    padding: 12,
    gap: 8,
  },
  edgePath: {
    fontSize: 14,
    fontWeight: '600',
  },
  edgeDesc: {
    fontSize: 12,
  },
  optionButton: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    borderWidth: 1,
    borderRadius: 8,
    paddingHorizontal: 10,
    paddingVertical: 8,
  },
  optionText: {
    fontSize: 13,
    fontWeight: '500',
  },
  optionHint: {
    fontSize: 11,
    marginTop: 4,
  },
});
