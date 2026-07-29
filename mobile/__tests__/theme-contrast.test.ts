// WCAG contrast ratio tests for all theme color pairs.
//
// Color pairs are automatically extracted from src/**/*.tsx by
// scripts/extract-color-pairs.ts. This test reads the generated JSON and
// verifies that every foreground/background pair meets WCAG AA contrast
// requirements across all four themes (light, dark, catppuccin, aura-soft-dark).
//
// Palette backgrounds (taskCardColor, abandonabilityColorFor, habitColorFor)
// are expanded into all concrete colors for each theme.
//
// Manual overrides (false positives from AST extraction limitations) are in
// color-pairs-overrides.json.

import { hex as contrastHex } from 'wcag-contrast';

import {
  APP_THEMES,
  COLORS,
  DARK_COLORS,
  CATPPUCCIN_COLORS,
  AURA_SOFT_DARK_COLORS,
  ABANDON_STEPS,
  HABIT_PALETTE_SIZE,
  abandonabilityColorFor,
  habitColorFor,
  taskCardColor,
  type AppTheme,
  type ColorSet,
} from '@/src/theme';

// ── Types (must match scripts/extract-color-pairs.ts) ──

interface ColorPair {
  fg: string;
  bg: string;
  file: string;
  kind: 'same-style' | 'parent-child';
  source: 'text' | 'icon';
}

interface ExtractResult {
  pairs: ColorPair[];
  palettePairs: ColorPair[];
  generatedAt: string;
}

interface Overrides {
  excludes: {
    fg: string;
    bg: string;
    file: string;
    reason: string;
  }[];
  skipBgTokens: string[];
  skipFgTokens: string[];
  skipFgOnBgPairs: { fg: string; bg: string; reason: string }[];
}

// ── Load data ──

// eslint-disable-next-line @typescript-eslint/no-require-imports
const extracted: ExtractResult = require('./__generated__/color-pairs.json');
// eslint-disable-next-line @typescript-eslint/no-require-imports
const overrides: Overrides = require('./color-pairs-overrides.json');

const THEME_COLORS: Record<AppTheme, ColorSet> = {
  light: COLORS,
  dark: DARK_COLORS,
  catppuccin: CATPPUCCIN_COLORS,
  'aura-soft-dark': AURA_SOFT_DARK_COLORS,
};

// ── Thresholds ──

const AA_NORMAL = 4.5;
const AA_LARGE = 3.0;

// ── Helpers ──

function isExcluded(pair: ColorPair): boolean {
  return overrides.excludes.some(
    (e) => e.fg === pair.fg && e.bg === pair.bg && e.file === pair.file,
  );
}

function shouldSkip(pair: ColorPair): boolean {
  if (overrides.skipBgTokens.includes(pair.bg)) return true;
  if (overrides.skipFgTokens.includes(pair.fg)) return true;
  if (isExcluded(pair)) return true;
  if (
    overrides.skipFgOnBgPairs?.some((p) => p.fg === pair.fg && p.bg === pair.bg)
  )
    return true;
  return false;
}

/** Resolve a token name to a hex color for a given theme. */
function resolveToken(token: string, theme: AppTheme): string | null {
  const colors = THEME_COLORS[theme];
  const value = (colors as Record<string, string>)[token];
  return value ?? null;
}

/** Expand a palette marker into all concrete hex colors for a theme. */
function expandPalette(marker: string, theme: AppTheme): string[] {
  const colors: string[] = [];

  if (marker === 'taskCardColor' || marker === 'abandonabilityColorFor') {
    // All abandonability bands
    for (const a of ABANDON_STEPS) {
      colors.push(abandonabilityColorFor(a, theme));
    }
  }
  if (marker === 'taskCardColor' || marker === 'habitColorFor') {
    // All habit colors
    for (let i = 0; i < HABIT_PALETTE_SIZE; i++) {
      colors.push(habitColorFor(i, theme));
    }
  }
  // Direct palette function variants
  if (marker === 'abandonabilityColor') {
    for (const a of ABANDON_STEPS)
      colors.push(abandonabilityColorFor(a, theme));
  }
  // Theme-specific abandonability functions — these always return colors for
  // a fixed theme regardless of the current theme, so expand using the
  // function's target theme. When the test's current theme matches, the
  // colors are valid backgrounds; when it doesn't, skip (mismatched theme).
  const themeSpecificAbandon: Record<string, AppTheme> = {
    abandonabilityColorDark: 'dark',
    abandonabilityColorCatppuccin: 'catppuccin',
    abandonabilityColorAuraSoftDark: 'aura-soft-dark',
  };
  if (themeSpecificAbandon[marker]) {
    // Only expand when the function's target theme matches the current test
    // theme — otherwise the colors belong to a different theme and would be
    // invalid backgrounds for this theme's foreground colors.
    if (themeSpecificAbandon[marker] === theme) {
      for (const a of ABANDON_STEPS)
        colors.push(abandonabilityColorFor(a, theme));
    }
  }
  // taskCardColor also covers the case where abandonability < 0.25 (red band)
  // which is already included via abandonabilityColorFor above.

  // Deduplicate
  return [...new Set(colors)];
}

/** Compute contrast ratio between two hex colors. */
function contrastRatio(fg: string, bg: string): number {
  return contrastHex(fg, bg);
}

// ── Build test cases ──

interface TestCase {
  description: string;
  fg: string;
  bg: string;
  fgHex: string;
  bgHex: string;
  theme: AppTheme;
  source: string;
  threshold: number;
}

function buildTestCases(): TestCase[] {
  const cases: TestCase[] = [];

  for (const theme of APP_THEMES) {
    // Direct pairs
    for (const pair of extracted.pairs) {
      if (shouldSkip(pair)) continue;

      const fgHex = resolveToken(pair.fg, theme);
      const bgHex = resolveToken(pair.bg, theme);
      if (!fgHex || !bgHex) continue;

      cases.push({
        description: `${pair.fg} on ${pair.bg} (${theme})`,
        fg: pair.fg,
        bg: pair.bg,
        fgHex,
        bgHex,
        theme,
        source: pair.file,
        threshold: pair.source === 'icon' ? AA_LARGE : AA_NORMAL,
      });
    }

    // Palette pairs — expand palette marker into all concrete colors
    for (const pair of extracted.palettePairs) {
      if (shouldSkip(pair)) continue;

      const fgHex = resolveToken(pair.fg, theme);
      if (!fgHex) continue;

      const marker = pair.bg.replace('palette:', '');
      const bgColors = expandPalette(marker, theme);

      for (const bgHex of bgColors) {
        cases.push({
          description: `${pair.fg} on ${marker}→${bgHex} (${theme})`,
          fg: pair.fg,
          bg: bgHex,
          fgHex,
          bgHex,
          theme,
          source: pair.file,
          threshold: pair.source === 'icon' ? AA_LARGE : AA_NORMAL,
        });
      }
    }
  }

  return cases;
}

const testCases = buildTestCases();

// ── Tests ──

describe('WCAG contrast — extracted color pairs', () => {
  // Sanity check: we actually have pairs to test
  it('has extracted color pairs to test', () => {
    expect(extracted.pairs.length).toBeGreaterThan(0);
  });

  it('has palette pairs to test', () => {
    expect(extracted.palettePairs.length).toBeGreaterThan(0);
  });

  // Group by unique (fg, bg, theme) to avoid redundant checks.
  // When the same fg/bg is used both as text (4.5:1) and as an icon (3:1),
  // keep the stricter threshold so both usages are covered.
  const bestByCase = new Map<string, TestCase>();
  for (const c of testCases) {
    const key = `${c.fg}|${c.bg}|${c.theme}`;
    const prev = bestByCase.get(key);
    if (!prev || c.threshold > prev.threshold) bestByCase.set(key, c);
  }
  const uniqueCases = [...bestByCase.values()];

  for (const tc of uniqueCases) {
    it(`${tc.description} ≥ ${tc.threshold}:1 [${tc.source}]`, () => {
      const ratio = contrastRatio(tc.fgHex, tc.bgHex);
      expect(ratio).toBeGreaterThanOrEqual(tc.threshold);
    });
  }
});

// ── Palette-specific tests ──
// Verify that the theme's textOnCard semantic tokens meet AA across ALL
// palette colors, not just the ones currently used in source. This ensures
// that adding a new task card with any abandonability/habit combination
// will be readable (issue #1181).

describe('WCAG contrast — textOnCard tokens on all palette colors', () => {
  const TEXT_TOKENS = [
    'textOnCard',
    'textOnCardSecondary',
    'textOnCardDone',
  ] as const;

  for (const theme of APP_THEMES) {
    const colors = THEME_COLORS[theme];

    for (const token of TEXT_TOKENS) {
      const fgHex = (colors as Record<string, string>)[token];
      if (!fgHex) continue;

      // All abandonability colors
      for (const a of ABANDON_STEPS) {
        const bgHex = abandonabilityColorFor(a, theme);
        it(`${token} (${fgHex}) on abandonability ${a} (${bgHex}) [${theme}]`, () => {
          const ratio = contrastRatio(fgHex, bgHex);
          expect(ratio).toBeGreaterThanOrEqual(AA_NORMAL);
        });
      }

      // All habit colors
      for (let i = 0; i < HABIT_PALETTE_SIZE; i++) {
        const bgHex = habitColorFor(i, theme);
        it(`${token} (${fgHex}) on habit ${i} (${bgHex}) [${theme}]`, () => {
          const ratio = contrastRatio(fgHex, bgHex);
          expect(ratio).toBeGreaterThanOrEqual(AA_NORMAL);
        });
      }

      // taskCardColor covers both abandonability and habit colors
      // (already tested above, but verify the function returns valid colors)
      for (const a of [0.0, 0.5, 1.0]) {
        const bgHex = taskCardColor(a, undefined, undefined, theme);
        it(`${token} on taskCardColor(${a}, no habit) [${theme}]`, () => {
          const ratio = contrastRatio(fgHex, bgHex);
          expect(ratio).toBeGreaterThanOrEqual(AA_NORMAL);
        });
      }
    }
  }
});

// ── Skia canvas pairs ──
// Skia renders text and shapes via imperative APIs (ParagraphBuilder, Path)
// that the static extraction script cannot parse. These pairs are verified
// manually here.

describe('WCAG contrast — Skia canvas (DependencyGraph labels)', () => {
  // Label text: black (active nodes) or grayDark (done nodes) on white pill bg.
  // Pill border: separator on white (graph background) — decorative, just
  // needs to be distinguishable (>= 1.1:1, not a WCAG UI component).
  for (const theme of APP_THEMES) {
    const colors = THEME_COLORS[theme];

    it(`graph label black on white pill [${theme}]`, () => {
      const ratio = contrastRatio(colors.black, colors.white);
      expect(ratio).toBeGreaterThanOrEqual(AA_NORMAL);
    });

    it(`graph label grayDark on white pill [${theme}]`, () => {
      const ratio = contrastRatio(colors.grayDark, colors.white);
      expect(ratio).toBeGreaterThanOrEqual(AA_NORMAL);
    });

    it(`graph pill border visible on white [${theme}]`, () => {
      const ratio = contrastRatio(colors.separator, colors.white);
      expect(ratio).toBeGreaterThanOrEqual(1.1);
    });
  }
});
