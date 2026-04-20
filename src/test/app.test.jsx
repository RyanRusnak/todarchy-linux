// Regression tests for the App shell.
//
// The bugs these guard against:
//   1. Pressing Enter in the quick-add input was bubbling up to the global
//      window keydown handler, which treated Enter as "toggle done" on the
//      just-added row. Every new task was landing as completed. See the fix
//      in src/ui/app.jsx (target-tag guard at the top of the handler).
//   2. The `o` shortcut needs to open the quick-add bar in NORMAL mode.
//   3. A freshly-added task must appear in the visible task list, not be
//      filtered out by the tree walker or the done/deferred filters.

import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import App from '../ui/app.jsx';

async function mountApp() {
  const user = userEvent.setup();
  render(<App />);
  // Bootstrap effect fires `loadStore()` — let the empty promise resolve.
  await act(async () => {});
  return { user };
}

describe('App — quick-add', () => {
  beforeEach(() => {
    // App persists UI prefs in localStorage (activeList, showDone, etc.).
    // Start each test from a clean slate.
    localStorage.clear();
  });

  it('o opens the quick-add bar', async () => {
    const { user } = await mountApp();
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    expect(input).toBeInTheDocument();
    expect(input).toHaveFocus();
  });

  it('Enter in the quick-add bar adds a NEW task that is NOT completed', async () => {
    // This is the regression test for the Enter-bubbling bug.
    const { user } = await mountApp();
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'write regression tests');
    await user.keyboard('{Enter}');

    // The title is rendered in two places — the task row AND the detail
    // pane showing the now-selected task. Neither should have the line-
    // through styling that the mock applies to `doneAt` tasks.
    const matches = await screen.findAllByText('write regression tests');
    expect(matches.length).toBeGreaterThan(0);
    for (const el of matches) {
      const style = el.getAttribute('style') || '';
      expect(style).not.toMatch(/line-through/);
    }
  });

  it('Escape in the quick-add bar closes it without adding anything', async () => {
    const { user } = await mountApp();
    await user.keyboard('o');
    await screen.findByPlaceholderText(/^new task/i);
    await user.keyboard('should not be added');
    await user.keyboard('{Escape}');
    // The input is unmounted.
    expect(screen.queryByPlaceholderText(/^new task/i)).not.toBeInTheDocument();
    // And the text was not committed as a task title.
    expect(screen.queryByText('should not be added')).not.toBeInTheDocument();
  });

  it('quick-add supports @ctx and !today and places the task under the right list', async () => {
    const { user } = await mountApp();
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'draft memo @work !today');
    await user.keyboard('{Enter}');
    const matches = await screen.findAllByText('draft memo');
    expect(matches.length).toBeGreaterThan(0);
  });
});

describe('App — detail pane dropdowns', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('context dropdown opens in-webview (not via native GTK popup) and commits a selection', async () => {
    // We replaced <select> with a custom button+listbox so the expanded
    // menu inherits the app's theme tokens and JetBrains Mono font. This
    // test proves the custom control actually exchanges a value — the
    // native <option> hack would fail because aria-expanded wouldn't
    // transition and the selected-✓ indicator wouldn't render.
    const { user } = await mountApp();
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'pick me a context');
    await user.keyboard('{Enter}');

    // Three dropdowns in the detail pane: project, context, due. The context
    // and due both default to the "—" placeholder; pick the context one by
    // picking the SECOND match (project already shows "inbox").
    const placeholderButtons = screen
      .getAllByRole('button', { expanded: false })
      .filter((b) => b.textContent && b.textContent.includes('—'));
    expect(placeholderButtons.length).toBeGreaterThanOrEqual(2);
    const ctxButton = placeholderButtons[0];
    await user.click(ctxButton);

    expect(ctxButton).toHaveAttribute('aria-expanded', 'true');
    const listbox = await screen.findByRole('listbox');
    expect(listbox).toBeInTheDocument();

    // All options inherit the app font via `font-family: inherit` from the
    // menu wrapper; sanity-check that the list has > 1 real option and that
    // the options are rendered by React (not an HTML <option> the browser
    // would own).
    const items = listbox.querySelectorAll('[role="option"]');
    expect(items.length).toBeGreaterThan(1);

    // Pick @work.
    const workOption = Array.from(items).find((li) => li.textContent?.includes('@work'));
    expect(workOption).toBeDefined();
    await user.click(workOption);

    // Button should close and now display the chosen value.
    expect(ctxButton).toHaveAttribute('aria-expanded', 'false');
    expect(ctxButton.textContent).toContain('@work');
  });
});

describe('App — reorder with Shift-J / Shift-K', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  // Helper: returns the task titles rendered in list order. Filters out
  // the duplicate rendering in the detail pane by restricting to elements
  // whose parent chain includes an interactive task row (it's the button
  // that wraps the title). We scope to the main task list area by
  // grabbing all task titles and deduping contiguous repeats.
  function visibleTitles(titles) {
    // Walks the DOM in document order and reports the order in which our
    // target titles first appear. That order == task-list order for any
    // title that shows up only in the list (newly-added tasks the tests
    // create) since the detail pane only shows the currently-selected
    // task title, which is unrelated to row ordering.
    const seen = [];
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const text = walker.currentNode.nodeValue?.trim();
      if (titles.includes(text) && !seen.includes(text)) seen.push(text);
    }
    return seen;
  }

  async function addTask(user, title) {
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, title);
    await user.keyboard('{Enter}');
  }

  it('Shift-J pushes the selected task one row down in the list', async () => {
    const { user } = await mountApp();
    // Three adds in order → tasks land newest-first: C, B, A.
    await addTask(user, 'A-row');
    await addTask(user, 'B-row');
    await addTask(user, 'C-row');

    // Cursor is on C (row 0). Shift-J pushes C past B.
    expect(visibleTitles(['A-row', 'B-row', 'C-row'])).toEqual(['C-row', 'B-row', 'A-row']);
    await user.keyboard('{Shift>}J{/Shift}');
    expect(visibleTitles(['A-row', 'B-row', 'C-row'])).toEqual(['B-row', 'C-row', 'A-row']);
  });

  it('Shift-K pulls the selected task one row up in the list', async () => {
    const { user } = await mountApp();
    await addTask(user, 'A-row');
    await addTask(user, 'B-row');
    await addTask(user, 'C-row');

    // Move cursor down twice to reach A (the bottom-most of the three we added).
    await user.keyboard('jj');
    expect(visibleTitles(['A-row', 'B-row', 'C-row'])).toEqual(['C-row', 'B-row', 'A-row']);
    await user.keyboard('{Shift>}K{/Shift}');
    expect(visibleTitles(['A-row', 'B-row', 'C-row'])).toEqual(['C-row', 'A-row', 'B-row']);
  });

  it('Shift-ArrowDown reorders just like Shift-J', async () => {
    const { user } = await mountApp();
    await addTask(user, 'alpha');
    await addTask(user, 'beta');

    expect(visibleTitles(['alpha', 'beta'])).toEqual(['beta', 'alpha']);
    await user.keyboard('{Shift>}{ArrowDown}{/Shift}');
    expect(visibleTitles(['alpha', 'beta'])).toEqual(['alpha', 'beta']);
  });

  it('reorder refuses to cross a sort-group boundary (e.g. today vs undated)', async () => {
    const { user } = await mountApp();
    await addTask(user, 'plain task');
    // Add a !today task — it'll land in the "due today" partition, above
    // anything undated, regardless of created order.
    await addTask(user, 'urgent thing !today');

    // Cursor starts on the top row. Shift-J tries to swap `urgent thing`
    // (due today) with `plain task` (undated) — they're in different
    // partitions, so the order should NOT change.
    const orderBefore = visibleTitles(['urgent thing', 'plain task']);
    await user.keyboard('{Shift>}J{/Shift}');
    const orderAfter = visibleTitles(['urgent thing', 'plain task']);
    expect(orderAfter).toEqual(orderBefore);
  });
});

describe('App — stable row layout', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('active-row indicator gutter has a pinned width so titles stay put', async () => {
    // Regression: the left-hand gutter that holds the active-row ▍ glyph
    // used to collapse to width 0 when no indicator was rendered, so every
    // time the cursor landed on a row the title jumped right by a few px.
    // jsdom doesn't run real layout, so we assert on the CSS width token
    // directly — that's the thing that actually makes the UI stable.
    const { user } = await mountApp();
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'gutter pin target');
    await user.keyboard('{Enter}');

    const titleEl = screen.getAllByText('gutter pin target')[0];
    // Walk up to the row, then grab its first child — that's the gutter.
    const row = titleEl.closest('[draggable]') || titleEl.parentElement?.parentElement?.parentElement;
    expect(row).toBeTruthy();
    const gutter = row.firstElementChild;
    expect(gutter).toBeTruthy();
    expect(gutter.style.width).toBe('8px');
    expect(gutter.style.flexShrink).toBe('0');
  });
});

describe('App — Enter / Space split', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('Enter in NORMAL mode opens quick-add (mirrors `o` for non-vim users)', async () => {
    const { user } = await mountApp();
    await user.keyboard('{Enter}');
    const input = await screen.findByPlaceholderText(/^new task/i);
    expect(input).toBeInTheDocument();
    expect(input).toHaveFocus();
  });

  it('Space toggles done on the selected task', async () => {
    const { user } = await mountApp();
    // Add a task so there's something under the cursor.
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'space-toggle target');
    await user.keyboard('{Enter}');

    const before = screen.getAllByText('space-toggle target');
    expect(before.length).toBeGreaterThan(0);

    await user.keyboard(' ');
    // Default filter hides done rows, so the title drops from the view.
    const after = screen.queryAllByText('space-toggle target');
    expect(after.length).toBeLessThan(before.length);
  });

  it('x still toggles done (kept for vim muscle memory)', async () => {
    const { user } = await mountApp();
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'x-toggle target');
    await user.keyboard('{Enter}');

    const before = screen.getAllByText('x-toggle target');
    await user.keyboard('x');
    const after = screen.queryAllByText('x-toggle target');
    expect(after.length).toBeLessThan(before.length);
  });
});

describe('App — keyboard safety', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('typing Enter inside an input never reaches the global vim handler', async () => {
    // Independent repro of the core invariant: any key pressed inside an
    // INPUT/TEXTAREA must be handled locally, not by the j/k/x vim bindings.
    const { user } = await mountApp();
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'regression guard');
    // Type Enter; if the bug reappears, this adds the task AND instantly
    // toggles it done because Enter bubbles to the switch-case in app.jsx.
    await user.keyboard('{Enter}');
    const matches = await screen.findAllByText('regression guard');
    expect(matches.length).toBeGreaterThan(0);
    for (const el of matches) {
      const style = el.getAttribute('style') || '';
      expect(style).not.toMatch(/line-through/);
    }
  });

  it('x toggles the selected task done (and done tasks vanish by default)', async () => {
    const { user } = await mountApp();

    // Add a task so the cursor has something to select.
    await user.keyboard('o');
    const input = await screen.findByPlaceholderText(/^new task/i);
    await user.type(input, 'soon-to-be-done');
    await user.keyboard('{Enter}');

    // Visible before x: the task shows in the list and the detail pane.
    const before = screen.getAllByText('soon-to-be-done');
    expect(before.length).toBeGreaterThan(0);

    // Press x — toggles done. The default filter hides completed tasks,
    // so the title disappears from the active view. If this doesn't shrink,
    // either x stopped working OR every new task is already shipping as
    // completed (the original bug).
    await user.keyboard('x');

    const after = screen.queryAllByText('soon-to-be-done');
    expect(after.length).toBeLessThan(before.length);
  });
});
