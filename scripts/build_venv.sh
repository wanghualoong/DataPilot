#!/usr/bin/env bash
# Install pinned data packages directly into the bundled (relocatable) python-build-standalone runtime.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROXY="${DP_PROXY:-}"
PY="$ROOT/build/vendor/python/bin/python3"
[ -x "$PY" ] || "$ROOT/scripts/fetch_vendor.sh"
# Set DP_PROXY=http://host:port if your network needs an HTTP proxy for downloads.
# Set DP_PYPI_PROXY=1 to force the proxy for PyPI.
if [ "${DP_PYPI_PROXY:-0}" = "1" ]; then export HTTPS_PROXY="$PROXY" HTTP_PROXY="$PROXY"; else unset HTTPS_PROXY HTTP_PROXY https_proxy http_proxy; fi
if [ ! -f "$ROOT/scripts/requirements.lock" ]; then
  uv pip compile --python "$PY" --python-platform aarch64-apple-darwin "$ROOT/scripts/requirements.in" -o "$ROOT/scripts/requirements.lock"
fi
# PyPI direct is very slow on this network; default to a fast mirror (override with DP_PYPI_INDEX).
INDEX="${DP_PYPI_INDEX:-https://pypi.tuna.tsinghua.edu.cn/simple}"
"$PY" -m pip install --no-cache-dir --break-system-packages -i "$INDEX" -r "$ROOT/scripts/requirements.lock"
# Trim: tests, caches, headers we don't need at runtime
find "$ROOT/build/vendor/python" -type d \( -name tests -o -name __pycache__ -o -name test \) -prune -exec rm -rf {} + 2>/dev/null || true
rm -rf "$ROOT/build/vendor/python/lib/python3.12/site-packages/pyarrow/include" "$ROOT/build/vendor/python/include" 2>/dev/null || true
"$PY" -c "import docx, pptx, pypdf, pdfplumber, reportlab, xlsxwriter, lxml, defusedxml"
"$PY" -c "import pandas, duckdb, openpyxl, sqlglot, pyarrow; print('pandas', pandas.__version__, 'duckdb', duckdb.__version__, 'pyarrow', pyarrow.__version__)"
du -sh "$ROOT/build/vendor/python"
