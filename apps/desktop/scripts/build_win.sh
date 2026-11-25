#!/usr/bin/env bash
# Cross-build the Windows installer on a Mac (M3): cargo-xwin + NSIS (makensis), unsigned.
#   apps/desktop/scripts/build_win.sh  → src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/DataPilot_*_x64-setup.exe
# Needs: rustup target x86_64-pc-windows-msvc, cargo-xwin, llvm-rc / lld-link / llvm-lib on PATH, makensis, uv.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
export PATH="$HOME/.local/xwin-bin:$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
T=x86_64-pc-windows-msvc
# DP_PUBLIC=1: open-source build without Resources/defaults.local.json (organisation presets); moved aside during the
# build and put back afterwards, then the bundled Resources are checked for organisation strings.
LOCAL_DEFAULTS="$ROOT/Resources/defaults.local.json"; ASIDE="$ROOT/build/defaults.local.json.aside"
if [ "${DP_PUBLIC:-0}" = 1 ] && [ -f "$LOCAL_DEFAULTS" ]; then
  mkdir -p "$ROOT/build" && mv "$LOCAL_DEFAULTS" "$ASIDE"
  trap 'mv "$ASIDE" "$LOCAL_DEFAULTS"' EXIT
fi
# the bundler's staging folder keeps files deleted from Resources: drop a stale local defaults file there
[ "${DP_PUBLIC:-0}" = 1 ] && rm -f "$HERE/src-tauri/target/$T/release/Resources/defaults.local.json"
[ -f "$ROOT/build/vendor-win/python/python.exe" ] || python3 -I "$ROOT/scripts/fetch_vendor.py" --target windows
( cd "$ROOT/datapilot-core" && cargo xwin build --release --target $T --quiet )
cp "$ROOT/datapilot-core/target/$T/release/datapilot-core.exe" "$HERE/src-tauri/binaries/datapilot-core-$T.exe"
cd "$HERE"
npx tauri build --runner cargo-xwin --target $T --bundles nsis --config src-tauri/tauri.windows.conf.json
RES_DIR="$HERE/src-tauri/target/$T/release/Resources"
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
ls -la "$HERE/src-tauri/target/$T/release/bundle/nsis/"
