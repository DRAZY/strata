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

CI (`.github/workflows/build.yml`) builds release binaries on an Apple silicon
macOS runner and a Windows x64 runner.

Dev aid: `STRATA_SCREENSHOT=out.ppm cargo run -- file` renders the window,
saves a screenshot, and exits.

## Test build for macOS (Apple silicon)

```sh
scripts/package-macos.sh   # → dist/Strata-<version>-arm64.dmg
```

The app is ad-hoc signed, not notarized. On the Mac that built it, it opens
normally. On another Mac, the first launch is blocked by Gatekeeper: right-click
Strata.app → Open, or run `xattr -dr com.apple.quarantine /Applications/Strata.app`.
Opening files by double-clicking in Finder isn't wired up yet; use Cmd+O.

## Current state (M0, first build)

Working: window, open/save, syntax highlighting for ~50 languages, line
numbers, cursor position, dirty marker in the title.

Known gaps: the buffer is a plain `String` inside egui's `TextEdit` (the
`ropey` buffer comes with the custom editor widget); no multiple cursors yet;
cursor position is recomputed each frame, so very large files will lag. The
bundled grammars come from syntect's defaults; their licensing must be
verified before a public release.
