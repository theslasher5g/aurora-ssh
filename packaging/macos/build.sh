#!/usr/bin/env bash
# Builds Aurora.app (universal: Apple Silicon + Intel) and a .dmg.
# Usage: packaging/macos/build.sh <version> <out-dir>
# Expects release builds for aarch64-apple-darwin and x86_64-apple-darwin of both
# aurora-term, crates/aurora-rdp and crates/aurora-web.
set -euo pipefail
VERSION="$1"
OUT="$2"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$OUT"
STAGE="$(mktemp -d)"
APP="$STAGE/Aurora.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

lipo -create \
  "$ROOT/target/aarch64-apple-darwin/release/aurora-term" \
  "$ROOT/target/x86_64-apple-darwin/release/aurora-term" \
  -output "$APP/Contents/MacOS/aurora"
lipo -create \
  "$ROOT/crates/aurora-rdp/target/aarch64-apple-darwin/release/aurora-rdp" \
  "$ROOT/crates/aurora-rdp/target/x86_64-apple-darwin/release/aurora-rdp" \
  -output "$APP/Contents/MacOS/aurora-rdp"
lipo -create \
  "$ROOT/crates/aurora-web/target/aarch64-apple-darwin/release/aurora-web" \
  "$ROOT/crates/aurora-web/target/x86_64-apple-darwin/release/aurora-web" \
  -output "$APP/Contents/MacOS/aurora-web"

# Icon: 1024px PNG -> .icns
ICONSET="$STAGE/aurora.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$ROOT/assets/icon-1024.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$ROOT/assets/icon-1024.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/aurora.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Aurora</string>
  <key>CFBundleDisplayName</key><string>Aurora</string>
  <key>CFBundleIdentifier</key><string>io.github.theslasher5g.aurora</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>aurora</string>
  <key>CFBundleIconFile</key><string>aurora</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

# Ad-hoc signature (required to run on Apple Silicon); not notarized.
codesign --force --deep --sign - "$APP"

DMGROOT="$STAGE/dmg"
mkdir -p "$DMGROOT"
cp -R "$APP" "$DMGROOT/"
ln -s /Applications "$DMGROOT/Applications"
# hdiutil sometimes fails with "Resource busy" on CI runners: try again a few times.
for attempt in 1 2 3 4 5; do
  if hdiutil create -volname "Aurora $VERSION" -srcfolder "$DMGROOT" -ov -format UDZO \
    "$OUT/Aurora-$VERSION-macos-universal.dmg"; then
    break
  fi
  [ "$attempt" = 5 ] && exit 1
  sleep $((attempt * 5))
done
ls -la "$OUT"
