#!/usr/bin/env bash
# Create build/DataPilot-<version>.dmg from the Tauri app (apps/desktop). Optional notarization when DP_NOTARY_PROFILE is set.
#   SKIP_APP_BUILD=1  reuse the existing apps/desktop/src-tauri/target/release/bundle/macos/DataPilot.app
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ "${SKIP_APP_BUILD:-0}" = "1" ] || bash "$ROOT/apps/desktop/scripts/build_mac.sh"
APP="$ROOT/apps/desktop/src-tauri/target/release/bundle/macos/DataPilot.app"
[ -d "$APP" ] || { echo "missing $APP" >&2; exit 1; }
VER=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP/Contents/Info.plist")
STAGE="$ROOT/build/dmg-stage"; DMG="$ROOT/build/DataPilot-$VER.dmg"
rm -rf "$STAGE" "$DMG" && mkdir -p "$STAGE"
ditto "$APP" "$STAGE/DataPilot.app" && ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "DataPilot $VER" -srcfolder "$STAGE" -ov -format ULFO "$DMG" >/dev/null 2>&1
if [ "${DP_SIGN_ID:--}" != "-" ]; then
  codesign --force -s "$DP_SIGN_ID" "$DMG"
  if [ -n "${DP_NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$DMG" --keychain-profile "$DP_NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
  fi
fi
rm -rf "$STAGE"
ls -lh "$DMG"
