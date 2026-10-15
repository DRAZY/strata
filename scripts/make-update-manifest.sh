#!/usr/bin/env bash
# Sign this release's update downloads and write dist/latest.json, the feed
# the in-app updater reads. See docs/UPDATES.md.
#
#   scripts/make-update-manifest.sh <version> <notes.md>
#
# Looks in dist/ for the files the packaging scripts make:
#   Strata-<version>-universal.app.zip   → macos-universal
#   Strata-<version>-arm64.app.zip       → macos-arm64
#   Strata-<version>-windows-x64.zip     → windows-x64
# (any that are missing are left out; at least one must be there).
#
# The private key comes from STRATA_UPDATE_KEY (a path to the key file, or
# the key's base64 itself, e.g. a CI secret), else ~/.strata-update-key.
# It's never printed. Download URLs point at the GitHub release for the tag
# v<version>; set RELEASE_BASE_URL to change that. Upload dist/latest.json to
# the same release as the files.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${1:?usage: make-update-manifest.sh <version> <notes.md>}
NOTES=${2:?usage: make-update-manifest.sh <version> <notes.md>}
VERSION=${VERSION#v}
DIST=${DIST:-dist}
BASE=${RELEASE_BASE_URL:-https://github.com/DRAZY/strata/releases/download/v$VERSION}

[ -f "$NOTES" ] || { echo "no notes file $NOTES" >&2; exit 1; }

ARGS=()
add() { # platform file
  if [ -f "$DIST/$2" ]; then ARGS+=("$1=$DIST/$2"); fi
}
add macos-universal "Strata-$VERSION-universal.app.zip"
add macos-arm64 "Strata-$VERSION-arm64.app.zip"
add windows-x64 "Strata-$VERSION-windows-x64.zip"
if [ ${#ARGS[@]} -eq 0 ]; then
  echo "no update files for $VERSION in $DIST/ (run the packaging scripts first)" >&2
  exit 1
fi

cargo run --quiet --release --example strata-sign -- manifest "$VERSION" "$NOTES" "$DIST/latest.json" "$BASE" "${ARGS[@]}"
# Check it the way the app will, with the key's public half.
cargo run --quiet --release --example strata-sign -- verify "$DIST/latest.json" "$DIST"
