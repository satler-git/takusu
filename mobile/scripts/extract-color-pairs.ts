// Extract foreground/background color pairs from React Native TSX source.
//
// Scans mobile/src/**/*.tsx for `colors.X` usage (where `colors` comes from
// `useColors()` or `useTheme()`), and pairs foreground (`color`) with
// background (`backgroundColor`) based on:
//   1. Same style object (ObjectLiteralExpression)
//   2. Nearest JSX ancestor with a backgroundColor
//
// Also recognises helper functions that return palette colors
// (`taskCardColor`, `abandonabilityColorFor`, `habitColorFor`,
// `abandonabilityColor*`) as "palette" backgrounds, including when assigned
// to a local variable first (e.g. `const bg = taskCardColor(...)`).
//
// Output: JSON file with pairs. The test file expands palette markers into
// all concrete colors for each theme.
//
// Usage: npx tsx scripts/extract-color-pairs.ts

import {
  Project,
  SyntaxKind,
  type Node,
  type ObjectLiteralExpression,
  type JsxOpeningElement,
  type JsxSelfClosingElement,
  type PropertyAccessExpression,
  type CallExpression,
  type SourceFile,
} from 'ts-morph';
import { writeFileSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

// ── Types ──

interface ColorPair {
  fg: string;
  bg: string;
  file: string;
  line: number;
  kind: 'same-style' | 'parent-child';
  source: 'text' | 'icon'; // text=style color prop, icon=JSX color prop
}

interface ExtractResult {
  pairs: ColorPair[];
  palettePairs: ColorPair[]; // bg is a palette function marker
  generatedAt: string;
}

// ── Constants ──

const FG_PROPS = new Set(['color']);
const BG_PROPS = new Set(['backgroundColor', 'background']);
const PALETTE_FUNCS = new Set([
  'taskCardColor',
  'abandonabilityColorFor',
  'abandonabilityColor',
  'abandonabilityColorDark',
  'abandonabilityColorCatppuccin',
  'abandonabilityColorAuraSoftDark',
  'habitColorFor',
]);

// ── Variable map ──

type VarValue =
  | { kind: 'token'; name: string }
  | { kind: 'palette'; marker: string };

function buildVarMap(sf: SourceFile): Map<string, VarValue> {
  const map = new Map<string, VarValue>();
  for (const vd of sf.getDescendantsOfKind(SyntaxKind.VariableDeclaration)) {
    const name = vd.getName();
    const init = vd.getInitializer();
    if (!init) continue;
    const token = resolveColorToken(init);
    if (token) {
      map.set(name, { kind: 'token', name: token });
      continue;
    }
    const marker = resolvePaletteMarker(init);
    if (marker) {
      map.set(name, { kind: 'palette', marker });
    }
  }
  return map;
}

// ── Style name map: "styles.foo" → { fg, bg, bgPalette } ──
//
// Resolves `styles.X` references by scanning StyleSheet.create() calls
// (including those wrapped in makeStyles factory functions).

interface StyleEntry {
  fg: string[];
  bg: string[];
  bgPalette: string[];
}

/** Find all ObjectLiteralExpression arguments to StyleSheet.create() calls. */
function getStyleSheetObjects(sf: SourceFile): ObjectLiteralExpression[] {
  const results: ObjectLiteralExpression[] = [];
  for (const call of sf.getDescendantsOfKind(SyntaxKind.CallExpression)) {
    const expr = call.getExpression().getText();
    if (expr === 'StyleSheet.create' || expr === 'StyleSheet.flatten') {
      const arg = call.getArguments()[0];
      if (arg?.getKind() === SyntaxKind.ObjectLiteralExpression) {
        results.push(arg as ObjectLiteralExpression);
      }
    }
  }
  return results;
}

/** Build a map from "styles.X" to its color properties.
 *  Also handles "makeStyles(colors).X" → "styles.X" via the styles variable. */
function buildStyleMap(
  sf: SourceFile,
  varMap: Map<string, VarValue>,
): Map<string, StyleEntry> {
  const map = new Map<string, StyleEntry>();

  // Find the variable name that holds StyleSheet.create result.
  // Common patterns:
  //   const styles = StyleSheet.create({...})
  //   const styles = useMemo(() => makeStyles(colors), [colors])
  //   const styles = makeStyles(colors)
  const styleVarNames = new Set<string>();
  for (const vd of sf.getDescendantsOfKind(SyntaxKind.VariableDeclaration)) {
    const name = vd.getName();
    const init = vd.getInitializer();
    if (!init) continue;
    // Direct: const styles = StyleSheet.create({...})
    if (init.getKind() === SyntaxKind.CallExpression) {
      const call = init as CallExpression;
      const expr = call.getExpression().getText();
      if (expr === 'StyleSheet.create' || expr === 'StyleSheet.flatten') {
        styleVarNames.add(name);
      }
      // const styles = makeStyles(colors)
      const fnName = call.getExpression().getText();
      if (fnName === 'makeStyles' || fnName.includes('makeStyles')) {
        styleVarNames.add(name);
      }
      // const styles = useMemo(() => makeStyles(colors), ...)
      const innerCall = call.getArguments()[0];
      if (innerCall && innerCall.getKind() === SyntaxKind.ArrowFunction) {
        const arrowBody = (innerCall as any).getBody();
        if (arrowBody.getKind() === SyntaxKind.CallExpression) {
          const innerExpr = arrowBody.getExpression().getText();
          if (innerExpr === 'makeStyles' || innerExpr.includes('makeStyles')) {
            styleVarNames.add(name);
          }
        }
      }
    }
  }

  // For each StyleSheet.create object, extract style entries.
  for (const obj of getStyleSheetObjects(sf)) {
    for (const prop of obj.getProperties()) {
      if (prop.getKind() !== SyntaxKind.PropertyAssignment) continue;
      const pa = prop.asKindOrThrow(SyntaxKind.PropertyAssignment);
      const styleName = pa.getName();
      const value = pa.getInitializer();
      if (!value || value.getKind() !== SyntaxKind.ObjectLiteralExpression)
        continue;
      const entry: StyleEntry = { fg: [], bg: [], bgPalette: [] };

      // Recursively collect from this style object (handles nested objects)
      function walk(node: Node): void {
        if (node.getKind() === SyntaxKind.ObjectLiteralExpression) {
          const r = collectFromStyleObject(
            node as ObjectLiteralExpression,
            varMap,
          );
          entry.fg.push(...r.fg);
          entry.bg.push(...r.bg);
          entry.bgPalette.push(...r.bgPalette);
        }
        node.forEachChild(walk);
      }
      walk(value);

      // Register under each style variable name
      for (const sv of styleVarNames) {
        map.set(`${sv}.${styleName}`, entry);
      }
    }
  }

  return map;
}

/** Resolve a `styles.X` property access to its StyleEntry. */
function resolveStyleRef(
  node: Node,
  styleMap: Map<string, StyleEntry>,
): StyleEntry | null {
  if (node.getKind() === SyntaxKind.PropertyAccessExpression) {
    const key = node.getText();
    return styleMap.get(key) ?? null;
  }
  return null;
}

// ── Resolution helpers ──

function resolveColorToken(node: Node): string | null {
  if (node.getKind() === SyntaxKind.PropertyAccessExpression) {
    const pa = node as PropertyAccessExpression;
    const expr = pa.getExpression();
    if (
      expr.getKind() === SyntaxKind.Identifier &&
      expr.getText() === 'colors'
    ) {
      return pa.getName();
    }
  }
  return null;
}

function resolvePaletteMarker(node: Node): string | null {
  if (node.getKind() === SyntaxKind.CallExpression) {
    const call = node as CallExpression;
    const name = call.getExpression().getText();
    if (PALETTE_FUNCS.has(name)) return name;
  }
  return null;
}

function resolveAny(
  node: Node,
  varMap: Map<string, VarValue>,
): VarValue | null {
  const token = resolveColorToken(node);
  if (token) return { kind: 'token', name: token };
  const marker = resolvePaletteMarker(node);
  if (marker) return { kind: 'palette', marker };
  if (node.getKind() === SyntaxKind.Identifier) {
    return varMap.get(node.getText()) ?? null;
  }
  return null;
}

/** Like resolveAny but handles ternary/conditional expressions, returning
 *  all possible values from both branches. */
function resolveAll(node: Node, varMap: Map<string, VarValue>): VarValue[] {
  if (node.getKind() === SyntaxKind.ConditionalExpression) {
    const cond = node as unknown as {
      getWhenTrue(): Node;
      getWhenFalse(): Node;
    };
    return [
      ...resolveAll(cond.getWhenTrue(), varMap),
      ...resolveAll(cond.getWhenFalse(), varMap),
    ];
  }
  const r = resolveAny(node, varMap);
  return r ? [r] : [];
}

function isFgProp(name: string): boolean {
  return FG_PROPS.has(name);
}

function isBgProp(name: string): boolean {
  return BG_PROPS.has(name);
}

// ── Style extraction from ObjectLiteralExpression ──

interface StyleColors {
  fg: string[];
  bg: string[];
  bgPalette: string[];
}

function collectFromStyleObject(
  obj: ObjectLiteralExpression,
  varMap: Map<string, VarValue>,
): StyleColors {
  const fg: string[] = [];
  const bg: string[] = [];
  const bgPalette: string[] = [];

  for (const prop of obj.getProperties()) {
    if (prop.getKind() !== SyntaxKind.PropertyAssignment) continue;
    const pa = prop.asKindOrThrow(SyntaxKind.PropertyAssignment);
    const name = pa.getName();
    const value = pa.getInitializer();
    if (!value) continue;
    const resolved = resolveAll(value, varMap);
    for (const r of resolved) {
      if (isFgProp(name) && r.kind === 'token') {
        fg.push(r.name);
      } else if (isBgProp(name)) {
        if (r.kind === 'token') bg.push(r.name);
        else bgPalette.push(r.marker);
      }
    }
  }

  return { fg, bg, bgPalette };
}

/** Collect all fg/bg color tokens from a JSX element's style prop. */
function collectStyleColors(
  element: JsxOpeningElement | JsxSelfClosingElement,
  varMap: Map<string, VarValue>,
  styleMap: Map<string, StyleEntry>,
): StyleColors {
  const fg: string[] = [];
  const bg: string[] = [];
  const bgPalette: string[] = [];

  const styleAttr = element.getAttribute('style');
  if (!styleAttr) return { fg, bg, bgPalette };

  let styleNode: Node | undefined;
  const styleAttrPa = styleAttr.asKind(SyntaxKind.JsxAttribute);
  if (styleAttrPa) {
    const initializer = styleAttrPa.getInitializer();
    if (initializer) {
      const jsxExpr = initializer.asKind(SyntaxKind.JsxExpression);
      styleNode = jsxExpr ? jsxExpr.getExpression() : initializer;
    }
  }
  if (!styleNode) return { fg, bg, bgPalette };

  function walk(node: Node): void {
    if (node.getKind() === SyntaxKind.ObjectLiteralExpression) {
      const r = collectFromStyleObject(node as ObjectLiteralExpression, varMap);
      fg.push(...r.fg);
      bg.push(...r.bg);
      bgPalette.push(...r.bgPalette);
    }
    // Resolve styles.X references inside style arrays
    if (node.getKind() === SyntaxKind.PropertyAccessExpression) {
      const entry = resolveStyleRef(node, styleMap);
      if (entry) {
        fg.push(...entry.fg);
        bg.push(...entry.bg);
        bgPalette.push(...entry.bgPalette);
      }
    }
    node.forEachChild(walk);
  }

  walk(styleNode);

  // style={bgVar} — single variable holding a color or palette result
  if (styleNode.getKind() === SyntaxKind.Identifier) {
    const resolved = resolveAny(styleNode, varMap);
    if (resolved?.kind === 'token') bg.push(resolved.name);
    else if (resolved?.kind === 'palette') bgPalette.push(resolved.marker);
  }

  // style={styles.foo} — single style reference
  if (styleNode.getKind() === SyntaxKind.PropertyAccessExpression) {
    const entry = resolveStyleRef(styleNode, styleMap);
    if (entry) {
      fg.push(...entry.fg);
      bg.push(...entry.bg);
      bgPalette.push(...entry.bgPalette);
    }
  }

  return { fg, bg, bgPalette };
}

/** Collect `color` prop on icon-like components. */
function collectIconColors(
  element: JsxOpeningElement | JsxSelfClosingElement,
  varMap: Map<string, VarValue>,
): string[] {
  const result: string[] = [];
  const colorAttr = element.getAttribute('color');
  if (!colorAttr) return result;
  const colorAttrPa = colorAttr.asKind(SyntaxKind.JsxAttribute);
  if (!colorAttrPa) return result;
  const init = colorAttrPa.getInitializer();
  if (!init) return result;
  const jsxExpr = init.asKind(SyntaxKind.JsxExpression);
  if (jsxExpr) {
    const expr = jsxExpr.getExpression();
    if (expr) {
      const resolved = resolveAny(expr, varMap);
      if (resolved?.kind === 'token') result.push(resolved.name);
    }
  }
  return result;
}

// ── JSX ancestor walking ──

/** Get the JsxOpeningElement or JsxSelfClosingElement from a JSX node. */
function getJsxElement(
  node: Node,
): JsxOpeningElement | JsxSelfClosingElement | null {
  if (
    node.getKind() === SyntaxKind.JsxOpeningElement ||
    node.getKind() === SyntaxKind.JsxSelfClosingElement
  ) {
    return node as JsxOpeningElement | JsxSelfClosingElement;
  }
  if (node.getKind() === SyntaxKind.JsxElement) {
    const opening = node.getChildAtIndex(0);
    if (opening?.getKind() === SyntaxKind.JsxOpeningElement) {
      return opening as JsxOpeningElement;
    }
  }
  return null;
}

/** Walk up JSX parents to find the nearest ancestor with a background color. */
function findAncestorBg(
  node: Node,
  varMap: Map<string, VarValue>,
  styleMap: Map<string, StyleEntry>,
): { bg: string[]; bgPalette: string[] } | null {
  let cur: Node | undefined = node.getParent();
  while (cur) {
    // Stop at function/component boundary
    if (
      cur.getKind() === SyntaxKind.FunctionDeclaration ||
      cur.getKind() === SyntaxKind.ArrowFunction ||
      cur.getKind() === SyntaxKind.FunctionExpression
    ) {
      break;
    }

    const element = getJsxElement(cur);
    if (element) {
      const { bg, bgPalette } = collectStyleColors(element, varMap, styleMap);
      if (bg.length > 0 || bgPalette.length > 0) {
        return { bg, bgPalette };
      }
    }
    cur = cur.getParent();
  }
  return null;
}

// ── Main extraction ──

function extract(): ExtractResult {
  const project = new Project({
    tsConfigFilePath: resolve(__dirname, '..', 'tsconfig.json'),
  });

  const srcDir = resolve(__dirname, '..', 'src');
  const sourceFiles = project.getSourceFiles(`${srcDir}/**/*.tsx`);

  const pairs: ColorPair[] = [];
  const palettePairs: ColorPair[] = [];

  for (const sf of sourceFiles) {
    const file = sf.getFilePath().replace(srcDir + '/', '');
    const varMap = buildVarMap(sf);
    const styleMap = buildStyleMap(sf, varMap);

    // ── Pattern 1: same style object ──
    for (const obj of sf.getDescendantsOfKind(
      SyntaxKind.ObjectLiteralExpression,
    )) {
      const { fg, bg, bgPalette } = collectFromStyleObject(obj, varMap);
      const line = obj.getStartLineNumber();
      for (const f of fg) {
        for (const b of bg) {
          pairs.push({
            fg: f,
            bg: b,
            file,
            line,
            kind: 'same-style',
            source: 'text',
          });
        }
        for (const bp of bgPalette) {
          palettePairs.push({
            fg: f,
            bg: `palette:${bp}`,
            file,
            line,
            kind: 'same-style',
            source: 'text',
          });
        }
      }
    }

    // ── Pattern 2: nearest JSX ancestor with backgroundColor ──
    // For every JSX element that has a foreground color (in style or color prop),
    // walk up to find the nearest ancestor with a background color.
    const jsxElements = [
      ...sf.getDescendantsOfKind(SyntaxKind.JsxOpeningElement),
      ...sf.getDescendantsOfKind(SyntaxKind.JsxSelfClosingElement),
    ];

    for (const element of jsxElements) {
      const { fg: styleFg } = collectStyleColors(element, varMap, styleMap);
      const iconFg = collectIconColors(element, varMap);
      if (styleFg.length === 0 && iconFg.length === 0) continue;

      // Skip if this element itself has a background — that's handled by
      // Pattern 1 (same style object) or will be the ancestor for children.
      const { bg: ownBg, bgPalette: ownBgPalette } = collectStyleColors(
        element,
        varMap,
        styleMap,
      );
      if (ownBg.length > 0 || ownBgPalette.length > 0) continue;

      const ancestor = findAncestorBg(element, varMap, styleMap);
      if (!ancestor) continue;

      const line = element.getStartLineNumber();
      // Text colors (from style) → 4.5:1 threshold
      for (const fg of styleFg) {
        for (const bg of ancestor.bg) {
          pairs.push({
            fg,
            bg,
            file,
            line,
            kind: 'parent-child',
            source: 'text',
          });
        }
        for (const bp of ancestor.bgPalette) {
          palettePairs.push({
            fg,
            bg: `palette:${bp}`,
            file,
            line,
            kind: 'parent-child',
            source: 'text',
          });
        }
      }
      // Icon colors (from color prop) → 3:1 threshold
      for (const fg of iconFg) {
        for (const bg of ancestor.bg) {
          pairs.push({
            fg,
            bg,
            file,
            line,
            kind: 'parent-child',
            source: 'icon',
          });
        }
        for (const bp of ancestor.bgPalette) {
          palettePairs.push({
            fg,
            bg: `palette:${bp}`,
            file,
            line,
            kind: 'parent-child',
            source: 'icon',
          });
        }
      }
    }
  }

  // Deduplicate
  const dedup = (arr: ColorPair[]): ColorPair[] => {
    const seen = new Set<string>();
    return arr.filter((p) => {
      const key = `${p.fg}|${p.bg}|${p.file}|${p.line}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  };

  return {
    pairs: dedup(pairs).sort((a, b) =>
      a.file === b.file ? a.line - b.line : a.file.localeCompare(b.file),
    ),
    palettePairs: dedup(palettePairs).sort((a, b) =>
      a.file === b.file ? a.line - b.line : a.file.localeCompare(b.file),
    ),
    generatedAt: new Date().toISOString(),
  };
}

// ── Run ──

const checkMode = process.argv.includes('--check');
const result = extract();
const outPath = resolve(
  __dirname,
  '..',
  '__tests__',
  '__generated__',
  'color-pairs.json',
);

if (checkMode) {
  // CI mode: verify the committed file matches what extraction would produce.
  // Compare pairs and palettePairs only (generatedAt is non-deterministic).
  let existing: ExtractResult;
  try {
    existing = JSON.parse(readFileSync(outPath, 'utf8'));
  } catch {
    console.error(
      'color-pairs.json not found. Run `npm run extract-colors` first.',
    );
    process.exit(1);
  }
  const match =
    JSON.stringify(existing.pairs) === JSON.stringify(result.pairs) &&
    JSON.stringify(existing.palettePairs) ===
      JSON.stringify(result.palettePairs);
  if (!match) {
    console.error(
      'color-pairs.json is out of date. Run `npm run extract-colors` and commit the result.',
    );
    process.exit(1);
  }
  console.log(
    `color-pairs.json is up to date (${result.pairs.length} pairs, ${result.palettePairs.length} palette pairs).`,
  );
} else {
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, JSON.stringify(result, null, 2) + '\n');
  console.log(
    `Extracted ${result.pairs.length} direct pairs + ${result.palettePairs.length} palette pairs → ${outPath}`,
  );
}
