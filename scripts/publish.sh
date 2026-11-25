#!/usr/bin/env bash
# Publish the current HEAD to GitHub as ONE snapshot commit on top of origin/main: the local commit history is not
# pushed, and the files in PRIVATE are left out of the public tree. Checks the public tree before pushing.
#   bash scripts/publish.sh ["message"]     → pushes and prints the new origin/main commit
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
VER=$(sed -n 's/^version = "\(.*\)"/\1/p' datapilot-core/Cargo.toml | head -1)
MSG="${1:-DataPilot $VER}"
PRIVATE=(CLAUDE.md)
[ -z "$(git status --porcelain)" ] || { echo "工作区有未提交的改动，先提交 / commit first" >&2; exit 1; }
git fetch -q origin main
IDX="$(mktemp)"; LIST="$(mktemp)"; trap 'rm -f "$IDX" "$LIST"' EXIT
GIT_INDEX_FILE="$IDX" git read-tree HEAD
for f in "${PRIVATE[@]}"; do GIT_INDEX_FILE="$IDX" git rm -q --cached --ignore-unmatch -- "$f"; done
TREE=$(GIT_INDEX_FILE="$IDX" git write-tree)
fail() { echo "publish: $*" >&2; exit 1; }
bad=$(git ls-tree -r --name-only "$TREE" | grep -E '(^|/)(config\.json|defaults\.local\.json|CHAT2DB_LICENSE|CLAUDE\.md|\.env)$' || true)
[ -z "$bad" ] || fail "public tree contains: $bad"
if git grep -I -l -i "adapted from chat""2db" "$TREE" -- .; then fail "Chat2DB-adapted file"; fi
if git grep -I -n -E "sk-[A-Za-z0-9]{32,}|ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}|-----BEGIN [A-Z ]*PRIVATE KEY" "$TREE" -- .; then fail "possible secret"; fi
# addresses named in the local (organisation) defaults must not be in the public tree
if [ -f Resources/defaults.local.json ]; then
  python3 -I -c 'import json,sys,re
d=json.load(open(sys.argv[1],encoding="utf-8"))
v=[p.get("baseURL","") for p in d.get("providerPresets",[])]+[d.get("docParseServer","")]+d.get("localModelSyncHosts",[])
for s in v:
    h=re.sub(r"^[a-z]+://","",s.strip()).split("/")[0].split(":")[0]
    if h and h!="localhost": print(h)' Resources/defaults.local.json | sort -u > "$LIST"
  if [ -s "$LIST" ] && git grep -I -l -i -F -f "$LIST" "$TREE" -- . ':!*.xsd'; then fail "address from defaults.local.json"; fi
fi
[ "$(git rev-parse 'origin/main^{tree}')" != "$TREE" ] || { echo "nothing to publish"; exit 0; }
C=$(git commit-tree "$TREE" -p origin/main -m "$MSG

Co-Authored-By: Claude <noreply@anthropic.com>")
# the keychain entry is stored per host (credential.useHttpPath=true in ~/.gitconfig would miss it)
# credentials: the GitHub CLI login when available (gh auth login), else the keychain entry stored per host
GH="$(command -v gh || echo "$HOME/.local/bin/gh")"
if [ -x "$GH" ] && "$GH" auth status >/dev/null 2>&1; then
  git -c credential.helper= -c "credential.helper=!\"$GH\" auth git-credential" push -q origin "$C:refs/heads/main"
else
  git -c credential.useHttpPath=false push -q origin "$C:refs/heads/main"
fi
git fetch -q origin main
echo "published $C (origin/main), tree without: ${PRIVATE[*]}"
