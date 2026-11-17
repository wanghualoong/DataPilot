#!/usr/bin/env python3
"""Cross-platform vendor fetch: python-build-standalone runtime + pinned data packages for a target.

  python3 -I scripts/fetch_vendor.py --target windows   → build/vendor-win/python  (python.exe, Lib/site-packages)
  python3 -I scripts/fetch_vendor.py --target mac       → build/vendor/python       (same as fetch_vendor.sh + build_venv.sh)

Windows packages are installed from a Mac with `pip install --platform win_amd64 --only-binary=:all: --target`, so no
code from the downloads is executed here. Env: DP_PROXY (download proxy), DP_PYPI_INDEX (default: TUNA mirror).
"""
import argparse, os, shutil, subprocess, sys, tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY_TAG = os.environ.get("PY_TAG", "20261026")
PY_VER = os.environ.get("PY_VER", "3.12.16")
TARGETS = {
    "windows": dict(triple="x86_64-pc-windows-msvc", out="build/vendor-win", lock="scripts/requirements-win.lock",
                    platform="win_amd64", site="Lib/site-packages", exe="python.exe"),
    "mac": dict(triple="aarch64-apple-darwin", out="build/vendor", lock="scripts/requirements.lock",
                platform=None, site="lib/python3.12/site-packages", exe="bin/python3"),
}

def download(url: str, dest: Path):
    # curl uses the system trust store (python.org builds often ship without CA certificates)
    cmd = ["/usr/bin/curl" if Path("/usr/bin/curl").exists() else "curl", "-fL", "--retry", "3", "-o", str(dest), url]
    if os.environ.get("DP_PROXY"): cmd[1:1] = ["-x", os.environ["DP_PROXY"]]
    subprocess.run(cmd, check=True)

def safe_extract(tgz: Path, dest: Path):
    with tarfile.open(tgz) as t:
        t.extractall(dest, filter="data")   # rejects absolute paths, '..', device files, links out of the tree

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", choices=TARGETS, default="windows")
    a = ap.parse_args()
    t = TARGETS[a.target]
    out = ROOT / t["out"]
    py = out / "python"
    if not (py / t["exe"]).exists():
        dl = out / "dl-python"
        dl.mkdir(parents=True, exist_ok=True)
        url = (f"https://github.com/astral-sh/python-build-standalone/releases/download/{PY_TAG}/"
               f"cpython-{PY_VER}%2B{PY_TAG}-{t['triple']}-install_only_stripped.tar.gz")
        tgz = dl / "python.tar.gz"
        if not tgz.exists():
            print("download", url); download(url, tgz)
        x = out / "python-x"
        shutil.rmtree(x, ignore_errors=True); shutil.rmtree(py, ignore_errors=True)
        safe_extract(tgz, x)
        (x / "python").rename(py); x.rmdir()
    index = os.environ.get("DP_PYPI_INDEX", "https://pypi.tuna.tsinghua.edu.cn/simple")
    lock = ROOT / t["lock"]
    uv = shutil.which("uv") or str(Path.home() / ".local/bin/uv")
    if t["platform"] and Path(uv).exists():
        # uv resolves for the target interpreter (3.12 / win_amd64), not the host's Python
        cmd = [uv, "pip", "install", "--no-cache", "--index-url", index, "--python-platform", t["triple"], "--python-version", "3.12",
               "--only-binary", ":all:", "--no-deps", "--target", str(py / t["site"]), "-r", str(lock)]
    elif t["platform"]:
        cmd = [sys.executable, "-m", "pip", "install", "--no-cache-dir", "--disable-pip-version-check", "-i", index,
               "--platform", t["platform"], "--python-version", "3.12", "--implementation", "cp", "--only-binary=:all:",
               "--ignore-requires-python", "--no-deps", "--upgrade", "--target", str(py / t["site"]), "-r", str(lock)]
    else:
        cmd = [str(py / t["exe"]), "-m", "pip", "install", "--no-cache-dir", "--break-system-packages", "-i", index, "-r", str(lock)]
    print(" ".join(cmd[:6]), "…"); subprocess.run(cmd, check=True)
    # trim: tests, caches, headers, pip's own Windows launchers
    for d in list(py.rglob("*")):
        if d.is_dir() and d.name in ("tests", "test", "__pycache__") and d.exists():
            shutil.rmtree(d, ignore_errors=True)
    for d in [py / "include", py / t["site"] / "pyarrow" / "include", py / t["site"] / "bin"]:
        shutil.rmtree(d, ignore_errors=True)
    size = sum(f.stat().st_size for f in py.rglob("*") if f.is_file())
    print(f"{py}  {size / 1e6:.0f} MB")

if __name__ == "__main__":
    main()
