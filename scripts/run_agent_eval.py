"""End-to-end agent eval: runs tests/eval/agent_tasks.json through the Rust core (datapilot-core app-server) against a
real model and checks the answers against gold SQL computed independently from the fixture files.

usage: DP_API_KEY=... build/vendor/python/bin/python3 -I scripts/run_agent_eval.py [--only id,id] [--jobs 4] [--config PATH]
Model/provider come from the app's config.json (roles.main). Writes tests/eval/out/<timestamp>.json.
"""
import argparse, concurrent.futures as cf, glob, json, os, re, shutil, subprocess, sys, tempfile, threading, time
import duckdb, pandas as pd

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
PY = os.path.join(ROOT, "build/vendor/python/bin/python3")
CORE = os.environ.get("DP_CORE_BIN") or os.path.join(ROOT, "datapilot-core/target/release/datapilot-core")


def build_fixtures():
    d = tempfile.mkdtemp(prefix="dp-evalfx-")
    subprocess.check_call([PY, "-I", os.path.join(ROOT, "tests/eval/make_eval_fixtures.py"), d], stdout=subprocess.DEVNULL)
    return d


def gold_db(fx):
    con = duckdb.connect()
    con.register("orders_df", pd.read_csv(f"{fx}/orders.csv", encoding="gb18030", parse_dates=["下单日期"]))
    con.register("customers_df", pd.read_excel(f"{fx}/master.xlsx", sheet_name="customers", skiprows=2))
    con.register("products_df", pd.read_excel(f"{fx}/master.xlsx", sheet_name="products"))
    con.register("crm_df", pd.read_csv(f"{fx}/crm_contacts.csv"))
    con.register("targets_df", pd.read_csv(f"{fx}/targets_2025.csv"))
    con.register("targets26_df", pd.read_csv(f"{fx}/targets_2026.csv"))
    con.register("returns_df", pd.read_csv(f"{fx}/returns_log.csv"))
    for t in ["orders", "customers", "products", "crm", "targets", "targets26", "returns"]:
        con.execute(f"CREATE TABLE {t} AS SELECT * FROM {t}_df")
    return con


NUM = re.compile(r"(-?\d[\d,]*(?:\.\d+)?)\s*(%|万|亿)?")


def numbers(text):
    out = []
    for m in NUM.finditer(text):
        try:
            v = float(m.group(1).replace(",", ""))
        except ValueError:
            continue
        u = m.group(2)
        out.append((v, u))
    return out


def has_num(text, g, tol=None):
    for v, u in numbers(text):
        x = v * (1e4 if u == "万" else 1e8 if u == "亿" else 1)
        if u == "%":
            continue
        t = tol if tol is not None else max(0.5, abs(g) * 0.005)
        if abs(x - g) <= t:
            return True
        if u in ("万", "亿") and abs(x - g) <= abs(g) * 0.01:   # 12.3万 style rounding
            return True
    return False


DECLINE = ("下降", "减少", "下滑", "降低", "负增长", "decrease", "decline", "down")


def has_pct(text, g):
    neg_words = any(w in text for w in DECLINE)
    for v, u in numbers(text):
        if u == "%" and abs(v - g * 100) <= 0.15:
            return True
        # "下降 1.87%" states -1.87% without the minus sign
        if u == "%" and g < 0 and neg_words and abs(v + g * 100) <= 0.15:
            return True
        if u is None and abs(g) < 5 and abs(v - g) <= 0.0015 and "." in str(v):
            return True
    return False


def check(task, text, ws, con):
    c = task["check"]
    problems = []
    rows = con.execute(task["gold"]).fetchall()
    if c.get("rows") and len(rows) != c["rows"]:
        problems.append(f"gold has {len(rows)} rows")
    positions = []
    for r in rows:
        for i in c.get("text_cols", []):
            s = str(r[i])
            p = text.find(s)
            if p < 0:   # "1月" vs "1 月"
                p = re.sub(r"\s+", "", text).find(re.sub(r"\s+", "", s))
            if p < 0:
                problems.append(f"missing text {s}")
            positions.append(p)
        for i in c.get("num_cols", []):
            if not has_num(text, float(r[i]), c.get("tol")):
                problems.append(f"missing number {r[i]:.4f}" if isinstance(r[i], float) else f"missing number {r[i]}")
        for i in c.get("pct_cols", []):
            if not has_pct(text, float(r[i])):
                problems.append(f"missing pct {float(r[i]) * 100:.2f}%")
    if c.get("ordered") and all(p >= 0 for p in positions) and positions != sorted(positions):
        problems.append("wrong order")
    if c.get("table"):
        try:
            k = duckdb.connect(os.path.join(ws, "data.duckdb"), read_only=True)
            n = k.execute(f'SELECT count(*) FROM "{c["table"]}"').fetchone()[0]
            k.close()
            if n != rows[0][0]:
                problems.append(f"table {c['table']} has {n} rows, expected {rows[0][0]}")
        except Exception as e:
            problems.append(f"table {c['table']}: {str(e)[:120]}")
    if c.get("file"):
        files = glob.glob(os.path.join(ws, "exports", "**", c["file"]), recursive=True) + glob.glob(os.path.join(ws, "outputs", "**", c["file"]), recursive=True)
        if not files:
            problems.append("no exported file")
        else:
            try:
                n = len(pd.read_csv(files[0], encoding="utf-8-sig"))
                if n != rows[0][c.get("file_rows_col", 0)]:
                    problems.append(f"file has {n} rows, expected {rows[0][0]}")
            except Exception as e:
                problems.append(f"bad csv: {e}")
    if c.get("dashboard"):
        ds = glob.glob(os.path.join(ws, "dashboards", "*.json"))
        if not ds:
            problems.append("no dashboard")
        else:
            spec = json.load(open(ds[0]))
            types = [w.get("type") for w in spec.get("widgets", [])]
            if "kpi" not in types or not ({"bar", "hbar"} & set(types)) or not ({"line", "area"} & set(types)):
                problems.append(f"widgets {types}")
            if not any("paid" in (d.get("sql") or "") for d in spec.get("datasets", {}).values()):
                problems.append("datasets do not filter paid")
    return problems


class Core:
    def __init__(self, support):
        self.p = subprocess.Popen([CORE], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                                  env={**os.environ, "DP_APP_SUPPORT": support})
        self.n = 0

    def send(self, method, params=None):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        return self.n

    def read(self):
        line = self.p.stdout.readline()
        if not line:
            raise RuntimeError("core exited")
        return json.loads(line)


def run_task(task, files_note, fx, config, con, timeout):
    support = tempfile.mkdtemp(prefix=f"dp-eval-{task['id']}-")
    cfg = json.load(open(config))
    cfg.setdefault("settings", {}).update({"memoryEnabled": False, "defaultMode": "full"})
    json.dump(cfg, open(f"{support}/config.json", "w"))
    core = Core(support)
    t0 = time.time()
    rec = {"id": task["id"], "q": task["q"], "tools": [], "errors": 0, "rounds": 0}
    try:
        rid = core.send("session/open", {"mode": "full"})
        while True:
            v = core.read()
            if v.get("id") == rid:
                break
        if "error" in v:
            raise RuntimeError(v["error"]["message"])
        sid, ws = v["result"]["session"], v["result"]["workspace"]
        for f in os.listdir(fx):
            shutil.copy(os.path.join(fx, f), os.path.join(ws, "uploads", f))
        core.send("turn/start", {"session": sid, "text": files_note + "\n\n" + task["q"]})
        texts, reason = [], ""
        while True:
            if time.time() - t0 > timeout:
                core.send("turn/cancel", {"session": sid}); reason = "timeout"; break
            v = core.read()
            e = (v.get("params") or {}).get("event")
            if not e:
                continue
            t = e["type"]
            if t == "assistant_message":
                rec["rounds"] += 1
                if e["message"].get("content"):
                    texts.append(e["message"]["content"])
            elif t == "tool_finished":
                rec["tools"].append(e["message"].get("name"))
                rec["errors"] += bool(e["is_error"])
            elif t == "approval_required":
                core.send("approval/respond", {"session": sid, "id": e["request"]["id"], "choice": "allowOnce"})
            elif t == "usage":
                rec["prompt_tokens"] = e["usage"]["prompt_tokens"]
            elif t == "error":
                texts.append("[error] " + e["text"])
            elif t == "turn_finished":
                reason = e["reason"]; break
        rec["reason"] = reason
        rec["answer"] = "\n".join(texts)
        core.send("shutdown")
        try:
            core.p.wait(timeout=30)
        except subprocess.TimeoutExpired:
            core.p.kill()
        rec["problems"] = check(task, rec["answer"], ws, con) if reason == "done" else [f"reason={reason}"]
    except Exception as ex:
        rec["problems"] = [f"runner: {ex}"]
        core.p.kill()
    rec["secs"] = round(time.time() - t0, 1)
    rec["ok"] = not rec["problems"]
    shutil.rmtree(support, ignore_errors=True)
    return rec


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--timeout", type=int, default=420)
    ap.add_argument("--config", default=os.path.expanduser("~/Library/Application Support/DataPilot/config.json"))
    ap.add_argument("--label", default="")
    a = ap.parse_args()
    if not os.environ.get("DP_API_KEY"):
        sys.exit("DP_API_KEY is not set")
    spec = json.load(open(os.path.join(ROOT, "tests/eval/agent_tasks.json")))
    tasks = [t for t in spec["tasks"] if not a.only or t["id"] in a.only.split(",")]
    fx = build_fixtures()
    con = gold_db(fx)
    lock = threading.Lock()
    results = []

    def one(t):
        with lock:
            c = con.cursor()
        r = run_task(t, spec["files"], fx, a.config, c, a.timeout)
        with lock:
            results.append(r)
            print(f"{'✅' if r['ok'] else '❌'} {r['id']:<20} {r['secs']:>6}s rounds={r['rounds']:<2} errs={r['errors']} "
                  + ("" if r["ok"] else "; ".join(r["problems"])[:200]), flush=True)
        return r

    with cf.ThreadPoolExecutor(a.jobs) as ex:
        list(ex.map(one, tasks))
    results.sort(key=lambda r: [t["id"] for t in tasks].index(r["id"]))
    ok = sum(r["ok"] for r in results)
    model = json.load(open(a.config))["roles"]["main"]["model"]
    summary = {"model": model, "label": a.label, "ok": ok, "total": len(results),
               "avg_rounds": round(sum(r["rounds"] for r in results) / max(1, len(results)), 2),
               "avg_secs": round(sum(r["secs"] for r in results) / max(1, len(results)), 1),
               "tool_errors": sum(r["errors"] for r in results)}
    print(f"\n{model}: {ok}/{len(results)} correct, avg rounds {summary['avg_rounds']}, avg {summary['avg_secs']}s, tool errors {summary['tool_errors']}")
    out = os.path.join(ROOT, "tests/eval/out")
    os.makedirs(out, exist_ok=True)
    json.dump({"summary": summary, "results": results}, open(os.path.join(out, time.strftime("%Y%m%d-%H%M%S") + (f"-{a.label}" if a.label else "") + ".json"), "w"),
              ensure_ascii=False, indent=1)
    shutil.rmtree(fx, ignore_errors=True)


if __name__ == "__main__":
    main()
