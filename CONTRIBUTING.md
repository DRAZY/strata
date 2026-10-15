# Contributing to Strata

Thanks for helping. Bug reports, fixes, documentation and new features are all
welcome. For anything bigger than a small fix, please open an issue first so we
can agree on the approach before you spend time on it.

## Clean-room policy

Strata is a clean-room editor. Before contributing, read
[`docs/CLEAN_ROOM.md`](docs/CLEAN_ROOM.md): no decompiled or disassembled code,
no files copied out of other editors' app bundles, and no third-party branding.
Openly licensed grammars, color schemes and fonts are fine when their license
allows it and they are credited in Help ▸ Licenses.

## Building and testing

You need Rust stable (install it from <https://rustup.rs>). On Linux, also
install the development packages listed in `.github/workflows/build.yml`.

```sh
cargo build                                    # debug build
cargo run -- path/to/file.rs                   # or a folder
cargo test                                     # unit tests and headless UI tests
cargo clippy --all-targets -- -D warnings      # must be clean
```

On macOS, also check that the Windows build still compiles (install the target
once with `rustup target add x86_64-pc-windows-msvc`):

```sh
cargo clippy --target x86_64-pc-windows-msvc -- -D warnings
```

To build a Windows zip locally from a Mac (with `brew install mingw-w64` and
`rustup target add x86_64-pc-windows-gnu`), run `scripts/package-windows.sh`.
`scripts/package-macos.sh` builds `Strata.app` and a DMG.

The repository is not formatted with `rustfmt`; please match the style of the
surrounding code instead of reformatting files.

### Tests that enforce project rules

`cargo test` also checks things that are easy to forget:

- **Dependencies:** every crate in `Cargo.toml` needs a credit in
  `src/app/more.rs` (`LICENSES`), and `assets/third-party-licenses.tsv` must
  match `Cargo.lock`. After changing dependencies, run
  `scripts/third-party-licenses.sh`.
- **The manual:** every page in `docs/manual/` must parse, every link and
  anchor must resolve, every setting must be documented and every command must
  appear on the shortcuts page.
- **Color schemes:** every scheme must meet the contrast requirements.

## Coding conventions

- Keep changes focused; one topic per pull request.
- Prefer the existing helpers and patterns in the module you're changing.
- Comments explain why, not what. Doc comments on public items are short and
  concrete.
- User-facing text: sentence case, plain words, "macOS" (not "Mac OS"),
  ⌘-style shortcuts in the macOS manual and Ctrl+ elsewhere (use
  `{{key:command_id}}` in the manual so both are filled in).
- Never log file contents or clipboard text; paths and errors only.
- Strata makes no network connections except the opt-in update check. Don't
  add any without discussing it in an issue first.
- New settings go in `Settings`, `src/settings_meta.rs` and the manual's
  Settings Reference (tests keep the three in step).
- Every new command needs a name in the command palette and an entry on the
  manual's shortcuts page.

## Screenshots

The app can render whole frames off screen with wgpu (no window needed), which
the tests use for screenshots:

```sh
scripts/readme-screenshots.sh                                  # README images → docs/screenshots/
STRATA_HEADLESS_SHOTS=/tmp/shots cargo test --release headless_shots   # manual scenes
STRATA_UI_SHOTS=/tmp/shots cargo test --release ui_shots               # window chrome at odd sizes
STRATA_TERMINAL_SHOTS=/tmp/shots cargo test --release terminal_shots   # terminal rendering
```

Scenes are saved as PPM; convert with `sips -s format png` (macOS) or
ImageMagick's `magick`. The README scenes live in
`src/app/headless/readme_shots.rs` and use sample projects written to a
temporary folder, so no personal paths appear. Keep README images under
400 KB each.

## Pull request checklist

- [ ] `cargo test` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] The Windows target still compiles (CI checks this too).
- [ ] User-visible changes are documented in `docs/manual/` and noted under
      `[Unreleased]` in `CHANGELOG.md`.
- [ ] New dependencies are credited and `scripts/third-party-licenses.sh` was
      run.
- [ ] The change follows [`docs/CLEAN_ROOM.md`](docs/CLEAN_ROOM.md).

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE). Please also follow our
[Code of Conduct](CODE_OF_CONDUCT.md).
