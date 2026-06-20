# todarchy-linux

Keyboard-first, terminal-inspired task manager for **Omarchy** (Arch + Hyprland).
Adopts your active Omarchy theme automatically — pick a theme from the
Omarchy menu (or run `omarchy-theme-set "Tokyo Night"`) and the app
re-colors live.

This is the Linux build; a mobile and web version are planned as separate
repos. The binary is called `todarchy`, the CLI is `tod`, and the Waybar
helper is `todarchy-waybar`.

<!-- TODO: add a real screenshot — `docs/screenshot.png` once you've taken one. -->
<!-- The canonical design lives at `design-mocks/Beautiful todo list.html` — open it in a browser to preview the UI without launching the app. -->


## Features

- Vim-style keyboard control (`j/k/x/o/gg/G/...`)
- Command palette (`Ctrl-K` or `:`)
- Inbox + projects + contexts + due dates + deferral
- `todo` / `next` / `all` view modes (cycle the chip in the list header)
- Natural-language defer (`tomorrow`, `+3d`, `+1w`, `fri`, `weekend`, ISO dates)
- Tree-nested tasks, drag-to-nest, or Tab / Shift-Tab
- Export to JSON / Markdown and import JSON (command palette)
- Live theme adoption from Omarchy — no config needed
- CLI companion (`tod add "buy milk @errands !today"`)
- Waybar module showing tasks due today
- Desktop notifications when deferred tasks come back
- Local-only; end-to-end encrypted sync is planned for v0.2

## Install (from source)

Requires `rustup` + `npm` + `webkit2gtk-4.1` + `libayatana-appindicator`
(all already on a fresh Omarchy install).

```bash
git clone https://github.com/ryanrusnak/todarchy-linux.git
cd todarchy-linux

npm install
npx tauri build --no-bundle                  # GUI binary
cargo build --release -p todarchy-cli -p todarchy-waybar

# Install binaries to ~/.local/bin
install -Dm755 target/release/todarchy        ~/.local/bin/todarchy
install -Dm755 target/release/tod             ~/.local/bin/tod
install -Dm755 target/release/todarchy-waybar ~/.local/bin/todarchy-waybar

# Optional: desktop entry so Walker / your app launcher finds it
install -Dm644 packaging/omarchy/todarchy.desktop \
  ~/.local/share/applications/todarchy.desktop
install -Dm644 src-tauri/icons/128x128.png \
  ~/.local/share/icons/hicolor/128x128/apps/todarchy.png
```

Launch with `todarchy`, or bind it in Hyprland — see below.

## Install (AUR PKGBUILD)

See [`packaging/omarchy/PKGBUILD`](packaging/omarchy/PKGBUILD). Drop it in a
clean build dir and run `makepkg -si`.

## Hyprland keybind

Add to `~/.config/hypr/hyprland.conf` (or drop the snippet in
`packaging/omarchy/hyprland.snippet.conf` into your conf folder):

```conf
bind = SUPER, T, exec, todarchy
```

## Waybar module

Add to your waybar config:

```jsonc
"custom/todarchy": {
  "exec": "todarchy-waybar",
  "interval": 30,
  "return-type": "json",
  "on-click": "todarchy"
}
```

And reference `custom/todarchy` in your `modules-right` (or wherever).

## CLI cheatsheet

```bash
tod add "fix bug @work !today"      # quick-add with context + due
tod add "buy milk @errands"          # no due date → lands in inbox view
tod list                             # today view (overdue + due today + inbox)
tod list --all                       # everything including done + deferred
tod done abc12345                    # prefix-match on the 8-char id
tod defer abc12345 tomorrow
tod defer abc12345 +3d
tod defer abc12345 mon
tod defer abc12345 2026-06-01
```

The CLI shares `~/.local/share/todarchy/tasks.json` with the GUI via file
locking, so anything you add from a shell shows up in the GUI on reload (and
vice-versa).

## Keyboard (GUI)

| key              | what                                         |
|------------------|----------------------------------------------|
| `j` / `k`        | move cursor                                   |
| `⇧J` / `⇧K`      | reorder: push task down / up within its group |
| `⇧↑` / `⇧↓`      | reorder (alternate binding)                   |
| `gg` / `G`       | top / bottom                                  |
| `h` / `l`        | prev / next list                              |
| `0` / `1`..`5`   | inbox / project 1-5                           |
| `o` / `a` / `↵`  | quick-add a new task                          |
| `x` / `␣` space  | toggle done                                   |
| `e`              | edit title                                    |
| `d`              | defer picker (type `tomorrow` / `+3d` / `fri`) |
| `Del` / `⌫`      | delete                                        |
| `u`              | undo                                          |
| `/`              | search in list                                |
| `:` / `Ctrl-K`   | command palette                                |
| `i`              | toggle detail pane                             |
| `Tab` / `S-Tab`  | indent / outdent (nest under sibling above)   |
| `z`              | collapse/expand children                       |
| `fd` / `fs`      | toggle show-done / show-deferred              |
| `gi` / `g1`-`g5` | jump to inbox / project                        |
| `m1`..`m5`       | move task to project                           |

## How theme adoption works

Omarchy stores the active theme at `~/.config/omarchy/current/theme/`. On
launch and whenever `omarchy-theme-set` swaps it out, the Rust watcher
re-parses `alacritty.toml` from that directory and emits a `theme-changed`
event. The frontend paints the tokens onto CSS custom properties on
`:root`, so the entire UI re-colors without a reload.

If you're curious about the full token map — see
[`src-tauri/src/theme.rs`](src-tauri/src/theme.rs) and
[`src/theme/cssVars.ts`](src/theme/cssVars.ts).

## Data layout

```
~/.local/share/todarchy/
├── tasks.json          primary store, human-readable JSON
└── tasks.json.bak      last revision, rotated on every save
```

Schema lives in [`schemas/tasks.schema.json`](schemas/tasks.schema.json).
The format is JSON (not SQLite) on purpose — `grep`-able, `jq`-able, and
the CLI + waybar module share it without a db driver.

## Development

```bash
npm install
npx tauri dev           # hot-reload dev build
```

Frontend is React + Vite, ported from the design mocks in `design-mocks/`.
Backend is Rust + Tauri 2.

See comments in [`src-tauri/src/main.rs`](src-tauri/src/main.rs) for the
command handlers and [`src/ui/app.jsx`](src/ui/app.jsx) for the main UI
tree (2 000-line single file — it's the design mock, ported as-is).

## Roadmap

- **v0.1** (this release) — local-only, theme adoption, CLI, waybar
- **v0.2** — end-to-end encrypted sync across devices
  (see [`docs/SYNC.md`](docs/SYNC.md) for the planned protocol)
- **v0.3** — project templates, due-time precision, recurring tasks

## Credits

- Designed by **Claude Design** (Anthropic design agent)
- Built by **Claude Code** (Anthropic coding agent) — working in this repo
- Inspired by Omarchy's aesthetic and its remarkable commitment to keyboard-
  first, terminal-native tools

## License

MIT — see [`LICENSE`](LICENSE).
