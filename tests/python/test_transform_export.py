"""data_transform / data_export / dictionary tests under the real sandbox."""
import os, sys, tempfile, json
sys.path.insert(0, os.path.dirname(__file__))
from test_kernel import Kernel  # noqa: E402


def main():
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpws"))
    k = Kernel(ws)
    fails = []

    def check(cond, msg):
        print(("PASS " if cond else "FAIL ") + msg)
        if not cond:
            fails.append(msg)

    k.call("exec", code="con.execute(\"create table orders as select range id, (range % 3) region, range*10.0 amount, range*6.0 cost, "
                        "case when range % 5 = 0 then '退货' else '正常' end status from range(100)\")")
    r = k.call("transform", sql="CREATE TABLE gm AS SELECT region, sum(amount-cost) AS 毛利, sum(amount-cost)/sum(amount) AS 毛利率 "
                                "FROM orders WHERE status <> '退货' GROUP BY region")
    check(r["ok"] and r["result"]["rows"] == 3, f"create derived table: {r.get('error')}")
    r = k.call("transform", sql="ALTER TABLE gm ADD COLUMN 等级 VARCHAR")
    r2 = k.call("transform", sql="UPDATE gm SET 等级 = CASE WHEN 毛利 > 1000 THEN '高' ELSE '低' END")
    check(r["ok"] and r2["ok"], f"alter+update derived: {r.get('error')} {r2.get('error')}")
    for bad in ["DROP TABLE orders", "UPDATE orders SET amount=0", "CREATE OR REPLACE TABLE orders AS SELECT 1",
                "COPY orders TO 'x.csv'", "CREATE TABLE z AS SELECT * FROM read_csv('/etc/passwd')"]:
        r = k.call("transform", sql=bad)
        check(not r["ok"], f"transform rejects: {bad}")
    r = k.call("transform", sql="CREATE TABLE gm AS SELECT 1")
    check(not r["ok"] and "CREATE OR REPLACE" in r["error"], "existing derived needs OR REPLACE")
    d = k.call("derived")["result"]["tables"]
    check(len(d) == 1 and d[0]["deps"] == ["orders"] and len(d[0]["steps"]) == 2, f"lineage recorded: {d}")

    # rebuild after source change
    k.call("exec", code="con.execute('insert into orders select 1000, 0, 1e6, 0, \\'正常\\'')")
    r = k.call("rebuild_derived", changed=["orders"])
    check(r["ok"] and r["result"]["rebuilt"] == ["gm"] and not r["result"]["failed"], f"rebuild: {r.get('result') or r.get('error')}")
    r = k.call("sql", sql="select 等级 from gm where region=0")
    check(r["ok"] and r["result"]["rows"][0][0] == "高", "rebuilt table replays ALTER/UPDATE steps")

    # export
    r = k.call("export", table="gm", format="csv", filename="毛利")
    p = r["result"]["path"] if r["ok"] else ""
    check(r["ok"] and r["result"]["rows"] == 3 and open(p, "rb").read(3) == b"\xef\xbb\xbf", f"csv export with BOM: {r.get('error')}")
    r = k.call("export", table="gm", format="csv", filename="毛利")
    check(r["ok"] and r["result"]["path"].endswith("毛利_2.csv"), "no overwrite: numbered name")
    r = k.call("export", sql="select region, amount from orders where amount > 500", format="xlsx", columns={"region": "地区", "amount": "金额"})
    check(r["ok"] and r["result"]["path"].endswith(".xlsx"), f"xlsx export with renamed columns: {r.get('error')}")
    q = k.call("sql", sql="select * from orders limit 7")
    r = k.call("export", result_id=q["result"]["result_id"], format="parquet")
    check(r["ok"] and r["result"]["rows"] == 7, f"export saved result: {r.get('error')}")
    r = k.call("export", sql="select 12345678901234567 big, 1.5::decimal(10,2) d, date '2026-01-02' dt, TIMESTAMPTZ '2026-01-02 03:04:05+00' ts, NULL n, true b, repeat('x', 40000) long", format="xlsx", filename="cells")
    ok = r["ok"]
    if ok:
        import openpyxl
        ws_ = openpyxl.load_workbook(r["result"]["path"]).active
        v = [c.value for c in ws_[2]]
        ok = v[0] == "12345678901234567" and v[1] == 1.5 and str(v[2]).startswith("2026-01-02") and v[4] is None and v[5] is True and len(v[6]) == 32767
    check(ok, f"xlsx cell chain: big int as text, decimal, dates, null, bool, 32767 cap: {r.get('error') or (v if r['ok'] else '')}")
    r = k.call("export", sql="DROP TABLE orders", format="csv")
    check(not r["ok"], "export rejects write SQL")
    for fmt in ("xlsx", "md", "html", "docx", "pdf"):
        r = k.call("export", kind="dictionary", format=fmt)
        check(r["ok"] and os.path.getsize(r["result"]["path"]) > 100, f"dictionary {fmt}: {r.get('error')}")
    k.call("exec", ns="c1", code="import pandas as pd\nmydf = pd.DataFrame({'a':[1,2,3]})")
    r = k.call("sql", ns="c1", sql="select sum(a) from mydf")
    check(r["ok"] and r["result"]["rows"][0][0] == 6, f"chat DataFrame queryable from SQL: {r.get('error')}")
    r = k.call("sql", ns="c2", sql="select sum(a) from mydf")
    check(not r["ok"], "other chat cannot see it")
    r = k.call("transform", sql="DROP TABLE gm")
    check(r["ok"] and k.call("derived")["result"]["tables"] == [], "drop derived clears lineage")
    # import column mapping: preview, rename / select, force types
    with open(os.path.join(ws, "uploads", "m.csv"), "w", encoding="utf-8") as f:
        f.write("编码,日期,金额,备注\n001,2026年1月2日,1,x\n002,2026年1月3日,2,y\n")
    r = k.call("import", path="uploads/m.csv", preview=True)
    check(r["ok"] and r["result"]["preview"] and r["result"]["tables"][0]["rows"] == 2, f"import preview: {r.get('error')}")
    check(not k.call("sql", sql="select 1 from m")["ok"], "preview writes nothing")
    r = k.call("import", path="uploads/m.csv", types={"不存在": "text"})
    check(not r["ok"] and "type mapping" in r.get("error", ""), "unknown column in types gives a clear error")
    r = k.call("import", path="uploads/m.csv", types={"编码": "text", "日期": "date"}, columns={"编码": "code", "日期": "dt", "金额": "amt", "__keep_only__": True})
    q = k.call("sql", sql="select * from m")
    ok = r["ok"] and q["ok"] and q["result"]["columns"] == ["code", "dt", "amt"] and q["result"]["rows"][0][0] == "001"
    check(ok, f"rename + keep only: {r.get('error') or q.get('error') or q['result']['columns']}")
    k.close()
    print(f"\n{len(fails)} failures")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
