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
| Toggle terminal / new terminal | ⌃\` / ⌃⇧\` | Ctrl+\` / Ctrl+Shift+\` |

In the find bar: Enter next, Shift+Enter previous, Alt+Enter Find All (a cursor
per match); in the replace field Cmd/Ctrl+Enter replaces all (one undo step).

## What works

- **Menus like Sublime's**: a native macOS menu bar (Strata, File, Edit,
  Selection, Find, View, Goto, Tools, Project, Window, Help) and an in-window
  menu bar on Windows. Every item runs a real command and shows its shortcut.
- **Files and formats**: open, save, save as, save all, revert, open recent,
  reopen closed tab. 40 encodings (UTF-8 with or without BOM, UTF-16 LE/BE,
  Windows/ISO Western, Cyrillic, Greek, Japanese, Chinese, Korean…) detected
  on open, with File ▸ Reopen with Encoding and Save with Encoding. LF/CRLF
  conversion. Saves are atomic (temp file + rename), keep symlinks and file
  permissions, and refuse text the chosen encoding can't hold.
- **Tabs and split panes**: preview tabs, drag to reorder or into another pane;
  1–4 columns, 2 rows or a 2×2 grid (Alt+Cmd+1…5), draggable dividers.
- **Hot exit**: quitting never asks. Tabs, panes, cursors and unsaved text come
  back next launch; the session is also saved every few seconds.
- **Navigation**: command palette, Goto Anything (`file:line`, `@symbol`), go to
  line, go to symbol, go to definition (F12), jump back/forward, bookmarks.
- **Editing**: multiple cursors and column selection, auto-closing brackets,
  word completions while typing (Ctrl+Space to ask), comments, indent/reindent,
  line moves, duplicate, join, transpose, case conversion, sort/shuffle/unique
  lines and selections, wrap paragraph at ruler, paste and indent.
- **View**: word wrap (Alt+Cmd+W; on by default for Markdown and plain text),
  minimap, indent guides, bracket matching, distraction-free mode, hide tabs or
  status bar, CJK/IME input, Strata Dark/Light color schemes.
- **Markdown preview** (View ▸ Markdown Preview, ⌃⇧V on macOS, Alt+Shift+V on
  Windows): a live, theme-colored preview beside the editor with GitHub-style
  tables, task lists, footnotes, alerts and highlighted code blocks; it follows
  the editor's scroll. File ▸ Export ▸ Markdown as HTML… saves a standalone page.
- **Find**: find/replace with regex, quick find, use selection for find, and
  find in folder (parallel, respects .gitignore).
- **Build systems** (Tools ▸ Build, ⌘B): Cargo, Rust, Python, Node, Go, C/C++,
  Make, npm, Ruby, shell, chosen automatically; output panel with clickable
  errors, F4 for the next result. Add your own as JSON in the config folder's
  `build_systems/`.
- **Terminal** (View ▸ Terminal, Ctrl+\`): your login shell in a bottom panel,
  started in the project folder, with tabs, 256-color/truecolor output in the
  theme's palette, selection and scrollback. While it has the keyboard it keeps
  Ctrl and Alt keys (Alt/Option is Meta); ⌘C/⌘V/⌘K copy, paste and clear on
  macOS, Ctrl+Shift+C/V (or Ctrl+C with a selection) elsewhere.
- **Projects**: open a folder (sidebar with New File/Folder, Rename, Reveal),
  save and reopen `.strata-project` files.
- **Settings** in `settings.json` (⌘, opens it): font size, theme, tab size,
  spaces vs tabs, indentation detection, word wrap, wrap width, minimap,
  auto-complete, hot exit. Stored in `~/Library/Application Support/Strata`
  (macOS) or `%APPDATA%\Strata` (Windows).

Measured on an M-series Mac (release build): a 95 MB, 1.6M-line Python file
opens and renders highlighted in ~0.6 s.

Also: code folding (⌥⌘[ / ⌥⌘], ⌘K ⌘1…9), rulers and a fixed wrap column
(View menu), soft undo, Emacs-style mark and yank, macros, paste from
history, copy as HTML.

Look and feel: a theme-colored gradient scrollbar with match and cursor
ticks, pick any installed font for the editor and the interface (with sizes),
cursor styles (line, block, underline; blink, fade, solid), window opacity,
bracket pair colors, visible whitespace and trailing-whitespace tint, color
swatches under CSS colors, sticky scroll headers, smooth scrolling, and git
change bars with the branch in the status bar (View ▸ Editor Extras toggles
each). Tools ▸ Text has JSON format/minify, Base64/URL/HTML codecs, UUID and
date; Tools ▸ On Save trims whitespace, adds a final newline, or saves on
focus loss.

Known gaps: no replace-in-files, no plugin/package system, no
spell check, no macros, one window per app instance (no New Window), cloned
views of one file in two panes aren't supported, and Finder "Open With" isn't
wired up (drag the file onto the window or use ⌘O). Word wrap turns off for
files over 8 MB. The bundled grammars come from syntect's defaults; their
licensing must be verified before a public release.

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
