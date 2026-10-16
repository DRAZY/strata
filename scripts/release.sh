#!/usr/bin/env bash
# Build, sign, notarize and publish a Strata release from this Mac, in one go.
#
#   scripts/release.sh               the version in Cargo.toml (e.g. 1.0.3)
#   scripts/release.sh --draft       publish as a draft to look over first
#   scripts/release.sh --keep-caches leave the Rust build caches afterwards
#
# Like the other DRAZY apps, releases are built locally and uploaded under
# your account; GitHub Actions only runs the checks. Before running:
#   1. Set `version` in Cargo.toml and add a `## [x.y.z] - date` section to
#      CHANGELOG.md. Commit and push that to main.
#   2. Have the Developer ID certificate in the keychain, the `strata-notary`
#      notarytool profile, and the update key at ~/.strata-update-key
#      (see RELEASING.md).
# It checks all of that first and stops before building if anything's off.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || { [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"; }

DRAFT=""; KEEP_CACHES=0
for a in "$@"; do
  case "$a" in
    --draft) DRAFT="--draft" ;;
    --keep-caches) KEEP_CACHES=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done

REPO=DRAZY/strata
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v$VERSION"
fail() { echo "✗ $*" >&2; exit 1; }
ok() { echo "✓ $*"; }

echo "==> Checks for Strata $VERSION"
[ "$(git branch --show-current)" = main ] || fail "not on main"
[ -z "$(git status --porcelain)" ] || fail "uncommitted changes (commit or stash them first)"
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || fail "main isn't the same as origin/main (push or pull first)"
grep -q "^## \[$VERSION\]" CHANGELOG.md || fail "CHANGELOG.md has no ## [$VERSION] section"
gh release view "$TAG" -R "$REPO" >/dev/null 2>&1 && fail "release $TAG already exists"
security find-identity -v -p codesigning | grep -q "Developer ID Application" || fail "no Developer ID Application certificate in the keychain"
xcrun notarytool history --keychain-profile strata-notary >/dev/null 2>&1 || fail "notarytool profile strata-notary is missing (RELEASING.md, step 2)"
[ -s "$HOME/.strata-update-key" ] || fail "update signing key ~/.strata-update-key is missing"
ok "branch, changelog, certificate, notarization and update key"

echo "==> Tests"
cargo test --quiet 2>&1 | tail -3
cargo clippy --quiet --all-targets -- -D warnings
ok "tests and clippy"

echo "==> Builds (this takes ~15 minutes, mostly Apple's notarization)"
# Only this version's files in dist/.
scripts/clean-local.sh >/dev/null
NOTARIZE=1 scripts/package-macos.sh
ARCH=arm64 NOTARIZE=1 scripts/package-macos.sh
scripts/package-windows.sh

echo "==> Update feed"
NOTES=$(mktemp)
awk -v v="$VERSION" '
  $0 ~ "^## \\[" v "\\]" { on = 1; next }
  on && /^## \[/ { exit }
  on && /^\[[^]]+\]: / { next }
  on { print }
' CHANGELOG.md > "$NOTES"
STRATA_UPDATE_KEY="$HOME/.strata-update-key" scripts/make-update-manifest.sh "$VERSION" "$NOTES"

echo "==> GitHub release $TAG"
BODY=$(mktemp)
PREVIOUS=$(gh release view -R "$REPO" --json tagName --jq .tagName 2>/dev/null || true)
{
  echo "## Downloads"
  echo
  echo "| Platform | File |"
  echo "|---|---|"
  echo "| **macOS 11+, any Mac** (Apple silicon + Intel) | \`Strata-$VERSION-universal.dmg\` |"
  echo "| **macOS, Apple silicon only** (smaller) | \`Strata-$VERSION-arm64.dmg\` |"
  echo "| **Windows 10/11, x64** (portable) | \`Strata-$VERSION-windows-x64.zip\` |"
  echo
  echo "Already using Strata? It offers this update by itself (or choose **Help ▸ Check for Updates…**)."
  echo "macOS builds are signed and notarized. The Windows exe isn't code-signed yet: if SmartScreen appears, choose **More info ▸ Run anyway**."
  echo "Every file has a \`.sha256\`; the \`.app.zip\` files and \`latest.json\` are for the built-in updater."
  echo
  echo "## What's changed"
  cat "$NOTES"
  [ -n "$PREVIOUS" ] && { echo; echo "**Full changelog:** https://github.com/$REPO/compare/$PREVIOUS...$TAG"; }
} > "$BODY"
case "$VERSION" in *-*) KIND="--prerelease" ;; *) KIND="--latest" ;; esac
gh release create "$TAG" -R "$REPO" --target main $KIND $DRAFT \
  --title "Strata $VERSION" --notes-file "$BODY" \
  dist/Strata-"$VERSION"-* dist/latest.json
rm -f "$NOTES" "$BODY"

if [ -z "$DRAFT" ] && [ "$KIND" = "--latest" ]; then
  LIVE=$(curl -fsSL "https://github.com/$REPO/releases/latest/download/latest.json" | sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' | head -1)
  [ "$LIVE" = "$VERSION" ] && ok "update feed serves $VERSION" || echo "! update feed says ${LIVE:-nothing} (GitHub can take a minute)"
fi

echo "==> Local cleanup"
if [ "$KEEP_CACHES" = 1 ]; then scripts/clean-local.sh; else scripts/clean-local.sh --builds; fi
ok "Strata $VERSION released: https://github.com/$REPO/releases/tag/$TAG"
