# Strata

A fast, open-source, keyboard-first text editor in the spirit of Sublime Text.

> Status: alpha. Icon: `assets/icon.svg`.

## Goals

- Opens instantly; stays smooth on huge files (100 MB+).
- Multiple cursors, Goto Anything, command palette, split panes, minimap.
- Reads existing `.sublime-syntax` / `.tmTheme` grammars and color schemes so the
  ecosystem's community grammars work on day one.
- MIT-licensed. Supported platforms: **macOS (Apple silicon)** and **Windows (x64)**.

## Clean-room rule

This project is **not** derived from Sublime Text's code, binaries, or bundled
assets. Read `docs/CLEAN_ROOM.md` before contributing.

## Proposed stack

| Layer | Choice | Why |
|---|---|---|
| Core | Rust | Speed, memory safety, single binary |
| Text buffer | `ropey` | Rope with O(log n) edits on huge files |
| Highlighting | `syntect` | Reads `.sublime-syntax` + `.tmTheme` natively |
| Parsing (later) | `tree-sitter` | Structural selection, symbols |
| UI | `winit` + `wgpu` | GPU text rendering, Sublime-level responsiveness |
| Plugins | Lua or WASM | Sandboxed, embeddable |

See `docs/FEATURES.md` for the feature spec and roadmap.

## Build & run

Requires Rust (install from https://rustup.rs).

```sh
cargo run --release -- path/to/file.rs     # or a folder, which opens in the sidebar
```

## Shortcuts

Cmd on macOS, Ctrl on Windows unless noted. Every command is also in the
command palette, which shows its shortcut.

| | macOS | Windows |
|---|---|---|
| **Command palette** | ⇧⌘P | Ctrl+Shift+P |
| **Goto Anything** (files; `:42` line; `@name` symbol; `file:42`) | ⌘P | Ctrl+P |
| Go to line / symbol | ⌃G / ⌘R | Ctrl+G / Ctrl+R |
| New tab / close tab / reopen closed | ⌘N / ⌘W / ⇧⌘T | Ctrl+N / Ctrl+W / Ctrl+Shift+T |
| Next / previous tab | ⌃Tab, ⌥⌘→, ⇧⌘] | Ctrl+Tab, Ctrl+PgDn |
| Tab 1–9 | ⌘1…⌘9 | Alt+1…Alt+9 |
| Open / open folder / save / save as / save all | ⌘O / ⇧⌘O / ⌘S / ⇧⌘S / ⌥⌘S | Ctrl+… |
| Toggle sidebar / full screen | ⌘\\ / ⌃⌘F | Ctrl+\\ / F11 |
| Settings | ⌘, | Ctrl+, |
| Add next occurrence / all occurrences | ⌘D / ⌃⌘G | Ctrl+D / Alt+F3 |
| Add cursor / cursor above, below | ⌘click / ⌃⇧↑↓ | Ctrl+click / Ctrl+Alt+↑↓ |
| Column (box) selection | ⌥drag or middle-drag | Alt+drag or middle-drag |
| Split selection into lines / expand to line | ⇧⌘L / ⌘L | Ctrl+Shift+L / Ctrl+L |
| Toggle comment | ⌘/ | Ctrl+/ |
| Indent / unindent | ⌘] / ⌘[ (or Tab / ⇧Tab) | Ctrl+] / Ctrl+[ |
| Duplicate / delete line | ⇧⌘D / ⌃⇧K | Ctrl+Shift+D / Ctrl+Shift+K |
| Move line up / down | ⌃⌘↑ / ⌃⌘↓ | Ctrl+Shift+↑ / ↓ |
| Join lines / insert line after, before | ⌘J / ⌘↩, ⇧⌘↩ | Ctrl+J / Ctrl+Enter, Ctrl+Shift+Enter |
| Jump to / select inside brackets | ⌃M / ⌃⇧M | Ctrl+M / Ctrl+Shift+M |
| Upper / lower case | ⌘K ⌘U / ⌘K ⌘L | Ctrl+K Ctrl+U / Ctrl+K Ctrl+L |
| Sort lines | F5 | F9 |
| Find / replace | ⌘F / ⌥⌘F | Ctrl+F / Ctrl+H |
| Next / previous match | ⌘G / ⇧⌘G | F3 / Shift+F3 |
| Find in folder | ⇧⌘F | Ctrl+Shift+F |

In the find bar: Enter next, Shift+Enter previous, Alt+Enter Find All (a cursor
per match); in the replace field Cmd/Ctrl+Enter replaces all (one undo step).

## What works

- **Tabs** with preview tabs (single-click in the sidebar previews; edit or
  double-click to keep), drag to reorder, middle-click to close, Open Files list.
- **Hot exit**: quitting never asks. Open tabs, cursors and unsaved text
  (including untitled tabs) come back next launch. The session is also saved
  every few seconds, so a crash loses little.
- **Outside changes**: a file changed on disk reloads if the tab is clean; a tab
  with unsaved edits is flagged instead, with a Reload button in the status bar.
- **Command palette and Goto Anything** with fuzzy matching, live preview of
  lines, symbols and color schemes (Esc puts things back).
- **Editing**: multiple cursors everywhere, auto-closing brackets and quotes
  (typing a closer steps over it), comment toggling for ~40 languages, line
  moves, duplicate, join, sort, case changes, bracket matching.
- **View**: minimap, indent guides, matching-bracket underline, outlines on
  other occurrences of the selection, blinking caret, CJK/IME input.
- **Strata Dark / Strata Light** color schemes plus syntect's bundled ones.
  The window chrome follows the scheme.
- **Sidebar**: folder tree with New File / New Folder / Rename / Copy Path /
  Reveal in Finder; find in folder (parallel, respects .gitignore).
- **Settings** in `settings.json` (⌘, opens it; saving applies it): font size,
  theme, tab size, spaces vs tabs, minimap, hot exit. Stored in
  `~/Library/Application Support/Strata` (macOS) or `%APPDATA%\Strata` (Windows).
- Status bar: syntax picker, tab size, line ending conversion, color scheme.
- Drag files or folders onto the window to open them.

Measured on an M-series Mac (release build): a 95 MB, 1.6M-line Python file
opens and renders highlighted in ~0.6 s.

Known gaps: no split panes, no word wrap, no replace-in-files, no native macOS
menu bar (use the command palette), Finder "Open With" isn't wired up (drag the
file onto the window, or use ⌘O), vertical cursor moves treat CJK characters as
one column wide. The find bar re-scans the whole file after each edit, which is
noticeable on ~100 MB files. The bundled grammars come from syntect's defaults;
their licensing must be verified before a public release.

## Development

`cargo test` runs editor-core unit tests plus headless egui tests that drive
the real widget and the whole window with simulated keys and clicks.

CI (`.github/workflows/build.yml`) builds release binaries on an Apple silicon
macOS runner and a Windows x64 runner.

Dev aids (all pair with `STRATA_SCREENSHOT=out.ppm`, which renders the window,
saves a screenshot and exits): `STRATA_DEV_FIND=query` (+ `STRATA_DEV_FIND_REGEX=1`),
`STRATA_DEV_FIND_IN_FILES=query`, `STRATA_DEV_PALETTE="p:query"` or
`"cmd:query"`. `STRATA_CONFIG_DIR=dir` uses a separate settings/session folder.

## Test build for macOS (Apple silicon)

```sh
scripts/package-macos.sh   # → dist/Strata-<version>-arm64.dmg
```

The app is ad-hoc signed, not notarized. On the Mac that built it, it opens
normally. On another Mac, the first launch is blocked by Gatekeeper: right-click
Strata.app → Open, or run `xattr -dr com.apple.quarantine /Applications/Strata.app`.
