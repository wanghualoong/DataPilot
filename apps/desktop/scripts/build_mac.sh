#!/usr/bin/env bash
# Mac build of the Tauri app: core sidecar + Python runtime + Resources, then sign nested code. DMG: scripts/build_dmg.sh.
#   apps/desktop/scripts/build_mac.sh          → src-tauri/target/release/bundle/macos/DataPilot.app (+ dmg with DMG=1)
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
[ -x "$ROOT/build/vendor/python/bin/python3" ] || { "$ROOT/scripts/fetch_vendor.sh"; "$ROOT/scripts/build_venv.sh"; }
( cd "$ROOT/datapilot-core" && cargo build --release --quiet )
cp "$ROOT/datapilot-core/target/release/datapilot-core" "$HERE/src-tauri/binaries/datapilot-core-$TRIPLE"
cd "$HERE"
BUNDLES="app"; [ "${DMG:-0}" = 1 ] && BUNDLES="app,dmg"
npx tauri build --bundles "$BUNDLES"
APP="$HERE/src-tauri/target/release/bundle/macos/DataPilot.app"
SIGN_ID="${DP_SIGN_ID:-$(security find-identity -v -p codesigning 2>/dev/null | grep -Eo '"(Developer ID Application|Apple Development)[^"]*"' | head -1 | tr -d '"' || true)}"
SIGN_ID="${SIGN_ID:--}"
echo "Signing nested code with: $SIGN_ID"
find "$APP/Contents/Resources/python" -type f \( -name "*.so" -o -name "*.dylib" \) -print0 | xargs -0 -n 50 codesign --force --timestamp=none -s "$SIGN_ID" 2>/dev/null || true
for b in "$APP/Contents/Resources/python/bin/python3"*; do [ -f "$b" ] && [ ! -L "$b" ] && codesign --force --timestamp=none -s "$SIGN_ID" "$b"; done
codesign --force --timestamp=none --options runtime --identifier io.github.datapilot.core -s "$SIGN_ID" "$APP/Contents/MacOS/datapilot-core"
codesign --force --timestamp=none --options runtime -s "$SIGN_ID" "$APP"
codesign --verify --strict "$APP" && echo "Signature OK"
du -sh "$APP"
