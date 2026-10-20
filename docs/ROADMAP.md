# Roadmap

What Strata 1.0 is, what's on `main` for the next release, and what comes
after that. The user manual (Help ▸ Documentation, sources in
[`manual/`](manual/)) describes everything that's in the app today.

## In 1.0

- **Editing**: multiple cursors and box selection, smart indentation,
  auto-closing brackets, word completion, snippets, code folding, macros
  (recorded, saved and bound to keys), bookmarks, marks, case conversion,
  line and selection permutations, spell check.
- **Navigation and search**: Goto Anything, the command palette, Go to
  Definition and Reference, jump history; the find bar with regex, In
  Selection, Preserve Case and search history; Find and Replace in Files with
  undo.
- **Layout**: tabs, up to four panes, several windows, views of one file,
  distraction-free mode, projects with several folders.
- **Built-in tools**: a terminal, build systems with clickable errors, a live
  Markdown preview with HTML export, side-by-side Compare (also against Git
  HEAD), printing and PDF export.
- **Files**: hot exit, auto save (opt-in), atomic saves, 40 encodings with
  detection of legacy ones, large-file mode, an image preview and a read-only
  hex view for binary files, Git change bars and permalinks.
- **Customizing**: about 190 languages, 35 color schemes with light/dark
  following, bundled fonts, JSON settings and key bindings that apply on save,
  user syntaxes, color schemes and snippets in widely shared formats.
- **Platforms**: macOS (Apple silicon and Intel, signed and notarized) and
  Windows on x64.
- **Updates**: opt-in, signed in-app updates from GitHub Releases.

## Next release (on `main`)

These are finished and merged, and will ship together in the next release.
They aren't in 1.0.x.

- **Terminal links**: ⌘-click (Ctrl+click) a file location in the terminal's
  output, such as a compiler error, a panic or a stack trace, to open the
  file at that line and column. **Send to Terminal** sends the selection or
  the current line to the shell or REPL.
- **Review Changes**: the side bar's Changes tab lists every file that
  differs from the last commit (or from a snapshot taken with Start Review),
  and each file opens side by side with Keep and Revert for every change, for
  checking what a coding agent did. No AI is involved.
- **Language servers (LSP)**: problems, completions, hover information, Go to
  Definition and Reference, rename across files, formatting and parameter
  hints from the language's own server (rust-analyzer,
  typescript-language-server, pyright, gopls, clangd, or any other set in
  `lsp_servers`). Servers start only in project folders you trust, and Strata
  never downloads them.
- **Notes**: Obsidian-compatible notes on plain Markdown files. `[[wikilinks]]`
  (with headings, block references, aliases and embeds) that ⌘-click
  (Ctrl+click) follows, completion as you type `[[`, a backlinks panel,
  `#tags`, daily notes and templates that follow the vault's Obsidian
  settings, renaming a note with its links updated (and an all-or-nothing
  Undo Rename), and notices for sync conflict copies. The Markdown preview
  shows wikilinks, tags, properties and embeds. Strata reads `.obsidian` but
  never writes it, and notes never use the network.
- **Optional AI helpers** ([#10](https://github.com/DRAZY/strata/issues/10)):
  summarize a review, explain a change or a selection, flag risky edits and
  draft a commit message, with a model you choose: Ollama or LM Studio on your
  computer or network, an OpenAI-compatible or Anthropic API, or the `claude`
  and `codex` command-line tools. Off until you turn them on; nothing goes to
  a provider outside your computer or network until you've seen exactly what
  will be sent and said yes. Keys are kept in the system keychain.
- **Plugins** ([#13](https://github.com/DRAZY/strata/issues/13)): extensions
  that are separate programs, in any language, talking to Strata over
  JSON-RPC like language servers. They add commands, keys, status bar items,
  panels and pick lists, and can read and edit open files and the files in
  the open folders, but only with the permissions you give them when they
  first run (asked again whenever the package changes). Install them from a
  folder or a zip; a crashing plugin can't take Strata with it.

## In progress

Being worked on, but not finished or merged yet. What they include may change
before they ship.

- **Runnable code blocks** ([#11](https://github.com/DRAZY/strata/issues/11)):
  run a fenced code block in a note and see its output.
- **Git sync for notes** ([#12](https://github.com/DRAZY/strata/issues/12)):
  keep a notes vault in step with a Git remote.

## After that

These are planned, in no particular order.

- **Vim mode**: modal editing (normal, insert and visual modes, motions,
  operators and registers).
- **Linux as an official platform**: the code builds and mostly runs on
  Linux today, but it isn't tested, packaged or supported yet.
- **Accessibility**: screen reader support for the editor area. Keyboard-only
  use is fully supported and high-contrast color schemes are included in 1.0,
  but screen readers can't read the text in the editor yet.

## Contributing

Contributions must be original work; see [CONTRIBUTING.md](../CONTRIBUTING.md).
