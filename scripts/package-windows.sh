#!/usr/bin/env bash
# Build Strata for 64-bit Windows from macOS or Linux, as a portable zip.
#
#   scripts/package-windows.sh   → dist/Strata-<version>-windows-x64.zip
#
# Cross-compiles with the GNU toolchain (needs `brew install mingw-w64` and
# `rustup target add x86_64-pc-windows-gnu`). Release builds for publishing
# come from CI on a Windows runner (MSVC); this is for local test builds.
# The zip isn't code-signed, so Windows SmartScreen warns on first run.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=${TARGET:-x86_64-pc-windows-gnu}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
DIST=dist
NAME="Strata-$VERSION-windows-x64"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "==> cargo build ($TARGET, release)"
cargo build --release --no-default-features --target "$TARGET"

echo "==> $NAME.zip"
mkdir -p "$WORK/$NAME" "$DIST"
cp "target/$TARGET/release/strata.exe" "$WORK/$NAME/Strata.exe"
cp LICENSE "$WORK/$NAME/LICENSE.txt"
cp README.md "$WORK/$NAME/README.md"
rm -f "$DIST/$NAME.zip"
(cd "$WORK" && zip -qr9 "$OLDPWD/$DIST/$NAME.zip" "$NAME")
(cd "$DIST" && shasum -a 256 "$NAME.zip" > "$NAME.zip.sha256")
echo "Built $DIST/$NAME.zip"
