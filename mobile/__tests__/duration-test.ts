import { parseDuration, formatDuration } from '@/src/utils/duration';

describe('parseDuration', () => {
  it('interprets a plain number as minutes', () => {
    expect(parseDuration('90')).toBe(90);
  });

  it('parses minute suffix', () => {
    expect(parseDuration('90m')).toBe(90);
  });

  it('parses compound hour and minute expressions', () => {
    expect(parseDuration('1h30m')).toBe(90);
    expect(parseDuration('2h')).toBe(120);
    expect(parseDuration('1h15m')).toBe(75);
  });

  it('parses 5-minute slot suffix', () => {
    expect(parseDuration('30s')).toBe(150);
  });

  it('rejects empty input', () => {
    expect(parseDuration('')).toBeNull();
  });

  it('rejects non-numeric input', () => {
    expect(parseDuration('abc')).toBeNull();
  });

  it('rejects malformed compound expressions', () => {
    expect(parseDuration('1h30')).toBeNull();
    expect(parseDuration('1h 30m')).toBeNull();
    expect(parseDuration('1x30m')).toBeNull();
  });
});

describe('formatDuration', () => {
  it('formats plain minutes', () => {
    expect(formatDuration(45)).toBe('45m');
  });

  it('formats whole hours', () => {
    expect(formatDuration(60)).toBe('1h');
    expect(formatDuration(120)).toBe('2h');
  });

  it('formats compound hours and minutes', () => {
    expect(formatDuration(90)).toBe('1h30m');
    expect(formatDuration(75)).toBe('1h15m');
  });

  it('formats zero as 0m', () => {
    expect(formatDuration(0)).toBe('0m');
  });

  it('throws on negative input', () => {
    expect(() => formatDuration(-1)).toThrow(
      'formatDuration: expected non-negative minutes, got -1',
    );
  });

  it('round-trips with parseDuration', () => {
    const cases = [0, 45, 60, 75, 90, 120, 150];
    for (const minutes of cases) {
      expect(parseDuration(formatDuration(minutes))).toBe(minutes);
    }
  });
});
