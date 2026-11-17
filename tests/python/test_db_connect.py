"""Direct DB connections against SQLite / DuckDB files: connect, metadata, FK→relations, extract, refresh,
live read-only query, guard, password never persisted, dialect transpile, saved queries, format/explain, paging."""
import os, sys, tempfile, json, sqlite3, subprocess
sys.path.insert(0, os.path.dirname(__file__))
from test_kernel import Kernel, PY  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def main():
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpws"))
    up = os.path.join(ws, "uploads"); os.makedirs(up, exist_ok=True)
    sq = os.path.join(up, "shop.sqlite")
    c = sqlite3.connect(sq)
    c.executescript("""
    PRAGMA foreign_keys=ON;
    CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT, city TEXT);
    CREATE TABLE orders(oid INTEGER PRIMARY KEY, cust_id INTEGER REFERENCES customers(id), amount REAL, day TEXT);
    CREATE VIEW big_orders AS SELECT * FROM orders WHERE amount > 500;
    """)
    c.executemany("INSERT INTO customers VALUES (?,?,?)", [(i, f"客户{i}", "上海" if i % 2 else "北京") for i in range(1, 51)])
    c.executemany("INSERT INTO orders VALUES (?,?,?,?)", [(i, i % 50 + 1, i * 3.5, f"2024-01-{i % 28 + 1:02d}") for i in range(1, 1001)])
    c.commit(); c.close()
    ddb = os.path.join(up, "wh.duckdb")
    subprocess.check_call([PY, "-I", "-c", f"""
import duckdb; c = duckdb.connect({ddb!r})
c.execute("create table dim(k integer primary key, label varchar)")
c.execute("create table fact(id integer, k integer references dim(k), v double)")
c.execute("insert into dim select range, 'L'||range from range(10)")
c.execute("insert into fact select range, range%10, range*1.5 from range(5000)")
c.close()"""])

    k = Kernel(ws)
    fails = []

    def check(cond, m):
        print(("PASS " if cond else "FAIL ") + m)
        if not cond:
            fails.append(m)

    r = k.call("db_types")
    check(r["ok"] and {"postgres", "mysql", "sqlite", "duckdb", "oracle", "sqlserver", "clickhouse"} <= {t["type"] for t in r["result"]["types"]}, "db_types lists 7 types")

    r = k.call("db_save", conn={"name": "shop", "type": "sqlite", "path": "uploads/shop.sqlite", "password": "s3cret!"})
    check(r["ok"], f"save sqlite conn {r.get('error')}")
    cid = r["result"]["connection"]["id"]
    raw = open(os.path.join(ws, "connections.json"), encoding="utf-8").read()
    check("s3cret!" not in raw and "password" not in raw, "password never written to connections.json")

    r = k.call("db_test", connection=cid)
    check(r["ok"] and r["result"]["tables"] == 3 and "SQLite" in r["result"]["version"], f"test: {r.get('result') or r.get('error')}")

    r = k.call("db_schema", connection=cid)
    res = r.get("result") or {}
    check(r["ok"] and any(f["from_table"] == "orders" and f["to_table"] == "customers" for f in res.get("foreign_keys", [])), f"FK read {res.get('foreign_keys')}")
    check("sqlite" in res.get("text", "") and "cust_id" in res.get("text", ""), "schema text has dialect + columns")

    r = k.call("db_query", connection=cid, sql="SELECT city, count(*) n, sum(amount) s FROM orders o JOIN customers c ON c.id=o.cust_id GROUP BY city ORDER BY city")
    check(r["ok"] and r["result"]["row_count"] == 2 and r["result"]["rows"][0][0] == "上海", f"live query {r.get('error')}")
    rid = r["result"]["result_id"] if r["ok"] else None

    for bad in ["DELETE FROM orders", "UPDATE orders SET amount=0", "DROP TABLE orders",
                "SELECT 1; DELETE FROM orders", "ATTACH 'x.db' AS x", "CREATE TABLE t AS SELECT 1",
                "SELECT load_extension('x')", "PRAGMA writable_schema=1"]:
        r = k.call("db_query", connection=cid, sql=bad)
        check(not r["ok"], f"blocked: {bad} -> {(r.get('error') or '')[:70]}")
    n = sqlite3.connect(sq).execute("select count(*) from orders").fetchone()[0]
    check(n == 1000, "remote data untouched")

    r = k.call("db_query", connection=cid, sql="SELECT * FROM orders", limit=100)
    check(r["ok"] and r["result"]["row_count"] == 100 and r["result"]["capped"], "row cap applied")
    r = k.call("db_query", connection=cid, sql="SELECT strftime(CAST(day AS DATE), '%Y') y, count(*) FROM orders GROUP BY 1", dialect="duckdb")
    check(r["ok"] and "STRFTIME" in r["result"]["sql"].upper(), f"duckdb→sqlite transpile {r.get('error') or r['result']['sql']}")

    # extract both tables: FK becomes a confirmed relation (source=db)
    r = k.call("db_extract", connection=cid, tables=["customers", "orders"])
    check(r["ok"] and [t["rows"] for t in r["result"]["tables"]] == [50, 1000], f"extract {r.get('error') or r['result']['tables']}")
    rels = r["result"]["relations"] if r["ok"] else []
    check(len(rels) == 1 and rels[0]["source"] == "db" and rels[0]["status"] == "confirmed" and rels[0]["match_rate"] == 1.0, f"FK → relation {rels}")
    r = k.call("sql", sql="select count(*) from orders o join customers c on c.id=o.cust_id")
    check(r["ok"] and r["result"]["rows"][0][0] == 1000, "local join on extracted tables")
    r = k.call("db_extract", connection=cid, sql="SELECT * FROM orders WHERE amount > 3000", **{"as": "大单"})
    check(r["ok"] and r["result"]["tables"][0]["as"] == "大单" and r["result"]["tables"][0]["rows"] == 143, f"extract query {r.get('error') or r['result']}")

    # derived table on top + refresh after remote change
    k.call("transform", sql="CREATE TABLE city_sum AS SELECT city, sum(amount) s FROM orders o JOIN customers c ON c.id=o.cust_id GROUP BY city")
    c = sqlite3.connect(sq); c.execute("INSERT INTO orders VALUES (5000, 1, 99999, '2024-02-01')"); c.commit(); c.close()
    r = k.call("db_refresh", connection=cid)
    res = r.get("result") or {}
    check(r["ok"] and {t["as"]: t["rows"] for t in res.get("tables", [])}.get("orders") == 1001, f"refresh re-pulled {r.get('error')}")
    check("city_sum" in (res.get("derived") or {}).get("rebuilt", []), f"derived rebuilt {res.get('derived')}")
    check("revalidate" in res, "relations revalidated")

    # extracted name colliding with an imported table is refused
    k.call("exec", code="con.execute(\"create table mine as select 1 x\")")
    r = k.call("db_extract", connection=cid, tables=[{"table": "customers", "as": "mine"}])
    check(not r["ok"] and "mine" in r.get("error", ""), "won't overwrite a non-extracted local table")

    # duckdb file + FK
    r = k.call("db_save", conn={"name": "wh", "type": "duckdb", "path": "uploads/wh.duckdb"})
    did = r["result"]["connection"]["id"]
    r = k.call("db_schema", connection=did)
    check(r["ok"] and any(f["from_table"] == "fact" for f in r["result"]["foreign_keys"]), f"duckdb FK {r.get('error') or r['result']['foreign_keys']}")
    r = k.call("db_query", connection=did, sql="select k, sum(v) from fact group by k order by k")
    check(r["ok"] and r["result"]["row_count"] == 10, f"duckdb live {r.get('error')}")
    r = k.call("db_query", connection=did, sql="select * from read_csv('/etc/passwd')")
    check(not r["ok"], "duckdb file functions blocked")
    r = k.call("explain", connection=did, sql="select * from fact where k=1")
    check(r["ok"] and r["result"]["plan"], "explain on connection")

    # password scrubbed from errors
    r = k.call("db_test", conn={"type": "postgres", "host": "127.0.0.1", "port": 1, "user": "u"}, password="TOPSECRET-123", timeout=3)
    check(not r["ok"] and "TOPSECRET" not in json.dumps(r), f"error without password: {r.get('error', '')[:90]}")

    # saved queries, format, explain, paging
    r = k.call("queries", action="save", name="城市汇总", sql="select city, count(*) from customers group by city")
    check(r["ok"], "query saved")
    r = k.call("queries")
    check(r["ok"] and r["result"]["queries"][0]["name"] == "城市汇总", "query listed")
    r = k.call("sql_format", sql="select a,b from t where x=1 and y=2")
    check(r["ok"] and "\n" in r["result"]["sql"], "sql_format pretty")
    r = k.call("explain", sql="select * from orders where amount > 10")
    check(r["ok"] and r["result"]["plan"], "local explain")
    q = k.call("sql", sql="select * from orders", limit=5)
    r = k.call("result_page", result_id=q["result"]["result_id"], offset=1000, limit=10)
    check(r["ok"] and r["result"]["total"] == 1001 and len(r["result"]["rows"]) == 1, f"result_page {r.get('error')}")
    r = k.call("sql", sql="select * from big_orders")
    check(not r["ok"] and "db_extract" in r.get("error", ""), "missing local table → hint to db_extract")
    r = k.call("db_connections")
    check(r["ok"] and len(r["result"]["connections"]) == 2 and all("password" not in c for c in r["result"]["connections"]), "connections listed")
    r = k.call("db_delete", connection=did)
    check(r["ok"] and len(k.call("db_connections")["result"]["connections"]) == 1, "delete connection")
    k.close()
    print(f"\n{len(fails)} failures")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
