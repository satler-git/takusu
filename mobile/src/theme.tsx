// Brand color and theme constants

import { createContext, useContext, useMemo, type ReactNode } from 'react';

export const BRAND_COLOR = '#7261A3';
export const BRAND_COLOR_LIGHT = '#9B8BC4';
export const BRAND_COLOR_DARK = '#5A4A85';

export const APP_THEMES = [
  'light',
  'dark',
  'catppuccin',
  'aura-soft-dark',
] as const;
export type AppTheme = (typeof APP_THEMES)[number];

// abandonability → background color mapping for task cards
// 0.0 = must do (red — important), 1.0 = can abandon (calm brand-tinted)
// Palette is tuned to the purple brand color; the lowest band is clearly red
// to signal importance (Issue #188).
export function abandonabilityColor(abandonability: number): string {
  if (abandonability >= 0.75) return '#EDE6F5'; // light brand purple — calm
  if (abandonability >= 0.5) return '#F0EDE8'; // warm neutral
  if (abandonability >= 0.25) return '#F5E5D5'; // warm amber — caution
  return '#F2C8C8'; // clear red/pink — must do
}

// Dark-theme variant (retuned purple palette).
export function abandonabilityColorDark(abandonability: number): string {
  if (abandonability >= 0.75) return '#3D3048'; // muted brand purple
  if (abandonability >= 0.5) return '#3A3A3E'; // neutral dark
  if (abandonability >= 0.25) return '#423A32'; // warm dark
  return '#4A2E2E'; // dark red — must do
}

// Catppuccin Macchiato variant (issue #388).
export function abandonabilityColorCatppuccin(abandonability: number): string {
  if (abandonability >= 0.75) return '#494D64'; // surface1 — calm
  if (abandonability >= 0.5) return '#363A4F'; // surface0 — neutral
  if (abandonability >= 0.25) return '#6B645D'; // warm dark
  return '#6A495A'; // dark red — must do
}

// Aura Soft Dark variant (issue #729).
// Palette: https://github.com/daltonmenezes/aura-theme
export function abandonabilityColorAuraSoftDark(
  abandonability: number,
): string {
  if (abandonability >= 0.75) return '#3d375e'; // calm purple
  if (abandonability >= 0.5) return '#29263c'; // neutral dark
  if (abandonability >= 0.25) return '#463a32'; // warm dark amber
  return '#462e2e'; // dark red — must do
}

// Theme-aware helper: picks the palette for the active theme.
export function abandonabilityColorFor(
  abandonability: number,
  theme: AppTheme,
): string {
  switch (theme) {
    case 'dark':
      return abandonabilityColorDark(abandonability);
    case 'catppuccin':
      return abandonabilityColorCatppuccin(abandonability);
    case 'aura-soft-dark':
      return abandonabilityColorAuraSoftDark(abandonability);
    default:
      return abandonabilityColor(abandonability);
  }
}

// ── Habit-based color palette (issue #309) ──
// 8 distinct pastel tints for light mode, dimmer tints for dark mode,
// and Catppuccin Macchiato tinted backgrounds.
// A task with a habit_id uses the habit's display_id to pick a color, so
// all tasks from the same habit share a recognizable tint. Low-abandon
// (must-do) tasks keep the red abandonability color regardless of habit.
const HABIT_COLORS_LIGHT: readonly string[] = [
  '#D6E4F5', // soft blue
  '#D6F0EA', // soft mint
  '#D6F2D6', // soft green
  '#F2F0D6', // soft yellow
  '#F2E0D6', // soft orange
  '#F2D6D6', // soft red
  '#F2D6E6', // soft pink
  '#E6D6F2', // soft lavender
];

const HABIT_COLORS_DARK: readonly string[] = [
  '#2E3746', // muted blue
  '#2E4640', // muted mint
  '#2E4632', // muted green
  '#46402E', // muted yellow
  '#463A32', // muted orange
  '#46322E', // muted red
  '#462E3A', // muted pink
  '#3A2E46', // muted lavender
];

const HABIT_COLORS_CATPPUCCIN: readonly string[] = [
  '#48567B', // macchiato blue
  '#48646C', // macchiato teal
  '#52665A', // macchiato green
  '#6B645D', // macchiato yellow
  '#6D5552', // macchiato peach
  '#6A495A', // macchiato red
  '#6D5C76', // macchiato pink
  '#5D517C', // macchiato mauve
];

const HABIT_COLORS_AURA_SOFT_DARK: readonly string[] = [
  '#3d375e', // muted purple
  '#2e464b', // muted teal
  '#3b4632', // muted green
  '#46402e', // muted amber
  '#463a32', // muted orange
  '#462e3a', // muted pink
  '#2e3a46', // muted blue
  '#462e2e', // muted red
];

export const HABIT_PALETTE_SIZE = 8;

// Pick a habit color from the palette by habit display_id.
export function habitColorFor(habitDisplayId: number, theme: AppTheme): string {
  const palette =
    theme === 'dark'
      ? HABIT_COLORS_DARK
      : theme === 'catppuccin'
        ? HABIT_COLORS_CATPPUCCIN
        : theme === 'aura-soft-dark'
          ? HABIT_COLORS_AURA_SOFT_DARK
          : HABIT_COLORS_LIGHT;
  const idx =
    ((habitDisplayId % HABIT_PALETTE_SIZE) + HABIT_PALETTE_SIZE) %
    HABIT_PALETTE_SIZE;
  return palette[idx]!;
}

// Combined color rule for a task card (issue #309):
//  - abandonability < 0.25 → red (must-do, keep abandonability color)
//  - has habit_id → habit palette color (by habit display_id)
//  - otherwise → abandonability color
export function taskCardColor(
  abandonability: number,
  habitId: string | null | undefined,
  habitDisplayId: number | undefined,
  theme: AppTheme,
): string {
  if (abandonability < 0.25) {
    return abandonabilityColorFor(abandonability, theme);
  }
  if (habitId && habitDisplayId !== undefined) {
    return habitColorFor(habitDisplayId, theme);
  }
  return abandonabilityColorFor(abandonability, theme);
}

export const ABANDON_STEPS = [0.0, 0.25, 0.5, 0.75, 1.0] as const;

// Number of filled pips (0..4) for a 5-segment abandonability meter.
// 0.0 → 0 filled, 0.25 → 1, 0.5 → 2, 0.75 → 3, 1.0 → 4.
export function filledPips(abandonability: number): number {
  return ABANDON_STEPS.slice(0, 4).filter((s) => s <= abandonability - 1e-9)
    .length;
}

// Light theme colors (default, backward-compatible export)
// Neutral scale is tinted slightly toward the brand purple to keep the
// whole UI coherent while keeping white/black semantics intact.
export const COLORS = {
  brand: BRAND_COLOR,
  brandLight: BRAND_COLOR_LIGHT,
  brandDark: BRAND_COLOR_DARK,
  white: '#FBFAFD',
  black: '#1C1824',
  gray: '#6C6578',
  grayLight: '#9B95AA',
  grayDark: '#4A4358',
  separator: '#E5DDEE',
  done: '#A29CA8',
  red: '#A04848',
  green: '#3D6E51',
  surface: '#FBFAFD',
  surfaceTint: '#F3EEF7', // brand-tinted surface (dep items, etc.)
  onBrand: '#FBFAFD',
  shadow: '#1C1824',
  scrim: '#1C1824',
  destructive: '#B33A3A',
  destructiveBg: 'rgba(179,58,58,0.15)',
  error: '#C62828',
  errorContainer: '#4D2A32',
  success: '#2E7D32',
  warning: '#A65B00',
  warningIcon: '#B07A00',
  warningBorder: '#E0B040',
  warningBg: '#FFF6E6',
  overlay: 'rgba(0,0,0,0.4)',
  pressed: 'rgba(0,0,0,0.05)',
  brandPressed: 'rgba(114,97,163,0.1)',
  cardOutline: 'rgba(0,0,0,0.08)',
  surfaceTranslucent: 'rgba(255,255,255,0.95)',
  redundantEdge: '#e85d04',
  disabled: '#999999',
  // Task card text on palette backgrounds (issue #1181)
  textOnCard: '#1C1824',
  textOnCardSecondary: '#4A4358',
  textOnCardDone: '#585265',
} as const;

// Dark theme colors (retuned purple palette).
export const DARK_COLORS = {
  brand: BRAND_COLOR,
  brandLight: BRAND_COLOR_LIGHT,
  brandDark: BRAND_COLOR_DARK,
  white: '#15131C', // dark background
  black: '#F0ECF5', // light text
  gray: '#B5B0C0',
  grayLight: '#9B95AA',
  grayDark: '#B5B0C0',
  separator: '#363049',
  done: '#908BA8',
  red: '#D67A7A',
  green: '#6AA67E',
  surface: '#1E1B27', // elevated surface (buttons, cards) — lighter than bg
  surfaceTint: '#272236', // brand-tinted dark surface
  onBrand: '#F0ECF5',
  shadow: '#0D0B12',
  scrim: '#0D0B12',
  destructive: '#D67A7A',
  destructiveBg: 'rgba(214,122,122,0.15)',
  error: '#D67A7A',
  errorContainer: '#4D2A32',
  success: '#6AA67E',
  warning: '#F5E5D5',
  warningIcon: '#F5E5D5',
  warningBorder: '#6B645D',
  warningBg: '#423A32',
  overlay: 'rgba(0,0,0,0.5)',
  pressed: 'rgba(255,255,255,0.05)',
  brandPressed: 'rgba(155,139,196,0.15)',
  cardOutline: 'rgba(255,255,255,0.10)',
  surfaceTranslucent: 'rgba(30,27,39,0.95)',
  redundantEdge: '#e85d04',
  disabled: '#A9A3B4',
  // Task card text on palette backgrounds (issue #1181)
  textOnCard: '#F0ECF5',
  textOnCardSecondary: '#B5B0C0',
  textOnCardDone: '#B0ABBC',
} as const;

// Catppuccin Macchiato theme colors (issue #388).
// Official palette: https://github.com/catppuccin/palette/blob/main/palette.json
export const CATPPUCCIN_COLORS = {
  brand: BRAND_COLOR,
  brandLight: BRAND_COLOR_LIGHT,
  brandDark: BRAND_COLOR_DARK,
  white: '#24273A', // base background
  black: '#CAD3F5', // text
  gray: '#A5ACC8',
  grayLight: '#B8BDD4',
  grayDark: '#A5ACC8',
  separator: '#494D64',
  done: '#8B92B0',
  red: '#ED8796',
  green: '#A6DA95',
  surface: '#363A4F', // elevated surface
  surfaceTint: '#494D64',
  onBrand: '#CAD3F5',
  shadow: '#181A28',
  scrim: '#181A28',
  destructive: '#ED8796',
  destructiveBg: 'rgba(237,135,150,0.15)',
  error: '#ED8796',
  errorContainer: '#6A495A',
  success: '#A6DA95',
  warning: '#F5A97F',
  warningIcon: '#F5A97F',
  warningBorder: '#EED49F',
  warningBg: '#6B645D',
  overlay: 'rgba(24,25,38,0.5)',
  pressed: 'rgba(255,255,255,0.05)',
  brandPressed: 'rgba(155,139,196,0.15)',
  cardOutline: 'rgba(255,255,255,0.10)',
  surfaceTranslucent: 'rgba(54,58,79,0.95)',
  redundantEdge: '#e85d04',
  disabled: '#8087A2',
  // Task card text on palette backgrounds (issue #1181).
  // Catppuccin palette colors span a narrow luminance range, so all text
  // levels must be very light. Strikethrough provides done-text distinction.
  textOnCard: '#DDE5F5',
  textOnCardSecondary: '#E0E5F2',
  textOnCardDone: '#E0E5F2',
} as const;

// Aura Soft Dark theme colors (issue #729).
// Palette: https://github.com/daltonmenezes/aura-theme
export const AURA_SOFT_DARK_COLORS = {
  brand: '#8464c6',
  brandLight: '#a48bd6',
  brandDark: '#6a509f',
  white: '#141414', // app background (accent11)
  black: '#bdbdbd', // foreground (accent7)
  gray: '#b4b4b4',
  grayLight: '#9B95AA',
  grayDark: '#b4b4b4',
  separator: '#2e2b38',
  done: '#9B95AA',
  red: '#d96868',
  green: '#5FD5AA',
  surface: '#21202e', // elevated surface (accent12)
  surfaceTint: '#3d375e', // brand-tinted surface (accent20)
  onBrand: '#bdbdbd',
  shadow: '#0A0A0A',
  scrim: '#0A0A0A',
  destructive: '#d96868',
  destructiveBg: 'rgba(217,104,104,0.15)',
  error: '#d96868',
  errorContainer: '#462e2e',
  success: '#54c59f',
  warning: '#ffca85',
  warningIcon: '#ffca85',
  warningBorder: '#ffe9aa',
  warningBg: '#463a32',
  overlay: 'rgba(20,20,30,0.5)',
  pressed: 'rgba(255,255,255,0.05)',
  brandPressed: 'rgba(132,100,198,0.15)',
  cardOutline: 'rgba(255,255,255,0.10)',
  surfaceTranslucent: 'rgba(33,32,46,0.95)',
  redundantEdge: '#e85d04',
  disabled: '#b4b4b4',
  // Task card text on palette backgrounds (issue #1181)
  textOnCard: '#bdbdbd',
  textOnCardSecondary: '#b4b4b4',
  textOnCardDone: '#b0b0b0',
} as const;

export type ColorSet = {
  brand: string;
  brandLight: string;
  brandDark: string;
  white: string;
  black: string;
  gray: string;
  grayLight: string;
  grayDark: string;
  separator: string;
  done: string;
  red: string;
  green: string;
  surface: string;
  surfaceTint: string;
  onBrand: string;
  shadow: string;
  scrim: string;
  destructive: string;
  destructiveBg: string;
  error: string;
  errorContainer: string;
  success: string;
  warning: string;
  warningIcon: string;
  warningBorder: string;
  warningBg: string;
  overlay: string;
  pressed: string;
  brandPressed: string;
  cardOutline: string;
  surfaceTranslucent: string;
  redundantEdge: string;
  disabled: string;
  // ── Semantic role tokens for task card text on palette backgrounds ──
  // These are tuned so that text meets WCAG AA (4.5:1) on ALL abandonability
  // and habit palette colors for the respective theme (issue #1181).
  textOnCard: string;
  textOnCardSecondary: string;
  textOnCardDone: string;
};

function colorsForTheme(theme: AppTheme): ColorSet {
  switch (theme) {
    case 'dark':
      return DARK_COLORS;
    case 'catppuccin':
      return CATPPUCCIN_COLORS;
    case 'aura-soft-dark':
      return AURA_SOFT_DARK_COLORS;
    default:
      return COLORS;
  }
}

function themeFromProps(props: { theme?: AppTheme; dark?: boolean }): AppTheme {
  if (props.theme) return props.theme;
  if (props.dark === true) return 'dark';
  return 'light';
}

// ── Theme Context ──

interface ThemeContextValue {
  theme: AppTheme;
  dark: boolean;
  colors: ColorSet;
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: 'light',
  dark: false,
  colors: COLORS,
});

export function ThemeProvider({
  theme,
  dark,
  children,
}: {
  theme?: AppTheme;
  dark?: boolean;
  children: ReactNode;
}) {
  const activeTheme = themeFromProps({ theme, dark });
  const colors = colorsForTheme(activeTheme);
  const value = useMemo(
    () => ({ theme: activeTheme, dark: activeTheme !== 'light', colors }),
    [activeTheme, colors],
  );
  return (
    <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  return useContext(ThemeContext);
}

export function useColors(): ColorSet {
  return useContext(ThemeContext).colors;
}
