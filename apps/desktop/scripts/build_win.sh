#!/usr/bin/env bash
# Cross-build the Windows installer on a Mac (M3): cargo-xwin + NSIS (makensis), unsigned.
#   apps/desktop/scripts/build_win.sh  → src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/DataPilot_*_x64-setup.exe
# Needs: rustup target x86_64-pc-windows-msvc, cargo-xwin, llvm-rc / lld-link / llvm-lib on PATH, makensis, uv.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
export PATH="$HOME/.local/xwin-bin:$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
T=x86_64-pc-windows-msvc
[ -f "$ROOT/build/vendor-win/python/python.exe" ] || python3 -I "$ROOT/scripts/fetch_vendor.py" --target windows
( cd "$ROOT/datapilot-core" && cargo xwin build --release --target $T --quiet )
cp "$ROOT/datapilot-core/target/$T/release/datapilot-core.exe" "$HERE/src-tauri/binaries/datapilot-core-$T.exe"
cd "$HERE"
npx tauri build --runner cargo-xwin --target $T --bundles nsis --config src-tauri/tauri.windows.conf.json
ls -la "$HERE/src-tauri/target/$T/release/bundle/nsis/"
