// theme.rs — Omarchy theme discovery, parsing, and live-watching.
//
// Omarchy stores the active theme under ~/.config/omarchy/current/.
// Current layout (`omarchy-theme-set` atomically swaps via rm -rf + mv):
//   ~/.config/omarchy/current/theme          directory with the active theme
//   ~/.config/omarchy/current/theme.name     text file naming the theme
//
// Each theme directory contains:
//   alacritty.toml      ← our PRIMARY source (bg, fg, 16 ANSI colors)
//   neovim.lua          ← secondary source for semantic tokens
//   waybar.css          ← tertiary fallback (css variables at :root)
//   hyprland.conf       ← border accents
//   btop.theme, ...
//
// This module:
//   1. Resolves the active theme path (dir or symlink — supports both)
//   2. Parses alacritty.toml → ThemeTokens
//   3. Watches ~/.config/omarchy/current/ for swaps via `notify` (inotify)
//   4. On change, re-reads + emits `theme-changed` to the webview
//
// On the frontend, `useOmarchyTheme()` listens for `theme-changed` and writes
// the tokens to CSS custom properties on `:root`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

/// Tokens derived from the active Omarchy theme. These map directly to the
/// CSS custom properties the UI uses — see `src/theme/cssVars.ts` on the
/// frontend for the mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeTokens {
    pub name: String,
    pub mode: ThemeMode,     // light / dark — inferred from bg luma
    pub bg: String,          // #rrggbb
    pub fg: String,
    pub bg_elev: String,     // derived: bg + 3% lightness
    pub bg_panel: String,    // derived: bg + 5%
    pub border: String,      // derived: bg + 8%
    pub fg_mute: String,     // mix(fg, bg, 40%)
    pub fg_faint: String,    // mix(fg, bg, 65%)
    pub accent: String,      // normal.blue
    pub accent_2: String,    // normal.magenta
    pub success: String,     // normal.green
    pub warn: String,        // normal.yellow
    pub danger: String,      // normal.red
    pub ctx_home: String,    // normal.cyan
    pub ctx_work: String,    // normal.yellow
    pub ctx_errands: String, // normal.magenta
    pub ctx_read: String,    // bright.blue
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
}

// ---------- Resolution ----------

/// Returns `~/.config/omarchy/current/theme`. May be a directory or a symlink.
fn current_theme_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no HOME"))?;
    Ok(home.join(".config/omarchy/current/theme"))
}

/// Returns `~/.config/omarchy/current/` — the dir that we watch for swaps.
fn current_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no HOME"))?;
    Ok(home.join(".config/omarchy/current"))
}

/// Resolves the active theme directory whether `current/theme` is a real dir
/// or a symlink to one. `omarchy-theme-set` currently uses a real dir (it
/// `rm -rf`s + `mv`s an atomically-built `next-theme/` into place), but we
/// keep symlink support since earlier Omarchy versions shipped that way.
fn resolve_theme_dir() -> Result<PathBuf> {
    let path = current_theme_path()?;
    let meta = std::fs::symlink_metadata(&path)
        .with_context(|| format!("stat {}", path.display()))?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&path)?;
        let resolved = if target.is_absolute() {
            target
        } else {
            path.parent().map(|p| p.join(&target)).unwrap_or(target)
        };
        Ok(resolved.canonicalize().unwrap_or(resolved))
    } else {
        Ok(path)
    }
}

/// Read the configured theme name from `theme.name` (a plain-text file that
/// `omarchy-theme-set` writes). Falls back to the dir name if missing.
fn read_theme_name(theme_dir: &PathBuf) -> String {
    let name_file = current_dir()
        .ok()
        .map(|d| d.join("theme.name"));
    if let Some(p) = name_file {
        if let Ok(s) = std::fs::read_to_string(&p) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    theme_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".into())
}

// ---------- Parsing ----------

#[derive(Debug, Deserialize)]
struct AlacrittyFile {
    colors: AlacrittyColors,
}

#[derive(Debug, Deserialize)]
struct AlacrittyColors {
    primary: AlacrittyPrimary,
    normal: AnsiBlock,
    bright: AnsiBlock,
}

#[derive(Debug, Deserialize)]
struct AlacrittyPrimary {
    background: String,
    foreground: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnsiBlock {
    // Alacritty gives us all 8 colors; we only read a subset on the GUI side
    // right now, but we keep the full shape so future tokens (e.g. a second
    // accent row) can land without a schema change.
    black: String,
    red: String,
    green: String,
    yellow: String,
    blue: String,
    magenta: String,
    cyan: String,
    white: String,
}

/// Read + parse alacritty.toml for a given theme dir.
fn parse_alacritty(theme_dir: &PathBuf) -> Result<AlacrittyFile> {
    let path = theme_dir.join("alacritty.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let parsed: AlacrittyFile = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(parsed)
}

/// Read the active theme from disk and build ThemeTokens.
pub fn read_current() -> Result<ThemeTokens> {
    let dir = resolve_theme_dir()?;
    let name = read_theme_name(&dir);
    let alac = parse_alacritty(&dir)?;
    let tokens = build_tokens(name, alac);
    Ok(tokens)
}

fn build_tokens(name: String, a: AlacrittyFile) -> ThemeTokens {
    let bg = normalize_hex(&a.colors.primary.background);
    let fg = normalize_hex(&a.colors.primary.foreground);
    let mode = if luma(&bg) < 0.5 { ThemeMode::Dark } else { ThemeMode::Light };

    // Derived neutrals — OKLCH-lightness-nudged variants of bg/fg.
    // See cssVars.ts for the matching frontend fallbacks.
    let bg_elev  = lift(&bg, 0.03);
    let bg_panel = lift(&bg, 0.05);
    let border   = lift(&bg, 0.08);
    let fg_mute  = mix(&fg, &bg, 0.40);
    let fg_faint = mix(&fg, &bg, 0.65);

    ThemeTokens {
        name,
        mode,
        bg,
        fg,
        bg_elev,
        bg_panel,
        border,
        fg_mute,
        fg_faint,
        accent:       normalize_hex(&a.colors.normal.blue),
        accent_2:     normalize_hex(&a.colors.normal.magenta),
        success:      normalize_hex(&a.colors.normal.green),
        warn:         normalize_hex(&a.colors.normal.yellow),
        danger:       normalize_hex(&a.colors.normal.red),
        ctx_home:     normalize_hex(&a.colors.normal.cyan),
        ctx_work:     normalize_hex(&a.colors.normal.yellow),
        ctx_errands:  normalize_hex(&a.colors.normal.magenta),
        ctx_read:     normalize_hex(&a.colors.bright.blue),
    }
}

// ---------- Watching ----------

/// Spawn the inotify watcher. Re-parses and emits `theme-changed` on change.
///
/// Watches `~/.config/omarchy/current/`. `omarchy-theme-set` swaps the
/// active theme by `rm -rf`'ing `current/theme` and `mv`ing a freshly-built
/// `next-theme/` in, which surfaces as create/remove events on the parent.
/// Non-recursive is enough: we only need to see the swap, not every
/// individual file rewrite inside the theme.
pub async fn spawn_watcher(app: AppHandle) -> Result<()> {
    // Emit the current theme immediately on startup.
    match read_current() {
        Ok(t) => {
            let _ = app.emit("theme-changed", &t);
        }
        Err(e) => tracing::warn!("initial theme read failed: {e}"),
    }

    let watch_dir = current_dir()?;

    // notify crate → channel
    let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(32);
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.blocking_send(res);
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )?;
    watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
    tracing::info!("watching omarchy current dir: {}", watch_dir.display());

    // Fire on writes to `theme.name`. `omarchy-theme-set` writes that file as
    // the LAST step of the swap (after rm/mkdir/cp/mv), so by the time we see
    // it we're guaranteed `current/theme/alacritty.toml` already holds the
    // new palette. A time-based debouncer was flakey: `cp`+`sed`-templates
    // can take longer than any fixed quiet window, so we'd read the old
    // theme while the swap was still in flight and silently drop the real
    // burst. Keying off the guaranteed-last write removes the race entirely.
    while let Some(ev) = rx.recv().await {
        let e = match ev {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!("watcher error: {err}");
                continue;
            }
        };

        // Verbose dump so we can diagnose "app lags one theme behind"-type
        // reports. Set RUST_LOG=info when launching todarchy to see this.
        tracing::info!(
            "notify event kind={:?} paths={:?}",
            e.kind,
            e.paths
        );

        let touches_theme_name = e.paths.iter().any(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "theme.name")
                .unwrap_or(false)
        });
        if !touches_theme_name {
            tracing::debug!("skipping — no theme.name in paths");
            continue;
        }

        // Tiny settle so the preceding `mv` of theme/ has flushed to fs cache.
        tokio::time::sleep(Duration::from_millis(50)).await;

        match read_current() {
            Ok(tokens) => {
                tracing::info!(
                    "theme changed → name={} bg={} accent={}",
                    tokens.name, tokens.bg, tokens.accent
                );
                let _ = app.emit("theme-changed", &tokens);
            }
            Err(err) => tracing::warn!("theme reload failed: {err}"),
        }
    }

    Ok(())
}

// ---------- Color utilities ----------

fn normalize_hex(s: &str) -> String {
    // Accepts: "0xrrggbb", "#rrggbb", "rrggbb"
    let trimmed = s.trim().trim_start_matches("0x").trim_start_matches('#');
    if trimmed.len() == 6 {
        format!("#{}", trimmed.to_ascii_lowercase())
    } else {
        // leave as-is; frontend will fall back to defaults
        s.trim().to_string()
    }
}

fn rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&h[0..2], 16).ok()?,
        u8::from_str_radix(&h[2..4], 16).ok()?,
        u8::from_str_radix(&h[4..6], 16).ok()?,
    ))
}

fn luma(hex: &str) -> f32 {
    match rgb(hex) {
        Some((r, g, b)) => {
            let r = r as f32 / 255.0;
            let g = g as f32 / 255.0;
            let b = b as f32 / 255.0;
            0.2126 * r + 0.7152 * g + 0.0722 * b
        }
        None => 0.0,
    }
}

/// Lighten/darken a bg color toward the opposite end of the tonal range.
/// amount in 0..1. On dark themes this brightens, on light themes it darkens.
fn lift(bg: &str, amount: f32) -> String {
    let (r, g, b) = match rgb(bg) {
        Some(v) => v,
        None => return bg.to_string(),
    };
    let l = luma(bg);
    let factor = if l < 0.5 { 1.0 + amount * 4.0 } else { 1.0 - amount * 2.0 };
    let clamp = |c: u8| -> u8 {
        let v = (c as f32 * factor).round().clamp(0.0, 255.0);
        v as u8
    };
    format!("#{:02x}{:02x}{:02x}", clamp(r), clamp(g), clamp(b))
}

/// Linearly mix two sRGB colors. t=0 → a, t=1 → b.
fn mix(a: &str, b: &str, t: f32) -> String {
    let ca = rgb(a);
    let cb = rgb(b);
    match (ca, cb) {
        (Some((ar, ag, ab)), Some((br, bg, bb))) => {
            let t = t.clamp(0.0, 1.0);
            let blend = |x: u8, y: u8| -> u8 {
                (x as f32 * (1.0 - t) + y as f32 * t).round() as u8
            };
            format!(
                "#{:02x}{:02x}{:02x}",
                blend(ar, br), blend(ag, bg), blend(ab, bb)
            )
        }
        _ => a.to_string(),
    }
}
