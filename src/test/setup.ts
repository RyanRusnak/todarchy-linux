// Vitest setup — runs once before all test files.
import '@testing-library/jest-dom/vitest';
import { vi, beforeEach } from 'vitest';

// jsdom doesn't implement layout APIs. Stub the ones the app reaches for so
// visible-row auto-scroll effects don't crash when they run in the test DOM.
if (typeof Element !== 'undefined') {
  Element.prototype.scrollBy = Element.prototype.scrollBy || function () {};
  Element.prototype.scrollIntoView = Element.prototype.scrollIntoView || function () {};
}

// Node 25 ships an experimental built-in `localStorage` that resolves before
// jsdom's polyfill and has no prototype methods (no `.clear()`, `.setItem()`,
// etc.). Install a tiny in-memory Storage-shaped replacement on both window
// and globalThis so the app code and test code see a real Storage.
{
  const store = new Map<string, string>();
  const storage = {
    get length() {
      return store.size;
    },
    clear(): void {
      store.clear();
    },
    getItem(key: string): string | null {
      return store.has(key) ? (store.get(key) ?? null) : null;
    },
    setItem(key: string, value: string): void {
      store.set(key, String(value));
    },
    removeItem(key: string): void {
      store.delete(key);
    },
    key(i: number): string | null {
      return Array.from(store.keys())[i] ?? null;
    },
  };
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: storage,
  });
  if (typeof window !== 'undefined') {
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: storage,
    });
  }
}

// Stub the Tauri `invoke` so components that touch the backend don't crash
// under jsdom. Individual tests can override these mocks with vi.mocked().
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'load_tasks') {
      return { version: 1, tasks: [], projects: [], contexts: [] };
    }
    if (cmd === 'save_tasks') return null;
    if (cmd === 'current_theme') throw new Error('no omarchy theme in test');
    return null;
  }),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}));

// Reset mock state between tests so counts and implementations don't leak.
beforeEach(async () => {
  const { invoke } = await import('@tauri-apps/api/core');
  (invoke as unknown as ReturnType<typeof vi.fn>).mockClear();
});
