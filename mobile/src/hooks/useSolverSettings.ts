import { useCallback, useMemo, useState } from 'react';

import { TakusuClient } from '@/src/api/client';
import { SettingsRow } from '@/src/api/types';
import { haptic } from '@/src/components/haptics';

import { showError } from '@/src/api/errors';
import {
  parseOptionalNonNegativeInt,
  SolverOption,
  SOLVER_OPTIONS,
} from '@/src/utils/settings';

export interface UseSolverSettingsResult {
  solverSettings: SettingsRow | null;
  solverValue: SolverOption;
  setSolverValue: (value: SolverOption) => void;
  timeBudgetInput: string;
  setTimeBudgetInput: (value: string) => void;
  seedInput: string;
  setSeedInput: (value: string) => void;
  warmStartValue: boolean;
  setWarmStartValue: (value: boolean) => void;
  loading: boolean;
  saving: boolean;
  menuVisible: boolean;
  setMenuVisible: (visible: boolean) => void;
  dirty: boolean;
  loadSolverSettings: () => Promise<void>;
  saveSolverSettings: () => Promise<void>;
}

export function useSolverSettings(
  client: TakusuClient | null,
): UseSolverSettingsResult {
  const [solverSettings, setSolverSettings] = useState<SettingsRow | null>(
    null,
  );
  const [solverValue, setSolverValue] = useState<SolverOption>('sa');
  const [timeBudgetInput, setTimeBudgetInput] = useState('');
  const [seedInput, setSeedInput] = useState('');
  const [warmStartValue, setWarmStartValue] = useState(false);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [menuVisible, setMenuVisible] = useState(false);

  const loadSolverSettings = useCallback(async () => {
    if (!client) return;
    setLoading(true);
    try {
      const s = await client.getSettings();
      setSolverSettings(s);
      const solver = SOLVER_OPTIONS.includes(s.solver as SolverOption)
        ? (s.solver as SolverOption)
        : 'sa';
      setSolverValue(solver);
      setTimeBudgetInput(
        s.time_budget_ms != null ? String(s.time_budget_ms) : '',
      );
      setSeedInput(s.seed != null ? String(s.seed) : '');
      setWarmStartValue(s.warm_start);
    } catch {
      setSolverSettings(null);
    } finally {
      setLoading(false);
    }
  }, [client]);

  const saveSolverSettings = useCallback(async () => {
    if (!client) return;
    if (!solverSettings) {
      void showError(
        '設定の読み込みに失敗しています。タブを開き直してください',
        'エラー',
      );
      return;
    }
    const timeBudget = parseOptionalNonNegativeInt(timeBudgetInput);
    const seed = parseOptionalNonNegativeInt(seedInput);
    if (timeBudget === null || seed === null) {
      void showError('数値は0以上の整数を入力してください');
      return;
    }
    setSaving(true);
    try {
      const s = await client.updateSettings({
        solver: solverValue,
        time_budget_ms: timeBudget,
        seed,
        warm_start: warmStartValue,
      });
      setSolverSettings(s);
      const nextSolver = SOLVER_OPTIONS.includes(s.solver as SolverOption)
        ? (s.solver as SolverOption)
        : 'sa';
      setSolverValue(nextSolver);
      setTimeBudgetInput(
        s.time_budget_ms != null ? String(s.time_budget_ms) : '',
      );
      setSeedInput(s.seed != null ? String(s.seed) : '');
      setWarmStartValue(s.warm_start);
      haptic.success();
    } catch (e) {
      void showError(e, 'エラー');
    } finally {
      setSaving(false);
    }
  }, [
    client,
    solverSettings,
    solverValue,
    timeBudgetInput,
    seedInput,
    warmStartValue,
  ]);

  const dirty = useMemo(() => {
    if (!solverSettings) return false;
    const storedSolver = SOLVER_OPTIONS.includes(
      solverSettings.solver as SolverOption,
    )
      ? (solverSettings.solver as SolverOption)
      : 'sa';
    const currentTimeBudget =
      parseOptionalNonNegativeInt(timeBudgetInput) ?? -1;
    const currentSeed = parseOptionalNonNegativeInt(seedInput) ?? -1;
    return (
      solverValue !== storedSolver ||
      currentTimeBudget !== (solverSettings.time_budget_ms ?? 0) ||
      currentSeed !== (solverSettings.seed ?? 0) ||
      warmStartValue !== solverSettings.warm_start
    );
  }, [solverSettings, solverValue, timeBudgetInput, seedInput, warmStartValue]);

  return {
    solverSettings,
    solverValue,
    setSolverValue,
    timeBudgetInput,
    setTimeBudgetInput,
    seedInput,
    setSeedInput,
    warmStartValue,
    setWarmStartValue,
    loading,
    saving,
    menuVisible,
    setMenuVisible,
    dirty,
    loadSolverSettings,
    saveSolverSettings,
  };
}
