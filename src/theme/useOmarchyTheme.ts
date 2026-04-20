// useOmarchyTheme — subscribes to the backend `theme-changed` event and
// paints the tokens onto CSS custom properties on :root.
//
// The Rust side (src-tauri/src/theme.rs) emits this on startup and whenever
// the ~/.config/omarchy/current/theme directory is swapped out
// (i.e. when the user runs `omarchy-theme-set` or picks from the menu).
//
// Components should NEVER read the hex values from JS — always use the CSS
// variables (e.g. `color: var(--accent)`). The JS tokens are provided only
// so you can branch on `theme.mode` when you truly need dark-vs-light logic.

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { applyTokensToRoot, type ThemeTokens } from './cssVars';

export function useOmarchyTheme(): ThemeTokens | null {
  const [theme, setTheme] = useState<ThemeTokens | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    // Pull once for immediate paint, then subscribe.
    invoke<ThemeTokens>('current_theme')
      .then((t) => {
        applyTokensToRoot(t);
        setTheme(t);
      })
      .catch((e) => {
        // Not fatal — fallback tokens in index.css will carry us.
        console.warn('current_theme unavailable:', e);
      });

    listen<ThemeTokens>('theme-changed', (event) => {
      applyTokensToRoot(event.payload);
      setTheme(event.payload);
    }).then((fn) => {
      unlisten = fn;
    });

    return () => {
      unlisten?.();
    };
  }, []);

  return theme;
}
