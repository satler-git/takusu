// Parse a duration string into minutes.
//
// Accepts plain numbers (interpreted as minutes) and compound expressions
// with units h (hours), m (minutes), s (5-minute slots, matching the
// backend's parse_duration in takusu-util).
//
// Examples:
//   "90"     → 90
//   "90m"    → 90
//   "1h30m"  → 90
//   "2h"     → 120
//   "1h15m"  → 75
//   "30s"    → 150 (30 * 5-minute slots)
//
// Returns null if the string cannot be parsed.

export function parseDuration(input: string): number | null {
  const s = input.trim();
  if (s.length === 0) return null;

  // Plain number → minutes
  if (/^\d+$/.test(s)) {
    const n = parseInt(s, 10);
    return isNaN(n) ? null : n;
  }

  // Compound: <num><unit> pairs (e.g. 1h30m, 2h, 30s)
  const re = /(\d+)(h|m|s)/g;
  let total = 0;
  let matched = false;
  let lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s)) !== null) {
    matched = true;
    // Check for skipped characters between matches
    if (m.index > lastIndex) {
      return null;
    }
    const num = parseInt(m[1], 10);
    if (isNaN(num)) return null;
    switch (m[2]) {
      case 'h':
        total += num * 60;
        break;
      case 'm':
        total += num;
        break;
      case 's':
        total += num * 5;
        break;
    }
    lastIndex = re.lastIndex;
  }

  // Trailing characters after last match → invalid
  if (matched && lastIndex !== s.length) return null;

  return matched ? total : null;
}

/// Format minutes back into a human-readable duration string.
/// e.g. 90 → "1h30m", 60 → "1h", 45 → "45m"
export function formatDuration(minutes: number): string {
  if (minutes < 0) {
    throw new RangeError(
      `formatDuration: expected non-negative minutes, got ${minutes}`,
    );
  }
  if (minutes === 0) return '0m';
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h > 0 && m > 0) return `${h}h${m}m`;
  if (h > 0) return `${h}h`;
  return `${m}m`;
}
