"""End-to-end kernel tests: run kernel.py under the real Seatbelt sandbox via JSON-lines."""
import json, os, subprocess, sys, tempfile, time
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
PY = os.path.join(ROOT, "build/vendor/python/bin/python3")
RES = os.path.join(ROOT, "Resources")

class Kernel:
    def __init__(self, ws):
        tmp = os.path.realpath(tempfile.mkdtemp(prefix="dptmp"))
        pol = open(os.path.join(RES, "sandbox/base.sbpl")).read()
        cmd = ["/usr/bin/sandbox-exec", "-p", pol, f"-DPYROOT={os.path.realpath(os.path.dirname(os.path.dirname(PY)))}",
               f"-DRESROOT={os.path.realpath(RES)}", f"-DWORKSPACE={ws}", f"-DTMPDIR={tmp}", "--",
               PY, "-I", "-u", os.path.join(RES, "kernel/kernel.py")]
        env = {"DP_WORKSPACE": ws, "TMPDIR": tmp, "HOME": ws, "PATH": "/usr/bin:/bin", "MPLCONFIGDIR": tmp}
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
        self.ready = json.loads(self.p.stdout.readline())
        self.n = 0
    def call(self, op, **kw):
        self.n += 1
        self.p.stdin.write(json.dumps({"id": str(self.n), "op": op, **kw}, ensure_ascii=False) + "\n"); self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise RuntimeError(self.p.stderr.read())
        return json.loads(line)
    def close(self):
        self.call("shutdown"); self.p.wait(5)

def main():
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpws"))
    fx = os.path.join(ws, "uploads")
    os.makedirs(fx)
    subprocess.check_call([PY, "-I", os.path.join(ROOT, "tests/python/make_fixtures.py"), fx], stdout=subprocess.DEVNULL)
    t0 = time.time(); k = Kernel(ws); print(f"kernel ready in {time.time()-t0:.2f}s", k.ready["result"]["duckdb"])
    fails = []
    def check(cond, msg):
        print(("PASS " if cond else "FAIL ") + msg)
        if not cond: fails.append(msg)

    r = k.call("import", path="uploads/orders.csv")
    check(r["ok"], f"import GBK csv: {r.get('error')}")
    t = r["result"]["tables"][0]
    check(t["rows"] == 20000 and t["encoding"] in ("gb18030", "gbk", "gb2312"), f"orders rows={t['rows']} enc={t['encoding']} strategy={t['strategy']}")
    check(t["strategy"] == "sql", "large table → sql strategy")
    r = k.call("import", path="uploads/master.xlsx")
    check(r["ok"], f"import xlsx: {r.get('error')}")
    names = {x["table"]: x for x in r["result"]["tables"]}
    check(set(names) == {"master_customers", "master_products"}, f"sheets → tables {list(names)}")
    cust_cols = [c["name"] for c in names["master_customers"]["profile"]["columns"]]
    check(cust_cols[:2] == ["customer_id", "客户名称"], f"title row skipped, header={cust_cols}")
    check(names["master_customers"]["strategy"] == "pandas+sql", "small table → pandas+sql")
    region = [c for c in names["master_customers"]["profile"]["columns"] if c["name"] == "大区"][0]
    check(region.get("low_cardinality") and "华东区" in region["values"], "low-card values profiled")

    r = k.call("schema")
    rels = r["result"]["relations"]
    pairs = {(x["from_table"], x["from_column"], x["to_table"], x["to_column"]) for x in rels}
    check(("orders", "customer_id", "master_customers", "customer_id") in pairs, "FK orders.customer_id → customers")
    check(("orders", "product_id", "master_products", "product_id") in pairs, f"FK orders.product_id → products; got {pairs}")
    check("【Relations" in r["result"]["m_schema"] and "JOIN" in r["result"]["m_schema"], "m_schema has relations section with JOIN snippets")

    r = k.call("sql", sql='SELECT c."大区", sum(o."数量"*p."单价") AS gmv FROM orders o JOIN master_customers c USING(customer_id) JOIN master_products p USING(product_id) WHERE o."状态"=\'paid\' GROUP BY 1 ORDER BY 2 DESC')
    check(r["ok"] and r["result"]["row_count"] == 5, f"join query: {r.get('error')}")
    check(os.path.exists(r["result"]["artifact"]), "result parquet saved")
    for bad in ["DROP TABLE orders", "DELETE FROM orders", "SELECT 1; DROP TABLE orders", "CREATE TABLE x AS SELECT 1", "COPY orders TO 'x.csv'"]:
        r = k.call("sql", sql=bad)
        check(not r["ok"], f"guard rejects: {bad}")
    r = k.call("sql", sql="SELECT * FROM orders WHERE \"状态\"='退款'")
    check(r["ok"] and r["result"]["row_count"] == 0 and "hint" in r["result"], "empty result hint")

    r = k.call("exec", code="top = master_customers.groupby('大区').size().reset_index(name='n')\ntop")
    check(r["ok"] and r["result"]["value"]["type"] == "dataframe", f"python exec: {r.get('error')}")
    check(any(c["name"] == "top" for c in r["result"]["changed_dataframes"]), "df change detected")
    r = k.call("exec", code="open('/Users/Shared/dp_escape.txt','w')")
    check(not r["ok"] and "PermissionError" in r["error"], "sandbox blocks write outside workspace")
    r = k.call("exec", code="import urllib.request\nurllib.request.urlopen('https://pypi.org', timeout=3)")
    check(not r["ok"], "sandbox blocks network")
    k.close()

    # resume: tables + checkpointed DataFrames restored
    t0 = time.time(); k2 = Kernel(ws); dt = time.time() - t0
    st = k2.ready["result"]
    check("orders" in st["tables"] and "top" in st["restored_vars"], f"resume restores tables+vars in {dt:.2f}s: {st['restored_vars']}")
    r = k2.call("exec", code="len(top)")
    check(r["ok"] and r["result"]["value"]["text"] == "5", "restored df usable")
    k2.close()
    print(f"\n{len(fails)} failures")
    sys.exit(1 if fails else 0)

if __name__ == "__main__":
    main()
