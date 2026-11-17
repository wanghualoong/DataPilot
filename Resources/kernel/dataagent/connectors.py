"""Direct database connections (read-only): connection-form definitions, metadata, live queries, batch fetch.

Connection fields / metadata queries per database follow the approach of Chat2DB Community v5.3.8's plugins
(Adapted from Chat2DB Community v5.3.8 (LicenseRef-Chat2DB) — design and catalog queries re-implemented in Python
with pure-Python drivers instead of JDBC).

Security model:
  * passwords arrive per call from the app (keychain) and are never stored, logged or returned; every error
    message is scrubbed (`scrub`) before it leaves this module;
  * sessions are opened read-only where the server supports it, and every live query passes `check_readonly`
    in the target dialect; rows are capped with fetchmany (no SQL rewriting needed) and a statement timeout is set.
"""
import os
import re
import sqlite3
import threading
import time

import sqlglot
from sqlglot import exp

# ---------- connection form definitions (shared by every UI: SwiftUI now, Tauri later) ----------
_NET = [
    {"key": "host", "label": "主机 / Host", "kind": "text", "required": True, "default": "localhost"},
    {"key": "port", "label": "端口 / Port", "kind": "int", "required": True},
    {"key": "database", "label": "数据库 / Database", "kind": "text"},
    {"key": "user", "label": "用户名 / User", "kind": "text", "required": True},
    {"key": "password", "label": "密码 / Password", "kind": "secret"},
]
# SSH tunnel (run by the DataPilot core outside the kernel sandbox; the kernel connects to its local end).
# `group: ssh` fields are shown only when the `ssh` toggle is on; `ssh_secret` is a password or key passphrase that
# goes to the keychain like the database password.
_SSH = [
    {"key": "ssh", "label": "通过 SSH 隧道连接 / Connect via SSH tunnel", "kind": "bool"},
    {"key": "ssh_host", "label": "SSH 主机 / SSH host", "kind": "text", "group": "ssh"},
    {"key": "ssh_port", "label": "SSH 端口 / SSH port", "kind": "int", "group": "ssh", "default": "22"},
    {"key": "ssh_user", "label": "SSH 用户 / SSH user", "kind": "text", "group": "ssh"},
    {"key": "ssh_key", "label": "私钥文件（可选）/ Private key (optional)", "kind": "path", "group": "ssh", "default": "~/.ssh/id_ed25519"},
    {"key": "ssh_secret", "label": "SSH 密码或私钥口令 / SSH password or key passphrase", "kind": "secret", "group": "ssh"},
]
_FILE = [{"key": "path", "label": "文件路径 / File", "kind": "file", "required": True}]

DB_TYPES = [
    {"type": "postgres", "label": "PostgreSQL", "dialect": "postgres", "default_port": 5432, "driver": "pg8000",
     "note": "也适用于金仓 KingbaseES、openGauss 等 PostgreSQL 协议数据库",
     "fields": _NET + [{"key": "schema", "label": "Schema", "kind": "text", "default": "public"},
                       {"key": "ssl", "label": "SSL", "kind": "bool"}] + _SSH},
    {"type": "mysql", "label": "MySQL / MariaDB", "dialect": "mysql", "default_port": 3306, "driver": "pymysql",
     "note": "也适用于 TiDB、Doris、StarRocks 等 MySQL 协议数据库",
     "fields": _NET + [{"key": "ssl", "label": "SSL", "kind": "bool"}] + _SSH},
    {"type": "sqlserver", "label": "SQL Server", "dialect": "tsql", "default_port": 1433, "driver": "pymssql",
     "fields": _NET + [{"key": "schema", "label": "Schema", "kind": "text", "default": "dbo"}] + _SSH},
    {"type": "oracle", "label": "Oracle", "dialect": "oracle", "default_port": 1521, "driver": "oracledb (thin)",
     "note": "数据库一栏填服务名（service name）",
     "fields": _NET + [{"key": "schema", "label": "Schema（默认用户名）", "kind": "text"}] + _SSH},
    {"type": "clickhouse", "label": "ClickHouse", "dialect": "clickhouse", "default_port": 8123,
     "driver": "clickhouse-connect (HTTP)", "fields": _NET + [{"key": "ssl", "label": "HTTPS", "kind": "bool"}] + _SSH},
    {"type": "sqlite", "label": "SQLite 文件", "dialect": "sqlite", "driver": "sqlite3", "fields": _FILE},
    {"type": "duckdb", "label": "DuckDB 文件", "dialect": "duckdb", "driver": "duckdb", "fields": _FILE},
]
TYPES = {t["type"]: t for t in DB_TYPES}
FILE_TYPES = ("sqlite", "duckdb")


class DBError(RuntimeError):
    pass


def dialect(conn):
    return TYPES[conn["type"]]["dialect"]


def scrub(msg, password=None):
    s = str(msg)
    if password:
        s = s.replace(password, "***")
    return re.sub(r"(?i)(password|pwd|passwd)\s*[=:]\s*\S+", r"\1=***", s)


def port_of(conn):
    if conn.get("type") in FILE_TYPES:
        return None
    try:
        return int(conn.get("port") or TYPES[conn["type"]].get("default_port") or 0) or None
    except (TypeError, ValueError, KeyError):
        return None


# ---------- read-only check in the target dialect ----------
_ALLOWED = (exp.Select, exp.Union, exp.Intersect, exp.Except, exp.Subquery, exp.Values)
_FORBIDDEN = (exp.Insert, exp.Update, exp.Delete, exp.Drop, exp.Create, exp.Alter, exp.Command, exp.Merge,
              exp.TruncateTable)
_PREFIX = re.compile(r"^\s*(select|with|show|describe|desc|explain|values|\()", re.I)
# Functions that write / reach outside the database even inside a SELECT.
_DANGEROUS = re.compile(r"\b(pg_read_file|pg_read_binary_file|pg_ls_dir|lo_import|lo_export|dblink\w*|pg_sleep|"
                        r"load_file|sleep|benchmark|xp_\w+|sp_\w+|openrowset|opendatasource|utl_\w+|dbms_\w+|"
                        r"url|s3|file|remote|remoteSecure|mysql|postgresql|read_csv\w*|read_parquet|read_json\w*|"
                        r"read_text|read_blob|glob|attach)\s*\(", re.I)


def check_readonly(sql, dial):
    s = (sql or "").strip().rstrip(";").strip()
    if not s:
        raise DBError("empty SQL")
    if not _PREFIX.match(s):
        raise DBError("db_query 只允许只读查询（SELECT / WITH / SHOW / DESCRIBE / EXPLAIN）")
    if re.search(r"(?i)\binto\s+(outfile|dumpfile)\b", s) or re.search(r"(?i)\bfor\s+update\b", s):
        raise DBError("forbidden clause (INTO OUTFILE / FOR UPDATE)")
    if _DANGEROUS.search(s):
        raise DBError("该函数可能读写服务器文件或访问外部资源，db_query 中不允许")
    if re.match(r"(?i)^\s*(show|describe|desc|explain)\b", s):
        if re.match(r"(?i)^\s*explain\s+analy[sz]e\b", s):
            raise DBError("EXPLAIN ANALYZE 会真正执行语句，不允许")
        return s
    try:
        parts = [p for p in sqlglot.parse(s, read=dial) if p is not None]
    except Exception as e:  # noqa: BLE001
        raise DBError(f"无法按 {dial} 方言解析 SQL：{str(e)[:300]}") from None
    if len(parts) != 1:
        raise DBError("只允许一条语句")
    tree = parts[0]
    if not isinstance(tree, _ALLOWED):
        raise DBError(f"只允许 SELECT 查询（得到 {type(tree).__name__}）")
    for node in tree.walk():
        if isinstance(node, _FORBIDDEN):
            raise DBError(f"forbidden statement: {type(node).__name__}")
        if isinstance(node, exp.Into):
            raise DBError("SELECT … INTO 会写表，不允许")
    return s


def transpile(sql, src, dst):
    if src == dst:
        return sql
    try:
        return sqlglot.transpile(sql, read=src, write=dst)[0]
    except Exception as e:  # noqa: BLE001
        raise DBError(f"无法从 {src} 转写到 {dst}：{str(e)[:300]}") from None


def qualified(dial, table, schema=None):
    return exp.select("*").from_(exp.table_(table, db=schema or None, quoted=True)).sql(dialect=dial)


# ---------- connect (read-only session + statement timeout) ----------
class Conn:
    """A DB-API-ish connection wrapper: execute(sql, params) → cursor; close()."""

    def __init__(self, raw, kind, cancel=None):
        self.raw, self.kind, self._cancel = raw, kind, cancel

    def cursor(self):
        return self.raw.cursor()

    def close(self):
        try:
            self.raw.close()
        except Exception:  # noqa: BLE001
            pass

    def interrupt(self):
        if self._cancel:
            try:
                self._cancel()
            except Exception:  # noqa: BLE001
                pass


def connect(conn, password=None, timeout=60, resolve=None):
    """Open a read-only session. `resolve(path)` maps relative file paths (project root)."""
    t = conn.get("type")
    if t not in TYPES:
        raise DBError(f"unknown database type {t!r}; one of {', '.join(TYPES)}")
    try:
        return _connect(t, conn, password, int(timeout), resolve or (lambda p: p))
    except DBError:
        raise
    except Exception as e:  # noqa: BLE001
        raise DBError(f"连接失败 ({type(e).__name__}): {scrub(e, password)[:500]}") from None


def _connect(t, c, pw, timeout, resolve):
    host, user, db = c.get("host") or "localhost", c.get("user") or "", c.get("database") or None
    port = port_of(c)
    if t == "postgres":
        import pg8000.dbapi
        ssl_ctx = None
        if c.get("ssl"):
            import ssl
            ssl_ctx = ssl.create_default_context()
        raw = pg8000.dbapi.connect(user=user, host=host, port=port, database=db, password=pw, timeout=timeout,
                                   ssl_context=ssl_ctx, application_name="DataPilot",
                                   startup_params={"default_transaction_read_only": "on",
                                                   "statement_timeout": str(timeout * 1000)})
        raw.autocommit = True
        return Conn(raw, t)
    if t == "mysql":
        import pymysql
        raw = pymysql.connect(host=host, port=port, user=user, password=pw or "", database=db, charset="utf8mb4",
                              connect_timeout=min(timeout, 20), read_timeout=timeout, write_timeout=timeout,
                              ssl={"ssl": {}} if c.get("ssl") else None, autocommit=True)
        cur = raw.cursor()
        for s in ("SET SESSION TRANSACTION READ ONLY", f"SET SESSION max_execution_time={timeout * 1000}",
                  f"SET SESSION max_statement_time={timeout}"):
            try:
                cur.execute(s)
            except Exception:  # noqa: BLE001  (MySQL vs MariaDB vs TiDB variants)
                pass
        return Conn(raw, t)
    if t == "sqlserver":
        import pymssql
        raw = pymssql.connect(server=host, port=str(port), user=user, password=pw or "", database=db or "",
                              login_timeout=min(timeout, 20), timeout=timeout, charset="UTF-8", appname="DataPilot")
        raw.autocommit(True)
        return Conn(raw, t)
    if t == "oracle":
        import oracledb
        dsn = oracledb.makedsn(host, port, service_name=db) if db else f"{host}:{port}"
        raw = oracledb.connect(user=user, password=pw, dsn=dsn, tcp_connect_timeout=min(timeout, 20))
        raw.call_timeout = timeout * 1000
        try:
            raw.cursor().execute("SET TRANSACTION READ ONLY")
        except Exception:  # noqa: BLE001
            pass
        return Conn(raw, t)
    if t == "clickhouse":
        import clickhouse_connect.dbapi
        raw = clickhouse_connect.dbapi.connect(host=host, port=port, username=user, password=pw or "",
                                               database=db or "default", secure=bool(c.get("ssl")),
                                               settings={"readonly": 2, "max_execution_time": timeout},
                                               connect_timeout=min(timeout, 20), send_receive_timeout=timeout)
        return Conn(raw, t)
    if t == "sqlite":
        path = resolve(c.get("path") or "")
        if not os.path.exists(path):
            raise DBError(f"文件不存在：{path}")
        from urllib.parse import quote
        uri = "file:" + quote(os.path.abspath(path)) + "?mode=ro"
        try:
            raw = sqlite3.connect(uri, uri=True, check_same_thread=False, timeout=5)
            raw.execute("SELECT count(*) FROM sqlite_master").fetchall()
        except sqlite3.OperationalError:          # WAL file without -shm write access → read a frozen view
            raw = sqlite3.connect(uri + "&immutable=1", uri=True, check_same_thread=False)
        deadline = [time.time() + timeout]
        raw.set_progress_handler(lambda: 1 if time.time() > deadline[0] else 0, 10000)
        cn = Conn(raw, t, cancel=raw.interrupt)
        cn.reset_deadline = lambda: deadline.__setitem__(0, time.time() + timeout)
        return cn
    if t == "duckdb":
        import duckdb
        path = resolve(c.get("path") or "")
        if not os.path.exists(path):
            raise DBError(f"文件不存在：{path}")
        raw = duckdb.connect(path, read_only=True)
        return Conn(raw, t, cancel=raw.interrupt)
    raise DBError(f"unsupported type {t}")


class _Timer:
    """Interrupt a running statement after `secs` for drivers without a server-side timeout (sqlite/duckdb)."""

    def __init__(self, cn, secs):
        self.t = threading.Timer(secs, cn.interrupt) if cn.kind in FILE_TYPES else None

    def __enter__(self):
        if self.t:
            self.t.daemon = True
            self.t.start()
        return self

    def __exit__(self, *a):
        if self.t:
            self.t.cancel()


def run(cn, sql, params=None, limit=None, timeout=60, batch=None):
    """Execute and return (columns, rows, truncated). `batch(cols, rows)`: streaming callback (no rows kept)."""
    if hasattr(cn, "reset_deadline"):
        cn.reset_deadline()
    with _Timer(cn, timeout):
        if cn.kind == "duckdb":
            cur = cn.raw.cursor()
            cur.execute(sql, params or [])
        else:
            cur = cn.cursor()
            cur.execute(sql, params) if params else cur.execute(sql)
        cols = [d[0] for d in (cur.description or [])]
        if batch is not None:
            n = 0
            while True:
                rows = cur.fetchmany(50_000)
                if not rows:
                    break
                n += len(rows)
                batch(cols, rows)
                if limit and n >= limit:
                    break
            return cols, [], False
        if limit:
            rows = cur.fetchmany(limit + 1)
            return cols, [tuple(r) for r in rows[:limit]], len(rows) > limit
        return cols, [tuple(r) for r in cur.fetchall()], False


# ---------- metadata ----------
def _p(kind):
    return {"postgres": "%s", "mysql": "%s", "sqlserver": "%s", "oracle": ":1", "clickhouse": "%(p)s",
            "sqlite": "?", "duckdb": "?"}[kind]


def default_schema(cn, conn):
    t = conn["type"]
    if conn.get("schema"):
        return conn["schema"]
    q = {"postgres": "SELECT current_schema()", "mysql": "SELECT database()", "sqlserver": "SELECT SCHEMA_NAME()",
         "oracle": "SELECT sys_context('USERENV','CURRENT_SCHEMA') FROM dual", "clickhouse": "SELECT currentDatabase()",
         "duckdb": "SELECT current_schema()"}.get(t)
    if not q:
        return None
    try:
        return run(cn, q)[1][0][0]
    except Exception:  # noqa: BLE001
        return None


def list_tables(cn, conn):
    """[{schema, table, type}] for the connection's default schema (or `conn.schema`)."""
    t, sch = conn["type"], default_schema(cn, conn)
    if t == "sqlite":
        rows = run(cn, "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")[1]
        return [{"schema": None, "table": r[0], "type": r[1]} for r in rows]
    q = {
        "postgres": "SELECT table_name, table_type FROM information_schema.tables WHERE table_schema=%s ORDER BY table_name",
        "mysql": "SELECT table_name, table_type FROM information_schema.tables WHERE table_schema=%s ORDER BY table_name",
        "sqlserver": "SELECT TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA=%s ORDER BY TABLE_NAME",
        "oracle": "SELECT table_name, 'TABLE' FROM all_tables WHERE owner=:1 UNION ALL "
                  "SELECT view_name, 'VIEW' FROM all_views WHERE owner=:1 ORDER BY 1",
        "clickhouse": "SELECT name, engine FROM system.tables WHERE database=%(p)s ORDER BY name",
        "duckdb": "SELECT table_name, table_type FROM information_schema.tables WHERE table_schema=? ORDER BY table_name",
    }[t]
    params = {"p": sch} if t == "clickhouse" else [sch] if t != "oracle" else [sch.upper() if sch else sch]
    rows = run(cn, q, params)[1]
    return [{"schema": sch, "table": r[0], "type": "view" if "VIEW" in str(r[1]).upper() else "table"} for r in rows]


def columns(cn, conn, table, schema=None):
    """[{name, type, nullable, pk}] of one table."""
    t, sch = conn["type"], schema or default_schema(cn, conn)
    if t == "sqlite":
        rows = run(cn, f"PRAGMA table_info({_sqlite_q(table)})")[1]
        return [{"name": r[1], "type": r[2] or "", "nullable": not r[3], "pk": bool(r[5])} for r in rows]
    if t == "clickhouse":
        rows = run(cn, "SELECT name, type, is_in_primary_key FROM system.columns WHERE database=%(p)s AND table=%(t)s "
                       "ORDER BY position", {"p": sch, "t": table})[1]
        return [{"name": r[0], "type": r[1], "nullable": str(r[1]).startswith("Nullable"), "pk": bool(r[2])} for r in rows]
    if t == "oracle":
        rows = run(cn, "SELECT c.column_name, c.data_type, c.nullable, "
                       "(SELECT count(*) FROM all_cons_columns cc JOIN all_constraints k ON k.owner=cc.owner AND "
                       "k.constraint_name=cc.constraint_name WHERE k.constraint_type='P' AND cc.owner=c.owner AND "
                       "cc.table_name=c.table_name AND cc.column_name=c.column_name) FROM all_tab_columns c "
                       "WHERE c.owner=:1 AND c.table_name=:2 ORDER BY c.column_id", [sch.upper(), table])[1]
        return [{"name": r[0], "type": r[1], "nullable": r[2] == "Y", "pk": bool(r[3])} for r in rows]
    ph = _p(t)
    pk_sql = ("SELECT kcu.column_name FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage kcu "
              "ON tc.constraint_name=kcu.constraint_name AND tc.table_schema=kcu.table_schema AND tc.table_name=kcu.table_name "
              f"WHERE tc.constraint_type='PRIMARY KEY' AND tc.table_schema={ph} AND tc.table_name={ph}")
    rows = run(cn, "SELECT column_name, data_type, is_nullable FROM information_schema.columns "
                   f"WHERE table_schema={ph} AND table_name={ph} ORDER BY ordinal_position", [sch, table])[1]
    try:
        pks = {r[0] for r in run(cn, pk_sql, [sch, table])[1]}
    except Exception:  # noqa: BLE001
        pks = set()
    return [{"name": r[0], "type": r[1], "nullable": str(r[2]).upper() == "YES", "pk": r[0] in pks} for r in rows]


def _sqlite_q(name):
    return '"' + str(name).replace('"', '""') + '"'


def foreign_keys(cn, conn, tables=None):
    """[{from_table, from_columns[], to_table, to_columns[]}] within the default schema."""
    t, sch = conn["type"], default_schema(cn, conn)
    out = {}
    if t == "sqlite":
        for tb in (tables or [x["table"] for x in list_tables(cn, conn)]):
            for r in run(cn, f"PRAGMA foreign_key_list({_sqlite_q(tb)})")[1]:
                fid, _seq, to_t, frm_c, to_c = r[0], r[1], r[2], r[3], r[4]
                e = out.setdefault((tb, fid), {"from_table": tb, "from_columns": [], "to_table": to_t, "to_columns": []})
                e["from_columns"].append(frm_c); e["to_columns"].append(to_c)
        return [e for e in out.values() if all(e["to_columns"])]
    if t == "clickhouse":
        return []
    if t == "duckdb":
        rows = run(cn, "SELECT table_name, constraint_column_names, referenced_table, referenced_column_names "
                       "FROM duckdb_constraints() WHERE constraint_type='FOREIGN KEY' AND schema_name=?", [sch])[1]
        res = [{"from_table": r[0], "from_columns": list(r[1]), "to_table": r[2], "to_columns": list(r[3])} for r in rows]
    elif t == "postgres":
        rows = run(cn, "SELECT c.conname, src.relname, sa.attname, dst.relname, da.attname, k.n FROM pg_constraint c "
                       "JOIN pg_class src ON src.oid=c.conrelid JOIN pg_namespace ns ON ns.oid=src.relnamespace "
                       "JOIN pg_class dst ON dst.oid=c.confrelid "
                       "CROSS JOIN LATERAL unnest(c.conkey, c.confkey) WITH ORDINALITY AS k(sk, dk, n) "
                       "JOIN pg_attribute sa ON sa.attrelid=c.conrelid AND sa.attnum=k.sk "
                       "JOIN pg_attribute da ON da.attrelid=c.confrelid AND da.attnum=k.dk "
                       "WHERE c.contype='f' AND ns.nspname=%s ORDER BY c.conname, k.n", [sch])[1]
        res = _group(rows)
    elif t == "mysql":
        rows = run(cn, "SELECT constraint_name, table_name, column_name, referenced_table_name, referenced_column_name, "
                       "ordinal_position FROM information_schema.key_column_usage WHERE table_schema=%s AND "
                       "referenced_table_name IS NOT NULL ORDER BY constraint_name, ordinal_position", [sch])[1]
        res = _group(rows)
    elif t == "sqlserver":
        rows = run(cn, "SELECT fk.name, tp.name, cp.name, tr.name, cr.name, fkc.constraint_column_id "
                       "FROM sys.foreign_keys fk JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id=fk.object_id "
                       "JOIN sys.tables tp ON tp.object_id=fkc.parent_object_id JOIN sys.columns cp ON "
                       "cp.object_id=fkc.parent_object_id AND cp.column_id=fkc.parent_column_id "
                       "JOIN sys.tables tr ON tr.object_id=fkc.referenced_object_id JOIN sys.columns cr ON "
                       "cr.object_id=fkc.referenced_object_id AND cr.column_id=fkc.referenced_column_id "
                       "WHERE SCHEMA_NAME(tp.schema_id)=%s ORDER BY fk.name, fkc.constraint_column_id", [sch])[1]
        res = _group(rows)
    elif t == "oracle":
        rows = run(cn, "SELECT a.constraint_name, a.table_name, a.column_name, c_pk.table_name, b.column_name, a.position "
                       "FROM all_cons_columns a JOIN all_constraints c ON a.owner=c.owner AND a.constraint_name=c.constraint_name "
                       "JOIN all_constraints c_pk ON c.r_owner=c_pk.owner AND c.r_constraint_name=c_pk.constraint_name "
                       "JOIN all_cons_columns b ON b.owner=c_pk.owner AND b.constraint_name=c_pk.constraint_name "
                       "AND b.position=a.position WHERE c.constraint_type='R' AND a.owner=:1 "
                       "ORDER BY a.constraint_name, a.position", [sch.upper()])[1]
        res = _group(rows)
    else:
        res = []
    if tables:
        keep = {x.lower() for x in tables}
        res = [r for r in res if r["from_table"].lower() in keep]
    return res


def _group(rows):
    """Rows (name, from_table, from_col, to_table, to_col, pos) → one entry per constraint."""
    out = {}
    for name, ft, fc, tt, tc, _pos in rows:
        e = out.setdefault((ft, name), {"from_table": ft, "from_columns": [], "to_table": tt, "to_columns": []})
        e["from_columns"].append(fc); e["to_columns"].append(tc)
    return list(out.values())


def server_version(cn, conn):
    q = {"postgres": "SELECT version()", "mysql": "SELECT version()", "sqlserver": "SELECT @@VERSION",
         "oracle": "SELECT banner FROM v$version WHERE ROWNUM=1", "clickhouse": "SELECT version()",
         "sqlite": "SELECT 'SQLite ' || sqlite_version()", "duckdb": "SELECT 'DuckDB ' || version()"}[conn["type"]]
    try:
        return str(run(cn, q)[1][0][0]).splitlines()[0][:120]
    except Exception:  # noqa: BLE001
        return ""


def render_schema(conn, tables, fks):
    """Compact text for the model: dialect, tables with columns, foreign keys."""
    d = dialect(conn)
    lines = [f"# 数据库 {conn.get('name') or conn['id']}（{TYPES[conn['type']]['label']}，SQL 方言 {d}）"]
    for tb in tables:
        cols = ", ".join(f"{c['name']} {c['type']}" + (" PK" if c.get("pk") else "") for c in tb.get("columns", [])[:80])
        more = f" …(+{len(tb['columns']) - 80})" if len(tb.get("columns", [])) > 80 else ""
        lines.append(f"- {tb['table']}{' (view)' if tb.get('type') == 'view' else ''}: {cols}{more}")
    if fks:
        lines.append("外键：")
        for f in fks:
            lines.append(f"- {f['from_table']}({', '.join(f['from_columns'])}) → {f['to_table']}({', '.join(f['to_columns'])})")
    return "\n".join(lines)
