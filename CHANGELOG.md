# Changelog

All notable changes to Strata are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Strata uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Language servers. When a language's server is installed (rust-analyzer,
  typescript-language-server, pyright, gopls or clangd), Strata starts it in
  the background and shows its problems as wavy underlines with a dot in the
  gutter, puts its suggestions first in the completion list, and shows type
  and documentation when the mouse pointer rests on a name. The status bar
  shows the server for the current file and its error and warning counts; a
  server that stops is restarted after a pause. Strata never downloads or
  installs servers. See Languages and Syntaxes ▸ Language servers in the
  manual.
- Language server commands, in Tools ▸ Language Server and the command
  palette: **LSP: Rename Symbol…** (across files, one Undo step per file),
  **LSP: Format Document**, **LSP: Show Hover Info**, **LSP: Show Parameter
  Hints**, **LSP: Show Problems…**, **LSP: Next Problem** / **Previous
  Problem** (⌘K ⌘N / ⌘K ⌘P; Ctrl+K Ctrl+N / Ctrl+K Ctrl+P elsewhere) and
  **LSP: Restart Language Servers**.
- Language servers start only in project folders you trust. The first time,
  Strata asks (**Start**, **Always for This Folder**, **Not Now**);
  `lsp_trust_all` skips the question. `lsp_servers` in settings.json changes
  the built-in servers or adds others, and `lsp_enabled` turns the feature
  off.
- Optional AI helpers with a model you choose (Tools ▸ AI): **Summarize
  Changes** (the whole review or one file), **Explain Change**, **Flag Risks**,
  **Draft Commit Message** and **Explain Selection**. Providers: any
  OpenAI-compatible server (Ollama and LM Studio on your computer or network,
  OpenRouter, OpenAI…), the Anthropic API, or the `claude` and `codex`
  command-line tools with your own login. Off until you turn them on in
  Settings ▸ AI Helpers. Before the first request to a provider outside your
  computer or local network, Strata shows exactly what it will send and to
  which host, and asks (Allow Once, Always for This Provider, Cancel).
  Answers stream into a panel as Markdown, can be cancelled, and never hold
  up the editor. API keys live in the macOS Keychain or Windows Credential
  Manager, are only sent to the address they were saved for, and never touch
  `settings.json` or the log. Ignored, binary and secret-looking files (and
  anything in folders like `.ssh` or `secrets`) are never sent, and a
  size limit (`ai_max_prompt_kb`) says what was left out. See the manual's
  AI Helpers page.
- Terminal: ⌘-click (Ctrl+click on Windows and Linux) a file location in the
  output to open the file at that line and column. Hold the key and point to
  see the location underlined. Works with `path:line:col`, `path(line,col)`
  (MSVC, C#, TypeScript), Rust errors and panics, Python tracebacks and Node
  stack traces, including locations that wrap onto the next row. Relative
  paths are found in the shell's current folder, then the side bar folders;
  only files that exist are links.
- **Send to Terminal** (Tools menu; ⌃↩ on macOS, Ctrl+Alt+Enter elsewhere)
  sends the selection, or the current line, to the terminal and presses
  Enter, starting a terminal if none is running. Multi-line text goes as one
  bracketed paste when the shell or REPL supports it.
- Review Changes, for checking what a coding agent changed and keeping or
  undoing each change (no AI involved). The **Changes** tab of the side bar
  lists every file that differs from the last commit, or from a snapshot taken
  with **Start Review**, with its status (modified, new, deleted, renamed) and
  lines added and removed, and updates while the agent works. Clicking a file
  opens its changes side by side with **Keep** and **Revert** for each change
  or the whole file; reverting is an ordinary edit, so Undo brings the change
  back and nothing is written until you save. Tick files off as reviewed; the
  status bar counts the rest. F8 and Shift+F8 step through the changes across
  files, and the Review commands are in Tools ▸ Review Changes and the command
  palette. See the new Review Changes page in the manual.
- Terminal: ⌘-click (Ctrl+click on Windows and Linux) a file location in the
  output to open the file at that line and column. Hold the key and point to
  see the location underlined. Works with `path:line:col`, `path(line,col)`
  (MSVC, C#, TypeScript), Rust errors and panics, Python tracebacks and Node
  stack traces, including locations that wrap onto the next row. Relative
  paths are found in the shell's current folder, then the side bar folders;
  only files that exist are links.
- **Send to Terminal** (Tools menu; ⌃↩ on macOS, Ctrl+Alt+Enter elsewhere)
  sends the selection, or the current line, to the terminal and presses
  Enter, starting a terminal if none is running. Multi-line text goes as one
  bracketed paste when the shell or REPL supports it.
- **Notes**: Obsidian-compatible notes on plain Markdown files. Open your
  vault folder and `[[wikilinks]]` work as in Obsidian (headings, block
  references, aliases, embeds, Markdown links too): they're underlined in the
  editor and ⌘-click (Ctrl+click) follows them, offering to create a note
  that doesn't exist yet. Typing `[[` completes note names, aliases, headings
  and block ids. A backlinks panel under each note lists the notes linking to
  it, kept up to date as Obsidian or a sync tool changes files. `#tags` (and
  frontmatter tags) are tinted and searchable with **Notes: Find Tag…**.
  **Notes: Open Daily Note** and **Notes: New Note from Template…** follow
  Obsidian's Daily notes and Templates settings. Renaming a note offers to
  update every link to it, showing each change first; **Notes: Undo Rename**
  puts the links and the name back, all or nothing. The Markdown preview shows wikilinks,
  tags, frontmatter and embedded images and notes. Sync conflict copies
  (Syncthing, Dropbox, Nextcloud) get a notice with a Compare button. Strata
  reads the vault's `.obsidian` settings but never writes them. See
  Help ▸ Documentation ▸ Notes.

### Changed

- Go to Definition and Go to Reference ask the file's language server first,
  across the whole project, and fall back to Strata's own symbol search when
  there's no server or it has no answer.
- The completion list stays inside the window; long details are cut with "…".
- Undo Replace in Files is all or nothing: if any rewritten file changed
  since the replace, nothing is restored and those files are listed. It never
  overwrites a file written while it runs (by a sync tool, say): what it
  couldn't put back stays in a marked folder next to the file. Files are
  restored in place (keeping permissions, owner, extended attributes and hard
  links), also through links inside the replaced folders. An undo interrupted
  by a crash is listed in the Changes tab with **Put Back**. Its record is
  sealed with a key only this installation has and is flushed to disk at
  every step; Put Back only acts on its own sealed records, never outside the
  folder it was in, and only after checking the file is the one it set aside.

### Fixed

- Compare: Japanese, Chinese and Korean text loads the same fonts as the
  editor, so Compare tabs no longer show boxes in place of those characters.

## [1.0.2] - 2026-10-15

### Changed

- Update checks: with automatic updates on, Strata checks every time it opens,
  then every 4 hours while it stays open. Choose 1, 2, 4, 6 or 12 hours in
  Settings ▸ Help (`update_check_interval_hours`), or **Manually only** to check
  just from Help ▸ Check for Updates…. A new version appears as a small notice
  with **Install and Relaunch**, **Later** and **Skip This Version**.

- Easier-to-read interface text. The default interface size is now 14 pt (was
  13); settings.json files that set `ui_font_size` keep their size.
  Descriptions, hints and shortcuts are a point smaller than the interface
  size (they were about three-quarters of it) and never under 12 pt, and the
  Settings window's descriptions and `settings.json` keys are larger and
  darker. Secondary text now meets WCAG AA contrast (4.5:1) in every bundled
  color scheme, and a test keeps it that way.
- **View ▸ Interface Size** offers Small (12 pt), Default (14 pt), Large
  (16 pt) and Larger (18 pt); the Settings window and the welcome card offer
  the same four. The Settings window opens larger to fit bigger text, and the
  manual's contents, the command palette, the update notice and Compare's
  header grow with the interface size.

## [1.0.1] - 2026-10-15

### Changed

- Update checks: with automatic checks on, Strata now looks when it opens (if
  the last check was over an hour ago) and every four hours while it stays
  open, so a new release shows up during a working session. It's still a small
  notice in the corner, never a pop-up.

### Fixed

- Windows: crash reports and Help ▸ Report a Problem hide your user folder
  (`C:\Users\<name>`) like they do on macOS.
- Windows: Copy Relative Path uses `\` separators.
- Closing a window right after opening its terminal no longer leaves the
  shell running in the background.
- Updated the cryptography libraries that check update signatures
  (ed25519-dalek 3, sha2 0.11).

## [1.0.0] - 2026-10-15

Strata 1.0, the first stable release: a fast, open-source code and text editor
for macOS and Windows. It contains everything in 1.0.0-rc.1 below, tested on
macOS and on Windows.

### Changed since 1.0.0-rc.1

- The Windows build has been tried on real hardware.
- Release builds from CI refuse to publish unless they can be signed.

### Highlights

- Multiple cursors, Goto Anything, the command palette, and find and replace
  across whole projects with undo.
- Split panes, multiple windows, several views of one file, and projects with
  several folders.
- A built-in terminal, live Markdown preview, side-by-side compare, build
  systems, Git change bars and permalinks, spell check, printing and PDF.
- About 190 languages, 35 color schemes, bundled coding fonts, EditorConfig
  and per-language settings, and a Settings window.
- Hot exit, atomic saves, crash recovery and fast large-file handling.
- Signed, opt-in updates from inside the app (Help ▸ Check for Updates…).

## [1.0.0-rc.1] - 2026-10-15

The first public release candidate. Everything below is new.

### Editing

- Multiple cursors and box (column) selection, Quick Add Next, Find All as
  cursors, split selection into lines, and soft undo for cursor moves.
- Auto-closing brackets, smart indentation, comment toggling, line moves,
  duplicate, join, transpose, case conversion, sort/shuffle/unique lines, wrap
  paragraph at the ruler, paste and indent, paste from history.
- Word completions while typing, snippets in the `.sublime-snippet` format
  (fields, mirrors, variables, transforms) with built-ins for common
  languages.
- Code folding, bookmarks, Emacs-style mark and yank, macros that you can save
  and bind to keys.
- Spell check for comments, strings and prose with bundled US and UK English
  dictionaries, suggestions and a personal word list.
- Text tools: JSON format and minify, Base64, URL and HTML encoding, UUIDs and
  dates.

### Navigation & Search

- Goto Anything for files, lines (`:42`), symbols (`@name`) and text
  (`#text`), and a command palette that lists every command with its shortcut.
- Go to line, go to symbol, go to definition, and jump back and forward.
- Find and replace with regular expressions, whole word, In Selection and
  Preserve Case.
- Find in Files and Replace in Files across whole folders, respecting
  `.gitignore`, with include and exclude patterns and an undo for the whole
  replacement.
- Search history: ↑ and ↓ in the find, replace and Find in Files fields.

### Layout & Windows

- Tabs with preview tabs and drag to reorder; up to four split panes in
  columns, rows or a grid.
- Several views of one file, each with its own cursors, scroll and folds.
- Multiple windows, each with its own tabs, panes, folders and terminal.
- Projects: one or more folders in the side bar, saved as `.strata-project`
  files.
- Distraction-free mode and full screen.
- Hot exit: quit at any time; every window, tab and unsaved change comes back.

### Built-in tools

- Terminal panel with tabs, true color and the color scheme's palette.
- Live Markdown preview with GitHub-style tables, task lists, alerts,
  footnotes and highlighted code, plus export to HTML.
- Compare two files, a file with another tab, or a file with its last Git
  commit, side by side and still editable.
- Build systems (Cargo, Python, Node, Go, C/C++, Make and more) with clickable
  errors, plus your own as JSON.
- Git change bars in the gutter and the current branch in the status bar;
  Copy Permalink (GitHub, GitLab, Bitbucket) and Copy Relative Path.
- Printing and PDF export with syntax colors.
- A built-in manual (Help ▸ Documentation) with search, and a generated
  Keyboard Shortcuts page.
- Built-in updates (opt-in): Strata can tell you about new versions and
  install them; Help ▸ Check for Updates…. It asks on first launch.

### Appearance

- 35 color schemes (Strata Dark and Light, One Dark, Dracula, Nord,
  Solarized, Gruvbox, Tokyo Night, Catppuccin, Rosé Pine and more) that can
  follow the system's light or dark mode, each checked for WCAG AA contrast,
  plus two high-contrast schemes.
- Bring your own `.tmTheme` and `.sublime-color-scheme` files, or override
  single colors in `settings.json`.
- Bundled coding fonts (JetBrains Mono, Fira Code, IBM Plex Mono, Source Code
  Pro) with ligatures, and Inter for the interface.
- Minimap, indent guides, bracket pair colors, sticky scroll, rulers, color
  swatches, visible whitespace and cursor styles.
- A Settings window with search, applied live, and a short welcome card on the
  first launch.

### Languages

- About 190 languages highlighted, from syntect's defaults and bat's
  collection, plus Strata's own PowerShell grammar.
- Symbols, smart indentation and comment toggling for many of them.
- Add your own `.sublime-syntax` files.
- EditorConfig support and per-language settings (`syntax_settings`).

### Files & Safety

- Large files: a 95 MB, 1.6 million-line file opens highlighted in about
  0.6 s on an M-series Mac; minified one-line files stay responsive.
- 40 encodings, detected on open (including legacy ones such as Shift_JIS,
  GBK and KOI8), with Reopen with Encoding and Save with Encoding; LF and CRLF
  line endings.
- Optional auto save after a pause in typing.
- Images open in a preview, and other binary files in a read-only hex view.
- Atomic saves that keep symlinks and permissions and refuse text the chosen
  encoding can't hold.
- Files changed or deleted on disk are noticed and reloaded or flagged.
- Logs and crash reports stay on your computer; after a crash every unsaved
  tab is kept and the next launch offers the report.
- No telemetry. Strata contacts GitHub only to check for and download updates,
  and only if you allow it.

### Platform

- macOS 11 or later, as a universal (Apple silicon and Intel) or Apple
  silicon DMG, signed with a Developer ID and notarized; native menu bar,
  Finder Open With and Dock drops, recent files in the Dock menu, and a
  `strata` shell command.
- Windows (x64), as a portable zip with the app icon and version
  details embedded in `Strata.exe`.
- The `strata` command opens files at a line and column, hands them to a
  running Strata, and can wait for tabs to close (`-w`, for `$EDITOR` and Git).

### Known issues

- The Windows build has not yet been tested on real hardware, and
  `Strata.exe` is not code-signed, so SmartScreen warns on first run (More
  info ▸ Run anyway).
- No screen-reader support in the editor area yet.
- Linux builds compile but are not officially supported or packaged.
- Coding ligatures draw one cell per character, and complex scripts (Arabic,
  Indic and other shaped scripts) and right-to-left text are not fully laid
  out.

[Unreleased]: https://github.com/DRAZY/strata/compare/v1.0.2...HEAD
[1.0.2]: https://github.com/DRAZY/strata/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/DRAZY/strata/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/DRAZY/strata/compare/v1.0.0-rc.1...v1.0.0
[1.0.0-rc.1]: https://github.com/DRAZY/strata/releases/tag/v1.0.0-rc.1
