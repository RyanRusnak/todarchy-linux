// Pure unit tests for the data helpers that the GUI, CLI, and waybar
// all rely on. These don't need DOM or Tauri mocks.

import { describe, it, expect } from 'vitest';
import {
  parseQuickAdd,
  parseDeferText,
  exportMarkdown,
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

describe('parseDeferText', () => {
  const at9 = (ts) => { const d = new Date(ts); return d.getHours() === 9 && d.getMinutes() === 0; };

  it('returns null for empty or unrecognized input', () => {
    expect(parseDeferText('')).toBeNull();
    expect(parseDeferText('   ')).toBeNull();
    expect(parseDeferText('someday maybe')).toBeNull();
  });

  it('resolves today / tomorrow at 09:00', () => {
    const today = parseDeferText('today');
    expect(at9(today)).toBe(true);
    expect(new Date(today).toDateString()).toBe(new Date().toDateString());

    const tmrw = parseDeferText('tomorrow');
    const expected = new Date(); expected.setDate(expected.getDate() + 1);
    expect(new Date(tmrw).toDateString()).toBe(expected.toDateString());
    expect(parseDeferText('tmrw')).toBe(tmrw);
  });

  it('resolves +Nd / +Nw / +Nm offsets', () => {
    const d3 = new Date(parseDeferText('+3d'));
    const e3 = new Date(); e3.setDate(e3.getDate() + 3);
    expect(d3.toDateString()).toBe(e3.toDateString());

    const w1 = new Date(parseDeferText('+1w'));
    const ew = new Date(); ew.setDate(ew.getDate() + 7);
    expect(w1.toDateString()).toBe(ew.toDateString());

    const m1 = new Date(parseDeferText('+1m'));
    const em = new Date(); em.setMonth(em.getMonth() + 1);
    expect(m1.getMonth()).toBe(em.getMonth());
  });

  it('resolves weekday names to the next future occurrence at 09:00', () => {
    for (const wd of ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat']) {
      const ts = parseDeferText(wd);
      expect(ts).not.toBeNull();
      expect(at9(ts)).toBe(true);
      expect(ts).toBeGreaterThan(Date.now());
    }
  });

  it('maps weekend → saturday and next week → monday', () => {
    expect(new Date(parseDeferText('weekend')).getDay()).toBe(6);
    expect(new Date(parseDeferText('next week')).getDay()).toBe(1);
  });

  it('accepts ISO dates and rejects impossible ones', () => {
    const iso = new Date(parseDeferText('2026-07-01'));
    expect(iso.getFullYear()).toBe(2026);
    expect(iso.getMonth()).toBe(6);
    expect(iso.getDate()).toBe(1);
    expect(at9(iso.getTime())).toBe(true);
    expect(parseDeferText('2026-02-31')).toBeNull();
  });
});

describe('exportMarkdown', () => {
  const projects = [{ id: 'p_work', name: 'work' }];
  const tasks = [
    { id: '1', list: 'inbox', title: 'capture me', ctx: '@phone', due: 'today' },
    { id: '2', list: 'p_work', title: 'ship it', doneAt: Date.now(), note: 'line a\nline b' },
  ];

  it('groups by list with checkboxes, chips, and quoted notes', () => {
    const md = exportMarkdown(tasks, projects, 0);
    expect(md).toContain('## inbox');
    expect(md).toContain('- [ ] capture me @phone !today');
    expect(md).toContain('## work');
    expect(md).toContain('- [x] ship it');
    expect(md).toContain('  > line a');
    expect(md).toContain('  > line b');
  });

  it('renders `this week` as the !week token', () => {
    const md = exportMarkdown([{ id: '3', list: 'inbox', title: 't', due: 'this week' }], [], 0);
    expect(md).toContain('- [ ] t !week');
  });

  it('omits lists with no tasks', () => {
    const md = exportMarkdown([{ id: '1', list: 'inbox', title: 'x' }], projects, 0);
    expect(md).not.toContain('## work');
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
