"""Engine ops for direct database connections, saved queries, SQL format / EXPLAIN and result paging.

Mixed into `Engine` (engine.py). Connection definitions live in <workspace>/connections.json WITHOUT passwords;
the app injects `password` per request from the keychain.
"""
import json
import os
import re
import shutil
import tempfile
import threading
import time
import uuid

import pyarrow as pa
import pyarrow.parquet as pq

from . import connectors as C
from . import profile, relations, sqlguard
from .util import json_default, qident, replace_file

_SECRET_KEYS = ("password", "passwd", "pwd", "secret")
_CONN_KEYS = ("id", "name", "type", "host", "port", "database", "user", "schema", "ssl", "path", "live", "note",
              "ssh", "ssh_host", "ssh_port", "ssh_user", "ssh_key")


def _clean_conn(c):
    out = {k: c[k] for k in _CONN_KEYS if c.get(k) not in (None, "")}
    for k in ("port", "ssh_port"):
        if k in out:
            try:
                out[k] = int(out[k])
            except (TypeError, ValueError):
                raise C.DBError(f"{k} must be a number: {out[k]!r}") from None
    if out.get("ssh") and not out.get("ssh_host"):
        raise C.DBError("SSH 隧道需要填写 SSH 主机 / SSH tunnel needs an SSH host")
    out.setdefault("live", True)
    return out


def _local_name(s):
    n = re.sub(r"[^0-9A-Za-z_一-鿿]+", "_", str(s)).strip("_")
    return (n or "t")[:60]


def _arrow_batch(cols, rows):
    arrays, names, seen = [], [], {}
    for i, c in enumerate(cols):
        name = str(c) or f"col{i + 1}"
        if name in seen:                       # duplicate labels from joins: a, a → a, a_2
            seen[name] += 1; name = f"{name}_{seen[name]}"
        else:
            seen[name] = 1
        vals = [r[i] for r in rows]
        try:
            arr = pa.array(vals)
        except (pa.ArrowInvalid, pa.ArrowTypeError, pa.ArrowNotImplementedError, OverflowError):
            arr = pa.array([None if v is None else (v.hex() if isinstance(v, (bytes, bytearray)) else str(v)) for v in vals],
                           type=pa.string())
        arrays.append(arr); names.append(name)
    return pa.Table.from_arrays(arrays, names=names)


class DBOps:
    _conn_lock = threading.Lock()

    # ---------- connections.json ----------
    @property
    def _conns_path(self):
        return os.path.join(self.ws, "connections.json")

    def _load_conns(self):
        try:
            with open(self._conns_path, encoding="utf-8") as f:
                return json.load(f)
        except Exception:  # noqa: BLE001
            return {"version": 1, "connections": []}

    def _save_conns(self, d):
        for c in d.get("connections", []):          # never persist a secret, whatever the caller sent
            for k in _SECRET_KEYS:
                c.pop(k, None)
        tmp = self._conns_path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(d, f, ensure_ascii=False, indent=1, default=str)
        replace_file(tmp, self._conns_path)

    def _conn(self, req):
        if isinstance(req.get("conn"), dict):
            return _clean_conn(req["conn"])
        cid = req.get("connection")
        for c in self._load_conns().get("connections", []):
            if c["id"] == cid or (cid and c.get("name") == cid):
                return c
        raise C.DBError(f"connection {cid!r} not found; existing: "
                        + ", ".join(c.get("name") or c["id"] for c in self._load_conns().get("connections", [])))

    def _open(self, conn, req):
        if conn.get("ssh"):
            # the core runs the SSH tunnel outside the sandbox and tells us its local end
            if not req.get("port_override"):
                raise C.DBError("这个连接走 SSH 隧道，需要由 DataPilot 核心建立隧道后再连接 / SSH tunnel not established")
            conn = dict(conn, host=req.get("host_override") or "127.0.0.1", port=int(req["port_override"]))
        return C.connect(conn, req.get("password"), timeout=int(req.get("timeout") or 60), resolve=self.resolve)

    def _scrubbed(self, fn, req):
        try:
            return fn()
        except C.DBError as e:
            raise C.DBError(C.scrub(e, req.get("password"))) from None
        except Exception as e:  # noqa: BLE001
            raise C.DBError(f"{type(e).__name__}: {C.scrub(e, req.get('password'))[:800]}") from None

    # ---------- ops: definitions ----------
    def op_db_types(self, req):
        return {"types": C.DB_TYPES}

    def op_db_connections(self, req):
        d = self._load_conns()
        out = []
        for c in d.get("connections", []):
            e = dict(c)
            e["dialect"] = C.dialect(c) if c.get("type") in C.TYPES else None
            e["port_effective"] = C.port_of(c)
            out.append(e)
        return {"connections": out, "ports": sorted({p for p in (C.port_of(c) for c in d.get("connections", [])) if p})}

    def op_db_save(self, req):
        c = _clean_conn(req.get("conn") or {})
        if c.get("type") not in C.TYPES:
            raise C.DBError(f"type must be one of {', '.join(C.TYPES)}")
        for f in C.TYPES[c["type"]]["fields"]:
            if f.get("required") and f["kind"] != "secret" and not c.get(f["key"]) and not (f["key"] == "port" and C.port_of(c)):
                raise C.DBError(f"missing field {f['key']}")
        with self._conn_lock:
            d = self._load_conns()
            items = d.get("connections", [])
            old = next((x for x in items if x["id"] == c.get("id")), None)
            if not c.get("id"):
                c["id"] = "db_" + uuid.uuid4().hex[:8]
            c.setdefault("name", c.get("database") or os.path.basename(c.get("path", "")) or c["type"])
            if old:
                c["extracts"] = old.get("extracts", [])
                c["created"] = old.get("created", time.time())
            else:
                c["extracts"], c["created"] = [], time.time()
            c["updated"] = time.time()
            d["connections"] = [x for x in items if x["id"] != c["id"]] + [c]
            self._save_conns(d)
        return {"connection": c, "port": C.port_of(c)}

    def op_db_delete(self, req):
        with self._conn_lock:
            d = self._load_conns()
            d["connections"] = [x for x in d.get("connections", []) if x["id"] != req["connection"]]
            self._save_conns(d)
        return {"ok": True}

    # ---------- ops: live ----------
    def op_db_test(self, req):
        conn = self._conn(req)

        def go():
            t0 = time.time()
            cn = self._open(conn, req)
            try:
                ver = C.server_version(cn, conn)
                n = len(C.list_tables(cn, conn))
                return {"ok": True, "version": ver, "tables": n, "schema": C.default_schema(cn, conn),
                        "dialect": C.dialect(conn), "ms": int((time.time() - t0) * 1000)}
            finally:
                cn.close()
        return self._scrubbed(go, req)

    def op_db_schema(self, req):
        """Tables (+ columns unless `columns:false`) and foreign keys. `tables` limits the detail to these."""
        conn = self._conn(req)

        def go():
            cn = self._open(conn, req)
            try:
                all_t = C.list_tables(cn, conn)
                want = {t.lower() for t in (req.get("tables") or [])}
                pick = [t for t in all_t if not want or t["table"].lower() in want]
                with_cols = req.get("columns", True) and len(pick) <= int(req.get("max_tables", 60))
                for i, t in enumerate(pick):
                    if with_cols:
                        self.progress(f"{conn.get('name')}: {t['table']} ({i + 1}/{len(pick)})")
                        try:
                            t["columns"] = C.columns(cn, conn, t["table"], t.get("schema"))
                        except Exception as e:  # noqa: BLE001
                            t["columns"], t["error"] = [], C.scrub(e, req.get("password"))[:200]
                try:
                    fks = C.foreign_keys(cn, conn, [t["table"] for t in pick] if want else None)
                except Exception:  # noqa: BLE001
                    fks = []
                text = C.render_schema(conn, pick, fks)
                if not with_cols and len(pick) > 0:
                    text += f"\n（共 {len(pick)} 张表，只列了表名；用 tables=[…] 查看指定表的列）"
                return {"connection": conn["id"], "dialect": C.dialect(conn), "tables": pick, "foreign_keys": fks,
                        "text": text, "total_tables": len(all_t)}
            finally:
                cn.close()
        return self._scrubbed(go, req)

    def op_db_query(self, req):
        """Live read-only query on the remote database (capped rows, timeout). `dialect`: SQL written in another dialect
        (e.g. duckdb) is transpiled to the target first."""
        conn = self._conn(req)
        if conn.get("live") is False:
            raise C.DBError("该连接未开启实时查询；请用 db_extract 抽取后在本地查询，或在连接设置中打开实时查询")
        target = C.dialect(conn)
        sql = req["sql"]
        if req.get("dialect") and req["dialect"] != target:
            sql = C.transpile(sql, req["dialect"], target)
        sql = C.check_readonly(sql, target)
        limit = max(1, min(int(req.get("limit") or 1000), 100_000))
        preview = int(req.get("preview_rows") or 50)

        def go():
            t0 = time.time()
            cn = self._open(conn, req)
            try:
                cols, rows, truncated = C.run(cn, sql, limit=limit, timeout=int(req.get("timeout") or 60))
            finally:
                cn.close()
            ms = int((time.time() - t0) * 1000)
            result_id = req.get("result_id") or f"q_{int(time.time() * 1000)}"
            path = None
            if rows:
                path = os.path.join(self.ws, "artifacts", f"{result_id}.parquet")
                pq.write_table(_arrow_batch(cols, rows), path)
            rows_json = json.loads(json.dumps([list(r) for r in rows[:preview]], default=json_default, ensure_ascii=False))
            return {"result_id": result_id, "columns": [str(c) for c in cols], "rows": rows_json, "row_count": len(rows),
                    "truncated": len(rows) > preview, "capped": truncated, "limit": limit, "ms": ms, "artifact": path,
                    "sql": sql, "dialect": target, "connection": conn["id"]}
        return self._scrubbed(go, req)

    # ---------- ops: extract / refresh ----------
    def _extract_one(self, cn, conn, spec, req, taken):
        """spec: {table, schema?, as?, where?, limit?} or {sql, as}. Streams batches → parquet → one DuckDB table."""
        dial = C.dialect(conn)
        if spec.get("sql"):
            if not spec.get("as"):
                raise C.DBError("extracting a query needs `as` (local table name)")
            src = C.check_readonly(spec["sql"], dial)
        else:
            src = C.qualified(dial, spec["table"], spec.get("schema"))
            if spec.get("where"):
                w = C.check_readonly(f"SELECT 1 FROM t WHERE {spec['where']}", dial)
                src += " WHERE " + w.split(" WHERE ", 1)[1]
        local = _local_name(spec.get("as") or spec["table"])
        existing = {t["table"].lower() for t in profile.list_tables(self.con)}
        if local.lower() in existing and local.lower() not in taken:
            raise C.DBError(f"本地已有表 {local}（不是从数据库抽取的）；请用 as 指定别的名字")
        tmpd = tempfile.mkdtemp(prefix="dbx_", dir=self.ws)
        files, n = [], [0]
        label = spec.get("table") or local

        def batch(cols, rows):
            p = os.path.join(tmpd, f"{len(files):05d}.parquet")
            pq.write_table(_arrow_batch(cols, rows), p)
            files.append(p); n[0] += len(rows)
            self.progress(f"{conn.get('name')}: {label} {n[0]:,} 行")
        try:
            cols, _, _ = C.run(cn, src, limit=spec.get("limit"), timeout=int(req.get("timeout") or 1800), batch=batch)
            if files:
                lst = "[" + ", ".join("'" + f.replace("'", "''") + "'" for f in files) + "]"
                body = f"SELECT * FROM read_parquet({lst}, union_by_name=true)"
                if spec.get("limit"):
                    body += f" LIMIT {int(spec['limit'])}"
            else:                                       # empty result: keep the columns, as VARCHAR
                body = "SELECT " + (", ".join(f"CAST(NULL AS VARCHAR) AS {qident(c)}" for c in cols) or "1 AS x") + " WHERE false"
            self.con.execute(f"CREATE OR REPLACE TABLE {qident(local)} AS {body}")
        finally:
            shutil.rmtree(tmpd, ignore_errors=True)
        rows = self.con.execute(f"SELECT count(*) FROM {qident(local)}").fetchall()[0][0]
        rec = {k: spec[k] for k in ("table", "schema", "sql", "where", "limit") if spec.get(k)}
        rec.update({"as": local, "rows": int(rows), "extracted_at": time.time()})
        return rec

    def _extracted_tables(self):
        return {e["as"].lower() for c in self._load_conns().get("connections", []) for e in c.get("extracts", [])}

    def _fk_relations(self, cn, conn, recs):
        """Remote foreign keys between extracted tables → confirmed relations (source=db)."""
        local = {r["table"].lower(): r["as"] for r in recs if r.get("table")}
        if len(local) < 2:
            return []
        try:
            fks = C.foreign_keys(cn, conn, list(local))
        except Exception:  # noqa: BLE001
            return []
        out = []
        for fk in fks:
            ft, tt = local.get(fk["from_table"].lower()), local.get(fk["to_table"].lower())
            if not ft or not tt:
                continue
            frm, to = {"table": ft, "columns": fk["from_columns"]}, {"table": tt, "columns": fk["to_columns"]}
            try:
                t = relations.test(self.con, frm, to)
            except Exception:  # noqa: BLE001
                t = {"match_rate": None, "cardinality": None, "unmatched_samples": []}
            saved = self.rels.upsert({"from": frm, "to": to, "status": "confirmed", "source": "db",
                                      "match_rate": t["match_rate"], "cardinality": t["cardinality"],
                                      "unmatched_samples": t["unmatched_samples"],
                                      "note": f"数据库外键（{conn.get('name')}）"})
            out.append(relations.flat(saved))
        return out

    def _save_extracts(self, conn_id, recs):
        with self._conn_lock:
            d = self._load_conns()
            for c in d.get("connections", []):
                if c["id"] == conn_id:
                    keep = [e for e in c.get("extracts", []) if e["as"].lower() not in {r["as"].lower() for r in recs}]
                    c["extracts"] = keep + recs
                    c["refreshed"] = time.time()
            self._save_conns(d)

    def op_db_extract(self, req):
        """Snapshot remote tables / a query into local DuckDB tables (full detail; no aggregation)."""
        conn = self._conn(req)
        specs = [({"table": t} if isinstance(t, str) else dict(t)) for t in (req.get("tables") or [])]
        if req.get("sql"):
            specs.append({"sql": req["sql"], "as": req.get("as"), "limit": req.get("limit")})
        if not specs:
            raise C.DBError("tables or sql is required")

        def go():
            cn = self._open(conn, req)
            try:
                taken = self._extracted_tables()
                recs = [self._extract_one(cn, conn, s, req, taken) for s in specs]
                rels = self._fk_relations(cn, conn, recs)
            finally:
                cn.close()
            self._save_extracts(conn["id"], recs)
            return {"connection": conn["id"], "tables": recs, "relations": rels}
        return self._scrubbed(go, req)

    def op_db_refresh(self, req):
        """Re-pull the connection's extract list, then rebuild derived tables and re-check relations."""
        conn = self._conn(req)
        specs = conn.get("extracts", [])
        if req.get("tables"):
            want = {t.lower() for t in req["tables"]}
            specs = [e for e in specs if e["as"].lower() in want or (e.get("table") or "").lower() in want]
        if not specs:
            return {"connection": conn["id"], "tables": [], "note": "nothing extracted yet"}

        def go():
            cn = self._open(conn, req)
            try:
                taken = self._extracted_tables()
                recs = [self._extract_one(cn, conn, dict(s), req, taken) for s in specs]
                rels = self._fk_relations(cn, conn, recs)
            finally:
                cn.close()
            self._save_extracts(conn["id"], recs)
            changed = [r["as"] for r in recs]
            out = {"connection": conn["id"], "tables": recs, "relations": rels}
            try:
                out["derived"] = self.op_rebuild_derived({"changed": changed})
            except Exception as e:  # noqa: BLE001
                out["derived_error"] = str(e)
            try:
                out["revalidate"] = self.op_relation_revalidate({"changed": changed})
            except Exception as e:  # noqa: BLE001
                out["revalidate_error"] = str(e)
            return out
        return self._scrubbed(go, req)

    # ---------- saved queries ----------
    @property
    def _queries_path(self):
        return os.path.join(self.ws, "queries.json")

    def op_queries(self, req):
        """list | save {name, sql, connection?, note?} | delete {query_id}."""
        action = req.get("action") or "list"
        with self._conn_lock:
            try:
                with open(self._queries_path, encoding="utf-8") as f:
                    d = json.load(f)
            except Exception:  # noqa: BLE001
                d = {"version": 1, "queries": []}
            if action == "list":
                return d
            items = d.get("queries", [])
            if action == "save":
                sql = (req.get("sql") or "").strip()
                if not sql or not req.get("name"):
                    raise ValueError("name and sql are required")
                q = next((x for x in items if x["id"] == req.get("query_id") or x["name"] == req["name"]), None)
                new = {"id": (q or {}).get("id") or "qs_" + uuid.uuid4().hex[:8], "name": str(req["name"])[:80], "sql": sql,
                       "connection": req.get("connection"), "note": req.get("note") or "",
                       "created": (q or {}).get("created", time.time()), "updated": time.time()}
                items = [x for x in items if x is not q] + [new]
                out = {"query": new}
            elif action == "delete":
                items = [x for x in items if x["id"] != req.get("query_id")]
                out = {"ok": True}
            else:
                raise ValueError("action must be list / save / delete")
            d["queries"] = items
            tmp = self._queries_path + ".tmp"
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump(d, f, ensure_ascii=False, indent=1)
            replace_file(tmp, self._queries_path)
            return out

    # ---------- SQL tools ----------
    def op_sql_format(self, req):
        import sqlglot
        dial = req.get("dialect") or "duckdb"
        try:
            parts = sqlglot.transpile(req["sql"], read=dial, write=req.get("to") or dial, pretty=True)
        except Exception as e:  # noqa: BLE001
            raise ValueError(f"cannot format: {str(e)[:300]}") from None
        return {"sql": ";\n\n".join(parts)}

    def op_explain(self, req):
        """Query plan: local DuckDB, or a connection (`connection` + password) for postgres / mysql / sqlite / duckdb / clickhouse."""
        if req.get("connection") or req.get("conn"):
            conn = self._conn(req)
            t = conn["type"]
            prefix = {"postgres": "EXPLAIN ", "mysql": "EXPLAIN ", "sqlite": "EXPLAIN QUERY PLAN ", "duckdb": "EXPLAIN ",
                      "clickhouse": "EXPLAIN "}.get(t)
            if not prefix:
                raise C.DBError(f"{C.TYPES[t]['label']} 的执行计划需要写入计划表，DataPilot 不支持")
            sql = C.check_readonly(req["sql"], C.dialect(conn))

            def go():
                cn = self._open(conn, req)
                try:
                    cols, rows, _ = C.run(cn, prefix + sql, timeout=int(req.get("timeout") or 30))
                finally:
                    cn.close()
                return {"plan": "\n".join(" | ".join("" if v is None else str(v) for v in r) for r in rows),
                        "columns": cols, "dialect": C.dialect(conn)}
            return self._scrubbed(go, req)
        sql = sqlguard.check_readonly(req["sql"])
        self._register_frames(sql)
        rows = self.con.execute("EXPLAIN " + sql).fetchall()
        return {"plan": "\n".join(str(r[-1]) for r in rows), "dialect": "duckdb"}

    def op_result_page(self, req):
        """A page of a saved query result (artifacts/<result_id>.parquet)."""
        rid = re.sub(r"[^A-Za-z0-9_\-]", "", str(req["result_id"]))
        p = os.path.join(self.ws, "artifacts", f"{rid}.parquet")
        if not os.path.exists(p):
            raise FileNotFoundError(f"result {rid} not found (re-run the query)")
        off, lim = max(0, int(req.get("offset") or 0)), max(1, min(int(req.get("limit") or 500), 5000))
        lit = "'" + p.replace("'", "''") + "'"
        order = ""
        if req.get("sort"):
            order = f" ORDER BY {qident(req['sort'])} {'DESC' if req.get('desc') else 'ASC'} NULLS LAST"
        total = self.con.execute(f"SELECT count(*) FROM read_parquet({lit})").fetchall()[0][0]
        df = self.con.execute(f"SELECT * FROM read_parquet({lit}){order} LIMIT {lim} OFFSET {off}").df()
        return {"result_id": rid, "columns": [str(c) for c in df.columns], "total": int(total), "offset": off,
                "rows": json.loads(df.to_json(orient="values", date_format="iso", force_ascii=False))}
