import { decodePresentation } from '../agentTypes';

// Canonical presentation wire fixtures, byte-identical to
// `crates/takusu-agent/src/fixtures/presentations.json` so Rust and
// TypeScript round-trip the same encoding.
const fixtures: unknown[] = require('../fixtures/presentations.json');

describe('presentation fixtures', () => {
  it('parses every canonical fixture and preserves its shape', () => {
    expect(fixtures.length).toBe(6);

    const work = decodePresentation(fixtures[0]);
    expect(work.type).toBe('work_transition');
    if (work.type === 'work_transition') {
      expect(work.kind).toBe('start');
      expect(work.title).toBe('レポート');
      expect(work.reference).toBe('#7');
    }

    const summary = decodePresentation(fixtures[1]);
    if (summary.type === 'schedule_summary') {
      expect(summary.next?.title).toBe('朝会');
      expect(summary.entries?.length).toBe(2);
    }

    const progress = decodePresentation(fixtures[2]);
    if (progress.type === 'progress_summary') {
      expect([progress.done, progress.in_progress, progress.scheduled]).toEqual(
        [3, 1, 5],
      );
    }

    const checkIn = decodePresentation(fixtures[3]);
    expect(checkIn.type).toBe('check_in');
    if (checkIn.type === 'check_in') {
      // A check-in always carries both 行動 and ズラす groups.
      expect(checkIn.act.actions.length).toBeGreaterThan(0);
      expect(checkIn.shift.actions.length).toBeGreaterThan(0);
      // Immediate actions always carry a server-issued capability.
      expect(checkIn.act.actions[0]?.capability).toBeDefined();
      expect(checkIn.shift.actions[0]?.capability).toBeDefined();
    }

    const clarification = decodePresentation(fixtures[4]);
    expect(clarification.type).toBe('clarification');

    const text = decodePresentation(fixtures[5]);
    if (text.type === 'text') {
      expect(text.text).toBe('こんにちは');
    }
  });

  it('degrades an unknown presentation tag to text (version tolerant)', () => {
    const p = decodePresentation({ type: 'future_kind', text: 'fallback' });
    expect(p.type).toBe('text');
    if (p.type === 'text') {
      expect(p.text).toBe('fallback');
    }
  });

  it('treats a missing type as text', () => {
    const p = decodePresentation({ text: 'hello' });
    expect(p.type).toBe('text');
  });

  it('degrades a known tag with a malformed payload to text', () => {
    // Mirror of the Rust deserializer: a known tag whose inner fields fail to
    // parse falls back to Text rather than yielding an object with undefined
    // fields.
    const p = decodePresentation({ type: 'work_transition', kind: 'start' });
    expect(p.type).toBe('text');
    if (p.type === 'text') {
      expect(p.text).toBe('');
    }

    const p2 = decodePresentation({
      type: 'progress_summary',
      done: 'not-a-number',
      in_progress: 1,
      scheduled: 2,
    });
    expect(p2.type).toBe('text');

    const p3 = decodePresentation({ type: 'check_in', question: 'ok?' });
    expect(p3.type).toBe('text');

    // A valid known tag still passes through.
    const ok = decodePresentation({
      type: 'work_transition',
      kind: 'start',
      reference: '#7',
      title: 'レポート',
    });
    expect(ok.type).toBe('work_transition');
  });
});
