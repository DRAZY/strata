# Releasing Strata

Strata releases are built, signed and notarized **on the maintainer's Mac** and
uploaded to GitHub Releases under the maintainer's account, the same way as the
other DRAZY apps. GitHub Actions only runs the checks (`build.yml`); it never
builds or holds signing keys.

## Cutting a release

1. Set `version` in `Cargo.toml` (for example `1.0.3`, or `1.1.0-rc.1` for a
   prerelease) and run `cargo build` so `Cargo.lock` follows.
2. In `CHANGELOG.md`, add `## [<version>] - <date>` under `[Unreleased]` with
   the changes, and update the links at the bottom.
3. Commit and push to `main`, and wait for the `build` checks to pass.
4. Run:

   ```sh
   scripts/release.sh            # or --draft to look it over before publishing
   ```

   It checks the branch, the changelog, the certificate, the notarization
   profile and the update key, runs the tests, then builds:

   | File | What it is |
   |---|---|
   | `Strata-<v>-universal.dmg` | macOS, Apple silicon + Intel; signed, notarized, stapled |
   | `Strata-<v>-arm64.dmg` | macOS, Apple silicon only |
   | `Strata-<v>-universal.app.zip`, `-arm64.app.zip` | The same apps, for the built-in updater |
   | `Strata-<v>-windows-x64.zip` | Windows x64, portable (cross-built; unsigned) |
   | `latest.json` | The signed update feed (see [docs/UPDATES.md](docs/UPDATES.md)) |
   | `*.sha256` | A checksum for every file |

   Then it tags `v<version>`, publishes the GitHub Release (prereleases for
   versions with a `-` suffix, otherwise as **Latest**), checks that the public
   update feed serves the new version, and cleans the local checkout
   (`scripts/clean-local.sh --builds`; `--keep-caches` skips the build caches).

Release tags are protected: once published, a `v*` tag can't be moved or
deleted. To fix a release, publish the next patch version.

## One-time setup on a Mac

1. **Rust and tools:** rustup with the `aarch64-apple-darwin`,
   `x86_64-apple-darwin` and `x86_64-pc-windows-gnu` targets;
   `brew install imagemagick mingw-w64`; Xcode; `gh auth login`.
2. **Developer ID certificate:** Xcode ▸ Settings ▸ Accounts ▸ Manage
   Certificates ▸ + ▸ Developer ID Application. To move it to another Mac,
   export it from Keychain Access (login ▸ My Certificates ▸ Export…) as a
   password-protected `.p12` and double-click it there.
3. **Notarization profile** (run in a normal Terminal window; the password
   prompt is hidden):

   ```sh
   xcrun notarytool store-credentials strata-notary \
     --apple-id <your Apple ID> --team-id <your Team ID>
   ```

   Use an app-specific password from account.apple.com ▸ Sign-In and
   Security ▸ App-Specific Passwords. If a release stops with "No Keychain
   password item found for profile: strata-notary", run this again.
4. **Update signing key:** `~/.strata-update-key` (mode 600). Keep a copy in
   your password manager: without it, installed copies can't verify future
   updates. See [docs/UPDATES.md](docs/UPDATES.md) for creating or rotating it.

## Building without publishing

- `NOTARIZE=1 scripts/package-macos.sh` (universal) or
  `ARCH=arm64 NOTARIZE=1 scripts/package-macos.sh`
- `scripts/package-windows.sh`
- `scripts/make-update-manifest.sh <version> notes.md`

## Windows code signing (not set up yet)

The Windows exe is unsigned, so SmartScreen warns on first run. Options:
Azure Trusted Signing (about $10/month after identity validation), an OV/EV
certificate from a CA such as DigiCert or Sectigo, or SignPath (free for
qualifying open-source projects). Any of them can be added to
`scripts/package-windows.sh`.
