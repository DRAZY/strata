## What this changes

<!-- What and why, in a few sentences. Link the issue it fixes: "Fixes #123". -->

## How it was tested

<!-- The tests you added or ran, and what you checked by hand (platform too). -->

## Checklist

- [ ] `cargo test` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] The Windows target still compiles (`cargo clippy --target x86_64-pc-windows-msvc -- -D warnings`, or CI).
- [ ] User-visible changes are documented in `docs/manual/` and noted under `[Unreleased]` in `CHANGELOG.md`.
- [ ] New dependencies are credited in `src/app/more.rs` and `scripts/third-party-licenses.sh` was run.
- [ ] This follows the clean-room policy in [`docs/CLEAN_ROOM.md`](../docs/CLEAN_ROOM.md).
