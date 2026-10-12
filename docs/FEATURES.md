# Feature spec & roadmap

Reference point: Sublime Text 4 (build 4215), observed behavior only.

## M0 — It edits text
- [x] Rope buffer, open/save, line-ending detection (UTF-16 still todo)
- [x] GPU text rendering, smooth scrolling, line numbers
- [x] Undo/redo (soft-undo of cursor moves still todo)
- [ ] Find / replace with regex, incremental highlight

## M1 — The signature features
- [~] Multiple cursors: Cmd/Ctrl+click done; Cmd+D, column select todo
- [ ] Goto Anything (fuzzy file open, `@` symbol, `:` line, `#` term)
- [ ] Command palette (fuzzy over all commands)
- [ ] Split panes / layouts, tabs, minimap
- [x] Syntax highlighting via `.sublime-syntax` (syntect, incremental)
- [ ] Color schemes via `.tmTheme`; light/dark auto-switch

## M2 — Project scale
- [ ] Sidebar folder tree, projects & workspaces
- [ ] Find in files (ripgrep-class speed)
- [ ] Hot exit: restore unsaved buffers across restarts
- [ ] Git status gutter + sidebar badges
- [ ] Indexing: Goto Definition / symbol index

## M3 — Ecosystem
- [ ] Plugin API (Lua or WASM) + package manager
- [ ] JSON settings with per-syntax overrides; keymap files
- [ ] Vim mode (equivalent of "Vintage")
- [ ] LSP client
- [ ] CLI launcher (`strata file.txt`, `strata -w` wait mode)

## Language coverage target (from the reference app's bundled set)
ActionScript, AppleScript, ASP, Batch, C#, C/C++, Clojure, CSS, D, Diff, Erlang,
Git formats, Go, Graphviz, Groovy, Haskell, HTML, Java, JavaScript/JSX, JSON,
LaTeX, Lisp, Lua, Makefile, Markdown, Matlab, Objective-C, OCaml, Pascal, Perl,
PHP, Python, R, Rails/Ruby, Regex, reStructuredText, Rust, Scala, Shell, SQL,
TCL, Textile, TOML, XML, YAML.

Grammars must come from openly licensed sources (e.g. community
`sublimehq/Packages` is MIT-style — verify license before vendoring).
