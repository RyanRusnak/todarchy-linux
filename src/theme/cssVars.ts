// cssVars.ts — maps ThemeTokens (from Rust) → CSS custom properties.
//
// This is the single place that knows the token names. If you add a new
// semantic color, add it here AND in src-tauri/src/theme.rs.

export interface ThemeTokens {
  name: string;
  mode: 'dark' | 'light';
  bg: string;
  fg: string;
  bg_elev: string;
  bg_panel: string;
  border: string;
  fg_mute: string;
  fg_faint: string;
  accent: string;
  accent_2: string;
  success: string;
  warn: string;
  danger: string;
  ctx_home: string;
  ctx_work: string;
  ctx_errands: string;
  ctx_read: string;
}

// Primary token → CSS variable mapping emitted by the Rust watcher.
const MAP: Array<[keyof ThemeTokens, string]> = [
  ['bg', '--bg'],
  ['fg', '--fg'],
  ['bg_elev', '--bg-elev'],
  ['bg_panel', '--bg-panel'],
  ['border', '--border'],
  ['fg_mute', '--fg-mute'],
  ['fg_faint', '--fg-faint'],
  ['accent', '--accent'],
  ['accent_2', '--accent-2'],
  ['success', '--success'],
  ['warn', '--warn'],
  ['danger', '--danger'],
  ['ctx_home', '--ctx-home'],
  ['ctx_work', '--ctx-work'],
  ['ctx_errands', '--ctx-errands'],
  ['ctx_read', '--ctx-read'],
];

// The design also uses a few tokens the Rust side doesn't emit directly
// (legacy design-mock names + two accent shorthands). Derive them from the
// primary tokens so the UI looks right under any Omarchy theme.
function deriveExtras(t: ThemeTokens, root: HTMLElement) {
  root.style.setProperty('--bg-soft', t.bg_elev);
  root.style.setProperty('--panel', t.bg_panel);
  root.style.setProperty(
    '--border-hi',
    `color-mix(in oklab, ${t.border} 70%, ${t.fg_mute})`
  );
  root.style.setProperty('--fg-dim', t.fg_mute);
  root.style.setProperty('--cyan', t.ctx_home);
  root.style.setProperty('--orange', t.warn);
}

export function applyTokensToRoot(t: ThemeTokens) {
  const root = document.documentElement;
  for (const [key, cssVar] of MAP) {
    const v = t[key];
    if (typeof v === 'string' && v.length > 0) {
      root.style.setProperty(cssVar, v);
    }
  }
  deriveExtras(t, root);
  root.dataset.theme = t.name;
  root.dataset.themeMode = t.mode;
}
