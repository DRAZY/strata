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

## Command line

```
strata [options] [file[:line[:column]] | folder]...

  -w, --wait       wait until the files' tabs are closed (for $EDITOR / git)
  -h, --help       show help
  -v, --version    show the version
```

- Files open in tabs, folders in the sidebar; `file:12` and `file:12:5` jump
  to a line and column (a file whose name really ends in `:12` wins if it
  exists). A path that doesn't exist yet opens as an empty tab and is created
  when you first save it.
- **One app, many windows.** If Strata is already running, `strata` hands the
  paths to it (over `strata.sock` in the config folder; a localhost port plus a
  random token in `strata.port` on Windows), brings it forward and returns at
  once. Files open in the focused window; a file already open in another
  window comes forward there instead. `-n/--new-window` opens them in a new
  window. A socket left behind by a crash is replaced by the next Strata to
  start.
- **Windows** (File ▸ New Window, ⇧⌘N / Ctrl+Shift+N, or a tab's "Move to New
  Window"): each has its own tabs, panes, folder, find bar, terminal and
  builds; settings, recent files and clipboard history are shared, and the
  Window menu lists them. Close Window (⇧⌘W) asks about that window's unsaved
  tabs and stops its shells. Quitting (or closing the last window, which quits,
  on macOS too) keeps every window, unsaved text and position included, under
  hot exit.
- `git config --global core.editor "strata -w"` works: `-w` blocks until the
  tab is closed (or Strata quits).
- **Install the command**: Strata menu ▸ Install Shell Command… (or the palette:
  "Strata: Install Shell Command") links `/usr/local/bin/strata` to the app,
  falling back to `~/.local/bin/strata` if `/usr/local/bin` isn't writable. By
  hand: `ln -sf /Applications/Strata.app/Contents/MacOS/strata /usr/local/bin/strata`.
  On Windows, add Strata's folder to `PATH`.
- macOS: typed in a terminal, the linked command starts Strata.app through
  Launch Services (so it gets its Dock icon and focus) and returns. Strata also
  opens files from Finder (Open With ▸ Strata, or drop on the Dock icon) and
  `open -a Strata file`, both when it's running and on a cold start.

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
- **Snippets** in Sublime's `.sublime-snippet` format (bring your own: put
  them anywhere under the config folder's `Snippets/`; they reload when
  changed). Type a trigger and press Tab, or pick one from the completion list
  or Tools ▸ Snippets…; Tab / Shift+Tab move through `$1…$n` fields, `$0` is
  the last stop, Esc finishes. Supports `${1:placeholders}` (nested), mirrored
  fields (edited together), `$TM_FILENAME`, `$TM_FILEPATH`, `$TM_SELECTED_TEXT`,
  `$TM_CURRENT_LINE`, `$TM_CURRENT_WORD`, `$TM_LINE_INDEX`, `$TM_LINE_NUMBER`,
  `${VAR:default}`, `${VAR/regex/format/flags}` and `${1/regex/format/flags}`
  (refreshed when Tab leaves the field), and `<scope>` selectors (`,`, `-`,
  descendants). Expands at every cursor. Tools ▸ Developer ▸ New Snippet…
  opens a template. Built-ins: Rust `fn` `impl` `test` `match` `struct`;
  Python `def` `class` `ifmain` `for`; JS/TS `fn` `arrow` `log` `imp`; Go
  `func` `iferr`; Markdown `link` `img` `code` `table`; HTML `html5` `a`
  `div`; shell `shebang` `if` `for`. A user snippet with the same trigger wins.
- **Languages**: about 200 grammars, chosen by file name, extension or first
  line. Beyond syntect's defaults (C/C++, C#, Go, Java, JavaScript, Python,
  Ruby, Rust, PHP, HTML, CSS, SQL, YAML, JSON, Markdown, shell, Makefile, Diff…)
  it bundles bat's collection: TypeScript, TSX, JSX, TOML, Swift, Kotlin, Dart,
  Elixir, Zig, Nix, Terraform/HCL, GraphQL, Protobuf, SCSS/Sass/Less, Vue,
  Svelte, Dockerfile, INI/.editorconfig, .env, CMake, nginx, Fish, CSV, Lua,
  Haskell, Julia, Groovy/Jenkinsfile and more, plus Strata's own PowerShell
  grammar. Markdown code fences highlight their language. Add your own
  `.sublime-syntax` files to the config folder's `Syntaxes/` and `.tmTheme`
  color schemes to `Themes/`; broken files are skipped and named in the status
  bar.
- **Find**: find/replace with regex, quick find, use selection for find, and
  find in folder (parallel, respects .gitignore). Its **Where** field takes
  folders, `*.rs`-style includes, `-*.min.js` excludes, `<open folders>` and
  `<open files>`. **Replace in Files** (Find ▸ Replace in Files…) asks
  "Replace N occurrences across M files?"; regex replacements take `$1`,
  `${name}` or `\1`. Open files change in their tabs (one undo step, left
  unsaved); the rest are rewritten atomically in their own encoding and line
  endings, skipping binary, undecodable, read-only, over-50 MB and
  changed-since-search files. Undo Replace in Files restores what was written.
- **Build systems** (Tools ▸ Build, ⌘B): Cargo, Rust, Python, Node, Go, C/C++,
  Make, npm, Ruby, shell, chosen automatically; output panel with clickable
  errors, F4 for the next result. Add your own as JSON in the config folder's
  `build_systems/`.
- **Terminal** (View ▸ Terminal, Ctrl+\`): your login shell in a bottom panel,
  started in the project folder, with tabs, 256-color/truecolor output in the
  theme's palette, selection and scrollback. While it has the keyboard it keeps
  Ctrl and Alt keys (Alt/Option is Meta); ⌘C/⌘V/⌘K copy, paste and clear on
  macOS, Ctrl+Shift+C/V (or Ctrl+C with a selection) elsewhere.
- **Right-click menus** everywhere, like Sublime's: the editor (Show Unsaved
  Changes as a diff, Cut/Copy/Paste, Select All, Reveal in Finder, Copy File
  Path, Reveal in Side Bar, Goto Definition, Toggle Comment, Fold), the line
  numbers (bookmark, fold, copy line), tabs (Close Others, to the Right,
  Unmodified), the terminal (Copy, Paste, Select All, Clear, New/Close, Hide)
  and its tabs, the Markdown preview (copy as Markdown or HTML, export, hide),
  and the find, replace, palette and find-in-folder fields.
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
spell check, no macros, Strata quits with its last window (macOS), cloned
views of one file in two panes aren't supported, and Finder "Open With" isn't
wired up (drag the file onto the window or use ⌘O). Word wrap turns off for
files over 8 MB. The bundled grammars come from syntect's defaults and the bat
project (via the `two-face` crate, MIT/Apache-2.0); each grammar's own license
text ships in Help ▸ Licenses.

## Development

`cargo test` runs editor-core unit tests plus headless egui tests that drive
the real widget and the whole window with simulated keys and clicks.

CI (`.github/workflows/build.yml`) builds release binaries on an Apple silicon
macOS runner and a Windows x64 runner.

Dev aids (all pair with `STRATA_SCREENSHOT=out.ppm`, which renders the window,
saves a screenshot and exits): `STRATA_DEV_FIND=query` (+ `STRATA_DEV_FIND_REGEX=1`),
`STRATA_DEV_FIND_IN_FILES=query` (+ `STRATA_DEV_WHERE`, `STRATA_DEV_REPLACE`,
`STRATA_DEV_REPLACE_STEP=confirm|run`), `STRATA_DEV_PALETTE="p:query"` or
`"cmd:query"`, `STRATA_SCREENSHOT_TYPE='fn\t'` (types into the editor first; `\t`
presses Tab). `STRATA_CONFIG_DIR=dir` uses a separate settings/session folder (and its own
single-instance socket, so a test copy never talks to your everyday Strata;
for the .app use `open --env STRATA_CONFIG_DIR=dir -a dist/Strata.app file`,
with your installed Strata quit, since both copies share a bundle id).

## Test build for macOS (Apple silicon)

```sh
scripts/package-macos.sh   # → dist/Strata-<version>-arm64.dmg
```

The app is ad-hoc signed, not notarized. On the Mac that built it, it opens
normally. On another Mac, the first launch is blocked by Gatekeeper: right-click
Strata.app → Open, or run `xattr -dr com.apple.quarantine /Applications/Strata.app`.
