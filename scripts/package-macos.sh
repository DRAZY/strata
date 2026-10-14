#!/usr/bin/env bash
# Build Strata.app and a test DMG for Apple silicon.
# Usage: scripts/package-macos.sh    → dist/Strata-<version>-arm64.dmg
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=aarch64-apple-darwin
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
DIST=dist
APP="$DIST/Strata.app"
DMG="$DIST/Strata-$VERSION-arm64.dmg"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "==> cargo build ($TARGET, release)"
cargo build --release --target "$TARGET"

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
cp "target/$TARGET/release/strata" "$APP/Contents/MacOS/strata"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Strata.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Strata</string>
  <key>CFBundleDisplayName</key><string>Strata</string>
  <key>CFBundleIdentifier</key><string>io.github.strata-editor.strata</string>
  <key>CFBundleExecutable</key><string>strata</string>
  <key>CFBundleIconFile</key><string>Strata</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleGetInfoString</key><string>Strata $VERSION, a fast, open-source code and text editor</string>
  <key>NSHumanReadableCopyright</key><string>© 2026 Strata contributors. MIT License.</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
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

# Ad-hoc signature: required on Apple silicon, but not notarized.
codesign --force --deep --sign - "$APP"

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
echo "Built $DMG"
