# Feature spec & roadmap

Reference point: Sublime Text 4 (build 4215), observed behavior only.

## M0 — It edits text
- [x] Rope buffer, open/save, line-ending detection (UTF-16 still todo)
- [x] GPU text rendering, smooth scrolling, line numbers
- [x] Undo/redo (soft-undo of cursor moves still todo)
- [x] Find / replace with regex, incremental highlight

## M1 — The signature features
- [x] IME / CJK input with inline composition
- [x] Multiple cursors: Cmd/Ctrl+click, Cmd+D, select all occurrences, add above/below, split into lines, column select
- [x] Goto Anything (fuzzy file open, `@` symbol, `:` line; `#` term todo)
- [x] Command palette (fuzzy over all commands)
- [x] Tabs (preview tabs, reorder, reopen closed), minimap
- [ ] Split panes / layouts
- [x] Syntax highlighting via `.sublime-syntax` (syntect, incremental)
- [x] Color schemes via `.tmTheme` (Strata Dark/Light + bundled); light/dark auto-switch todo
- [x] Editing commands: comments, indent, line moves, duplicate, join, sort, case, brackets, auto-pairs

## M2 — Project scale
- [x] Sidebar folder tree (projects & workspaces todo)
- [x] Find in files (ripgrep's `ignore` walker, parallel)
- [x] Hot exit: restore unsaved buffers across restarts
- [x] Reload files changed on disk (flag conflicts with unsaved edits)
- [ ] Git status gutter + sidebar badges
- [ ] Indexing: Goto Definition / symbol index

## M3 — Ecosystem
- [ ] Plugin API (Lua or WASM) + package manager
- [x] JSON settings (per-syntax overrides and keymap files todo)
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
