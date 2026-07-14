# todokase

Keyboard-first, terminal-native task manager for **Omarchy** (Arch + Hyprland).
Runs as a TUI inside your terminal, so it inherits your active Omarchy theme
automatically — pick a theme from the Omarchy menu (or run
`omarchy-theme-set "Tokyo Night"`) and todokase re-colors with the terminal.
No browser engine, no config: a small static binary in a floating window.

This is the Linux build; a mobile and web version are planned as separate
repos. The binary is called `todokase` (the TUI), the CLI is `tod`, and the
Waybar helper is `todokase-waybar`. All three share one JSON store.

> Internal note: the Cargo crates are still named `todarchy-*` and the data
> dir is `~/.local/share/todarchy/` — a deliberately stable internal id that
> won't churn if the public name changes again. Only public surfaces are
> "todokase".

<!-- TODO: add a real screenshot — `docs/screenshot.png` once you've taken one. -->


## Features

- Vim-style keyboard control (`j/k/x/o/gg/G/...`)
- Command palette (`Ctrl-K` or `:`)
- Inbox + projects + contexts + due dates + deferral
- `todo` / `next` / `all` view modes (cycle the chip in the list header)
- Natural-language defer (`tomorrow`, `+3d`, `+1w`, `fri`, `weekend`, ISO dates)
- Tree-nested tasks via Tab / Shift-Tab
- Export to JSON / Markdown and import JSON (command palette)
- Inherits your terminal's Omarchy theme automatically — zero config
- CLI companion (`tod add "buy milk @errands !today"`)
- Waybar module showing tasks due today
- Desktop notifications (`notify-send`) when deferred tasks come back
- Sync across devices: a shared folder (Syncthing / Dropbox / iCloud), a
  self-hosted relay (`todarchy-server`), and end-to-end-encrypted per-project
  sharing — all optional, off by default (see [`docs/SYNC.md`](docs/SYNC.md))

## Install (from source)

Requires `rustup` (plus `libsecret` for shared-project keys — already on a
fresh Omarchy install). No Node, no WebKit.

```bash
git clone https://github.com/ryanrusnak/todokase.git
cd todokase

cargo build --release          # builds todokase (TUI), tod (CLI), todokase-waybar

# Install binaries to ~/.local/bin
install -Dm755 target/release/todokase        ~/.local/bin/todokase
install -Dm755 target/release/tod             ~/.local/bin/tod
install -Dm755 target/release/todokase-waybar ~/.local/bin/todokase-waybar

# Optional: desktop entry + share-link handler so Walker / your launcher
# finds it and todarchy:// links open the app (the URL scheme stays
# todarchy:// — it's the cross-platform share protocol, not a brand surface)
install -Dm644 packaging/omarchy/todokase.desktop \
  ~/.local/share/applications/todokase.desktop
install -Dm644 packaging/omarchy/todokase-accept.desktop \
  ~/.local/share/applications/todokase-accept.desktop
install -Dm644 packaging/omarchy/todokase.png \
  ~/.local/share/icons/hicolor/128x128/apps/todokase.png
xdg-mime default todokase-accept.desktop x-scheme-handler/todarchy
```

Launch by running `todokase` in any terminal, or bind it in Hyprland — see below.

## Install (AUR PKGBUILD)

See [`packaging/omarchy/PKGBUILD`](packaging/omarchy/PKGBUILD). Drop it in a
clean build dir and run `makepkg -si`.

## Hyprland keybind

todokase is a terminal app, so `Super+T` opens it in a floating terminal
window as a scratchpad. Drop the snippet in
`packaging/omarchy/hyprland.snippet.conf` into your conf folder (it installs a
`todokase-toggle` helper for summon/dismiss). The essentials:

```conf
bind = SUPER, T, exec, todokase-toggle

windowrule = float on,        match:class todokase
windowrule = size 1200 760,   match:class todokase
windowrule = center on,       match:class todokase
windowrule = opacity 0.85 0.78, match:class todokase
```

## Waybar module

Add to your waybar config:

```jsonc
"custom/todokase": {
  "exec": "todokase-waybar",
  "interval": 30,
  "return-type": "json",
  "on-click": "alacritty --class todokase -e todokase"
}
```

And reference `custom/todokase` in your `modules-right` (or wherever).

## MCP server

`todokase-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server (stdio transport) that lets an LLM read and edit your tasks. It drives
the same store as everything else, so reads pull the latest state and writes
ride your configured sync — a task Claude adds shows up on your other devices,
and shared projects stay end-to-end encrypted.

Tools: `list_projects`, `list_tasks`, `add_task`, `complete_task`,
`update_task`, `delete_task`. Tasks can be referenced by id, 8-char id prefix,
or a unique title substring.

Register it with Claude Code:

```bash
claude mcp add todokase -- ~/.local/bin/todokase-mcp
```

Or in Claude Desktop's `claude_desktop_config.json`:

```jsonc
{
  "mcpServers": {
    "todokase": { "command": "todokase-mcp" }
  }
}
```

Then ask e.g. "add oat milk to my groceries list" or "what's on my inbox?".

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
| `e`              | edit task line (title · `@context` · `!due`)  |
| `c`              | edit note/body in `$EDITOR` (markdown)        |
| `⇧C`             | add a comment (in `$EDITOR`)                  |
| `d`              | defer picker (type `tomorrow` / `+3d` / `fri`) |
| `Del` / `⌫`      | delete                                        |
| `u`              | undo                                          |
| `/`              | search in list                                |
| `:` / `Ctrl-K`   | command palette                                |
| `?`              | keymap cheat sheet (this table, in-app)        |
| `i`              | toggle detail pane                             |
| `Ctrl-d`/`Ctrl-u`| scroll the detail note/comments               |
| `Tab` / `S-Tab`  | indent / outdent (nest under sibling above)   |
| `z`              | collapse/expand children                       |
| `v`              | cycle view mode (todo → next → all)           |
| `fd` / `fs`      | toggle show-done / show-deferred              |
| `gi` / `g1`-`g5` | jump to inbox / project                        |
| `m1`..`m5`       | move task to project                           |

## How theme adoption works

There's no theme code at all — that's the point. todokase renders with the
terminal's ANSI palette (plus `REVERSED` for the cursor row), and Omarchy
already themes your terminal. Run `omarchy-theme-set "Tokyo Night"` and every
terminal recolors; todokase, living inside one, comes along for free. The
old Tauri build needed a 500-line theme watcher to parse `alacritty.toml` and
repaint CSS variables because a WebView doesn't inherit terminal colors — a
TUI deletes that whole problem.

## Configuration & sync

Sync is configured by editing a text file — no in-app settings screen, true to
Omarchy. The app **reads** `~/.config/todarchy/config.toml` (internal dir keeps
the stable name; live: the watchers
re-read it every tick, so edits apply within a second or two) and never rewrites
it, so your comments stay put. It's created, commented, on first run:

```toml
# todokase configuration — edit this file by hand.

# A folder your OS keeps in sync across devices (Syncthing / Dropbox / iCloud).
sync_folder = ""

# A self-hosted todarchy-server relay (alternative or additional transport).
server_base_url = ""

# Shared doc id for the relay — must be IDENTICAL on all your devices.
# Generate one with:  todokase gen-id
server_main_doc_id = ""
```

Turn on folder sync by pointing `sync_folder` at a synced directory; turn on the
relay by setting `server_base_url` + a shared `server_main_doc_id` (run
`todokase gen-id` to mint one, paste the same value on every device). Both can
run at once. From inside the app, the command palette offers **"sync: edit
config"** (opens this file in `$EDITOR`) and **"sync: check server"** — but no
config lives in the UI.

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
cargo run -p todarchy-tui      # run the TUI
cargo test                     # core + tui unit tests
```

Pure Rust, one Cargo workspace:

- **`crates/todarchy-core`** — the Tauri-free heart: the Automerge task
  store, all three sync transports (folder / relay / encrypted sharing), the
  keyring wrapper, and the due-task notifier. UI-agnostic; the only thing a
  front end supplies is an implementation of the 3-method `EventSink` trait.
- **`crates/todarchy-tui`** — the Ratatui front end. `model.rs` is the data
  model + parsers, `app.rs` the state machine + keymap, `ui.rs` the
  rendering, `main.rs` the async runtime that wires core's watchers to the
  event loop.
- **`crates/todarchy-cli`** (`tod`) and **`crates/todarchy-waybar`** — share
  the same JSON store.

## Roadmap

- **v0.1** — local-only, theme adoption, CLI, waybar (Tauri/WebView build)
- **v0.2** — Automerge sync: shared folder, relay, encrypted per-project sharing
- **v0.3** (this release) — Mac parity, then the rewrite to a native Ratatui
  TUI: no WebKit, no Node, themed by the terminal, ~11 MB static binary
- **next** — recurring tasks, due-time precision, project templates

## Credits

- Originally designed by **Claude Design** (Anthropic design agent)
- Built and rewritten to a TUI by **Claude Code** (Anthropic coding agent)
- Inspired by Omarchy's aesthetic and its remarkable commitment to keyboard-
  first, terminal-native tools

## License

MIT — see [`LICENSE`](LICENSE).
