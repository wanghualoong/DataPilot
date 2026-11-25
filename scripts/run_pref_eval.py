"""Cross-project preference eval: does a personal preference taught once carry over to OTHER projects?

Session 1 (plain chat): one data question, then the user states a standing preference ("all projects: amounts in 万元
with two decimals, conclusion first"). Sessions 2-3 run in two different, fresh project folders and never repeat it.
Scored: answers give the gold amounts as X.XX万 (and the first sentence contains a number). Also reports what landed
in the user memory folder (AppSupport/memory). Conditions: --memory on|off.

usage: build/vendor/python/bin/python3 -I scripts/run_pref_eval.py --memory on
(API key: DP_API_KEY if set, else the keychain entry of the configured provider)
"""
import argparse, json, os, re, shutil, sys, tempfile, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_agent_eval as base  # noqa: E402
from run_memory_eval import Core  # noqa: E402

ROOT = base.ROOT
PREF = "记住：以后所有项目、所有分析里，金额一律用「万元」为单位并保留两位小数（例如 123.45万），回答第一句话先给结论。"
SALES = "SELECT {g}, sum(o.数量*p.单价) FROM orders o JOIN products p USING(product_id) WHERE o.状态='paid' AND {w} GROUP BY 1 ORDER BY 2 DESC"
SERIES = [
    {"id": "p2_category", "q": "2025 年各品类的已支付销售额分别是多少？", "gold": SALES.format(g="p.品类", w="year(o.下单日期)=2025")},
    {"id": "p3_region", "q": "2026 年各大区的已支付销售额是多少？哪个最高？",
     "gold": "SELECT c.大区, sum(o.数量*p.单价) FROM orders o JOIN products p USING(product_id) JOIN customers c USING(customer_id) "
             "WHERE o.状态='paid' AND year(o.下单日期)=2026 GROUP BY 1 ORDER BY 2 DESC"},
]
FILES = "data/ 下有 orders.csv（订单，GBK 编码）、master.xlsx（customers / products 两个 sheet，customers 表头前有一行标题）。销售额 = 数量 × 单价。\n\n"


def wan_hits(text, golds, tol=0.06):
    """How many gold amounts appear as X.XX万 (two decimals; ±0.06万 so a model rounding to 0.1万 still counts)."""
    found = [float(m.replace(",", "")) for m in re.findall(r"(\d[\d,]*\.\d{2})\s*万", text)]
    return sum(any(abs(f - g / 1e4) <= tol for f in found) for g in golds)


def yuan_amounts(text):
    """Amounts still written in 元 (≥ 5 digits without 万) — the preference says never."""
    return len(re.findall(r"(?<![\d.])\d{1,3}(?:,\d{3}){1,}(?:\.\d+)?(?!\s*万)|(?<![\d.,])\d{5,}(?:\.\d+)?(?!\s*万)", text))


def first_sentence_has_number(text):
    t = re.sub(r"^[#>\s*\-|]+", "", text.strip())
    first = re.split(r"[。\n]", t, maxsplit=1)[0]
    return bool(re.search(r"\d", first))


def project(fx):
    d = tempfile.mkdtemp(prefix="dp-prefeval-proj-")
    os.makedirs(f"{d}/data")
    for f in ["orders.csv", "master.xlsx"]:
        shutil.copy(f"{fx}/{f}", f"{d}/data/{f}")
    return d


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--memory", choices=["on", "off"], default="on")
    ap.add_argument("--timeout", type=int, default=420)
    ap.add_argument("--config", default=os.path.expanduser("~/Library/Application Support/DataPilot/config.json"))
    a = ap.parse_args()
    fx = base.build_fixtures()
    con = base.gold_db(fx)
    support = tempfile.mkdtemp(prefix="dp-prefeval-sup-")
    cfg = json.load(open(a.config))
    cfg.setdefault("settings", {}).update({"memoryEnabled": a.memory == "on", "defaultMode": "full"})
    json.dump(cfg, open(f"{support}/config.json", "w"))
    core = Core(support)
    results, projs = [], []
    try:
        # session 1: plain chat, teach the preference
        info = core.call("session/open", {"mode": "full"})
        sid = info["session"]
        for f in ["orders.csv", "master.xlsx"]:
            shutil.copy(f"{fx}/{f}", os.path.join(info["workspace"], "uploads", f))
        r1 = core.turn(sid, FILES.replace("data/", "uploads/") + "2025 年已支付订单的总销售额是多少？", a.timeout)
        # turn_finished arrives before the turn releases the session: wait, or the next turn/start is refused
        core.call("session/flush", {"session": sid, "timeout": 300}, timeout=330)
        r2 = core.turn(sid, PREF, a.timeout)
        core.call("session/flush", {"session": sid, "timeout": 300}, timeout=330)
        core.call("session/close", {"session": sid})
        print(f"teach: reasons={r1.get('reason')},{r2.get('reason')} mem={r1['memory'] + r2['memory']}", flush=True)
        for t in SERIES:
            p = project(fx); projs.append(p)
            info = core.call("session/open", {"project_root": p, "mode": "full"})
            sid = info["session"]
            t0 = time.time()
            rec = core.turn(sid, FILES + t["q"], a.timeout)
            golds = [float(r[1]) for r in con.execute(t["gold"]).fetchall()]
            fin = rec.get("final") or rec["answer"]
            rec.update({"id": t["id"], "secs": round(time.time() - t0, 1), "wan": wan_hits(fin, golds), "n": len(golds),
                        "yuan": yuan_amounts(fin), "first_num": first_sentence_has_number(fin)})
            rec["ok"] = rec.get("reason") == "done" and rec["wan"] == rec["n"] and rec["first_num"]
            core.call("session/flush", {"session": sid, "timeout": 300}, timeout=330)
            core.call("session/close", {"session": sid})
            results.append(rec)
            print(f"{'✅' if rec['ok'] else '❌'} {rec['id']:<12} {rec['secs']:>6}s 万元×2位 {rec['wan']}/{rec['n']} "
                  f"元写法={rec['yuan']} 结论先行={'是' if rec['first_num'] else '否'} recalled={'recalled' in rec['memory']}", flush=True)
        core.call("shutdown", timeout=30)
    finally:
        try:
            core.p.wait(timeout=20)
        except Exception:
            core.p.kill()
    md = f"{support}/memory"
    mem = {f: open(f"{md}/{f}", encoding="utf-8").read() for f in sorted(os.listdir(md)) if f.endswith(".md")} if os.path.isdir(md) else {}
    ok = sum(r["ok"] for r in results)
    print(f"\nmemory={a.memory}: {ok}/{len(results)} answers in 万元 with 2 decimals; user memory files: {[f for f in mem if f != 'MEMORY.md']}")
    for f, v in mem.items():
        print(f"--- {f}\n{v.strip()[:600]}")
    out = os.path.join(ROOT, "tests/eval/out"); os.makedirs(out, exist_ok=True)
    json.dump({"memory": a.memory, "model": cfg["roles"]["main"]["model"], "ok": ok, "total": len(results), "results": results, "user_memory": mem},
              open(os.path.join(out, time.strftime("%Y%m%d-%H%M%S") + f"-pref-{a.memory}.json"), "w"), ensure_ascii=False, indent=1)
    for d in [fx, support, *projs]:
        shutil.rmtree(d, ignore_errors=True)


if __name__ == "__main__":
    main()
