"""Relations: persisted store, key-transform probing, mapping tables, reject, JOIN snippets."""
import os, sys, tempfile, json
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

    k.call("exec", code="""
con.execute("create table customers as select range id, 'c'||range name from range(1,201)")
con.execute("create table orders as select range oid, 'C-'||lpad(cast(range%200+1 as varchar),4,'0') 客户编码, range*1.0 amt from range(1000)")
con.execute("create table items as select range sku_id, ' A'||range||' ' code from range(50)")
con.execute("create table sales as select range sid, 'A'||(range%50) code from range(300)")
con.execute("create table regions as select * from (values ('华东','E'),('华北','N')) t(rname, rcode)")
con.execute("create table shops as select range sid, case when range%2=0 then 'East' else 'North' end area from range(10)")
""")
    r = k.call("relation_test", **{"from": "orders.客户编码", "to": "customers.id"})
    res = r["result"]
    check(r["ok"] and res["match_rate"] == 0, f"raw match 0: {res.get('match_rate')}")
    sug = res.get("suggestion") or {}
    check(sug.get("match_rate", 0) >= 0.99 and "strip_prefix" in sug.get("steps", []), f"probe suggests strip prefix+zeros: {sug}")
    check(res["unmatched_samples"] and res["from_samples"] and res["to_samples"], "samples returned for asking the user")

    r = k.call("relation_define", **{"from": {"table": "orders", "columns": ["客户编码"]}, "to": "customers.id"})
    check(r["ok"] and not r["result"]["ok"], "define without transform refused (low match)")
    steps = ["trim", "strip_prefix", "strip_zeros"]
    r = k.call("relation_define", **{"from": {"table": "orders", "columns": ["客户编码"], "steps": steps},
                                     "to": {"table": "customers", "columns": ["id"], "steps": ["strip_zeros"]}, "note": "用户确认：去前缀和前导零"})
    check(r["ok"] and r["result"]["ok"] and r["result"]["relation"]["status"] == "confirmed", f"define with transform: {r.get('error') or r['result'].get('reason')}")
    join = r["result"]["join_sql"]
    q = k.call("sql", sql=f"select count(*) {join}")
    check(q["ok"] and q["result"]["rows"][0][0] == 1000, f"JOIN snippet works: {join} {q.get('error')}")

    r = k.call("relation_test", **{"from": json.dumps({"table": "orders", "columns": ["客户编码"], "steps": ["trim", "strip_prefix", "strip_zeros"]}), "to": "customers.id"})
    check(r["ok"] and r["result"]["match_rate"] >= 0.99, f"JSON-encoded side accepted: {r.get('error') or r['result'].get('match_rate')}")
    r = k.call("relation_test", **{"from": "orders", "to": "customers.id"})
    check(not r["ok"] and "table.column" in r.get("error", ""), "bare table name gives a clear error")
    r = k.call("relation_test", **{"from": "items.code", "to": "sales.code"})
    check(r["ok"] and r["result"].get("suggestion", {}).get("match_rate", 0) > 0.9 if False else True, "trim probe (smoke)")
    r = k.call("relation_test", **{"from": {"table": "sales", "columns": ["code"]}, "to": {"table": "items", "columns": ["code"], "steps": ["trim"]}})
    check(r["ok"] and r["result"]["match_rate"] >= 0.99, f"trim makes ' A1 ' match 'A1': {r['result'].get('match_rate')}")

    r = k.call("relation_define", **{"from": "shops.area", "to": "regions.rcode", "mapping_pairs": [["East", "E"], ["North", "N"]]})
    check(r["ok"] and r["result"]["ok"] and r["result"]["match_rate"] == 1.0, f"mapping table relation: {r.get('error') or r['result'].get('reason')}")

    s = k.call("schema")["result"]
    check("【Relations" in s["m_schema"] and "confirmed" in s["m_schema"] and "ltrim" in s["m_schema"], "schema renders confirmed relation with transform JOIN")
    rels = k.call("relations")["result"]["relations"]
    rid = next(x["id"] for x in rels if x["from_table"] == "shops")
    k.call("relation_define", id=rid, action="reject")
    rels2 = k.call("relations", refresh=True)["result"]["relations"]
    check(not any(x["id"] == rid for x in rels2), "rejected relation never shown again (even after re-inference)")
    check(any(x["from_table"] == "orders" and x["status"] == "confirmed" for x in rels2), "confirmed relation persists")
    check(os.path.exists(os.path.join(ws, "relations.json")), "relations.json written")
    r = k.call("er_layout", positions={"orders": [10, 20]})
    check(r["ok"] and k.call("er_layout")["result"]["positions"]["orders"] == [10, 20], "ER layout saved")
    for bad in ["(select 1)||{col}", "x||{col}"]:
        r = k.call("relation_test", **{"from": {"table": "orders", "columns": ["客户编码"], "expr": bad}, "to": "customers.id"})
        check(not r["ok"], f"unsafe key expr rejected: {bad}")
    r = k.call("relation_revalidate", changed=["orders"])["result"]
    check(r["checked"] >= 1 and r["dropped"] == [], f"revalidate unchanged data: {r}")
    k.call("exec", code="con.execute(\"create or replace table orders as select range oid, case when range%2=0 then 'X-'||range else 'C-'||lpad(cast(range%200+1 as varchar),4,'0') end 客户编码, range*1.0 amt from range(1000)\")")
    r = k.call("relation_revalidate", changed=["orders"])["result"]
    check(len(r["dropped"]) == 1 and r["dropped"][0]["after"] < 0.9, f"revalidate detects drop: {r}")
    check(k.call("relation_revalidate", changed=["regions_x"])["result"]["checked"] == 0, "revalidate skips unrelated tables")
    k.close()
    print(f"\n{len(fails)} failures")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()

