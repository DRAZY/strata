# Contributing to Strata

Thanks for helping. Bug reports, fixes, documentation and new features are all
welcome. For anything bigger than a small fix, please open an issue first so we
can agree on the approach before you spend time on it.

## Original work

Contributions must be your own original work: don't copy code, assets or text from other editors or projects whose licenses don't allow it. Openly licensed grammars, color schemes and fonts are fine when their
license allows it and they are credited in Help ▸ Licenses.

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

### Language servers, Review Changes and the terminal

- **Language servers:** the tests use a small fake server in
  `src/lsp/fake.rs` (the test binary starts itself as the server; see
  `fake::config`), so `cargo test` needs no real servers. If a test needs a
  new answer, teach the fake server rather than relying on an installed one.
  To try real servers, run
  `STRATA_LSP_REAL=/tmp/lsp cargo test --release lsp_real_servers -- --nocapture`:
  it drives rust-analyzer (and clangd, when installed) through the window and
  saves a screenshot of each step in that folder. Add
  `STRATA_LSP_REPO=<a file in a big Rust project>` to try hover and Go to
  Definition there too, and `STRATA_LSP_SHOTS=/tmp/lsp` saves screenshots
  from the popup tests.
- **Review Changes:** the tests make throwaway Git repositories
  (`review::tests::temp_repo`) and skip themselves when `git` isn't
  installed. Moving files to the Trash (`src/claim.rs`) calls a hook between
  its steps (`trash_listed_with`), so tests can swap a file at the worst
  moment; use it for new race tests instead of sleeping.
- **Terminal:** the link and Send to Terminal tests run a real `/bin/sh` (and
  `python3`, when it's installed).

## Coding conventions

- Keep changes focused; one topic per pull request.
- Prefer the existing helpers and patterns in the module you're changing.
- Comments explain why, not what. Doc comments on public items are short and
  concrete.
- User-facing text: sentence case, plain words, "macOS" (not "Mac OS"),
  ⌘-style shortcuts in the macOS manual and Ctrl+ elsewhere (use
  `{{key:command_id}}` in the manual so both are filled in). Command palette
  names and menu items are the exception: they use title case
  ("Terminal: Send to Terminal"), like the existing ones.
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
temporary folder, so no personal paths appear; the language server scene uses
the fake server, so it looks the same on every machine. Keep README images
under 400 KB each. Look at every image after regenerating them, and if an
unchanged scene differs only by noise, keep the old file.

## Pull request checklist

- [ ] `cargo test` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] The Windows target still compiles (CI checks this too).
- [ ] User-visible changes are documented in `docs/manual/` and noted under
      `[Unreleased]` in `CHANGELOG.md`.
- [ ] New dependencies are credited and `scripts/third-party-licenses.sh` was
      run.
- [ ] The change is your own original work (see [Original work](#original-work)).

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE). Please also follow our
[Code of Conduct](CODE_OF_CONDUCT.md).
