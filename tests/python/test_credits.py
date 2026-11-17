"""credits.json (About window) ↔ THIRD_PARTY_NOTICES.md must list the same bundled / adapted components."""
import json, os, re, sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def tokens(name):
    """'pandas / numpy / pyarrow' → ['pandas','numpy','pyarrow']; drops parentheticals and version suffixes."""
    name = re.sub(r"\(.*?\)", "", name)
    out = []
    for part in name.split("/") if " / " in name else [name]:
        t = part.strip().split(" ")[0].strip().lower()
        if t:
            out.append(t)
    return out


def main():
    credits = json.load(open(os.path.join(ROOT, "Resources/about/credits.json"), encoding="utf-8"))
    notices = open(os.path.join(ROOT, "THIRD_PARTY_NOTICES.md"), encoding="utf-8").read()
    low = notices.lower()
    rows = [l for l in notices.splitlines() if l.startswith("| ") and not l.startswith("| Component") and not l.startswith("|---")]
    fails = []
    all_tokens = []
    for c in credits["code"]:
        toks = tokens(c["name"])
        all_tokens += toks
        missing = [t for t in toks if t not in low]
        if missing:
            fails.append(f"credits '{c['name']}' not in THIRD_PARTY_NOTICES: {missing}")
        for k in ("url", "license", "use_zh", "use_en"):
            if not c.get(k):
                fails.append(f"credits '{c['name']}' lacks {k}")
    for r in rows:
        first = r.split("|")[1].lower()
        if not any(t in first for t in all_tokens):
            fails.append(f"THIRD_PARTY_NOTICES row not in credits.json: {first.strip()[:80]}")
    for f in fails:
        print("FAIL", f)
    print(f"{len(credits['code'])} credits, {len(rows)} notice rows, {len(fails)} failures")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
