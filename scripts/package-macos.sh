#!/usr/bin/env bash
# Build Strata.app and its DMG.
#
#   scripts/package-macos.sh              → dist/Strata-<version>-universal.dmg
#                                           and Strata-<version>-universal.app.zip (for updates)
#
# Universal (Apple silicon + Intel) unless ARCH=arm64 or ARCH=x86_64.
# Signing: with a "Developer ID Application" certificate in the keychain the
# app and DMG are signed (hardened runtime, timestamp); otherwise ad-hoc.
# Override with SIGN_IDENTITY="Developer ID Application: …" or SIGN_IDENTITY=-.
# Notarization: NOTARIZE=1 submits the DMG with the `notarytool` keychain
# profile NOTARY_PROFILE (default strata-notary), waits, and staples the
# ticket to the app and the DMG. Set it up once with
#   xcrun notarytool store-credentials strata-notary --apple-id … --team-id …
set -euo pipefail
cd "$(dirname "$0")/.."

ARCH=${ARCH:-universal}
NOTARIZE=${NOTARIZE:-0}
NOTARY_PROFILE=${NOTARY_PROFILE:-strata-notary}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# CFBundleVersion only ever goes up: the number of commits.
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 1)
DIST=dist
APP="$DIST/Strata.app"
DMG="$DIST/Strata-$VERSION-$ARCH.dmg"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

if [ -z "${SIGN_IDENTITY:-}" ]; then
  SIGN_IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)
  SIGN_IDENTITY=${SIGN_IDENTITY:--}
fi

case "$ARCH" in
  universal) TARGETS="aarch64-apple-darwin x86_64-apple-darwin" ;;
  arm64) TARGETS="aarch64-apple-darwin" ;;
  x86_64) TARGETS="x86_64-apple-darwin" ;;
  *) echo "ARCH must be universal, arm64 or x86_64" >&2; exit 1 ;;
esac
# An Intel-only build updates to the universal one.
case "$ARCH" in arm64) DIST_KEY=macos-arm64 ;; *) DIST_KEY=macos-universal ;; esac
BINS=""
for T in $TARGETS; do
  echo "==> cargo build ($T, release)"
  # Release builds leave out the screenshot/dev aids (the `devtools` feature).
  # STRATA_DIST: which update download this build asks for (src/update/manifest.rs).
  STRATA_DIST="$DIST_KEY" cargo build --release --no-default-features --target "$T"
  BINS="$BINS target/$T/release/strata"
done

echo "==> icon"
# Quick Look renders SVG gradients; it fills transparent areas with white,
# so cut the squircle back out with a mask matching icon.svg's rounded rect.
qlmanage -t -s 1024 -o "$WORK" assets/icon.svg >/dev/null 2>&1
magick -size 1024x1024 xc:black -fill white \
  -draw "roundrectangle 64,64 959,959 200,200" "$WORK/mask.png"
magick "$WORK/icon.svg.png" "$WORK/mask.png" -alpha off \
  -compose CopyOpacity -composite "$WORK/icon-1024.png"
ICONSET="$WORK/Strata.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  magick "$WORK/icon-1024.png" -resize ${s}x${s} "$ICONSET/icon_${s}x${s}.png"
  magick "$WORK/icon-1024.png" -resize $((s*2))x$((s*2)) "$ICONSET/icon_${s}x${s}@2x.png"
done

echo "==> Strata.app"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# shellcheck disable=SC2086
lipo -create $BINS -output "$APP/Contents/MacOS/strata"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Strata.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Strata</string>
  <key>CFBundleDisplayName</key><string>Strata</string>
  <key>CFBundleIdentifier</key><string>io.github.drazy.strata</string>
  <key>CFBundleExecutable</key><string>strata</string>
  <key>CFBundleIconFile</key><string>Strata</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>CFBundleGetInfoString</key><string>Strata $VERSION, a fast, open-source code and text editor</string>
  <key>NSHumanReadableCopyright</key><string>© 2026 Strata contributors. MIT License.</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <!-- Programs run in Strata's terminal ask macOS for access as Strata. -->
  <key>NSAppleEventsUsageDescription</key><string>Programs you run in Strata's terminal want to control another app.</string>
  <key>NSDesktopFolderUsageDescription</key><string>Strata opens and saves the files you choose on your Desktop.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>Strata opens and saves the files you choose in Documents.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>Strata opens and saves the files you choose in Downloads.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>Strata opens and saves the files you choose on removable drives.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>Strata opens and saves the files you choose on network drives.</string>
  <!-- Open With ▸ Strata for text, code and any other file, plus folders.
       Alternate: offered, but never taken over as the default app. -->
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Document</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.text</string>
        <string>public.plain-text</string>
        <string>public.source-code</string>
        <string>public.script</string>
        <string>public.data</string>
      </array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Folder</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array><string>public.folder</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST

if [ "$SIGN_IDENTITY" = "-" ]; then
  echo "==> ad-hoc signature (no Developer ID certificate found; not for distribution)"
  codesign --force --sign - "$APP"
else
  echo "==> signing as $SIGN_IDENTITY"
  # Hardened runtime, no exceptions needed. The one entitlement lets programs
  # in the terminal send Apple Events (osascript) after the usual prompt.
  cat > "$WORK/entitlements.plist" <<ENT
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.automation.apple-events</key><true/>
</dict>
</plist>
ENT
  codesign --force --options runtime --timestamp --entitlements "$WORK/entitlements.plist" --sign "$SIGN_IDENTITY" "$APP"
  codesign --verify --strict --verbose=1 "$APP"
fi

notarize() { # file
  echo "==> notarizing $(basename "$1") (profile $NOTARY_PROFILE; this takes a few minutes)"
  xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait --output-format plist > "$WORK/notary.plist" || true
  STATUS=$(/usr/libexec/PlistBuddy -c "Print :status" "$WORK/notary.plist" 2>/dev/null || echo "unknown")
  ID=$(/usr/libexec/PlistBuddy -c "Print :id" "$WORK/notary.plist" 2>/dev/null || echo "")
  if [ "$STATUS" != "Accepted" ]; then
    echo "Notarization: $STATUS" >&2
    cat "$WORK/notary.plist" >&2 || true
    [ -n "$ID" ] && xcrun notarytool log "$ID" --keychain-profile "$NOTARY_PROFILE" >&2
    exit 1
  fi
}

if [ "$NOTARIZE" = "1" ]; then
  [ "$SIGN_IDENTITY" != "-" ] || { echo "Notarizing needs a Developer ID signature" >&2; exit 1; }
  # The app first, so the copy that goes in the DMG carries its own ticket
  # (it then opens without a network check once dragged to Applications).
  ditto -c -k --keepParent "$APP" "$WORK/Strata.zip"
  notarize "$WORK/Strata.zip"
  xcrun stapler staple "$APP"
  spctl --assess --type execute --verbose=2 "$APP"
fi

# The in-app updater downloads this: the signed (and stapled) app, zipped
# with ditto so it unpacks with its signature intact. See docs/UPDATES.md.
echo "==> update zip"
UPDATE_ZIP="$DIST/Strata-$VERSION-$ARCH.app.zip"
rm -f "$UPDATE_ZIP"
ditto -c -k --keepParent "$APP" "$UPDATE_ZIP"
shasum -a 256 "$UPDATE_ZIP" > "$UPDATE_ZIP.sha256"

echo "==> DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# Prefer `diskutil image` (macOS 26+). The older `hdiutil create` is
# deprecated there and can fail with "Resource busy".
if [ "$(sw_vers -productVersion | cut -d. -f1)" -ge 26 ]; then
  diskutil image create from --format UDZO --volumeName "Strata $VERSION" "$STAGE" "$DMG" >/dev/null
else
  hdiutil create -volname "Strata $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
fi
if [ "$SIGN_IDENTITY" != "-" ]; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
fi

if [ "$NOTARIZE" = "1" ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
fi
shasum -a 256 "$DMG" > "$DMG.sha256"
echo "Built $DMG"
