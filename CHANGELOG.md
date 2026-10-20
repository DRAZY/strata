# Changelog

All notable changes to Strata are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Strata uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.0.0] - 2026-10-20

This release adds Obsidian-compatible notes (with runnable code blocks
and optional Git sync), Review Changes, language servers, clickable file
locations and Send to Terminal in the terminal, optional AI helpers, plugins,
and markers for hidden and direction-changing characters. Everything that
runs other programs or reaches the network is off, or asks, until you turn it
on; see Security below.

### Added

- Notes: Obsidian-compatible notes on plain Markdown files. Open your notes
  folder (an Obsidian vault or any folder of Markdown files) and
  `[[wikilinks]]` work as in Obsidian, with headings, block references,
  aliases and embeds, and ordinary Markdown links to notes too. Links are
  underlined in the editor (dashed for a note that doesn't exist yet), and
  ⌘-click (Ctrl+click) or **Notes: Follow Link** opens them, offering to
  create a missing note where Obsidian would put it. Strata reads the vault's
  `.obsidian` settings but never writes them, and notes use the network
  only through Git sync, which you turn on per vault. See the new Notes page
  in the manual.
- Notes: typing `[[` completes note names, aliases, headings and block ids,
  and writes the link the way the vault's Obsidian settings ask.
- Notes: a backlinks panel under each note lists the notes linking to it,
  with the line that holds each link, and the note's tags. It's kept up to
  date as Obsidian, a sync tool or Git changes files, and counts notes open
  in tabs as they are, saved or not; `notes_backlinks` chooses when it
  shows. `#tags` and frontmatter tags are tinted, and **Notes: Find Tag…**
  lists every tag with its notes.
- Notes: **Notes: Open Daily Note**, **Notes: New Note…** and **Notes: New
  Note from Template…** follow Obsidian's Daily notes and Templates settings,
  filling in `{{date}}`, `{{time}}` and `{{title}}`.
- Notes: **Notes: Rename Note…** (also Rename… in the side bar) offers to
  update every link to the note, showing each changed line first. **Notes:
  Undo Rename** puts the links and the name back, all or nothing, and never
  overwrites a file written meanwhile.
- Notes: sync conflict copies (Syncthing, Dropbox, Nextcloud and ownCloud)
  get a notice above the note with **Compare**, **Open Copy** and **Move Copy
  to Trash…**; **Notes: Compare with Conflict Copy** does the same from the
  keyboard.
- Notes: the Markdown preview shows wikilinks you can click, tags,
  frontmatter as a block of properties, and embedded images, notes, sections
  and blocks (one level deep, images only from the vault).
- Notes: runbooks you can click through. In the Markdown preview, fenced
  `bash`, `sh`, `zsh`, `fish`, `pwsh`, `powershell`, `cmd`, `python`, `node`
  (`js`) and `ruby` blocks have a **▶ Run** button. Shell blocks go to the
  current terminal tab the way Send to Terminal sends text; language blocks
  start their REPL in a terminal tab of their own. **Notes: Run Code Block**
  does the same for the block at the cursor, and **Notes: Run Code Block and
  Capture Output** runs it in a separate process and puts what it prints in
  an `output` block under the code, as one edit Undo takes back (stopped
  after `code_block_timeout_seconds`, 60 by default). Nothing runs when a
  file opens or the preview draws. Before a file's first block runs, Strata
  shows exactly what will run and where, and asks; **Don't ask again for
  this file** is remembered in Strata's settings folder, never in the file,
  and a different file at the same path, or a change made outside Strata,
  asks again. Files outside the open folders ask every time, and a block
  with invisible or text-reordering characters never runs. See Notes ▸ Running code blocks in the manual.
- Notes: optional Git sync, off until you turn it on for a vault with
  **Notes: Enable Git Sync for This Vault…** (which says first that the
  repository's hooks, filters and credential helpers will run). **Notes: Sync
  Notes** saves the vault's notes, commits what changed in the vault
  (respecting `.gitignore`, with a message from `notes_sync_message`),
  rebases onto the remote and pushes, never forced; an optional timer does the
  same every few minutes and skips a vault with unsaved notes. Strata never
  asks for a password: Git uses your credential helper or SSH agent. When a
  note changed on both sides, nothing is pulled or pushed and your files stay
  as they were; the notes in conflict are listed with **Compare with
  origin/main** and **Keep My Versions and Sync**. No upstream, offline, a
  refused sign-in or a replaced remote history change nothing and say so in
  the status bar, which also shows the last sync. Turning sync on is tied to
  that repository: a different one later at the same path is asked about
  again before anything runs. Before Strata ever undoes a rebase Git
  started, it backs up every file it touched (on disk and staged) to
  `sync-backups` in its settings folder, kept 30 days (**Notes: Show Sync
  Backups**); a sync interrupted by quitting is only undone when you say so.
  See Notes ▸ Git sync in the manual.
- Optional AI helpers with a model you choose (Tools ▸ AI and the command
  palette): **Summarize Changes** (the whole review or one file), **Explain
  Change**, **Flag Risks**, **Draft Commit Message** and **Explain
  Selection**. Providers: any OpenAI-compatible server (Ollama and LM Studio
  on your computer or network, OpenRouter, OpenAI…), the Anthropic API, or
  the `claude` and `codex` command-line tools with your own login. Answers
  stream into a panel as Markdown, can be cancelled, copied or opened in a new
  tab, and never hold up the editor. See the new AI Helpers page in the
  manual.
- AI helpers are off until you turn them on in Settings ▸ AI Helpers
  (`ai_enabled`). Local providers are used without asking. Before the first
  request to any other provider, Strata shows exactly what it will send and
  to which host, and asks (**Allow Once**, **Always for This Provider**,
  **Cancel**); **AI: Forget Allowed Providers** asks again. **Test
  Connection** checks a provider without sending any of your text.
- AI helpers send only what they're about. Files that look like secrets
  (`.env`, keys, anything in folders like `.ssh` or `secrets`), files Git
  ignores, binary files and symbolic links are never sent, and a size limit
  (`ai_max_prompt_kb`) says what was left out. API keys live in the macOS
  Keychain or Windows Credential Manager, are only sent to the address they
  were saved for, and never touch `settings.json` or the log.
- Plugins: extensions that are separate programs, in any language, which
  Strata starts and talks to with JSON-RPC over their standard input and
  output (the framing language servers use). A plugin can add commands to
  the command palette and Tools ▸ Plugins, keys (never one Strata already
  uses), status bar items (for every window, or for the file in front of
  each), a Markdown panel and pick lists; read and edit open files (one
  Undo takes back each edit); and read and write files in the side bar
  folders. A plugin that crashes is restarted, and left stopped
  after three crashes in five minutes; one that floods Strata with messages
  or stops answering can't hold up the editor. Plugin settings go in
  `plugins` in settings.json. See the new Plugins page in the manual, with
  the full API reference; two small examples are in `examples/plugins`.
- Plugins are installed from a folder or a zip (Tools ▸ Plugins ▸ **Install
  from Folder…** / **Install from Zip…**) into Strata's settings folder;
  there's no online directory. Zips with paths that would leave the package,
  links, or sizes over the limits are refused. **Plugins: Manage Plugins…**
  lists them, turns them on and off, starts and stops them, shows what they
  may do and their log, and removes them.
- A plugin asks before it first runs: Strata lists the permissions it wants
  in plain words and remembers your answer for that exact package (by its
  SHA-256), so any change to it asks again; it then runs from a read-only
  copy of exactly that package, checked again right before it starts, with
  the environment variables that load code from elsewhere (`PYTHONPATH`,
  `NODE_OPTIONS`, `DYLD_*`…) removed. Every request a plugin makes is
  checked against what you allowed, and its file access stays inside the
  open folders, never following links or reaching hard-linked files. Text
  from plugins shows hidden and direction-changing characters as markers.
  Strata can't sandbox a plugin's own program, and the question says so:
  running other programs and using the network are shown as things Strata
  can't limit, and an interpreter's own libraries aren't part of the check.
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
- Invisible and direction-changing characters (such as U+202E RIGHT-TO-LEFT
  OVERRIDE, zero-width spaces and soft hyphens) are shown as small labeled
  boxes in the editor, Compare and Review Changes, so a file can't make code
  read differently from what it is ("Trojan Source"). Each box is one
  character for the caret, selection and copying, and the file is never
  changed; rest the pointer on it to see what it is. The status bar counts
  them, and **Find: Find Hidden Characters** steps through them.
  `show_hidden_characters` (View ▸ Editor Extras ▸ Hidden Characters) turns
  the boxes off. The Markdown preview, help, language server hovers and
  completions, tab titles, the side bar, the palette, Find in Files results,
  the status bar and AI answers show them as named markers too, and a
  completion holding one is marked with ⚠.

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
  sealed with a private key only this installation has (an undo is refused
  if the key file isn't private) and is flushed to disk at every step; Put Back only acts on its own sealed records, never outside the
  folder it was in, and only after checking the file is the one it set aside.

### Fixed

- Compare: Japanese, Chinese and Korean text loads the same fonts as the
  editor, so Compare tabs no longer show boxes in place of those characters.

### Security

- Nothing new runs or connects by itself. Language servers start only in
  folders you trust; code blocks run only when you ask, after Strata shows
  exactly what will run; Git sync is off until you turn it on for a vault;
  plugins ask before their first run; AI helpers are off until you turn them
  on and ask before sending text off your computer or network.
- Hidden and direction-changing characters can't disguise text: they're
  shown as markers on every surface, and a code block holding one (or
  another control character) never runs.
- Code blocks: "Don't ask again" is tied to the file's real path, its
  identity on disk and a SHA-256 of its bytes, so a replaced or changed file
  asks again. A captured block runs in its own process group (a job object
  on Windows, started suspended so nothing gets out first), and what it
  started is stopped when it ends or times out.
- Git sync: runs only in the repository it was turned on for (its Git
  folder, remote and first commit, checked before any Git runs), never
  forces a push, never prompts for credentials, allows only SSH, HTTPS and
  local remotes, and redacts tokens from error text. It undoes only a rebase
  it started itself (a record sealed with this installation's key), only
  after backing up every file the rebase touched, and fails closed. A Git
  that runs too long is stopped with its hooks. The read-only Git used for
  the gutter, Compare and Review Changes no longer inherits `GIT_DIR` and
  similar variables.
- Plugins: consent is for an exact package (SHA-256); each start runs a
  read-only copy of it that's checked again just before starting. Plugins
  start without environment variables that load code from elsewhere, every
  request is checked against the permissions you gave, file access stays in
  the open folders and refuses links and hard-linked files, and zips with
  unsafe paths, links or oversized contents are refused. On Windows,
  language servers and plugins start suspended and run only once they're in
  a job object.
- The temporary file used when creating the private undo key has a random
  name, so two threads can't remove each other's file.

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

[Unreleased]: https://github.com/DRAZY/strata/compare/v2.0.0...HEAD
[2.0.0]: https://github.com/DRAZY/strata/compare/v1.0.2...v2.0.0
[1.0.2]: https://github.com/DRAZY/strata/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/DRAZY/strata/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/DRAZY/strata/compare/v1.0.0-rc.1...v1.0.0
[1.0.0-rc.1]: https://github.com/DRAZY/strata/releases/tag/v1.0.0-rc.1
