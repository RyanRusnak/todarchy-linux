// Pure unit tests for the data helpers that the GUI, CLI, and waybar
// all rely on. These don't need DOM or Tauri mocks.

import { describe, it, expect } from 'vitest';
import {
  parseQuickAdd,
  fuzzyScore,
  formatDeferUntil,
  timeAgo,
  nid,
} from '../ui/data.jsx';

describe('parseQuickAdd', () => {
  it('returns the raw title when nothing special is present', () => {
    const p = parseQuickAdd('buy milk');
    expect(p.title).toBe('buy milk');
    expect(p.ctx).toBe('');
    expect(p.due).toBe('');
    expect(p.note).toBe('');
  });

  it('extracts @context and strips it from the title', () => {
    const p = parseQuickAdd('call mom @phone');
    expect(p.title).toBe('call mom');
    expect(p.ctx).toBe('@phone');
  });

  it('extracts !due and maps `week` to `this week`', () => {
    expect(parseQuickAdd('ship !today').due).toBe('today');
    expect(parseQuickAdd('ship !tomorrow').due).toBe('tomorrow');
    expect(parseQuickAdd('ship !week').due).toBe('this week');
  });

  it('extracts / note and preserves spaces inside the note', () => {
    const p = parseQuickAdd('call dentist /ask about crown');
    expect(p.title).toBe('call dentist');
    expect(p.note).toBe('ask about crown');
  });

  it('handles all three modifiers in one line', () => {
    const p = parseQuickAdd('prep review @work !today /bring laptop');
    expect(p.title).toBe('prep review');
    expect(p.ctx).toBe('@work');
    expect(p.due).toBe('today');
    expect(p.note).toBe('bring laptop');
  });
});

describe('fuzzyScore', () => {
  it('returns null for non-matches', () => {
    expect(fuzzyScore('xyz', 'abc')).toBeNull();
  });

  it('returns a positive score for an in-order match', () => {
    const score = fuzzyScore('abc', 'a-b-c');
    expect(score).not.toBeNull();
    expect(score).toBeGreaterThan(0);
  });

  it('prefers contiguous matches over scattered ones', () => {
    const tight = fuzzyScore('cmd', 'cmd palette');
    const loose = fuzzyScore('cmd', 'c... m... d');
    expect(tight).toBeGreaterThan(loose);
  });

  it('is case-insensitive', () => {
    expect(fuzzyScore('ABC', 'abc')).not.toBeNull();
  });
});

describe('nid', () => {
  it('produces unique ids for sequential calls', () => {
    const seen = new Set();
    for (let i = 0; i < 200; i++) seen.add(nid());
    expect(seen.size).toBe(200);
  });
});

describe('formatDeferUntil', () => {
  it('returns empty string for null/undefined', () => {
    expect(formatDeferUntil(null)).toBe('');
    expect(formatDeferUntil(undefined)).toBe('');
  });

  it('says "today" for a same-day timestamp', () => {
    const now = new Date();
    now.setHours(15, 30, 0, 0);
    expect(formatDeferUntil(now.getTime())).toMatch(/^today/);
  });

  it('says "tomorrow" for the next day', () => {
    const t = new Date();
    t.setDate(t.getDate() + 1);
    t.setHours(9, 0, 0, 0);
    expect(formatDeferUntil(t.getTime())).toMatch(/^tomorrow/);
  });
});

describe('timeAgo', () => {
  it('uses `s` for seconds, `m` for minutes, `h` for hours, `d` for days', () => {
    const now = Date.now();
    expect(timeAgo(now - 5_000)).toMatch(/^\d+s$/);
    expect(timeAgo(now - 5 * 60_000)).toMatch(/^\d+m$/);
    expect(timeAgo(now - 5 * 60 * 60_000)).toMatch(/^\d+h$/);
    expect(timeAgo(now - 5 * 24 * 60 * 60_000)).toMatch(/^\d+d$/);
  });
});
