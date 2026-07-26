// GraphView — task dependency DAG visualization
// Uses @shopify/react-native-skia + d3-force via DependencyGraph component
// Shows transitive dependencies of incomplete tasks (completed nodes are gray)
// Edit mode: long-press-drag node-to-node to add dependency,
//            long-press-drag on empty space to cut crossing edges (#382)
// Non-edit mode: pan/zoom enabled, node drag enabled (#383)

import { useCallback, useEffect, useState } from 'react';
import { StyleSheet, View } from 'react-native';
import { Button, IconButton } from 'react-native-paper';
import { useIsFocused } from 'expo-router';
import type { TakusuClient } from '@/src/api/client';
import { showError } from '@/src/api/errors';
import type { TaskRow, HabitRow, RedundantDependency } from '@/src/api/types';
import { parseDepends } from '@/src/api/types';
import { useTheme, habitColorFor } from '@/src/theme';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { haptic } from '@/src/components/haptics';
import {
  DependencyGraph,
  type GraphNode,
  type GraphEdge,
} from '@/src/components/graph/DependencyGraph';

// GraphView uses a larger font size and node radius than the embedded TaskDetailView graph (#379, #421)
const GRAPHVIEW_FONT_SIZE = 21;
const GRAPHVIEW_NODE_RADIUS = 36;

interface GraphViewProps {
  client: TakusuClient | null;
  onBack: () => void;
  onTaskPress?: (taskId: string) => void;
  refreshKey?: number | null;
}

export function GraphView({
  client,
  onBack,
  onTaskPress,
  refreshKey,
}: GraphViewProps) {
  const { theme, colors } = useTheme();
  const insets = useSafeAreaInsets();
  const [editMode, setEditMode] = useState(false);
  const [tasks, setTasks] = useState<TaskRow[]>([]);
  const [graphNodes, setGraphNodes] = useState<GraphNode[]>([]);
  const [graphEdges, setGraphEdges] = useState<GraphEdge[]>([]);

  const refresh = useCallback(async () => {
    if (!client) return;
    let allTasks: TaskRow[];
    let redundant: RedundantDependency[];
    let habitList: HabitRow[];
    try {
      [allTasks, redundant, habitList] = await Promise.all([
        client.listTasks(),
        client
          .analyzeTaskDependencies()
          .then((r) => r.redundant)
          .catch(() => []),
        client.listHabits().catch(() => [] as HabitRow[]),
      ]);
    } catch (e) {
      showError(e, 'タスク一覧の取得に失敗');
      return;
    }
    setTasks(allTasks);

    // Build a set of redundant edge keys for quick lookup (#387).
    // Rust API returns from=dependent, to=dependency, but GraphView edges
    // use source=dependency, target=dependent, so we flip the key direction.
    const redundantSet = new Set(redundant.map((r) => `${r.to}→${r.from}`));

    // habit_id (UUID) → display_id map for habit-based node coloring (#423).
    const habitDisplayIdMap = new Map(
      habitList.map((h) => [h.id, h.display_id]),
    );

    // Build transitive dependency graph from incomplete tasks
    const incomplete = allTasks.filter(
      (t) => t.status !== 'completed' && t.status !== 'skipped',
    );
    const taskMap = new Map(allTasks.map((t) => [t.id, t]));
    const visited = new Set<string>();
    const nodes: GraphNode[] = [];
    const edges: GraphEdge[] = [];

    function visit(id: string) {
      if (visited.has(id)) return;
      visited.add(id);
      const task = taskMap.get(id);
      if (!task) return;
      const isDone = task.status === 'completed' || task.status === 'skipped';
      const habitDisplayId = task.habit_id
        ? habitDisplayIdMap.get(task.habit_id)
        : undefined;
      const color = isDone
        ? colors.done
        : habitDisplayId !== undefined
          ? habitColorFor(habitDisplayId, theme)
          : colors.brand;
      nodes.push({
        id: task.id,
        label: task.title,
        color,
        x: 0,
        y: 0,
        vx: 0,
        vy: 0,
      });
      const deps = parseDepends(task.depends);
      for (const depId of deps) {
        const key = `${depId}→${task.id}`;
        edges.push({
          source: depId,
          target: task.id,
          redundant: redundantSet.has(key),
        });
        visit(depId);
      }
    }

    for (const t of incomplete) visit(t.id);

    setGraphNodes(nodes);
    setGraphEdges(edges);
  }, [client, colors, theme]);

  // Refresh when focused and the client is ready. This covers both the
  // initial mount and returning from TaskDetailView after editing edges (#386).
  // refreshKey lets HomeView trigger a reload after a schedule operation finishes.
  const isFocused = useIsFocused();
  useEffect(() => {
    if (client && isFocused) {
      refresh();
    }
  }, [client, isFocused, refresh, refreshKey]);

  function handleTapNode(taskId: string) {
    haptic.light();
    if (onTaskPress) {
      onTaskPress(taskId);
    } else {
      onBack();
    }
  }

  async function handleCutEdges(edges: { source: string; target: string }[]) {
    if (!client || edges.length === 0) return;
    haptic.medium();
    // Group by target task to batch updates
    const byTarget = new Map<string, string[]>();
    for (const { source, target } of edges) {
      const arr = byTarget.get(target) ?? [];
      arr.push(source);
      byTarget.set(target, arr);
    }
    const updates: Promise<unknown>[] = [];
    for (const [target, sources] of byTarget) {
      const targetTask = tasks.find((t) => t.id === target);
      if (targetTask) {
        const deps = parseDepends(targetTask.depends).filter(
          (d) => !sources.includes(d),
        );
        updates.push(client.updateTask(target, { depends: deps }));
      }
    }
    const results = await Promise.allSettled(updates);
    const rejected = results.filter(
      (r): r is PromiseRejectedResult => r.status === 'rejected',
    );
    if (rejected.length > 0) {
      showError(rejected[0].reason, '依存関係の削除に失敗');
    }
    if (results.some((r) => r.status === 'fulfilled')) {
      await refresh();
    }
  }

  async function handleAddEdge(source: string, target: string) {
    if (!client) return;
    haptic.medium();
    const targetTask = tasks.find((t) => t.id === target);
    if (targetTask) {
      const deps = parseDepends(targetTask.depends);
      if (!deps.includes(source)) {
        deps.push(source);
        try {
          await client.updateTask(target, { depends: deps });
          await refresh();
        } catch (e) {
          showError(e, '依存関係の追加に失敗');
        }
      }
    }
  }

  function toggleEditMode() {
    haptic.medium();
    setEditMode((v) => !v);
  }

  return (
    <View style={[styles.container, { backgroundColor: colors.white }]}>
      <View style={[styles.topBar, { paddingTop: 4 + insets.top }]}>
        <View style={styles.topBarLeft}>
          <IconButton
            icon="chevron-left"
            iconColor={colors.brand}
            size={28}
            onPress={() => {
              haptic.light();
              onBack();
            }}
          />
        </View>
        <View style={styles.topBarCenter}>
          <Button
            mode={editMode ? 'contained' : 'outlined'}
            onPress={toggleEditMode}
            textColor={editMode ? colors.white : colors.brand}
            buttonColor={editMode ? colors.brand : undefined}
            style={styles.editButton}
            labelStyle={styles.editButtonLabel}
            contentStyle={styles.editButtonContent}
          >
            {editMode ? '編集中' : '編集'}
          </Button>
        </View>
        <View style={styles.topBarRight} />
      </View>

      <DependencyGraph
        nodes={graphNodes}
        edges={graphEdges}
        editMode={editMode}
        fontSize={GRAPHVIEW_FONT_SIZE}
        nodeRadius={GRAPHVIEW_NODE_RADIUS}
        onTapNode={handleTapNode}
        onCutEdges={handleCutEdges}
        onAddEdge={handleAddEdge}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
  },
  topBar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 4,
    paddingBottom: 4,
  },
  topBarLeft: {
    width: 48,
    alignItems: 'flex-start',
  },
  topBarCenter: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  topBarRight: {
    width: 48,
  },
  editButton: {
    borderRadius: 4,
    minWidth: 96,
  },
  editButtonLabel: {
    fontSize: 18,
    fontWeight: '600',
  },
  editButtonContent: {
    paddingVertical: 6,
    paddingHorizontal: 8,
  },
});
