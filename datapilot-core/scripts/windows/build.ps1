# Fallback: build the Windows installer on a Windows PC (when cross-building on the Mac is not possible).
# Needs: Rust (rustup, MSVC toolchain + Visual Studio Build Tools), Node.js 20+, Python 3.12+ (for fetch_vendor.py), uv (optional).
#   powershell -ExecutionPolicy Bypass -File scripts\windows\build.ps1
# Output: apps\desktop\src-tauri\target\release\bundle\nsis\DataPilot_<ver>_x64-setup.exe
$ErrorActionPreference = "Stop"
$Root = (Resolve-Path "$PSScriptRoot\..\..").Path
$T = "x86_64-pc-windows-msvc"

if (-not (Test-Path "$Root\build\vendor-win\python\python.exe")) {
    python -I "$Root\scripts\fetch_vendor.py" --target windows
}
Push-Location "$Root\datapilot-core"
cargo build --release
Pop-Location
Copy-Item "$Root\datapilot-core\target\release\datapilot-core.exe" "$Root\apps\desktop\src-tauri\binaries\datapilot-core-$T.exe" -Force

Push-Location "$Root\apps\desktop"
if (-not (Test-Path node_modules)) { npm ci }
npx tauri build --bundles nsis --config src-tauri/tauri.windows.conf.json
Pop-Location
Get-ChildItem "$Root\apps\desktop\src-tauri\target\release\bundle\nsis"
