#!/usr/bin/env bash
# Full-history backup of the repo outside the project folder (a git bundle), so losing the folder or .git loses nothing.
#   bash scripts/backup.sh            → ~/DataPilot-backups/datapilot-<时间>-<提交>.bundle（保留最近 30 份）
# Restore: git clone ~/DataPilot-backups/<file>.bundle DataPilot-restored
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
DEST="${DP_BACKUP_DIR:-$HOME/DataPilot-backups}"; mkdir -p "$DEST"
if [ -n "$(git status --porcelain)" ]; then echo "工作区有未提交的改动，先 git commit 再备份 / commit first" >&2; exit 1; fi
OUT="$DEST/datapilot-$(date +%Y%m%d-%H%M%S)-$(git rev-parse --short HEAD).bundle"
git bundle create "$OUT" --all --quiet
git bundle verify --quiet "$OUT"
ls -1t "$DEST"/datapilot-*.bundle | tail -n +31 | while read -r f; do rm -f "$f"; done
echo "backup: $OUT ($(du -h "$OUT" | cut -f1))"
