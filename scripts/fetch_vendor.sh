#!/usr/bin/env bash
# Download third-party runtime assets into build/vendor (each into its own dir).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROXY="${DP_PROXY:-}"
PY_TAG="${PY_TAG:-20261026}"
PY_VER="${PY_VER:-3.12.16}"
ECHARTS_VER="${ECHARTS_VER:-6.1.0}"
V="$ROOT/build/vendor"
mkdir -p "$V/dl-python" "$V/dl-echarts"
PY_URL="https://github.com/astral-sh/python-build-standalone/releases/download/${PY_TAG}/cpython-${PY_VER}%2B${PY_TAG}-aarch64-apple-darwin-install_only_stripped.tar.gz"
if [ ! -x "$V/python/bin/python3" ]; then
  curl -fL --retry 3 ${PROXY:+-x "$PROXY"} -o "$V/dl-python/python.tar.gz" "$PY_URL"
  rm -rf "$V/python" && mkdir -p "$V/python-x"
  tar -xzf "$V/dl-python/python.tar.gz" -C "$V/python-x"
  mv "$V/python-x/python" "$V/python" && rmdir "$V/python-x"
fi
if [ ! -f "$ROOT/Resources/web/vendor/echarts.min.js" ]; then
  curl -fL --retry 3 ${PROXY:+-x "$PROXY"} -o "$V/dl-echarts/echarts.tgz" "https://registry.npmjs.org/echarts/-/echarts-${ECHARTS_VER}.tgz"
  mkdir -p "$V/dl-echarts/x" "$ROOT/Resources/web/vendor"
  tar -xzf "$V/dl-echarts/echarts.tgz" -C "$V/dl-echarts/x" package/dist/echarts.min.js package/LICENSE
  cp "$V/dl-echarts/x/package/dist/echarts.min.js" "$ROOT/Resources/web/vendor/"
  cp "$V/dl-echarts/x/package/LICENSE" "$ROOT/Resources/web/vendor/ECHARTS_LICENSE"
fi
"$V/python/bin/python3" --version
ls -la "$ROOT/Resources/web/vendor"
