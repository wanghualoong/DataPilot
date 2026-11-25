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
ICNS="$APP/Contents/Resources/icon.icns"
# writable image first: volume icon (.VolumeIcon.icns + custom-icon flag on the volume root), then compress
RW="$ROOT/build/DataPilot-$VER-rw.dmg"; rm -f "$RW"
hdiutil create -volname "DataPilot $VER" -srcfolder "$STAGE" -ov -format UDRW "$RW" >/dev/null 2>&1
MNT=$(hdiutil attach -nobrowse -noautoopen "$RW" | awk -F'\t' '/\/Volumes\//{print $NF}' | tail -1)
if [ -n "$MNT" ] && [ -f "$ICNS" ]; then
  cp "$ICNS" "$MNT/.VolumeIcon.icns"
  SetFile -a C "$MNT" 2>/dev/null || true
fi
[ -n "$MNT" ] && hdiutil detach "$MNT" -quiet
hdiutil convert "$RW" -format ULFO -ov -o "$DMG" >/dev/null 2>&1
rm -f "$RW"
# icon of the .dmg file itself in Finder
[ -f "$ICNS" ] && osascript -e 'use framework "AppKit"' \
  -e "set img to current application's NSImage's alloc()'s initWithContentsOfFile:\"$ICNS\"" \
  -e "current application's NSWorkspace's sharedWorkspace()'s setIcon:img forFile:\"$DMG\" options:0" >/dev/null 2>&1 || true
if [ "${DP_SIGN_ID:--}" != "-" ]; then
  codesign --force -s "$DP_SIGN_ID" "$DMG"
  if [ -n "${DP_NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$DMG" --keychain-profile "$DP_NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
  fi
fi
rm -rf "$STAGE"
ls -lh "$DMG"
