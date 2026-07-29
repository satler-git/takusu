// Habit step helpers (#95).
//
// A StepDraft is the client-side working copy of a habit step. Existing
// steps carry their server `id` (preserved on save so generated tasks stay
// linked); new steps have no `id` and are assigned one by the server on
// `PUT /api/habits/:id/steps`. `tempId` is a stable client-only identifier
// used to reference steps in `depends_on` before the server assigns real
// ids — `saveHabitSteps` resolves tempIds to real ids in a two-phase PUT.

import type { TakusuClient } from '@/src/api/client';
import type { HabitStepInput, HabitStepRow } from '@/src/api/types';

export interface StepDraft {
  // Server-assigned id for existing steps; undefined for new steps.
  id?: string;
  // Stable client-only id used for depends_on references before save.
  tempId: string;
  position: number;
  title: string;
  description?: string;
  start_time: string;
  end_time: string;
  avg_minutes: number;
  sigma_minutes: number;
  parallelizable: boolean;
  allows_parallel: boolean;
  abandonability: number;
  fixed: boolean;
  // tempId references to other drafts in the same array.
  depends_on: string[];
}

let tempIdCounter = 0;
export function newTempId(): string {
  tempIdCounter += 1;
  return `t${tempIdCounter}`;
}

export function stepRowToDraft(row: HabitStepRow): StepDraft {
  let deps: string[] = [];
  try {
    const parsed = JSON.parse(row.depends_on);
    if (Array.isArray(parsed)) deps = parsed as string[];
  } catch {
    deps = [];
  }
  return {
    id: row.id,
    tempId: row.id, // existing steps use their real id as tempId too
    position: row.position,
    title: row.title,
    description: row.description ?? undefined,
    start_time: row.start_time,
    end_time: row.end_time,
    avg_minutes: row.avg_minutes,
    sigma_minutes: row.sigma_minutes,
    parallelizable: row.parallelizable,
    allows_parallel: row.allows_parallel,
    abandonability: row.abandonability,
    fixed: row.fixed,
    depends_on: deps,
  };
}

export function newStepDraft(position: number): StepDraft {
  return {
    tempId: newTempId(),
    position,
    title: '',
    start_time: '09:00',
    end_time: '10:00',
    avg_minutes: 60,
    sigma_minutes: 0,
    parallelizable: false,
    allows_parallel: false,
    abandonability: 0.5,
    fixed: false,
    depends_on: [],
  };
}

function draftToInput(
  d: StepDraft,
  idMap: Map<string, string>,
): HabitStepInput {
  const resolvedDeps = d.depends_on
    .map((t) => idMap.get(t))
    .filter((v): v is string => Boolean(v));
  return {
    id: idMap.get(d.tempId) ?? d.id,
    position: d.position,
    title: d.title,
    description: d.description,
    start_time: d.start_time,
    end_time: d.end_time,
    avg_minutes: d.avg_minutes,
    sigma_minutes: d.sigma_minutes > 0 ? d.sigma_minutes : undefined,
    parallelizable: d.parallelizable,
    allows_parallel: d.allows_parallel,
    abandonability: d.abandonability,
    fixed: d.fixed,
    depends_on: resolvedDeps,
  };
}

// Detect a cycle in the drafts' depends_on graph (tempId references).
// Returns true if a cycle is found.
export function hasCycle(drafts: StepDraft[]): boolean {
  const idxByTemp = new Map<string, number>();
  drafts.forEach((d, i) => idxByTemp.set(d.tempId, i));
  const adj: number[][] = drafts.map((d) =>
    d.depends_on
      .map((t) => idxByTemp.get(t))
      .filter((v): v is number => v !== undefined),
  );
  // DFS-based cycle detection
  const WHITE = 0;
  const GRAY = 1;
  const BLACK = 2;
  const color = new Array(drafts.length).fill(WHITE);
  function dfs(u: number): boolean {
    color[u] = GRAY;
    for (const v of adj[u] ?? []) {
      if (color[v] === GRAY) return true;
      if (color[v] === WHITE && dfs(v)) return true;
    }
    color[u] = BLACK;
    return false;
  }
  for (let i = 0; i < drafts.length; i++) {
    if (color[i] === WHITE && dfs(i)) return true;
  }
  return false;
}

// Save step drafts via PUT /api/habits/:id/steps. New steps (no id) get
// server-assigned ids in the response; if any new step has depends_on (or
// an existing step depends on a new step), a second PUT remaps tempId
// references to the real ids. Returns the final step rows.
export async function saveHabitSteps(
  client: TakusuClient,
  habitId: string,
  drafts: StepDraft[],
): Promise<HabitStepRow[]> {
  if (drafts.length === 0) {
    return client.replaceHabitSteps(habitId, []);
  }
  // Phase 1: send with depends_on resolved only for existing ids. New
  // steps send depends_on: [] (the server rejects references to unknown
  // ids, and new steps have no id yet).
  const idMapPhase1 = new Map<string, string>();
  drafts.forEach((d) => {
    if (d.id) idMapPhase1.set(d.tempId, d.id);
  });
  const phase1 = drafts.map((d) => draftToInput(d, idMapPhase1));
  const result = await client.replaceHabitSteps(habitId, phase1);

  // Build tempId → real id map from the response. The server returns
  // steps in the same order as the input array (this is an API contract
  // of PUT /api/habits/:id/steps — the worker and local-lib handlers
  // both preserve input order). If that ever changes, this mapping
  // would need to match by position/title instead.
  const idMap = new Map<string, string>();
  result.forEach((row, i) => {
    idMap.set(drafts[i]!.tempId, row.id);
  });

  // If any draft referenced a tempId that wasn't in idMapPhase1 (i.e. a
  // new step), we need a second pass to wire those deps now that every
  // step has a real id.
  const needsPhase2 = drafts.some(
    (d) =>
      d.depends_on.length > 0 && d.depends_on.some((t) => !idMapPhase1.has(t)),
  );
  if (!needsPhase2) return result;

  const phase2 = drafts.map((d) => draftToInput(d, idMap));
  return client.replaceHabitSteps(habitId, phase2);
}
