# Releasing Strata

Releases are built by GitHub Actions (`.github/workflows/release.yml`) when a
version tag is pushed. The workflow builds and notarizes the macOS DMGs, builds
the Windows zip, and opens a **draft** GitHub Release; nothing is published
until you publish the draft.

## Cutting a release

1. Update `version` in `Cargo.toml` (for example `1.0.0` or `1.0.0-rc.2`) and
   run `cargo build` so `Cargo.lock` follows.
2. In `CHANGELOG.md`, rename `[Unreleased]` to `## [<version>] - <date>`, add a
   fresh empty `[Unreleased]` above it, and update the links at the bottom.
3. Make sure `main` is green (the `build` workflow), then commit and tag:

   ```sh
   git commit -am "Release <version>"
   git tag -a v<version> -m "Strata <version>"
   git push origin main v<version>
   ```

4. Wait for the `release` workflow. It fails early if the tag doesn't match
   `Cargo.toml` or `CHANGELOG.md` has no section for the version.
5. Open the draft under **Releases**, check the notes and the files, download
   a DMG and the zip and try them, then **Publish release**. Tags with a
   pre-release suffix (`-rc.1`, `-beta.2`) are marked as pre-releases.

### What the release contains

| File | What it is |
|---|---|
| `Strata-<version>-universal.dmg` | macOS app for any Mac (Apple silicon and Intel), signed and notarized |
| `Strata-<version>-arm64.dmg` | macOS app for Apple silicon only (smaller) |
| `Strata-<version>-universal.app.zip`, `-arm64.app.zip` | The notarized app alone, which the built-in updater downloads |
| `Strata-<version>-windows-x64.zip` | `Strata.exe` (with its icon and version info), `LICENSE.txt` and `README.md`; not code-signed yet |
| `*.sha256` | SHA-256 checksum of each file, in `shasum -a 256` format |
| `latest.json` | The signed update manifest (once the manifest step is wired in; see below) |

Release notes are the version's section of `CHANGELOG.md`, followed by a short
list of downloads.

## Repository secrets

Set these under **Settings ▸ Secrets and variables ▸ Actions ▸ New repository
secret**. Never commit any of these values.

| Secret | What it holds |
|---|---|
| `MACOS_CERT_P12` | Your **Developer ID Application** certificate and private key, exported as a `.p12` file, base64-encoded |
| `MACOS_CERT_PASSWORD` | The password you set when exporting the `.p12` |
| `APPLE_API_KEY_P8` | An App Store Connect API key (`AuthKey_XXXXXXXXXX.p8`), base64-encoded |
| `APPLE_API_KEY_ID` | That key's Key ID (10 characters) |
| `APPLE_API_ISSUER_ID` | Your Issuer ID (a UUID, shown above the keys list) |
| `STRATA_UPDATE_KEY` | The ed25519 private key that signs the update manifest (`latest.json`); see [`docs/UPDATES.md`](docs/UPDATES.md) |

Without `MACOS_CERT_P12` the workflow still runs, but builds ad-hoc signed,
un-notarized DMGs and says so in a warning; don't publish those.

### Exporting the Developer ID certificate (.p12)

You need a paid Apple Developer Program membership.

1. If you don't have one yet, create a **Developer ID Application**
   certificate: in Xcode, **Settings ▸ Accounts ▸ (your team) ▸ Manage
   Certificates… ▸ + ▸ Developer ID Application**, or at
   <https://developer.apple.com/account/resources/certificates> with a
   certificate signing request from Keychain Access.
2. In **Keychain Access**, find "Developer ID Application: Your Name (TEAMID)"
   under **My Certificates**, expand it to check the private key is there,
   select the certificate, and choose **File ▸ Export Items…** as a Personal
   Information Exchange (`.p12`) file with a strong password.
3. Base64-encode it and copy it to the clipboard, then paste it as
   `MACOS_CERT_P12`:

   ```sh
   base64 -i DeveloperID.p12 | pbcopy
   ```

4. Delete the exported file afterwards.

### Creating the App Store Connect API key (notarization)

1. At <https://appstoreconnect.apple.com/access/integrations/api> (Users and
   Access ▸ Integrations ▸ App Store Connect API), create a **Team Key** with
   the **Developer** role.
2. Download the `.p8` file (Apple lets you download it only once) and note its
   **Key ID** and the **Issuer ID** shown on that page.
3. Store them:

   ```sh
   base64 -i AuthKey_XXXXXXXXXX.p8 | pbcopy   # → APPLE_API_KEY_P8
   ```

   and paste the Key ID and Issuer ID as `APPLE_API_KEY_ID` and
   `APPLE_API_ISSUER_ID`.

### How CI uses them

The macOS job creates a temporary keychain, imports the certificate into it,
and stores notarization credentials there with

```sh
xcrun notarytool store-credentials strata-notary --key AuthKey.p8 --key-id … --issuer … --keychain <temp keychain>
```

It then runs `scripts/package-macos.sh` with `NOTARIZE=1` and
`NOTARY_KEYCHAIN=<temp keychain>` for `ARCH=universal` and `ARCH=arm64`. The
script signs with the hardened runtime, notarizes and staples the app, builds
and signs the DMG, then notarizes and staples the DMG. The keychain and key
files are deleted at the end of the job.

## Update manifest

Strata's opt-in updater reads a signed `latest.json` from the latest GitHub
Release. `release.yml` has a clearly marked placeholder step for generating it
(`scripts/make-update-manifest.sh`, using the `STRATA_UPDATE_KEY` secret); it is
wired in together with the updater. Key generation, the manifest format and
how the app verifies it are described in [`docs/UPDATES.md`](docs/UPDATES.md).

## Building release packages locally

```sh
scripts/package-macos.sh                     # universal DMG (ARCH=arm64 or x86_64 for one)
NOTARIZE=1 scripts/package-macos.sh          # with a notarytool profile named strata-notary
scripts/package-windows.sh                   # Windows zip, cross-built with mingw (test builds)
```

Local packages use your login keychain's Developer ID identity if there is
one, and ad-hoc signing otherwise. Published Windows builds come from CI (MSVC).
