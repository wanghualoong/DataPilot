#!/usr/bin/env bash
# Mac build of the Tauri app: core sidecar + Python runtime + Resources, then sign nested code. DMG: scripts/build_dmg.sh.
#   apps/desktop/scripts/build_mac.sh          → src-tauri/target/release/bundle/macos/DataPilot.app (+ dmg with DMG=1)
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"
# DP_PUBLIC=1: open-source build without Resources/defaults.local.json (organisation presets); moved aside during the
# build and put back afterwards, then the bundled Resources are checked for organisation strings.
LOCAL_DEFAULTS="$ROOT/Resources/defaults.local.json"; ASIDE="$ROOT/build/defaults.local.json.aside"
if [ "${DP_PUBLIC:-0}" = 1 ] && [ -f "$LOCAL_DEFAULTS" ]; then
  mkdir -p "$ROOT/build" && mv "$LOCAL_DEFAULTS" "$ASIDE"
  trap 'mv "$ASIDE" "$LOCAL_DEFAULTS"' EXIT
fi
# the bundler's staging folder keeps files deleted from Resources: drop a stale local defaults file there
[ "${DP_PUBLIC:-0}" = 1 ] && rm -f "$HERE/src-tauri/target/release/Resources/defaults.local.json"
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
RES_DIR="$APP/Contents/Resources/Resources"
if [ "${DP_PUBLIC:-0}" = 1 ]; then
  # hosts / addresses named in the (moved-aside) local defaults must not appear anywhere in the bundled Resources
  if [ -f "$ASIDE" ]; then
    python3 -I -c 'import json,sys,re
d=json.load(open(sys.argv[1],encoding="utf-8"))
v=[p.get("baseURL","") for p in d.get("providerPresets",[])]+[d.get("docParseServer","")]+d.get("localModelSyncHosts",[])
for s in v:
    h=re.sub(r"^[a-z]+://","",s.strip()).split("/")[0].split(":")[0]
    if h and h!="localhost": print(h)' "$ASIDE" | sort -u > "$ROOT/build/public-check.txt"
    if [ -s "$ROOT/build/public-check.txt" ] && grep -rIl -i -F -f "$ROOT/build/public-check.txt" "$RES_DIR" 2>/dev/null; then echo "public build contains addresses from defaults.local.json" >&2; exit 1; fi
  fi
  [ ! -e "$RES_DIR/defaults.local.json" ] || { echo "public build contains defaults.local.json" >&2; exit 1; }
  echo "Public build: no organisation defaults"
fi
du -sh "$APP"
