# Strata

A fast, open-source, keyboard-first text editor in the spirit of Sublime Text.

> Status: pre-alpha scaffold. Icon: `assets/icon.svg`.

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
cargo run --release -- path/to/file.rs
```

Shortcuts (Cmd on macOS, Ctrl on Windows): **O** open, **S** save,
**Shift+S** save as, **N** new, **+ / −** font size.

Multi-cursor & search:

| Action | macOS | Windows |
|---|---|---|
| Add next occurrence | Cmd+D | Ctrl+D |
| Select all occurrences | Ctrl+Cmd+G | Alt+F3 |
| Add cursor | Cmd+click | Ctrl+click |
| Back to one cursor | Esc | Esc |
| Find / Replace | Cmd+F / Cmd+Opt+F | Ctrl+F / Ctrl+H |
| Next / previous match | Cmd+G / Shift+Cmd+G | F3 / Shift+F3 |
| In the find bar | Enter next, Shift+Enter previous, Alt+Enter Find All (a cursor per match) | same |
| In the replace field | Enter replace, Cmd+Enter replace all | Ctrl+Enter replace all |

Find options: regex (`.*`, with `$1` groups in replacements), case-sensitive
(`Aa`), whole word. Replace All is a single undo step.

CI (`.github/workflows/build.yml`) builds release binaries on an Apple silicon
macOS runner and a Windows x64 runner.

Dev aid: `STRATA_SCREENSHOT=out.ppm cargo run -- file` renders the window,
saves a screenshot, and exits. Add `STRATA_DEV_FIND=query` (and
`STRATA_DEV_FIND_REGEX=1`) to capture it with the find/replace bar open.

## Test build for macOS (Apple silicon)

```sh
scripts/package-macos.sh   # → dist/Strata-<version>-arm64.dmg
```

The app is ad-hoc signed, not notarized. On the Mac that built it, it opens
normally. On another Mac, the first launch is blocked by Gatekeeper: right-click
Strata.app → Open, or run `xattr -dr com.apple.quarantine /Applications/Strata.app`.
Opening files by double-clicking in Finder isn't wired up yet; use Cmd+O.

## Current state (M0)

Working: rope buffer (`ropey`) with a custom editor widget that paints only
visible lines; incremental syntax highlighting (checkpointed parser state,
per-frame budget); multiple cursors (Cmd/Ctrl+click, Esc to collapse) with
every command applied to all of them; undo/redo with typing coalesced into one
step; Sublime-style copy/cut of the whole line when nothing is selected; smart
Home; word jumps (Option on macOS, Ctrl on Windows); CRLF files stay CRLF;
draggable scrollbar; open/save; Cmd+D / select-all-occurrences; find/replace
bar with regex, case and whole-word options, live match outlines and counts.
Searching re-scans the whole file after each edit while the bar is open, so
very large files (~100 MB) will feel it.

Measured on an M-series Mac (release build): a 95 MB, 1.6M-line Python file
opens and renders highlighted in ~0.6 s.

Known gaps: no IME (CJK/emoji-picker input), no find-in-files, no word wrap,
Tab with a selection replaces it instead of indenting, no Shift+Tab unindent,
no caret blink. The bundled grammars come from syntect's defaults; their
licensing must be verified before a public release.

Tests: `cargo test` runs editor-core unit tests plus headless egui tests that
drive the real widget with simulated keys and clicks.
