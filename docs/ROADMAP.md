# Roadmap

What Strata 1.0 is, and what comes after it. The user manual (Help ▸
Documentation, sources in [`manual/`](manual/)) describes everything that's
in the app today.

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

## After 1.0

These are planned, in no particular order. None of them is in 1.0.

- **Language servers (LSP)**: completions, diagnostics, hover, rename and
  go-to across files from the language's own server.
- **Plugins**: an extension API and a way to install packages.
- **Vim mode**: modal editing (normal, insert and visual modes, motions,
  operators and registers).
- **Linux as an official platform**: the code builds and mostly runs on
  Linux today, but it isn't tested, packaged or supported yet.
- **Accessibility**: screen reader support for the editor area. Keyboard-only
  use is fully supported and high-contrast color schemes are included in 1.0,
  but screen readers can't read the text in the editor yet.

## Contributing

Contributions follow the clean-room rules in [CLEAN_ROOM.md](CLEAN_ROOM.md).
