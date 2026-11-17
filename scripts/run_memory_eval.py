"""Cross-session learning eval (CL-Bench style, see tests/eval/memory_series.json).

Each question runs in a FRESH session of the same project; after the answer the model is told the reference answer and
the business rule (rules are in no file, so later questions can only benefit through memory). Then the background
memory work is flushed and the session closed. Conditions: --memory off|on (run both to compare).

usage: DP_API_KEY=... build/vendor/python/bin/python3 -I scripts/run_memory_eval.py --memory on [--label x]
"""
import argparse, json, os, shutil, subprocess, sys, tempfile, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_agent_eval as base  # noqa: E402

ROOT = base.ROOT


class Core(base.Core):
    def call(self, method, params=None, timeout=600):
        rid = self.send(method, params)
        t0 = time.time()
        while time.time() - t0 < timeout:
            v = self.read()
            if v.get("id") == rid:
                if "error" in v:
                    raise RuntimeError(v["error"]["message"])
                return v["result"]
        raise RuntimeError(f"{method} timed out")

    def turn(self, sid, text, timeout):
        self.send("turn/start", {"session": sid, "text": text})
        texts, rec, t0 = [], {"rounds": 0, "tools": [], "errors": 0, "memory": []}, time.time()
        while True:
            if time.time() - t0 > timeout:
                self.send("turn/cancel", {"session": sid}); rec["reason"] = "timeout"; break
            v = self.read()
            e = (v.get("params") or {}).get("event")
            if not e:
                continue
            t = e["type"]
            if t == "assistant_message":
                rec["rounds"] += 1
                if e["message"].get("content"):
                    texts.append(e["message"]["content"])
            elif t == "tool_finished":
                rec["tools"].append(e["message"].get("name")); rec["errors"] += bool(e["is_error"])
            elif t == "approval_required":
                self.send("approval/respond", {"session": sid, "id": e["request"]["id"], "choice": "allowOnce"})
            elif t == "question":
                self.send("question/answer", {"session": sid, "id": e["call_id"], "answer": "按你的判断继续"})
            elif t == "context_injected" and "<relevant_memories>" in e["text"]:
                rec["memory"].append("recalled")
            elif t == "memory_saved":
                rec["memory"].append("saved:" + e["text"][:60])
            elif t == "turn_finished":
                rec["reason"] = e["reason"]; break
        rec["answer"] = "\n".join(texts)
        return rec


def fmt_gold(rows):
    out = []
    for r in rows:
        out.append("、".join(f"{v:.4f}".rstrip("0").rstrip(".") if isinstance(v, float) else str(v) for v in r))
    return "；".join(out)


def hygiene(dump, spec):
    """Memory quality: topic files per key (ideal 1) and files whose headline still asserts a stale rule (ideal 0).
    headline = name + description + first body line (later lines may legitimately mention the old rule as obsolete)."""
    import re
    out, heads, names = {}, {}, {}
    for f, v in dump.items():
        if f == "MEMORY.md":
            continue
        meta, body = {}, v
        if v.startswith("---") and v.count("---") >= 2:
            _, fm, body = v.split("---", 2)
            meta = dict(l.split(":", 1) for l in fm.strip().splitlines() if ":" in l)
        first = next((l for l in body.strip().splitlines() if l.strip()), "")
        heads[f] = " ".join([meta.get("name", ""), meta.get("description", ""), first])
        names[f] = meta.get("name", f)
    out["files"] = len(heads)
    for key, pat in spec.get("topics", {}).items():
        # topic = what the file is about: its name only (descriptions of other rules may mention GMV etc.)
        out[f"topic:{key}"] = sum(bool(re.search(pat, n)) for n in names.values())
    out["stale"] = sorted(f for f, h in heads.items() for pat in spec.get("stale", []) if re.search(pat, h))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--memory", choices=["on", "off"], default="on")
    ap.add_argument("--timeout", type=int, default=420)
    ap.add_argument("--config", default=os.path.expanduser("~/Library/Application Support/DataPilot/config.json"))
    ap.add_argument("--label", default="")
    ap.add_argument("--prompts", default="", help="prompt folder override (A/B test of prompt variants)")
    ap.add_argument("--no-seed", action="store_true", help="do not pre-load seed_memories")
    ap.add_argument("--isolate-catalog", action="store_true",
                    help="clear catalog.json metrics/synonyms before each session, so only the memory system can carry rules")
    a = ap.parse_args()
    if not os.environ.get("DP_API_KEY"):
        sys.exit("DP_API_KEY is not set")
    spec = json.load(open(os.path.join(ROOT, "tests/eval/memory_series.json")))
    fx = base.build_fixtures()
    con = base.gold_db(fx)
    proj = tempfile.mkdtemp(prefix="dp-memeval-proj-")
    os.makedirs(f"{proj}/data")
    for f in ["orders.csv", "master.xlsx", "targets_2026.csv"]:
        shutil.copy(f"{fx}/{f}", f"{proj}/data/{f}")
    if not a.no_seed and spec.get("seed_memories"):
        md = f"{proj}/.datapilot/memory"
        os.makedirs(md)
        idx = []
        for m in spec["seed_memories"]:
            fp = f"{md}/{m['file']}"
            open(fp, "w", encoding="utf-8").write(f"---\nname: {m['name']}\ndescription: {m['description']}\ntype: {m['type']}\n---\n\n{m['body']}\n")
            t = time.time() - m.get("age_days", 0) * 86400
            os.utime(fp, (t, t))
            idx.append(f"- [{m['name']}]({m['file']}) — {m['description']}")
        open(f"{md}/MEMORY.md", "w", encoding="utf-8").write("\n".join(idx) + "\n")
    if a.prompts:
        os.environ["DP_PROMPTS_DIR"] = os.path.abspath(a.prompts)
    support = tempfile.mkdtemp(prefix="dp-memeval-sup-")
    cfg = json.load(open(a.config))
    cfg.setdefault("settings", {}).update({"memoryEnabled": a.memory == "on", "defaultMode": "full"})
    json.dump(cfg, open(f"{support}/config.json", "w"))
    core = Core(support)
    results = []
    try:
        for i, t in enumerate(spec["series"]):
            if a.isolate_catalog:
                cp = f"{proj}/.datapilot/data/catalog.json"
                if os.path.exists(cp):
                    try:
                        cat = json.load(open(cp, encoding="utf-8"))
                        cat["metrics"], cat["synonyms"] = [], {}
                        json.dump(cat, open(cp, "w", encoding="utf-8"), ensure_ascii=False)
                    except Exception:  # noqa: BLE001
                        os.remove(cp)
            info = core.call("session/open", {"project_root": proj, "mode": "full"})
            sid = info["session"]
            q = (spec["files"] + "\n\n" if i == 0 else "") + t["q"]
            t0 = time.time()
            rec = core.turn(sid, q, a.timeout)
            rec["id"] = t["id"]
            rec["problems"] = base.check(t, rec["answer"], proj, con) if rec["reason"] == "done" else [f"reason={rec['reason']}"]
            rec["ok"] = not rec["problems"]
            rec["secs"] = round(time.time() - t0, 1)
            gold = fmt_gold(con.execute(t["gold"]).fetchall())
            fb = core.turn(sid, f"{'回答正确' if rec['ok'] else '回答不对'}。参考答案：{gold}。{t['rule']}", a.timeout)
            rec["memory"] += fb["memory"]
            core.call("session/flush", {"session": sid, "timeout": 300}, timeout=330)
            core.call("session/close", {"session": sid})
            results.append(rec)
            print(f"{'✅' if rec['ok'] else '❌'} {rec['id']:<18} {rec['secs']:>6}s rounds={rec['rounds']:<2} "
                  f"mem={','.join(m.split(':')[0] for m in rec['memory']) or '-'} "
                  + ("" if rec["ok"] else "; ".join(rec["problems"])[:160]), flush=True)
        core.call("shutdown", timeout=30)
    finally:
        try:
            core.p.wait(timeout=20)
        except Exception:
            core.p.kill()
    mem_files = sorted(os.listdir(f"{proj}/.datapilot/memory")) if os.path.isdir(f"{proj}/.datapilot/memory") else []
    ok = sum(r["ok"] for r in results)
    later = results[1:]
    # questions whose rule is first stated in their own feedback can't be answered from memory: score the rest separately
    intro = set(spec.get("intro_questions", []))
    learnable = [r for r in results if r["id"] not in intro]
    summary = {"memory": a.memory, "label": a.label, "model": cfg["roles"]["main"]["model"], "ok": ok, "total": len(results),
               "ok_after_first": sum(r["ok"] for r in later),
               "ok_learnable": sum(r["ok"] for r in learnable), "learnable": len(learnable), "isolate_catalog": a.isolate_catalog, "avg_rounds": round(sum(r["rounds"] for r in results) / max(1, len(results)), 2),
               "memory_files": mem_files}
    print(f"\nmemory={a.memory}: {ok}/{len(results)} correct (after first: {summary['ok_after_first']}/{len(later)}; "
          f"learnable: {summary['ok_learnable']}/{len(learnable)}), "
          f"avg rounds {summary['avg_rounds']}, memory files {mem_files}")
    out = os.path.join(ROOT, "tests/eval/out")
    os.makedirs(out, exist_ok=True)
    mem_dump = {f: open(f"{proj}/.datapilot/memory/{f}", encoding="utf-8").read() for f in mem_files if f.endswith(".md")}
    summary["hygiene"] = hygiene(mem_dump, spec.get("hygiene", {}))
    print("hygiene:", json.dumps(summary["hygiene"], ensure_ascii=False))
    json.dump({"summary": summary, "results": results, "memory": mem_dump},
              open(os.path.join(out, time.strftime("%Y%m%d-%H%M%S") + f"-memory-{a.memory}{('-' + a.label) if a.label else ''}.json"), "w"),
              ensure_ascii=False, indent=1)
    shutil.rmtree(fx, ignore_errors=True); shutil.rmtree(support, ignore_errors=True); shutil.rmtree(proj, ignore_errors=True)


if __name__ == "__main__":
    main()
