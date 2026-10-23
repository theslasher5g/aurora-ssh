#!/usr/bin/env bash
# Builds Aurora.app (universal: Apple Silicon + Intel) and a .dmg.
# Usage: packaging/macos/build.sh <version> <out-dir>
# Expects release builds for aarch64-apple-darwin and x86_64-apple-darwin of both
# aurora-term, crates/aurora-rdp and crates/aurora-web.
#
# Signing is optional and driven by environment variables (see packaging/macos/SIGNING.md):
#   MACOS_CERT_P12 (base64 .p12 of a "Developer ID Application" certificate) + MACOS_CERT_PASSWORD
#       -> Developer ID signing with Hardened Runtime + secure timestamp (inside-out)
#   MACOS_SIGN_IDENTITY   optional: identity name/hash if the .p12 holds several
#   Notarization (needs the signing above), either an App Store Connect API key:
#       MACOS_NOTARY_KEY_ID, MACOS_NOTARY_ISSUER, MACOS_NOTARY_KEY (contents of the AuthKey_*.p8)
#   or an Apple ID with an app-specific password:
#       MACOS_NOTARY_APPLE_ID, MACOS_NOTARY_TEAM_ID, MACOS_NOTARY_PASSWORD
# Without MACOS_CERT_P12 the app is ad-hoc signed (runs, but Gatekeeper warns), as before.
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

ENTITLEMENTS="$ROOT/packaging/macos/entitlements.plist"
SIGN_ID=""
KEYCHAIN=""
NOTARY_ARGS=()
SECRETS_DIR="$(mktemp -d)"
cleanup() {
  [ -n "$KEYCHAIN" ] && security delete-keychain "$KEYCHAIN" >/dev/null 2>&1 || true
  rm -rf "$SECRETS_DIR"
}
trap cleanup EXIT

if [ -n "${MACOS_CERT_P12:-}" ]; then
  # Temporary keychain on the runner; removed again on exit.
  KEYCHAIN="$SECRETS_DIR/aurora-signing.keychain-db"
  KCPASS="$(uuidgen)"
  printf '%s' "$MACOS_CERT_P12" | base64 --decode > "$SECRETS_DIR/cert.p12"
  security create-keychain -p "$KCPASS" "$KEYCHAIN"
  security set-keychain-settings -lut 3600 "$KEYCHAIN"
  security unlock-keychain -p "$KCPASS" "$KEYCHAIN"
  security import "$SECRETS_DIR/cert.p12" -P "${MACOS_CERT_PASSWORD:-}" -A -t cert -f pkcs12 -k "$KEYCHAIN"
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KCPASS" "$KEYCHAIN" >/dev/null
  security list-keychains -d user -s "$KEYCHAIN" $(security list-keychains -d user | tr -d '"')
  SIGN_ID="${MACOS_SIGN_IDENTITY:-}"
  if [ -z "$SIGN_ID" ]; then
    SIGN_ID="$(security find-identity -v -p codesigning "$KEYCHAIN" | awk '/Developer ID Application/ {print $2; exit}')"
  fi
  [ -n "$SIGN_ID" ] || { echo "No 'Developer ID Application' identity in MACOS_CERT_P12" >&2; exit 1; }
  echo "Signing with Developer ID identity $SIGN_ID"

  # Inside-out: helpers first, then the main executable, then the bundle itself.
  # --options runtime = Hardened Runtime, --timestamp = Apple's secure timestamp (both
  # required for notarization). No --deep: every code file is signed explicitly.
  for f in "$APP/Contents/MacOS/aurora-rdp" "$APP/Contents/MacOS/aurora-web" "$APP/Contents/MacOS/aurora"; do
    codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGN_ID" --keychain "$KEYCHAIN" "$f"
  done
  codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$SIGN_ID" --keychain "$KEYCHAIN" "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"

  if [ -n "${MACOS_NOTARY_KEY_ID:-}" ] && [ -n "${MACOS_NOTARY_ISSUER:-}" ] && [ -n "${MACOS_NOTARY_KEY:-}" ]; then
    printf '%s\n' "$MACOS_NOTARY_KEY" > "$SECRETS_DIR/AuthKey.p8"
    NOTARY_ARGS=(--key "$SECRETS_DIR/AuthKey.p8" --key-id "$MACOS_NOTARY_KEY_ID" --issuer "$MACOS_NOTARY_ISSUER")
  elif [ -n "${MACOS_NOTARY_APPLE_ID:-}" ] && [ -n "${MACOS_NOTARY_TEAM_ID:-}" ] && [ -n "${MACOS_NOTARY_PASSWORD:-}" ]; then
    NOTARY_ARGS=(--apple-id "$MACOS_NOTARY_APPLE_ID" --team-id "$MACOS_NOTARY_TEAM_ID" --password "$MACOS_NOTARY_PASSWORD")
  else
    echo "No notarization credentials - signed with Developer ID, but NOT notarized"
  fi
else
  # Ad-hoc signature (required to run on Apple Silicon); not notarized.
  codesign --force --deep --sign - "$APP"
fi

# Submits a file to Apple's notary service and waits; prints the log when it is rejected.
notarize() {
  local out
  if ! out="$(xcrun notarytool submit "$1" "${NOTARY_ARGS[@]}" --wait --output-format json)"; then
    echo "$out" >&2
    echo "Notarization failed for $1" >&2
    exit 1
  fi
  echo "$out"
  if ! echo "$out" | grep -q '"status" *: *"Accepted"'; then
    local id
    id="$(echo "$out" | sed -n 's/.*"id" *: *"\([^"]*\)".*/\1/p' | head -1)"
    [ -n "$id" ] && xcrun notarytool log "$id" "${NOTARY_ARGS[@]}" >&2 || true
    echo "Notarization was not accepted for $1" >&2
    exit 1
  fi
}

if [ ${#NOTARY_ARGS[@]} -gt 0 ]; then
  # Notarize the app first and staple the ticket to it, so the copy dragged out of the .dmg
  # also opens offline.
  ditto -c -k --keepParent "$APP" "$STAGE/Aurora-notarize.zip"
  notarize "$STAGE/Aurora-notarize.zip"
  xcrun stapler staple "$APP"
  rm -f "$STAGE/Aurora-notarize.zip"
fi

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
DMG="$OUT/Aurora-$VERSION-macos-universal.dmg"
if [ -n "$SIGN_ID" ]; then
  codesign --force --timestamp --sign "$SIGN_ID" --keychain "$KEYCHAIN" "$DMG"
  if [ ${#NOTARY_ARGS[@]} -gt 0 ]; then
    notarize "$DMG"
    xcrun stapler staple "$DMG"
    xcrun stapler validate "$DMG"
    spctl --assess --type open --context context:primary-signature -v "$DMG" || true
  fi
fi
ls -la "$OUT"
