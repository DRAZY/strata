<p align="center"><img src="assets/icon-app.png" width="112" alt="Strata icon"></p>

<h1 align="center">Strata</h1>

<p align="center"><strong>A fast, open-source code and text editor.</strong></p>

<p align="center">
  <a href="https://github.com/DRAZY/strata/actions/workflows/build.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/DRAZY/strata/build.yml?branch=main&style=for-the-badge&label=build"></a>
  <a href="https://github.com/DRAZY/strata/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/DRAZY/strata?style=for-the-badge&label=latest&color=1fb6a8"></a>
  <a href="https://github.com/DRAZY/strata/releases"><img alt="Total downloads" src="https://img.shields.io/github/downloads/DRAZY/strata/total?style=for-the-badge&label=downloads&color=1fb6a8"></a>
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-1fb6a8?style=for-the-badge"></a>
  <img alt="macOS and Windows" src="https://img.shields.io/badge/platforms-macOS%20%7C%20Windows-252532?style=for-the-badge">
  <br>
  <a href="https://github.com/DRAZY/strata/stargazers"><img alt="Stars" src="https://img.shields.io/github/stars/DRAZY/strata?style=for-the-badge&color=f5c542"></a>
  <img alt="Last commit" src="https://img.shields.io/github/last-commit/DRAZY/strata?style=for-the-badge&color=f5c542">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-B7410E?style=for-the-badge&logo=rust&logoColor=white">
  <img alt="egui" src="https://img.shields.io/github/cargo-toml/dependency-version/DRAZY/strata/egui?style=for-the-badge&label=egui&color=3b82f6">
  <img alt="Notarized" src="https://img.shields.io/badge/macOS-notarized-252532?style=for-the-badge&logo=apple&logoColor=white">
</p>

<p align="center"><img src="docs/screenshots/hero.png" width="900" alt="Strata in the Tokyo Night scheme: a Rust project in the side bar, a file with four cursors, git change bars in the gutter, the minimap and the status bar"></p>

Strata is a native editor for macOS and Windows, written in Rust. It opens
instantly, stays smooth on files of hundreds of megabytes, and keeps every
command a keystroke away. Multiple cursors, Goto Anything, project-wide search
and replace, split panes, a built-in terminal and a live Markdown preview are
all there out of the box, with no accounts, no telemetry and nothing to set up.

> **Status: 1.0.** The first stable release. Please [report
> problems](https://github.com/DRAZY/strata/issues) (Help ▸ Report a Problem…
> fills in the details). See [CHANGELOG.md](CHANGELOG.md) and
> [Not in 1.0](#not-in-10) for what's planned next.

## Features

- **Multiple cursors** and box selection, Quick Add Next, split selection into
  lines, and soft undo for cursor changes.
- **Goto Anything** for files, lines (`:42`), symbols (`@name`) and text
  (`#text`), plus a **command palette** that lists every command with its
  shortcut.
- **Find and replace** with regular expressions, In Selection and Preserve
  Case; **Find in Files** and **Replace in Files** across whole folders,
  respecting `.gitignore`, with undo.
- **Tabs, split panes and multiple windows**: up to four panes in columns,
  rows or a grid, several views of one file, and a distraction-free mode.
- **Projects**: one or more folders in the side bar, saved as
  `.strata-project` files.
- **Built-in terminal** with tabs, true color and your scheme's palette.
- **Live Markdown preview** with GitHub-style tables, task lists, alerts,
  footnotes and highlighted code, plus HTML export.
- **About 190 languages** highlighted, with symbols, smart indentation and
  comment toggling for many of them.
- **35 color schemes** (One Dark, Dracula, Nord, Solarized, Gruvbox, Tokyo
  Night, Catppuccin, Rosé Pine and more) that can follow the system's light
  or dark mode, every one checked for WCAG AA contrast. Bundled coding fonts
  with ligatures.
- **Compare** two files, a file with another tab, or a file with its last Git
  commit, side by side and still editable.
- **Git change bars** in the gutter and the current branch in the status bar.
- **Spell check** for comments, strings and prose, with built-in US and UK
  English dictionaries.
- **Snippets**, **build systems** with clickable errors, macros, bookmarks,
  code folding, and text tools (JSON format, Base64, URL encoding and more).
- **Hot exit**: quit at any time without being asked to save. Every window,
  tab and unsaved change comes back next time.
- **Big files**: a 95 MB, 1.6 million-line file opens highlighted in about
  0.6 s on an M-series Mac, and minified one-line files stay responsive.
- **40 encodings** with detection of legacy ones (Shift_JIS, GBK, KOI8…),
  atomic saves, opt-in auto save, **printing** and **PDF export**. Images open
  in a preview and other binary files in a read-only hex view.
- **EditorConfig and per-language settings**: `.editorconfig` files set
  indentation, line endings, charset and save-time trimming per project;
  `syntax_settings` gives each language its own tab size, wrap, rulers and
  more.
- **Settings window** (⌘, / Ctrl+,): every setting by category, with search,
  applied live; a short welcome card on the first launch picks a scheme and
  font size.
- **Plain-file configuration**: settings and key bindings are JSON files that
  apply as soon as you save; the Settings window edits `settings.json` in
  place, keeping your comments. Add your own snippets (`.sublime-snippet`),
  syntaxes (`.sublime-syntax`) and color schemes (`.tmTheme`,
  `.sublime-color-scheme`); these widely shared formats mean existing
  collections work unchanged.
- **Private by design**: no accounts, no telemetry. Strata only contacts
  GitHub to check for and download updates, and only if you allow it (or when
  you choose Help ▸ Check for Updates…); it sends nothing else. Logs and crash
  reports stay on your computer.
- **Signed updates**: a new version is offered in the app and installs with
  one click; every download is checked against an ed25519 signature (and, on
  macOS, Strata's Developer ID) before it replaces anything, and the previous
  version comes back if the new one doesn't start.

## Screenshots

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/light-split.png" alt="GitHub Light scheme with a TypeScript file and a Python file in two panes"><br><sub>Split panes in GitHub Light</sub></td>
    <td width="50%"><img src="docs/screenshots/markdown.png" alt="A Markdown file beside its live preview with a table, task list and alert"><br><sub>Live Markdown preview</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/terminal.png" alt="The built-in terminal under the editor, showing a colored test run and a directory listing"><br><sub>Built-in terminal</sub></td>
    <td><img src="docs/screenshots/command-palette.png" alt="The command palette listing Toggle commands with their shortcuts"><br><sub>Command palette</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/compare.png" alt="A file compared with its last Git commit, side by side with the added and changed lines marked"><br><sub>Compare with the last commit</sub></td>
    <td><img src="docs/screenshots/settings.png" alt="The Settings window on the Appearance category"><br><sub>Settings window</sub></td>
  </tr>
  <tr>
    <td colspan="2" align="center"><img src="docs/screenshots/help.png" width="50%" alt="The built-in manual open at Find and Replace"><br><sub>The built-in manual</sub></td>
  </tr>
</table>

## Not in 1.0

Planned for after 1.0 (details in [`docs/ROADMAP.md`](docs/ROADMAP.md)):
language servers (LSP), plugins, a Vim mode, official Linux support and
screen reader support in the editor.

## Accessibility

Strata can be used entirely from the keyboard: every command is in the
command palette and can have a shortcut, and focus moves between the editor,
panes, side bar and panels without the mouse. High-contrast color schemes
(High Contrast Dark and Light) are included, and every bundled scheme is
checked for WCAG AA contrast. Screen readers aren't supported in the editor
area yet; that's on the [roadmap](docs/ROADMAP.md).

## Install

Download Strata from the [**Releases**](https://github.com/DRAZY/strata/releases)
page.

| Platform | File |
|---|---|
| macOS 11 or later, any Mac | `Strata-<version>-universal.dmg` |
| macOS, Apple silicon only (smaller) | `Strata-<version>-arm64.dmg` |
| Windows x64 | `Strata-<version>-windows-x64.zip` |

**macOS.** Open the DMG and drag Strata to Applications. Releases are signed
and notarized by Apple, so Strata opens normally (macOS asks once whether to
open an app downloaded from the internet). To use Strata from a terminal,
choose **Strata ▸ Install Shell Command…**; see [Command line](#command-line).

**Windows.** Unzip the folder anywhere and run `Strata.exe`. It isn't
code-signed yet, so the first run shows a SmartScreen warning: click **More
info ▸ Run anyway**. Add the folder to `PATH` to use the `strata` command. The
download is portable: nothing is installed, and settings live in
`%APPDATA%\Strata`.

**Linux.** The code builds for Linux, but it isn't an officially supported
platform yet; build it from source (below).

**Checksums.** Every file has a `.sha256` next to it. Download both into the
same folder, then:

```sh
shasum -a 256 -c Strata-<version>-universal.dmg.sha256      # macOS: prints "OK"
```

```powershell
Get-FileHash Strata-<version>-windows-x64.zip -Algorithm SHA256   # Windows: compare with the .sha256 file
```

## Build from source

You need Rust (install it from <https://rustup.rs>).

```sh
cargo build --release              # → target/release/strata (strata.exe on Windows)
cargo run --release -- path/to/file.rs   # or a folder, which opens in the side bar
```

To build the macOS app and a DMG (on an Apple silicon Mac, with the
`aarch64-apple-darwin` Rust target and ImageMagick's `magick` installed):

```sh
scripts/package-macos.sh           # → dist/Strata.app and dist/Strata-<version>-arm64.dmg
```

## Documentation

The user manual is built into the app: **Help ▸ Documentation** (F1, or
"Help: Documentation" in the command palette). It covers editing, find and
replace, snippets, projects, panes and windows, comparing files, printing, the
terminal, Markdown preview, build systems, Git, appearance, every setting, key
bindings, the command line and troubleshooting, with search across all pages.
It opens in its own window so you can read along while you work. **Help ▸
Keyboard Shortcuts** lists every command and its keys, generated from the key
bindings in effect (your `keybindings.json` included).

The pages are Markdown files in [`docs/manual/`](docs/manual/), compiled into
the binary. Shortcuts in them are written as `{{key:command_id}}` and filled in
with the current platform's keys. `cargo test` checks that every page parses,
every link and anchor resolves, every setting is documented and every command
appears on the shortcuts page.

## Command line

```
strata [options] [file[:line[:column]] | folder]...

  -n, --new-window open the files in a new window
  -w, --wait       wait until the files' tabs are closed (for $EDITOR / git)
  -h, --help       show help
  -v, --version    show the version
```

- Files open in tabs, folders in the side bar; `file:12` and `file:12:5` jump
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

## Keyboard shortcuts

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
| Toggle side bar / full screen | ⌘\\ / ⌃⌘F | Ctrl+\\ / F11 |
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
| Find: in selection / preserve case | ⌥⌘L / ⌥⌘A | Alt+L / Alt+A |
| Find: regex / case sensitive | ⌥⌘R / ⌥⌘C | Alt+R / Alt+C |
| Spell check / next, previous misspelling | F6 / ⌃F6, ⌃⇧F6 | F6 / Ctrl+F6, Ctrl+Shift+F6 |
| Toggle terminal / new terminal | ⌃\` / ⌃⇧\` | Ctrl+\` / Ctrl+Shift+\` |

In the find bar: Enter next, Shift+Enter previous, Alt+Enter Find All (a cursor
per match); in the replace field Cmd/Ctrl+Enter replaces all (one undo step).

### Custom key bindings

Preferences ▸ Key Bindings (macOS: Strata ▸ Settings ▸ Key Bindings, or
"Preferences: Key Bindings" in the palette) opens Strata's defaults, read-only,
next to your `keybindings.json` in the settings folder. It's a JSON list of
bindings, with comments and trailing commas allowed:

```jsonc
[
  { "keys": ["primary+shift+k"], "command": "delete_line" },          // primary = ⌘ / Ctrl
  { "keys": ["super+k", "super+u"], "command": "upper_case" },        // a chord
  { "keys": ["ctrl+alt+p"], "command": "show_overlay", "args": {"overlay": "command_palette"} },
  { "keys": ["super+shift+d"], "command": "" },                      // unbind
  { "keys": ["ctrl+`"], "command": "toggle_terminal", "context": [{"key": "terminal_focus"}] },
]
```

- **Commands** use Strata's ids (`command_palette`, `goto_anything`,
  `toggle_side_bar`, `split_selection_into_lines`, `focus_group` with
  `{"group": 0}`…; the defaults pane lists them all) or common command
  names from other editors' keymaps, with their args (`show_overlay`, `show_panel`, `toggle_comment`
  `{"block": true}`, `expand_selection` `{"to": "line"}`, `select_lines`,
  `run_macro_file` for Delete Line / Add Line, `set_layout`, `select_by_index`,
  `fold_by_level`…), so a pasted keymap mostly just works. Unknown
  commands are reported and skipped.
- **Keys**: `super`/`cmd` (macOS), `ctrl`, `alt`/`option`, `shift`, `primary`;
  letters, digits, `f1`–`f24`, arrows, `home`/`end`, `pageup`/`pagedown`,
  `enter`, `tab`, `escape`, `space`, `backspace`, `delete`, `keypad0`–`9`, and
  punctuation (`/`, `[`, `` ` ``, `,`, `.`, `plus`, `minus`, `equals`…).
- **Rules**: later entries win. Binding a command replaces its default keys (in
  the same context); `"command": ""` (or `"unbound"`) frees a key, `"noop"`
  swallows it. Taking a key from another command, or a single key that starts
  a chord, is reported in the status bar. Contexts: `terminal_focus`,
  `panel_has_focus`, `panel_visible`, `overlay_visible` (`operator` equal /
  not_equal, `operand` true / false); bindings with other contexts are skipped.
- Saving the file applies it right away (outside edits too). A file that
  doesn't parse keeps the last working key bindings and shows the line in the
  status bar. Menus, native macOS accelerators, the palette and right-click
  menus all show your keys. Cut, Copy, Paste and Quit keep their standard keys
  unless you bind those keys to something else.

## Feature reference

- **Menus**: a native macOS menu bar (Strata, File, Edit,
  Selection, Find, View, Goto, Tools, Project, Window, Help) and an in-window
  menu bar on Windows. Every item runs a real command and shows its shortcut.
- **Files and formats**: open, save, save as, save all, revert, open recent,
  reopen closed tab. 40 encodings (UTF-8 with or without BOM, UTF-16 LE/BE,
  Windows/ISO Western, Cyrillic, Greek, Japanese, Chinese, Korean…) detected
  on open, with File ▸ Reopen with Encoding and Save with Encoding. LF/CRLF
  conversion. Saves are atomic (temp file + rename), keep symlinks and file
  permissions, and refuse text the chosen encoding can't hold.
- **Tabs and split panes**: preview tabs, drag to reorder or into another pane;
  1–4 columns, 2 or 3 rows, or a 2×2 grid (Alt+Cmd+1…5), draggable dividers.
- **Several views of one file** (File ▸ New View into File, or a tab's Split
  View): each view has its own cursors, scroll and folds; text, undo history,
  saving and unsaved state are shared, and edits in one view move the others'
  selections and bookmarks along. Hot exit keeps every view (the text once).
- **Compare** (File ▸ Compare ▸ Compare with…, a tab's or the side bar's
  Compare with…, or Git: Compare with HEAD): two texts side by side with
  aligned lines and filler rows, changed lines and words marked, one scroll
  position, F7 / Shift+F7 for the next / previous difference, swap sides and
  ignore whitespace. The right side is the real file, editable and saveable;
  the diff (patience + linear-space Myers) follows your edits and handles
  50k-line files in milliseconds.
- **Printing** (File ▸ Print…, Export ▸ PDF… / HTML…): syntax-colored pages on
  white with line numbers, a header and page numbers, optionally just the
  selection, in black and white, on Letter or A4; Markdown prints as rendered.
  Print opens the PDF (fonts embedded, text searchable) in the system viewer.
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
  status bar, CJK/IME input, 35 color schemes and bundled coding fonts (see
  [Color schemes and fonts](#color-schemes-and-fonts)).
- **Markdown preview** (View ▸ Markdown Preview, ⌃⇧V on macOS, Alt+Shift+V on
  Windows): a live, theme-colored preview beside the editor with GitHub-style
  tables, task lists, footnotes, alerts and highlighted code blocks; it follows
  the editor's scroll. File ▸ Export ▸ Markdown as HTML… saves a standalone page.
- **Snippets** in the `.sublime-snippet` XML format (bring your own: put
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
- **Languages**: about 190 grammars, chosen by file name, extension or first
  line. Beyond syntect's defaults (C/C++, C#, Go, Java, JavaScript, Python,
  Ruby, Rust, PHP, HTML, CSS, SQL, YAML, JSON, Markdown, shell, Makefile, Diff…)
  it bundles bat's collection: TypeScript, TSX, JSX, TOML, Swift, Kotlin, Dart,
  Elixir, Zig, Nix, Terraform/HCL, GraphQL, Protobuf, SCSS/Sass/Less, Vue,
  Svelte, Dockerfile, INI/.editorconfig, .env, CMake, nginx, Fish, CSV, Lua,
  Haskell, Julia, Groovy/Jenkinsfile and more, plus Strata's own PowerShell
  grammar. Markdown code fences highlight their language. Add your own
  `.sublime-syntax` files to the config folder's `Syntaxes/` and `.tmTheme` or
  `.sublime-color-scheme` color schemes to `Themes/`; broken files are skipped and named in the status
  bar.
- **Spell check** (View ▸ Spell Check, F6): wavy underlines for misspelled
  words in comments, strings and prose (chosen by the `spelling_selector`
  scope selector),
  camelCase/snake_case aware; right-click for suggestions, Ignore Word and Add
  Word to Dictionary (`added_words`); Next/Prev Misspelling; bundled SCOWL
  en_US and en_GB Hunspell dictionaries via the pure-Rust `spellbook` crate.
- **Find**: find/replace with regex, case, whole word, In Selection (searches
  and replaces only inside the selection, multi-cursor too, tracking edits;
  `auto_find_in_selection`), Wrap, Preserve Case and Highlight Matches
  toggles, all saved in the session; quick find, use selection for find, and
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
- **Right-click menus** everywhere: the editor (Show Unsaved
  Changes as a diff, Cut/Copy/Paste, Select All, Reveal in Finder, Copy File
  Path, Reveal in Side Bar, Goto Definition, Toggle Comment, Fold), the line
  numbers (bookmark, fold, copy line), tabs (Close Others, to the Right,
  Unmodified), the terminal (Copy, Paste, Select All, Clear, New/Close, Hide)
  and its tabs, the Markdown preview (copy as Markdown or HTML, export, hide),
  and the find, replace, palette and find-in-folder fields.
- **Projects**: open one or more folders (side bar with New File/Folder,
  Rename, Reveal), save and reopen `.strata-project` files.
- **Settings** in `settings.json` (⌘, opens it): fonts, line spacing, color
  scheme (fixed or following the system), color overrides, tab size,
  spaces vs tabs, indentation detection, word wrap, wrap width, minimap,
  auto-complete, hot exit. Stored in `~/Library/Application Support/Strata`
  (macOS) or `%APPDATA%\Strata` (Windows), next to `keybindings.json` (see
  [Custom key bindings](#custom-key-bindings)).

Measured on an M-series Mac (release build): a 95 MB, 1.6M-line Python file
opens and renders highlighted in ~0.6 s.

Also: code folding (⌥⌘[ / ⌥⌘], ⌘K ⌘1…9), rulers and a fixed wrap column
(View menu), soft undo, Emacs-style mark and yank, macros, paste from
history, copy as HTML.

Look and feel: a theme-colored gradient scrollbar with match and cursor
ticks, bundled or installed fonts for the editor and the interface (with sizes),
cursor styles (line, block, underline; blink, fade, solid), window opacity,
bracket pair colors, visible whitespace and trailing-whitespace tint, color
swatches under CSS colors, sticky scroll headers, smooth scrolling, and git
change bars with the branch in the status bar (View ▸ Editor Extras toggles
each). Tools ▸ Text has JSON format/minify, Base64/URL/HTML codecs, UUID and
date; Tools ▸ On Save trims whitespace, adds a final newline, or saves on
focus loss.

Known gaps: no plugin or package system yet, and Strata quits with its last
window (macOS too). Word wrap turns off for files over about 8 million
characters. The bundled grammars come from syntect's defaults and the bat
project (via the `two-face` crate, MIT/Apache-2.0); each grammar's own license
text ships in Help ▸ Licenses.

## Color schemes and fonts

**33 curated color schemes** plus Strata Dark and Strata Light, grouped in
View ▸ Color Scheme (also under Settings ▸ Color Scheme and the status bar's Theme
menu) as Dark, Light and High Contrast. **Preferences: Color Scheme…** (in the
command palette) previews each one as you arrow through the list; Esc puts
the old one back.

| Dark | Light | High contrast |
|------|-------|---------------|
| Strata Dark, One Dark, Dracula, Nord, Solarized Dark, Gruvbox Dark, Tokyo Night, Tokyo Night Storm, Catppuccin Mocha / Macchiato / Frappé, GitHub Dark, Monokai Extended, Ayu Dark, Ayu Mirage, Night Owl, Material Palenight, Rosé Pine, Rosé Pine Moon, Everforest Dark, Kanagawa Wave | Strata Light, One Light, Solarized Light, Gruvbox Light, Tokyo Night Day, Catppuccin Latte, GitHub Light, Ayu Light, Night Owl Light, Rosé Pine Dawn, Everforest Light, Kanagawa Lotus | High Contrast Dark, High Contrast Light |

syntect's classic schemes (InspiredGitHub, base16 Ocean/Eighties/Mocha) are
under Classic. Each curated scheme is the project's published palette (MIT,
or Apache-2.0 for Material; credits in Help ▸ Licenses) mapped onto one shared
set of scope rules, so every language gets the same coverage: comments,
strings, escapes, keywords, operators, functions, types, constants, numbers,
variables, parameters, properties, punctuation, tags, attributes,
macros/decorators, Markdown headings/links/code, diffs and invalid code. Each
also sets the editor colors (caret, selection, current line, line numbers,
find matches, indent guides, bracket match, whitespace, accent) and its own
terminal palette; tabs, side bar, status bar, menus, scrollbar, minimap and
Markdown preview take their colors from the scheme. Text in every scheme
meets WCAG AA contrast (4.5:1) and every syntax color at least 3:1; the
high-contrast schemes meet 7:1 for text and 4.5:1 for every color (a test
checks all of them).

**Follow the system** (light or dark): View ▸ Color Scheme ▸ Follow System
Appearance, or in `settings.json`:

```jsonc
"theme_mode": "auto",            // "fixed" (default: uses "theme"), "auto", "dark", "light"
"dark_theme": "Tokyo Night",
"light_theme": "Catppuccin Latte",
```

In Auto mode, picking a dark scheme sets `dark_theme` and a light one sets
`light_theme`. `"theme": "auto"` works too.

**Your own colors**, without writing a scheme (applied live; names are the
roles above, plus `background`, `foreground`, `caret`, `selection`,
`line_highlight`, `gutter`, `gutter_foreground`, `find_highlight`, `guide`,
`bracket`, `whitespace`, `accent`; colors as `#rgb`, `#rrggbb`, `#rrggbbaa`,
`rgb()`, `hsl()`):

```jsonc
"theme_overrides": { "background": "#1e1e2e", "caret": "#f5e0dc", "comment": "#7f849c" },
"theme_overrides_by_theme": { "Nord": { "selection": "#4c566a" } },
"font_options": ["no_italic"],   // also "no_underline"
```

**Bring a scheme**: put `.tmTheme` or `.sublime-color-scheme`
files (variables, globals, rules with `scope`/`foreground`/`background`/
`font_style`; `var()`, `color(… alpha() blend())`; comments and trailing
commas allowed) in the config folder's `Themes/`. They appear under Your
Themes, named after the file.

**Fonts.** Four coding fonts ship with Strata, all under the SIL Open Font
License 1.1: **JetBrains Mono**, **Fira Code** (both draw their coding
ligatures, one cell per character), **IBM Plex Mono** and **Source Code Pro**,
plus **Inter** for the interface. Settings ▸ Font ▸ Editor Font / Interface
Font lists them first, then every font installed on the computer; a font that
can't be found falls back to the built-in Hack (editor) or Ubuntu (interface)
and says so in the status bar. Font ▸ Line Spacing picks 1.0–1.6× line
height; in `settings.json`:

```jsonc
"font_face": "JetBrains Mono",   // a bundled or installed font's name, or a font file path
"font_size": 14,
"ui_font_face": "Inter",
"ui_font_size": 13,
"line_height": 1.3,              // multiple of the font's own line height
"line_padding_top": 0,           // extra points above / below each line
"line_padding_bottom": 0,
"caret_width": 2,
```

Settings ▸ Settings… (⌘, / Ctrl+,) opens the Settings window, and Settings ▸
Settings (JSON) opens `settings.json`; a new file is written with every setting
at its default, the ones above included. The window's rows come from
`src/settings_meta.rs`, with descriptions taken from the manual's Settings
Reference; tests keep the two in step with `Settings`.

## Privacy

Strata has no telemetry and no accounts. Its only network connections of its
own are update checks and downloads from GitHub, and only if you allow them
(Strata asks once; the `update_check` setting) or choose Help ▸ Check for
Updates…; they send nothing but the request itself. Updates are fetched with
the system's `curl` (HTTPS only) and verified against an ed25519 key built
into the app before anything is installed; see [docs/UPDATES.md](docs/UPDATES.md).
Markdown images aren't fetched. A link you click (in
the Markdown preview, the manual, or Help ▸ Report a Problem…) opens in your
browser; only `http`, `https` and `mailto` links leave Strata (other local
files are shown in the file manager, other schemes are refused; see
`src/links.rs`). The `strata` command talks to the running app over a local socket
(a localhost-only port with a random token on Windows). The terminal and build
systems run the programs you choose, and Git integration only reads your local
repository.

## Development

Strata is written in Rust on [egui](https://github.com/emilk/egui)/eframe
(GPU rendering through wgpu), with ropey for the text buffer, syntect and
two-face for highlighting, alacritty_terminal for the terminal, pulldown-cmark
for Markdown, spellbook for spell check, encoding_rs and chardetng for
encodings and image for image previews. [`docs/ROADMAP.md`](docs/ROADMAP.md)
has what's in 1.0 and what comes next. Contributions must follow the
clean-room rules in [`docs/CLEAN_ROOM.md`](docs/CLEAN_ROOM.md).

`cargo test` runs editor-core unit tests plus headless egui tests that drive
the real widget and the whole window with simulated keys and clicks.

CI (`.github/workflows/build.yml`) runs clippy and the tests on macOS (Apple
silicon) and Windows (x64), builds on Linux, and audits dependencies; tagged
releases are built, notarized and drafted by `.github/workflows/release.yml`
(see [RELEASING.md](RELEASING.md)). [CONTRIBUTING.md](CONTRIBUTING.md) has the
details.

**Logs and crash reports** (nothing is ever uploaded):

- The log is `Logs/strata.log` in the config folder (rotated at 1 MB, three
  files kept): startup, settings/keymap/theme/syntax errors, open and save
  failures, session restore problems, IPC, terminal and build launch errors,
  panics. Paths and errors only, never file contents or the clipboard.
  `STRATA_LOG=debug` (or `debug,wgpu_core=info`) raises the level. When stderr
  is a terminal the lines are copied there too.
- A panic on the UI thread writes `Crashes/strata-crash-<UTC time>.txt`
  (version, commit from `build.rs`, OS, panic, backtrace, last 200 log lines),
  then saves every unsaved tab (with the session under hot exit, else into
  `backups.recovered-<time>/`) and exits. The next launch offers the report in
  a notice. Panics on other threads are logged and don't take the app down.
  See `src/logging.rs` and `src/crash.rs`.
- Help ▸ Report a Problem… opens a pre-filled issue in a tab to review and
  copy. The issues URL is `crash::ISSUES_URL`.

Dev aids (all pair with `STRATA_SCREENSHOT=out.ppm`, which renders the window,
saves a screenshot and exits): `STRATA_DEV_FIND=query` (+ `STRATA_DEV_FIND_REGEX=1`),
`STRATA_DEV_FIND_IN_FILES=query` (+ `STRATA_DEV_WHERE`, `STRATA_DEV_REPLACE`,
`STRATA_DEV_REPLACE_STEP=confirm|run`), `STRATA_DEV_PALETTE="p:query"`,
`"cmd:query"`, `"theme:query"` (the color scheme picker), `"settings:query"`
(the Settings window, searching), `"welcome:"` (the first-run card), `"licenses:"` or
`"fif:query"` (Find in Files on the first frame, after files given on the
command line are open), `STRATA_SCREENSHOT_TYPE='fn\t'` (types into the editor first; `\t`
presses Tab). `STRATA_DEV_PANIC=40` crashes on purpose at frame 40 (try it
with `STRATA_SCREENSHOT_TYPE` and `STRATA_SCREENSHOT_DELAY=60` to have unsaved
text when it does); `STRATA_DEV_PANIC=thread:40` panics a background thread
instead. `STRATA_CONFIG_DIR=dir` uses a separate settings/session folder (and its own
single-instance socket, so a test copy never talks to your everyday Strata;
for the .app use `open --env STRATA_CONFIG_DIR=dir -a dist/Strata.app file`,
with your installed Strata quit, since both copies share a bundle id).

Which dev aids a build has (see `src/dev.rs`): the ones that only look
(screenshots, first-frame panels, `STRATA_DEV_WINDOWS`, `STRATA_DEV_CONTEXT`)
come with the `devtools` feature, on by default; release packages should be
built with `--no-default-features` to leave them out. The ones that run or
change things (`STRATA_DEV_PANIC`, `STRATA_DEV_TERMINAL`,
`STRATA_DEV_REPLACE_STEP=run`, `STRATA_DEV_PALETTE="run:..."`) work only in
debug builds, or in a release build made with `--features dev-hooks`.

## License

Strata is released under the [MIT License](LICENSE). Bundled fonts, color
schemes, syntax definitions and dictionaries come from other open-source
projects under their own licenses; see **Help ▸ Licenses and Attribution…**
in the app.
