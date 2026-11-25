"""Data compare (dataagent.compare): keyed diff, normalisation, duplicates, row-wise, many tables, reconcile, report."""
import ast, os, sys, tempfile
sys.path.insert(0, os.path.dirname(__file__))
from test_kernel import Kernel  # noqa: E402


def main():
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpws"))
    k = Kernel(ws)
    fails = []

    def check(c, m):
        print(("PASS " if c else "FAIL ") + m)
        if not c:
            fails.append(m)

    def val(name):
        x = k.call("exec", code=name)["result"]["value"]
        t = x.get("text") if isinstance(x, dict) else x
        return ast.literal_eval(t) if isinstance(t, str) else t

    def run(code):
        r = k.call("exec", code=code)
        if not r["ok"]:
            print(r.get("error"), r.get("stdout"))
        return r

    r = run("""
import pandas as pd
a = pd.DataFrame({"订单号": [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], "客户": ["张三", "李四 ", "王五", "赵六", "钱七", "孙八"],
                  "金额": [100.0, 200.0, 300.0, 400.0, 500.0, 600.0], "日期": pd.to_datetime(["2026-01-01"] * 6), "备注": list("abcdef")})
b = pd.DataFrame({"order_id": ["1", "2", "3", "4", "7", "7", None], "客户": ["张三", "李四", "王五", "赵六x", "周九", "周九", "空"],
                  "金额": ["100.004", "200", "1,350.5", "400", "1", "2", "3"], "日期": ["2026-01-01", "2026-01-01", "2026-01-02", "2026-01-01", "2026-01-01", "2026-01-01", "2026-01-01"],
                  "新列": [1] * 7})
con.execute("create table ta as select * from a")
con.execute("create table tb as select * from b")
from dataagent.compare import compare, compare_many, reconcile, suggest_keys, schema_diff
r = compare(con, "ta", "tb", keys=["订单号=order_id"], tol=0.01)
print(r["text"])
R = {k: r[k] for k in ("left_only", "right_only", "compared", "same", "diff_rows", "dup_keys_right", "null_keys_right", "matched_keys", "diff_by_column", "left_only_columns", "right_only_columns", "report")}
""")
    check(r["ok"], "compare runs")
    v = val("R")
    check(isinstance(v, dict), f"result dict: {type(v)}")
    if isinstance(v, dict):
        check(v["left_only"] == 2 and v["right_only"] == 2, f"only-left 5,6 / only-right the two rows of key 7: {v['left_only']} {v['right_only']}")
        check(v["dup_keys_right"] == 1 and v["null_keys_right"] == 1, "duplicate key 7 and empty key found")
        check(v["compared"] == 4 and v["diff_rows"] == 2 and v["same"] == 2, f"1.0 = '1', trimmed text, 100.004≈100 within tol: {v['compared']} {v['diff_rows']}")
        check(v["diff_by_column"].get("金额") == 1 and v["diff_by_column"].get("客户") == 1 and v["diff_by_column"].get("日期") == 1, f"by column {v['diff_by_column']}")
        check(v["left_only_columns"] == ["备注"] and v["right_only_columns"] == ["新列"], "one-sided columns")
        check(str(v["report"]).endswith(".xlsx") and os.path.exists(v["report"]), "excel report written")
    r = run("""
d = con.execute("select 列, 左值, 右值, 差额 from cmp_ta_vs_tb_diff where 列 = '金额'").fetchall()
D = [list(map(str, x)) for x in d]
""")
    D = val("D")
    check("1,350.5" in str(D) and "1050.5" in str(D), f"diff row keeps original values and delta: {D}")
    # automatic key + text that is not a number + query input
    r = run("""
x = pd.DataFrame({"客户编码": ["C001", "C002", "C003"], "名称": ["甲", "乙", "丙"], "余额": [1, 2, 3]})
y = pd.DataFrame({"客户编码": ["C001", "C002", "C004"], "名称": ["甲", "乙乙", "丁"], "余额": [1, 2, 9]})
con.execute("create table x as select * from x"); con.execute("create table y as select * from y")
s = suggest_keys(con, "x", "y")
r2 = compare(con, "x", "SELECT * FROM y WHERE 余额 < 100", report=False)
S = (s[0]["keys"], r2["keys"], r2["diff_rows"], r2["left_only"], r2["right_only"], bool(r2["warnings"]))
""")
    S = val("S")
    check(r["ok"] and "客户编码" in str(S) and list(S) == [["客户编码"], ["客户编码"], 1, 1, 1, True], f"auto key + query input + warning to confirm: {S}")
    # whole-row comparison
    r = run("""
p = pd.DataFrame({"a": [1, 2, 2, 3], "b": ["x", "y", "y", "z"]}); q = pd.DataFrame({"a": [1, 2, 4], "b": ["x", "y", "w"]})
con.execute("create table p as select * from p"); con.execute("create table q as select * from q")
r3 = compare(con, "p", "q", keys=[], report=False)
W = (r3["left_only"], r3["right_only"])
""")
    W = val("W")
    check(r["ok"] and list(W) == [2, 1], f"row-wise EXCEPT ALL keeps duplicates: {W}")
    # several tables
    r = run("""
m1 = pd.DataFrame({"id": [1, 2, 3], "v": [1, 2, 3]}); m2 = pd.DataFrame({"id": [1, 2, 3], "v": [1, 5, 3]}); m3 = pd.DataFrame({"id": [1, 2], "v": [1, 5], "extra": [0, 0]})
for n, df in (("m1", m1), ("m2", m2), ("m3", m3)): con.execute(f"create table {n} as select * from df")
import os
rm = compare_many(con, ["m1", "m2", "m3"], keys=["id"], chain=True)
M = (len(rm["pairs"]), rm["pairs"][0]["diff_rows"], rm["pairs"][1]["left_only"], os.path.exists(rm["report"]), list(rm["schema"]["df"]["说明"]))
""")
    M = val("M")
    check(r["ok"] and list(M)[:4] == [2, 1, 1, True] and "部分缺失" in M[4], f"compare_many chain + schema matrix: {M}")
    # reconcile: database-style aggregate vs local detail
    r = run("""
det = pd.DataFrame({"大区": ["华东", "华东", "华北", "华南"], "金额": [10.0, 20.0, 5.0, 7.0]})
agg = pd.DataFrame({"大区": ["华东", "华北", "西南"], "行数": [2, 1, 3], "金额": [30.0, 6.0, 1.0]})
con.execute("create table det as select * from det"); con.execute("create table agg as select * from agg")
rr = reconcile(con, "SELECT 大区, count(*) AS 行数, sum(金额) AS 金额 FROM det GROUP BY 1", "agg", by=["大区"], measures=["行数", "金额"], counts=False, report=False)
C = rr["status"]
""")
    C = val("C")
    check(r["ok"] and C == {"一致": 1, "不一致": 1, "仅左表": 1, "仅右表": 1}, f"reconcile statuses: {C}")
    # table vs a connected database (SQLite): extract then compare; big table: reconcile aggregates pulled from the DB
    import sqlite3
    up = os.path.join(ws, "uploads"); os.makedirs(up, exist_ok=True)
    c = sqlite3.connect(os.path.join(up, "erp.sqlite"))
    c.execute("CREATE TABLE orders(order_id TEXT, region TEXT, amount REAL)")
    c.executemany("INSERT INTO orders VALUES (?,?,?)", [(f"{i:05d}", "华东" if i % 2 else "华北", i * 1.0) for i in range(1, 101)])
    c.execute("UPDATE orders SET amount = 999 WHERE order_id = '00010'")
    c.execute("DELETE FROM orders WHERE order_id = '00100'")
    c.commit(); c.close()
    r = k.call("db_save", conn={"name": "erp", "type": "sqlite", "path": "uploads/erp.sqlite"})
    cid = r["result"]["connection"]["id"]
    r = k.call("db_extract", connection=cid, tables=[{"table": "orders", "as": "db_orders"}])
    check(r["ok"], f"extract from db {r.get('error')}")
    r = k.call("db_extract", connection=cid, sql="SELECT region, count(*) AS 行数, sum(amount) AS 金额 FROM orders GROUP BY region", **{"as": "db_orders_agg"})
    check(r["ok"], f"extract aggregate from db {r.get('error')}")
    r = run("""
loc = pd.DataFrame({"订单号": list(range(1, 101)), "大区": ["华东" if i % 2 else "华北" for i in range(1, 101)], "金额": [float(i) for i in range(1, 101)]})
con.execute("create table 本地订单 as select * from loc")
rd = compare(con, "本地订单", "db_orders", keys=["订单号=order_id"], columns={"大区": "region", "金额": "amount"}, key_steps=["strip_zeros"], report=False)
ra = reconcile(con, "SELECT 大区 AS region, count(*) AS 行数, sum(金额) AS 金额 FROM 本地订单 GROUP BY 1", "db_orders_agg", by=["region"], measures=["行数", "金额"], counts=False, report=False)
B = (rd["matched_keys"], rd["diff_rows"], rd["left_only"], rd["right_only"], rd["diff_by_column"], ra["status"])
""")
    B = val("B")
    check(r["ok"] and list(B)[:4] == [99, 1, 1, 0] and B[4].get("金额 ↔ amount") == 1, f"table vs database detail (00010 changed, 00100 missing, '00007' = 7): {B}")
    check(r["ok"] and B[5] == {"不一致": 1, "一致": 1}, f"table vs database totals: {B[5] if r['ok'] else ''}")
    # bad key name gives a helpful error
    r = k.call("exec", code="compare(con, 'ta', 'tb', keys=['不存在'])")
    check(not r["ok"] and "没有对应的列" in (r.get("error") or ""), "unknown key → clear error")
    k.close()
    print("ALL PASS" if not fails else f"{len(fails)} FAIL")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
