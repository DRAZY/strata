# Updates: how they work and how to publish one

Strata checks GitHub for new versions (only when the user allows it, or
chooses Help ▸ Check for Updates…), shows the release notes, and installs the
new version in place. The code is in `src/update/` (the work) and
`src/app/update_ui.rs` (the notices). The user-facing description is in the
manual, `docs/manual/about.md#updates`.

## The feed

Each release has an asset `latest.json`:

```json
{
  "version": "1.2.0",
  "pub_date": "2026-11-01T12:00:00Z",
  "notes": "## What's new\n\n- …",
  "platforms": {
    "macos-universal": { "url": "https://github.com/DRAZY/strata/releases/download/v1.2.0/Strata-1.2.0-universal.app.zip", "size": 24100000, "sha256": "…", "signature": "…" },
    "macos-arm64":     { "url": "…/Strata-1.2.0-arm64.app.zip", "size": …, "sha256": "…", "signature": "…" },
    "windows-x64":     { "url": "…/Strata-1.2.0-windows-x64.zip", "size": …, "sha256": "…", "signature": "…" }
  }
}
```

- **Stable channel** (default): `https://github.com/DRAZY/strata/releases/latest/download/latest.json`,
  which GitHub resolves to the newest release that isn't a prerelease.
- **Prerelease channel** (`"update_channel": "prerelease"`): the list
  `https://api.github.com/repos/DRAZY/strata/releases`, newest version (semver
  order, so `1.0.0-rc.1 < 1.0.0`) that isn't a draft and has a `latest.json`.
- `notes` is Markdown, shown in the notice (cut at 32 KB).
- Which entry a build uses is fixed when it's built: the packaging scripts set
  `STRATA_DIST` (`macos-universal`, `macos-arm64`, `windows-x64`). Without it,
  macOS builds use `macos-universal` and Windows x64 builds `windows-x64`.
- Platforms Strata doesn't know are ignored, so new ones can be added later.

**While the repository is private** the feed URLs answer 404 to anyone not
signed in, so checks report "GitHub has no update information yet". That
fixes itself when the repository becomes public.

## What's checked

| Check | Where |
|---|---|
| Every URL is `https://` on github.com, api.github.com, objects.githubusercontent.com or release-assets.githubusercontent.com (no user info, ports, spaces) | `manifest::check_url` |
| Redirects stay on HTTPS, at most 5; `~/.curlrc` is ignored; connect timeout, minimum speed, size caps (1 MB feed, 4 MB API, the feed's `size` for the download, 512 MB max) | `fetch::curl_args` |
| The download's size and SHA-256 match the feed | `update::verify_download` |
| An ed25519 signature (strict verification) over `strata-update-v1\n<platform>\n<version>\n<sha256>\n<size>\n`, checked with the public key compiled into the app. Binding the version and platform stops a genuine older or other-platform file being passed off as this update. | `sig`, `key` |
| The offered version is newer than the running one (no downgrades) | `state::should_offer` |
| macOS: the unpacked app passes `codesign --verify --deep --strict` with a requirement of Developer ID Application by team `W966GFT83K`, and `codesign -dv` shows `TeamIdentifier=W966GFT83K` | `install::verify_mac` |
| macOS: `spctl --assess` accepts it, whenever the running copy is itself accepted (notarized builds) | `install::verify_staged` |
| macOS: bundle id `io.github.drazy.strata` and `CFBundleShortVersionString` equal the feed's version | `install::verify_mac` |
| Both: the new program runs here and `--version` prints the feed's version | `install::verify_staged` |

Only after all of that is the app swapped: the running `Strata.app` is renamed
to `.Strata.app.old` (Windows: `Strata.exe` to `Strata.exe.old`) and the new one
renamed into place, in the same folder. On quit, the *previous* version's
program runs as a helper (`strata --strata-update-helper …`): it waits for the
old process to exit, starts the new version, and if that exits before showing
its first window, puts the previous version back and leaves a note. The new
version deletes the backup once it's running.

The network goes through the system's `curl` (`/usr/bin/curl`, Windows'
`System32\curl.exe`, Windows 10 1803+) instead of a Rust TLS stack: no TLS
library in the binary, the system's certificate store, proxy settings and
security fixes, and nothing to cross-compile (ring/aws-lc need C toolchains).
Integrity doesn't depend on TLS; the signature covers it.

## The signing key

An ed25519 key pair. The **public** half is in `src/update/key.rs`. The
**private** half must never be committed. It's a 32-byte seed stored as one
line of base64.

The maintainer's key is at `~/.strata-update-key` (mode 600). Back it up
somewhere safe (a password manager): losing it means shipped copies can't
verify any future update, and users would have to download a new version by
hand.

Optionally keep it in the macOS keychain instead of a file:

```sh
security add-generic-password -s strata-update-key -a "$USER" -w "$(cat ~/.strata-update-key)"
rm -P ~/.strata-update-key
# when signing:
STRATA_UPDATE_KEY="$(security find-generic-password -s strata-update-key -w)" scripts/make-update-manifest.sh 1.2.0 notes.md
```

`STRATA_UPDATE_KEY` is either a path to the key file or the base64 itself.

Tools (`examples/strata-sign.rs`, which shares `src/update/sig.rs` with the app):

```sh
cargo run --release --example strata-sign -- keygen [PATH]   # new key; refuses to overwrite; prints the public key for key.rs
cargo run --release --example strata-sign -- pubkey          # public key of the current private key
cargo run --release --example strata-sign -- verify dist/latest.json dist
```

**Rotating the key**: make a new key, put its public half in `key.rs`, and
ship that version signed with the *old* key (which the installed copies
trust). Releases after it are signed with the new key.

## Publishing an update

1. Build the packages. `scripts/package-macos.sh` (with `NOTARIZE=1`) now also
   writes `dist/Strata-<version>-<arch>.app.zip`, the stapled app zipped with
   `ditto`; that's what macOS updates download. `scripts/package-windows.sh`
   (or the Windows CI build) writes `dist/Strata-<version>-windows-x64.zip`.
   Both set `STRATA_DIST`.
2. Write the release notes (Markdown) to a file.
3. `scripts/make-update-manifest.sh <version> <notes.md>` signs whatever
   update files are in `dist/`, writes `dist/latest.json`, and checks it.
4. Upload the `.app.zip`, the Windows zip and `latest.json` to the GitHub
   release tagged `v<version>` (alongside the DMG). The names in `latest.json`
   must match the uploaded names. Mark release candidates as prereleases:
   the stable channel skips them.

### In the release workflow

Add a repository secret **`STRATA_UPDATE_KEY`** holding the contents of
`~/.strata-update-key` (the base64 line). Then, after the macOS and Windows
artifacts are in `dist/` on one runner (macOS, since it has `ditto` and the
notarized app), add this step to `.github/workflows/release.yml` before the
upload step:

```yaml
      # ---- Update feed (docs/UPDATES.md) ----
      - name: Sign updates and write latest.json
        env:
          STRATA_UPDATE_KEY: ${{ secrets.STRATA_UPDATE_KEY }}
        run: |
          VERSION="${GITHUB_REF_NAME#v}"
          # Release notes for this version (from CHANGELOG.md, or the tag message).
          awk -v v="$VERSION" '/^## /{p = index($0, v) > 0; next} p' CHANGELOG.md > "$RUNNER_TEMP/notes.md"
          [ -s "$RUNNER_TEMP/notes.md" ] || echo "See the release page for what's new." > "$RUNNER_TEMP/notes.md"
          scripts/make-update-manifest.sh "$VERSION" "$RUNNER_TEMP/notes.md"
      # Then upload, with the other assets:
      #   dist/latest.json
      #   dist/Strata-*.app.zip
      #   dist/Strata-*-windows-x64.zip
```

The Windows zip is built on a Windows runner; download it into `dist/` on the
macOS job (`actions/download-artifact`) before this step. The key exists only
in this step's environment; it's never written to disk or printed.

## Testing

`cargo test update` covers manifest parsing, the URL policy, semver order,
signatures (valid, wrong key, tampered file, forged hash), the swap and
rollback with temporary folders, the helper's rollback, scheduling (24 hours,
skipped versions, `ask`/`auto`/`off`), cancelling a download, and the UI
buttons. `STRATA_HEADLESS_SHOTS=<dir> cargo test --release update_headless_shots`
renders the notices.

An end-to-end run against a real release (download, codesign checks, swap,
relaunch) needs a Developer ID–signed, notarized `.app.zip` signed with the
real key, published to a release; try it with the first release that ships
this code by installing the previous version and checking for updates.
