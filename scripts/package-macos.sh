#!/usr/bin/env bash
# Build Strata.app and its DMG.
#
#   scripts/package-macos.sh              → dist/Strata-<version>-universal.dmg
#                                           and dist/Strata-<version>-universal.app.zip
#
# Universal (Apple silicon + Intel) unless ARCH=arm64 or ARCH=x86_64.
# Signing: with a "Developer ID Application" certificate in the keychain the
# app and DMG are signed (hardened runtime, timestamp); otherwise ad-hoc.
# Override with SIGN_IDENTITY="Developer ID Application: …" or SIGN_IDENTITY=-.
# Notarization: NOTARIZE=1 submits the DMG with the `notarytool` keychain
# profile NOTARY_PROFILE (default strata-notary), waits, and staples the
# ticket to the app and the DMG. Set it up once with
#   xcrun notarytool store-credentials strata-notary --apple-id … --team-id …
# or, with an App Store Connect API key (what CI uses),
#   xcrun notarytool store-credentials strata-notary --key AuthKey_….p8 --key-id … --issuer …
# NOTARY_KEYCHAIN names the keychain holding that profile when it isn't the
# login keychain (CI keeps it in a temporary one; see RELEASING.md).
# The .app.zip next to the DMG is the app alone (stapled when notarized), for
# the updater to download.
set -euo pipefail
cd "$(dirname "$0")/.."

ARCH=${ARCH:-universal}
NOTARIZE=${NOTARIZE:-0}
NOTARY_PROFILE=${NOTARY_PROFILE:-strata-notary}
NOTARY_ARGS=(--keychain-profile "$NOTARY_PROFILE")
if [ -n "${NOTARY_KEYCHAIN:-}" ]; then
  NOTARY_ARGS+=(--keychain "$NOTARY_KEYCHAIN")
fi
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# CFBundleVersion only ever goes up: the number of commits.
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 1)
DIST=dist
APP="$DIST/Strata.app"
DMG="$DIST/Strata-$VERSION-$ARCH.dmg"
APP_ZIP="$DIST/Strata-$VERSION-$ARCH.app.zip"
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
BINS=""
for T in $TARGETS; do
  echo "==> cargo build ($T, release)"
  # Release builds leave out the screenshot/dev aids (the `devtools` feature).
  cargo build --release --no-default-features --target "$T"
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
  xcrun notarytool submit "$1" "${NOTARY_ARGS[@]}" --wait --output-format plist > "$WORK/notary.plist" || true
  STATUS=$(/usr/libexec/PlistBuddy -c "Print :status" "$WORK/notary.plist" 2>/dev/null || echo "unknown")
  ID=$(/usr/libexec/PlistBuddy -c "Print :id" "$WORK/notary.plist" 2>/dev/null || echo "")
  if [ "$STATUS" != "Accepted" ]; then
    echo "Notarization: $STATUS" >&2
    cat "$WORK/notary.plist" >&2 || true
    [ -n "$ID" ] && xcrun notarytool log "$ID" "${NOTARY_ARGS[@]}" >&2
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
rm -f "$APP_ZIP"
ditto -c -k --keepParent "$APP" "$APP_ZIP"

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
(cd "$DIST" && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256" && shasum -a 256 "$(basename "$APP_ZIP")" > "$(basename "$APP_ZIP").sha256")
echo "Built $DMG and $APP_ZIP"
