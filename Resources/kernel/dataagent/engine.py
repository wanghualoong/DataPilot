"""Engine: owns the DuckDB connection + Python namespace; dispatches kernel ops."""
import ast
import json
import os
import re
import threading
import time
import traceback

import duckdb
import pandas as pd

from . import importer, profile, relations, sqlguard
from .dbops import DBOps
from .util import clip, is_within, json_default, qident, replace_file  # noqa: F401  (re-exported)

PREVIEW_ROWS = 50
MAX_CHECKPOINT_MB = 200


def _md_table(df):
    """Markdown table without the optional `tabulate` dependency."""
    def cell(v):
        return "" if v is None or (isinstance(v, float) and v != v) else str(v).replace("|", "\\|").replace("\n", " ")
    cols = [str(c) for c in df.columns]
    out = ["| " + " | ".join(cols) + " |", "|" + "---|" * len(cols)]
    for row in df.itertuples(index=False):
        out.append("| " + " | ".join(cell(v) for v in row) + " |")
    return "\n".join(out)


class Engine(DBOps):
    def __init__(self, workspace):
        self.ws = workspace
        self.project_root = os.environ.get("DP_PROJECT_ROOT") or None
        for d in ("uploads", "artifacts", "checkpoints", "exports"):
            os.makedirs(os.path.join(workspace, d), exist_ok=True)
        self.db_path = os.path.join(workspace, "data.duckdb")
        # One DuckDB connection for the process; every request runs on its own cursor (see kernel.py) so
        # chats can query / import concurrently. Each session namespace has a dedicated cursor for `con`.
        self._main = duckdb.connect(self.db_path)
        # 8 threads halve large dashboard queries vs 4 (1m-row Contoso cube: 1.23 s → 0.58 s on a 10-core Mac)
        self._main.execute(f"SET threads TO {max(2, min(8, os.cpu_count() or 4))}")
        self._tl = threading.local()          # per-thread request binding: ns_name + cursor
        self._ns_lock = threading.RLock()     # guards namespace creation
        self.ns_cons = {}
        self._src_lock = threading.Lock()     # guards sources.json (concurrent imports)
        self._rel_lock = threading.Lock()     # guards relations.json
        self.rels = relations.Store(workspace, self._rel_lock)
        self.small_rows = int(os.environ.get("DP_SMALL_ROWS", importer.SMALL_ROWS_DEFAULT))
        # Per-session Python namespaces (a project kernel is shared by several sessions; DuckDB is shared).
        self.nss = {}
        self.sigs = {}
        self.restored = self._restore_checkpoints()

    # ---------- namespaces ----------
    # ---------- per-request binding (thread-local) ----------
    @staticmethod
    def ns_key(ns):
        k = re.sub(r"[^A-Za-z0-9_-]", "", str(ns)) if ns else ""
        return k or "_default"

    @property
    def con(self):
        c = getattr(self._tl, "con", None)
        return self._main if c is None else c

    @property
    def ns_name(self):
        return getattr(self._tl, "ns_name", "_default")

    @ns_name.setter
    def ns_name(self, v):
        self._tl.ns_name = v

    def bind(self, ns_name, con):
        self._tl.ns_name = ns_name
        self._tl.con = con

    def unbind(self):
        self._tl.ns_name = "_default"
        self._tl.con = None
        self._tl.progress = None

    def progress(self, msg):
        """Report progress of the current request (shown as the task detail in the app's kernel task list)."""
        f = getattr(self._tl, "progress", None)
        if f:
            f(msg)

    def cursor(self):
        return self._main.cursor()

    def ns_con(self, name):
        """The session's dedicated cursor (what user code sees as `con`)."""
        with self._ns_lock:
            if name not in self.ns_cons:
                self.ns_cons[name] = self._main.cursor()
            return self.ns_cons[name]

    def _new_ns(self, name):
        con = self.ns_con(name)
        ns = {"__name__": "__main__", "pd": pd, "duckdb": duckdb, "con": con, "WORKSPACE": self.ws,
              "PROJECT_ROOT": self.project_root, "OUTPUT_DIR": os.environ.get("DP_OUTPUT_DIR") or os.path.join(self.ws, "exports")}
        ns["sql"] = lambda q: con.sql(q).df()
        return ns

    @property
    def ns(self):
        name = self.ns_name
        if name not in self.nss:
            with self._ns_lock:
                if name not in self.nss:
                    self.nss[name] = self._new_ns(name)
                    self.sigs[name] = {}
                    if name != "_default":
                        self._restore_checkpoints()
        return self.nss[name]

    @property
    def _df_sigs(self):
        self.ns  # ensure created
        return self.sigs[self.ns_name]

    def _ckpt_dir(self):
        d = os.path.join(self.ws, "checkpoints") if self.ns_name == "_default" else os.path.join(self.ws, "checkpoints", self.ns_name)
        os.makedirs(d, exist_ok=True)
        return d

    @property
    def cells_path(self):
        return os.path.join(self.ws, "cells.jsonl" if self.ns_name == "_default" else f"cells_{self.ns_name}.jsonl")

    # ---------- dispatch ----------
    def dispatch(self, op, req):
        """Run one op. The caller binds the namespace + cursor first (`bind`); unbound calls use _default."""
        fn = getattr(self, f"op_{op}", None)
        if fn is None:
            raise ValueError(f"unknown op {op}")
        try:
            return fn(req)
        except duckdb.CatalogException as e:
            # A table that lives in a connected database is not a local DuckDB table: say how to get it.
            if "does not exist" in str(e) and not op.startswith("db_") and self._load_conns().get("connections"):
                names = ", ".join(c.get("name") or c["id"] for c in self._load_conns()["connections"])
                raise duckdb.CatalogException(
                    f"{e}\n提示：本地 DuckDB 没有这张表。数据库连接（{names}）里的表需要先用 db_extract 抽取到本地，"
                    f"才能用 sql_query / chart_create / 看板；只查数可以用 db_query。") from None
            raise

    def status(self):
        return {"workspace": self.ws, "project_root": self.project_root, "tables": [t["table"] for t in profile.list_tables(self.con)],
                "restored_vars": self.restored, "namespace": self.ns_name, "duckdb": duckdb.__version__, "pandas": pd.__version__}

    def op_ping(self, req):
        return self.status()

    # ---------- python exec ----------
    def op_exec(self, req):
        code = req["code"]
        tree = ast.parse(code, mode="exec")
        last_val = None
        if tree.body and isinstance(tree.body[-1], ast.Expr):
            last = ast.Expression(tree.body.pop().value)
            exec(compile(tree, "<cell>", "exec"), self.ns)  # noqa: S102
            last_val = eval(compile(last, "<cell>", "eval"), self.ns)  # noqa: S307
        else:
            exec(compile(tree, "<cell>", "exec"), self.ns)  # noqa: S102
        with open(self.cells_path, "a", encoding="utf-8") as f:
            f.write(json.dumps({"ts": time.time(), "code": code}, ensure_ascii=False) + "\n")
        changed = self._sync_dataframes()
        out = {"value": self._render(last_val) if last_val is not None else None, "changed_dataframes": changed,
               "images": self._collect_figures()}
        return out

    def _render(self, v):
        if isinstance(v, pd.DataFrame):
            return {"type": "dataframe", "shape": list(v.shape), "columns": [str(c) for c in v.columns],
                    "rows": json.loads(v.head(PREVIEW_ROWS).to_json(orient="values", date_format="iso", force_ascii=False)),
                    "text": clip(v.head(30).to_string(max_cols=30, max_colwidth=40), 8000)}
        if isinstance(v, pd.Series):
            return {"type": "text", "text": clip(v.head(100).to_string(), 8000)}
        return {"type": "text", "text": clip(repr(v), 8000)}

    def _collect_figures(self):
        imgs = []
        try:
            import sys
            if "matplotlib.pyplot" in sys.modules:
                plt = sys.modules["matplotlib.pyplot"]
                for num in plt.get_fignums():
                    p = os.path.join(self.ws, "artifacts", f"fig_{int(time.time()*1000)}_{num}.png")
                    plt.figure(num).savefig(p, dpi=120, bbox_inches="tight")
                    imgs.append(p)
                plt.close("all")
        except Exception:  # noqa: BLE001
            pass
        return imgs

    # ---------- dataframe sync / checkpoints ----------
    def _user_dfs(self):
        return {k: v for k, v in self.ns.items() if isinstance(v, pd.DataFrame) and not k.startswith("_")}

    def _sig(self, df):
        try:
            return (df.shape, tuple(map(str, df.columns)), int(pd.util.hash_pandas_object(df.head(200), index=False).sum()))
        except Exception:  # noqa: BLE001
            return (df.shape, tuple(map(str, df.columns)))

    def _sync_dataframes(self):
        """Checkpoint changed DataFrames to parquet (SQL sees them via _register_frames, by their variable name)."""
        changed = []
        for name, df in self._user_dfs().items():
            sig = self._sig(df)
            if self._df_sigs.get(name) == sig:
                continue
            self._df_sigs[name] = sig
            changed.append({"name": name, "shape": list(df.shape)})
            try:
                if df.memory_usage(deep=False).sum() / 1e6 <= MAX_CHECKPOINT_MB:
                    df2 = df.copy()
                    df2.columns = [str(c) for c in df2.columns]
                    df2.to_parquet(os.path.join(self._ckpt_dir(), f"{name}.parquet"), index=False)
            except Exception:  # noqa: BLE001
                pass
        return changed

    def _restore_checkpoints(self):
        names = []
        ns, sigs = self.ns, self._df_sigs
        d = self._ckpt_dir()
        for f in sorted(os.listdir(d)):
            if f.endswith(".parquet"):
                n = f[:-8]
                try:
                    ns[n] = pd.read_parquet(os.path.join(d, f))
                    sigs[n] = self._sig(ns[n])
                    names.append(n)
                except Exception:  # noqa: BLE001
                    pass
        return names

    def op_checkpoint(self, req):
        self._df_sigs.clear()
        return {"changed": self._sync_dataframes()}

    def op_vars(self, req):
        out = []
        for k, v in self.ns.items():
            if k.startswith("_") or k in ("pd", "duckdb", "con", "sql", "WORKSPACE", "PROJECT_ROOT", "OUTPUT_DIR") or callable(v) or type(v).__name__ == "module":
                continue
            desc = f"DataFrame{list(v.shape)}" if isinstance(v, pd.DataFrame) else type(v).__name__
            out.append({"name": k, "type": desc})
        return {"vars": out}

    # ---------- data ops ----------
    def op_import(self, req):
        path = self.resolve(req["path"])
        importer._progress.hook = self.progress
        results, frames = importer.import_file(self.con, path, table=req.get("table"), header_row=req.get("header_row"),
                                               sheet=req.get("sheet"), mode=req.get("mode", "replace"),
                                               small_rows=self.small_rows, columns=req.get("columns"), types=req.get("types"),
                                               dry_run=bool(req.get("preview")))
        if req.get("preview"):
            return {"preview": True, "tables": results}
        for r in results:
            if r["rows"] <= self.small_rows:
                self.ns[r["table"]] = frames[r["table"]][0]   # small: also expose as pandas DataFrame
                self._df_sigs[r["table"]] = self._sig(self.ns[r["table"]])
            self.progress(f"分析列：{r['table']}")
            p = profile.profile_table(self.con, r["table"])
            r["fingerprint"], r["column_set"] = profile.schema_fingerprint(p)
            r["profile"] = p
        try:
            self._record_source(req, path, results)
        except Exception:  # noqa: BLE001  (bookkeeping must never fail an import)
            traceback.print_exc()
        return {"tables": results}

    # ---------- source manifest (project files → tables; used to detect changed / deleted sources) ----------
    @property
    def _sources_path(self):
        return os.path.join(self.ws, "sources.json")

    def _load_sources(self):
        try:
            with open(self._sources_path, encoding="utf-8") as f:
                return json.load(f)
        except Exception:  # noqa: BLE001
            return {"imports": []}

    def _save_sources(self, m):
        tmp = self._sources_path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(m, f, ensure_ascii=False, indent=1)
        replace_file(tmp, self._sources_path)

    def _record_source(self, req, path, results):
        """Remember which project file (+ mtime / size / import options) produced which tables."""
        if not self.project_root:
            return
        if not is_within(self.project_root, path):
            return
        root, ap = os.path.realpath(self.project_root), os.path.realpath(path)
        rel, st = os.path.relpath(ap, root), os.stat(ap)
        params = {k: req[k] for k in ("table", "sheet", "header_row", "columns", "types") if req.get(k) is not None}
        mode = req.get("mode") or "replace"
        tables = [r["table"] for r in results]
        with self._src_lock:
            m = self._load_sources()
            imps = [e for e in m.get("imports", []) if not (e.get("path") == rel and (e.get("params") or {}) == params)]
            if mode != "append":
                for e in imps:
                    e["tables"] = [t for t in e.get("tables", []) if t not in tables]
                imps = [e for e in imps if e.get("tables")]
            imps.append({"path": rel, "params": params, "mode": mode, "tables": tables,
                         "mtime": st.st_mtime, "size": st.st_size, "imported_at": time.time()})
            m["imports"] = imps
            self._save_sources(m)

    def resolve(self, path):
        """Relative paths: project root first (original files, read-only), then the data workspace."""
        if os.path.isabs(path):
            return path
        for base in [self.project_root, self.ws]:
            if base and os.path.exists(os.path.join(base, path)):
                return os.path.join(base, path)
        return os.path.join(self.project_root or self.ws, path)

    def op_extract_text(self, req):
        from . import office
        return office.extract_text(self.resolve(req["path"]), max_chars=int(req.get("max_chars", 60000)), pages=req.get("pages"))

    def op_drop_tables(self, req):
        """Drop tables whose source file was deleted (user confirmed in the app)."""
        dropped = []
        for t in req.get("tables") or []:
            self.con.execute(f"DROP TABLE IF EXISTS {qident(t)}")
            dropped.append(t)
        with self._src_lock:
            m = self._load_sources()
            for e in m.get("imports", []):
                e["tables"] = [t for t in e.get("tables", []) if t not in dropped]
            m["imports"] = [e for e in m.get("imports", []) if e.get("tables")]
            self._save_sources(m)
        return {"dropped": dropped}

    def op_tables(self, req):
        return {"tables": profile.list_tables(self.con)}

    def op_profile(self, req):
        tables = req.get("tables") or [t["table"] for t in profile.list_tables(self.con)]
        return {"profiles": [profile.profile_table(self.con, t) for t in tables]}

    def _relations(self, tables=None, refresh=False, legacy=None):
        """Stored relations (+ cached inference, recomputed only when the table structure changed or on refresh)."""
        if legacy:   # migrate relations passed in from catalog.json once
            d = self.rels.load()
            if not d["relations"]:
                for r in legacy:
                    try:
                        n = relations.normalize(r); n.update(status="confirmed", source="model"); self.rels.upsert(n)
                    except Exception:  # noqa: BLE001
                        pass
        lst = profile.list_tables(self.con)
        stamp = relations.tables_stamp(lst)
        if (refresh or self.rels.cached_stamp() != stamp) and len(lst) > 1:
            profs = [profile.profile_table(self.con, t["table"]) for t in lst]
            self.rels.cache_inferred(stamp, relations.infer(self.con, profs))
        return self.rels.effective(tables)

    def op_schema(self, req):
        tables = req.get("tables") or [t["table"] for t in profile.list_tables(self.con)]
        profs = [profile.profile_table(self.con, t) for t in tables]
        rels = self._relations(tables if req.get("tables") else None, legacy=req.get("relations")) if len(tables) > 1 else []
        text = profile.m_schema(profs, None, req.get("descriptions"))
        if rels:
            text += "\n" + relations.render(rels)
        return {"m_schema": text, "relations": [relations.flat(r) for r in rels],
                "fingerprints": {p["table"]: profile.schema_fingerprint(p)[0] for p in profs}}

    def op_relations(self, req):
        rels = self._relations(refresh=bool(req.get("refresh")))
        return {"relations": [relations.flat(r) for r in rels]}

    def _side(self, v):
        if isinstance(v, str) and v.strip().startswith("{"):   # models sometimes send the object JSON-encoded
            v = json.loads(v)
        if isinstance(v, str):                      # "table.column"
            t, _, c = v.rpartition(".")
            if not t or not c:
                raise ValueError(f"expected 'table.column' or {{table, columns}}, got {v!r}")
            return {"table": t, "columns": [c]}
        if not isinstance(v, dict) or not v.get("table"):
            raise ValueError(f"relation side needs a table: {v!r}")
        side = dict(v)
        if side.get("column") and not side.get("columns"):
            side["columns"] = [side.pop("column")]
        if isinstance(side.get("columns"), str):
            side["columns"] = [side["columns"]]
        return side

    def op_relation_test(self, req):
        """Match quality of a candidate join (+ a suggested transform when it is poor). Read-only."""
        frm, to = self._side(req["from"]), self._side(req["to"])
        r = relations.test(self.con, frm, to, req.get("mapping"))
        if r["match_rate"] < 0.95 and not req.get("mapping"):
            best = relations.probe(self.con, frm, to, base_rate=r["match_rate"])
            if best:
                r["suggestion"] = best
        return r

    def op_relation_define(self, req):
        """Confirm / reject / delete a relation, or save a new one after verifying its match rate."""
        if req.get("id") and req.get("action") in ("confirm", "reject"):
            hit = self.rels.set_status(req["id"], "confirmed" if req["action"] == "confirm" else "rejected")
            return {"ok": hit is not None, "relation": relations.flat(hit) if hit else None}
        if req.get("id") and req.get("action") == "delete":
            self.rels.delete(req["id"]); return {"ok": True}
        frm, to = self._side(req["from"]), self._side(req["to"])
        mapping = req.get("mapping")
        if req.get("mapping_pairs"):               # [[from_value, to_value], ...] given in the chat
            name = "map_" + re.sub(r"[^0-9A-Za-z_\u4e00-\u9fff]", "_", f"{frm['table']}_{to['table']}")[:60]
            df = pd.DataFrame([[str(a), str(b)] for a, b in req["mapping_pairs"]], columns=["from_value", "to_value"])
            self.con.register("_dp_map_df", df)
            self.con.execute(f"CREATE OR REPLACE TABLE {qident(name)} AS SELECT * FROM _dp_map_df")
            self.con.unregister("_dp_map_df")
            mapping = {"table": name, "from_col": "from_value", "to_col": "to_value"}
        t = relations.test(self.con, frm, to, mapping)
        need = float(req.get("min_match", 0.9))
        if t["match_rate"] < need and not req.get("force"):
            out = {"ok": False, "reason": f"match rate {t['match_rate']:.1%} < {need:.0%}", **t}
            if not mapping:
                best = relations.probe(self.con, frm, to, base_rate=t["match_rate"])
                if best:
                    out["suggestion"] = best
            return out
        rel = {"from": frm, "to": to, "status": "confirmed", "source": req.get("source", "model"),
               "match_rate": t["match_rate"], "cardinality": t["cardinality"], "unmatched_samples": t["unmatched_samples"],
               "note": req.get("note", "")}
        if mapping:
            rel["mapping"] = mapping
        saved = self.rels.upsert(rel)
        out = {"ok": True, "relation": relations.flat(saved), **t}
        if req.get("materialize"):
            view = req.get("view_name") or f"v_{frm['table']}_key"
            key = relations.key_sql(frm, "a")
            sql = f"CREATE OR REPLACE VIEW {qident(view)} AS SELECT a.*, {key} AS {qident('_key_' + to['table'])} FROM {qident(frm['table'])} a"
            try:
                out["view"] = self.op_transform({"sql": sql})["target"]
            except Exception as e:  # noqa: BLE001
                out["view_error"] = str(e)
        return out

    def op_relation_revalidate(self, req):
        """Re-test confirmed relations touching `changed` tables (after a source re-import). Reports drops."""
        changed = {t.lower() for t in (req.get("changed") or [])}
        existing = {t["table"].lower() for t in profile.list_tables(self.con)}
        d = self.rels.load()
        dropped, checked = [], 0
        for r in d["relations"]:
            if r.get("status") != "confirmed":
                continue
            ft, tt = r["from"]["table"].lower(), r["to"]["table"].lower()
            if changed and not ({ft, tt} & changed):
                continue
            if ft not in existing or tt not in existing:
                dropped.append({"id": r["id"], "before": r.get("match_rate"), "after": None, "error": "table missing",
                                "relation": f"{r['from']['table']}.{','.join(r['from']['columns'])} → {r['to']['table']}.{','.join(r['to']['columns'])}"})
                continue
            checked += 1
            try:
                t = relations.test(self.con, r["from"], r["to"], r.get("mapping"))
            except Exception as e:  # noqa: BLE001
                dropped.append({"id": r["id"], "before": r.get("match_rate"), "after": None, "error": str(e),
                                "relation": f"{r['from']['table']}.{','.join(r['from']['columns'])} → {r['to']['table']}.{','.join(r['to']['columns'])}"})
                continue
            before = r.get("match_rate") or 0
            r.update(match_rate=t["match_rate"], unmatched_samples=t["unmatched_samples"], cardinality=t["cardinality"],
                     checked=time.time())
            if t["match_rate"] < 0.9 and t["match_rate"] < before - 0.05:
                dropped.append({"id": r["id"], "before": before, "after": t["match_rate"], "unmatched_samples": t["unmatched_samples"],
                                "relation": f"{r['from']['table']}.{','.join(r['from']['columns'])} → {r['to']['table']}.{','.join(r['to']['columns'])}"})
        with self._rel_lock:
            cur = self.rels.load()
            byid = {x["id"]: x for x in d["relations"]}
            cur["relations"] = [byid.get(x["id"], x) for x in cur["relations"]]
            self.rels.save(cur)
        return {"checked": checked, "dropped": dropped}

    def op_er_layout(self, req):
        """ER diagram node positions (data/er_layout.json): get, or save {positions:{table:[x,y]}}."""
        p = os.path.join(self.ws, "er_layout.json")
        if req.get("positions") is not None:
            with open(p, "w", encoding="utf-8") as f:
                json.dump({"positions": req["positions"]}, f, ensure_ascii=False)
            return {"ok": True}
        try:
            with open(p, encoding="utf-8") as f:
                return json.load(f)
        except Exception:  # noqa: BLE001
            return {"positions": {}}

    def _register_frames(self, sql):
        """Python DataFrames of the calling chat become queryable by name in SQL (registered on this request's
        cursor only, so other chats never see them). DuckDB tables with the same name win."""
        try:
            names = set(sqlguard.tables_in(sql))
            if not names:
                return
            existing = {t[0].lower() for t in self.con.execute(
                "SELECT table_name FROM information_schema.tables WHERE table_schema='main'").fetchall()}
            ns = self.nss.get(self.ns_name) or {}
            for n in names:
                v = ns.get(n)
                if n.lower() not in existing and isinstance(v, pd.DataFrame):
                    self.con.register(n, v)
        except Exception:  # noqa: BLE001
            pass

    def op_sql(self, req):
        sql = sqlguard.check_readonly(req["sql"])
        self._register_frames(sql)
        limit = int(req.get("limit", PREVIEW_ROWS))
        t0 = time.time()
        rel = self.con.sql(sql)
        if rel is None:
            return {"columns": [], "rows": [], "row_count": 0}
        df = rel.df()
        ms = int((time.time() - t0) * 1000)
        result_id = req.get("result_id") or f"q_{int(time.time()*1000)}"
        path = None
        if req.get("save", True) and len(df):
            path = os.path.join(self.ws, "artifacts", f"{result_id}.parquet")
            df2 = df.copy(); df2.columns = [str(c) for c in df2.columns]
            df2.to_parquet(path, index=False)
        preview = df.head(limit)
        out = {"result_id": result_id, "columns": [str(c) for c in df.columns],
               "dtypes": [str(t) for t in df.dtypes],
               "rows": json.loads(preview.to_json(orient="values", date_format="iso", force_ascii=False)),
               "row_count": int(len(df)), "truncated": len(df) > limit, "ms": ms, "artifact": path,
               "tables": sqlguard.tables_in(sql)}
        if len(df) == 0:
            out["hint"] = ("Empty result. Check filter values: query DISTINCT values of the filtered columns "
                           "(e.g. SELECT DISTINCT col FROM t LIMIT 50) and retry with exact matches.")
        return out

    def op_query_data(self, req):
        """Unrestricted-shape data fetch for dashboards: read-only SQL, returns columns + all rows (capped)."""
        sql = sqlguard.check_readonly(req["sql"])
        cap = int(req.get("cap", 100_000))
        df = self.con.sql(f"SELECT * FROM ({sql}) _q LIMIT {cap + 1}").df()
        meta = {"columns": [str(c) for c in df.columns], "dtypes": [str(t) for t in df.dtypes],
                "row_count": int(min(len(df), cap)), "capped": len(df) > cap}
        rows_json = df.head(cap).to_json(orient="values", date_format="iso", force_ascii=False)
        if req.get("to_file"):
            # Large dashboard datasets: write the rows JSON once to a temp file; the app hands the text to the web
            # view verbatim (no json.loads → json.dumps here, no parse → re-encode in Swift).
            import tempfile
            fd, path = tempfile.mkstemp(prefix="qd_", suffix=".json", dir=os.environ.get("TMPDIR") or self.ws)
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(rows_json)
            meta["rows_file"] = path
            return meta
        meta["rows"] = json.loads(rows_json)
        return meta

    # ---------- derived tables (data_transform) ----------
    @property
    def _derived_path(self):
        return os.path.join(self.ws, "derived.json")

    def _load_derived(self):
        try:
            with open(self._derived_path, encoding="utf-8") as f:
                return json.load(f)
        except Exception:  # noqa: BLE001
            return {"tables": []}

    def _save_derived(self, d):
        tmp = self._derived_path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(d, f, ensure_ascii=False, indent=1)
        replace_file(tmp, self._derived_path)

    def _base_tables(self, derived_names):
        return [t["table"] for t in profile.list_tables(self.con) if t["table"].lower() not in derived_names]

    def op_transform(self, req):
        """Controlled write SQL: CREATE [OR REPLACE] TABLE|VIEW … AS SELECT, or ALTER/UPDATE/DROP of a derived table.
        Imported tables are never modified. Lineage is kept in derived.json (rebuilt after source re-imports)."""
        with self._src_lock:
            d = self._load_derived()
        derived = {e["name"].lower() for e in d.get("tables", [])}
        sql, kind, target, replaces = sqlguard.check_transform(req["sql"], self._base_tables(derived), derived)
        if req.get("dry_run"):
            return {"kind": kind, "target": target, "replaces": replaces}
        t0 = time.time()
        self.con.execute(sql)
        ms = int((time.time() - t0) * 1000)
        with self._src_lock:
            d = self._load_derived()
            items = d.get("tables", [])
            if kind == "drop":
                items = [e for e in items if e["name"].lower() != target.lower()]
            else:
                e = next((e for e in items if e["name"].lower() == target.lower()), None)
                if kind.startswith("create_"):
                    deps = [x for x in sqlguard.tables_in(sql) if x.lower() != target.lower()]
                    new = {"name": target, "kind": kind[7:], "sql": sql, "deps": deps, "steps": [],
                           "created": time.time(), "session": self.ns_name}
                    items = [x for x in items if x is not e] + [new]
                elif e is not None:          # alter / update: replayed after the CREATE when rebuilding
                    e.setdefault("steps", []).append(sql)
            d["tables"] = items
            self._save_derived(d)
        out = {"kind": kind, "target": target, "replaced": replaces, "ms": ms}
        if kind != "drop":
            n = self.con.execute(f"SELECT count(*) FROM {qident(target)}").fetchall()[0][0]
            cols = self.con.execute("SELECT column_name, data_type FROM information_schema.columns WHERE table_schema='main' "
                                    "AND table_name=? ORDER BY ordinal_position", [target]).fetchall()
            out.update(rows=int(n), columns=[{"name": c, "type": t} for c, t in cols])
            out["preview"] = self.op_preview({"table": target, "limit": 10})
        return out

    def op_derived(self, req):
        with self._src_lock:
            return self._load_derived()

    def op_rebuild_derived(self, req):
        """Re-run derived tables (dependency order) after their source tables changed. `changed`: base tables re-imported."""
        with self._src_lock:
            items = list(self._load_derived().get("tables", []))
        changed = {t.lower() for t in (req.get("changed") or [])}
        names = {e["name"].lower() for e in items}
        dirty, rebuilt, failed = set(changed), [], []
        progress = True
        pending = list(items)
        while pending and progress:
            progress = False
            for e in list(pending):
                deps = {x.lower() for x in e.get("deps", [])}
                if deps & {p["name"].lower() for p in pending if p is not e}:
                    continue                 # wait until derived deps are rebuilt first
                pending.remove(e); progress = True
                if not changed or deps & dirty:
                    try:
                        sql = e["sql"]
                        if not re.match(r"(?is)^\s*create\s+or\s+replace", sql):
                            sql = re.sub(r"(?is)^\s*create\s+", "CREATE OR REPLACE ", sql, count=1)
                        self.con.execute(sql)
                        for st in e.get("steps", []):
                            self.con.execute(st)
                        rebuilt.append(e["name"]); dirty.add(e["name"].lower())
                    except Exception as ex:  # noqa: BLE001
                        failed.append({"name": e["name"], "error": f"{type(ex).__name__}: {ex}"})
        _ = names
        return {"rebuilt": rebuilt, "failed": failed}

    # ---------- export (data_export) ----------
    EXPORT_FORMATS = ("csv", "xlsx", "parquet", "json")
    XLSX_MAX_ROWS = 1_048_575

    def _out_dir(self):
        d = os.environ.get("DP_OUTPUT_DIR") or os.path.join(self.ws, "exports")
        os.makedirs(d, exist_ok=True)
        return d

    @staticmethod
    def _safe_name(name, ext):
        base = re.sub(r'[\\/:*?"<>|\x00-\x1f]', "_", (name or "").strip()) or f"export_{time.strftime('%Y%m%d_%H%M%S')}"
        if base.lower().endswith("." + ext):
            base = base[: -(len(ext) + 1)]
        return base[:120]

    def _unique_path(self, base, ext):
        d = self._out_dir()
        p = os.path.join(d, f"{base}.{ext}")
        i = 2
        while os.path.exists(p):
            p = os.path.join(d, f"{base}_{i}.{ext}"); i += 1
        return p

    def _export_source_sql(self, req):
        if req.get("sql"):
            sql = sqlguard.check_readonly(req["sql"])
            self._register_frames(sql)
            return sql
        if req.get("table"):
            return f"SELECT * FROM {qident(req['table'])}"
        if req.get("result_id"):
            rid = re.sub(r"[^A-Za-z0-9_\-]", "", str(req["result_id"]))
            p = os.path.join(self.ws, "artifacts", f"{rid}.parquet")
            if not os.path.exists(p):
                raise FileNotFoundError(f"result {rid} not found (re-run the query)")
            return "SELECT * FROM read_parquet(" + "'" + p.replace("'", "''") + "')"
        raise ValueError("one of sql / table / result_id is required")

    def op_export(self, req):
        """Write a query / table / saved result to OUTPUT_DIR as csv | xlsx | parquet | json. Never overwrites."""
        if req.get("kind") == "dictionary":
            return self.op_export_doc(req)
        fmt = (req.get("format") or "csv").lower()
        if fmt not in self.EXPORT_FORMATS:
            raise ValueError(f"format must be one of {self.EXPORT_FORMATS}")
        src = self._export_source_sql(req)
        cols = req.get("columns")             # [name] or {old: new}
        if isinstance(cols, dict) and cols:
            src = "SELECT " + ", ".join(f"{qident(k)} AS {qident(v)}" for k, v in cols.items()) + f" FROM ({src}) _s"
        elif isinstance(cols, list) and cols:
            src = "SELECT " + ", ".join(qident(c) for c in cols) + f" FROM ({src}) _s"
        limit = req.get("limit")
        if limit:
            src = f"SELECT * FROM ({src}) _l LIMIT {int(limit)}"
        n = self.con.execute(f"SELECT count(*) FROM ({src}) _c").fetchall()[0][0]
        if fmt == "xlsx" and n > self.XLSX_MAX_ROWS:
            raise ValueError(f"{n} rows exceed Excel's limit ({self.XLSX_MAX_ROWS + 1} rows incl. header); export as csv instead")
        path = self._unique_path(self._safe_name(req.get("filename"), fmt), fmt)
        lit = "'" + path.replace("'", "''") + "'"
        if fmt == "csv":
            if req.get("excel_compatible", True):
                # UTF-8 BOM so Excel opens Chinese text correctly; DuckDB writes the rows after it
                tmp = path + ".part"
                self.con.execute(f"COPY ({src}) TO '{tmp.replace(chr(39), chr(39) * 2)}' (FORMAT csv, HEADER true)")
                with open(path, "wb") as out, open(tmp, "rb") as inp:
                    out.write(b"\xef\xbb\xbf")
                    while True:
                        b = inp.read(1 << 20)
                        if not b:
                            break
                        out.write(b)
                os.remove(tmp)
            else:
                self.con.execute(f"COPY ({src}) TO {lit} (FORMAT csv, HEADER true)")
        elif fmt == "parquet":
            self.con.execute(f"COPY ({src}) TO {lit} (FORMAT parquet)")
        elif fmt == "json":
            self.con.execute(f"COPY ({src}) TO {lit} (FORMAT json, ARRAY true)")
        else:
            self._write_xlsx(src, path, int(n), sheet=(req.get("sheet") or "data")[:31])
        return {"path": path, "format": fmt, "rows": int(n), "bytes": os.path.getsize(path)}

    def _write_xlsx(self, src, path, total, sheet="data", batch=20_000):
        """Stream rows into XlsxWriter (constant_memory) in batches, each cell through export_cells.CHAIN."""
        import xlsxwriter
        from . import export_cells
        # tz-aware timestamps: convert in SQL (fetching them into Python would need pytz; Excel has no tz anyway)
        desc = self.con.execute(f"DESCRIBE SELECT * FROM ({src}) _d").fetchall()
        if any("WITH TIME ZONE" in str(d[1]).upper() for d in desc):
            sel = ", ".join(f"CAST({qident(d[0])} AS TIMESTAMP) AS {qident(d[0])}" if "WITH TIME ZONE" in str(d[1]).upper()
                            else qident(d[0]) for d in desc)
            src = f"SELECT {sel} FROM ({src}) _tz"
        cur = self.con.execute(src)
        cols = [d[0] for d in cur.description]
        types = [str(d[1]) for d in cur.description]
        wb = xlsxwriter.Workbook(path, {"constant_memory": True, "strings_to_urls": False, "strings_to_formulas": False})
        try:
            ws = wb.add_worksheet(sheet)
            fmts = export_cells.Formats(wb)
            head = wb.add_format({"bold": True, "bg_color": "#F2F2F2", "border": 1})
            for j, c in enumerate(cols):
                ws.write_string(0, j, str(c), head)
                ws.set_column(j, j, min(40, max(8, len(str(c)) * 2)))
            ws.freeze_panes(1, 0)
            ctxs = [export_cells.Ctx(column=c, db_type=t, fmt="xlsx") for c, t in zip(cols, types)]
            i = 0
            while True:
                rows = cur.fetchmany(batch)
                if not rows:
                    break
                for row in rows:
                    i += 1
                    for j, v in enumerate(row):
                        cell = export_cells.CHAIN.process(ctxs[j], v)
                        export_cells.write(ws, i, j, cell, fmts)
                self.progress(f"xlsx {i:,}/{total:,} rows ({i * 100 // max(total, 1)}%)")
        finally:
            wb.close()

    _DOC_COLS = ["列 column", "类型 type", "说明 description", "空值% null", "唯一 unique", "示例 examples"]
    _REL_COLS = ["from_table", "from_column", "to_table", "to_column", "status", "transform"]

    @staticmethod
    def _cell(v):
        return "" if v is None or (isinstance(v, float) and v != v) else str(v)

    def _dict_docx(self, df, rels, path):
        import docx
        from docx.shared import Pt
        d = docx.Document()
        st = d.styles["Normal"]; st.font.size = Pt(9); st.font.name = "Calibri"   # creates rPr/rFonts
        st.element.rPr.rFonts.set(docx.oxml.ns.qn("w:eastAsia"), "PingFang SC")
        d.add_heading("数据字典 / Data dictionary", 0)
        for t, g in df.groupby("表 table", sort=False):
            d.add_heading(str(t), 1)
            meta = f"{g['行数 rows'].iloc[0]} 行"
            if self._cell(g["表说明"].iloc[0]):
                meta += " · " + self._cell(g["表说明"].iloc[0])
            d.add_paragraph(meta)
            tb = d.add_table(rows=1, cols=len(self._DOC_COLS)); tb.style = "Light Grid Accent 1"
            for j, c in enumerate(self._DOC_COLS):
                tb.rows[0].cells[j].text = c
            for row in g[self._DOC_COLS].itertuples(index=False):
                cells = tb.add_row().cells
                for j, v in enumerate(row):
                    cells[j].text = self._cell(v)[:300]
        if rels:
            d.add_heading("表间关系 / Relations", 1)
            tb = d.add_table(rows=1, cols=len(self._REL_COLS)); tb.style = "Light Grid Accent 1"
            for j, c in enumerate(self._REL_COLS):
                tb.rows[0].cells[j].text = c
            for r in rels:
                cells = tb.add_row().cells
                for j, c in enumerate(self._REL_COLS):
                    cells[j].text = self._cell(r.get(c))
        d.save(path)

    def _dict_pdf(self, df, rels, path):
        from reportlab.lib import colors
        from reportlab.lib.pagesizes import A4, landscape
        from reportlab.lib.styles import ParagraphStyle
        from reportlab.pdfbase import pdfmetrics
        from reportlab.pdfbase.cidfonts import UnicodeCIDFont
        from reportlab.platypus import Paragraph, SimpleDocTemplate, Spacer, Table, TableStyle
        from xml.sax.saxutils import escape
        if "STSong-Light" not in pdfmetrics.getRegisteredFontNames():
            pdfmetrics.registerFont(UnicodeCIDFont("STSong-Light"))   # built-in CJK font, no font file needed
        base = ParagraphStyle("b", fontName="STSong-Light", fontSize=8, leading=10)
        h1 = ParagraphStyle("h1", parent=base, fontSize=16, leading=20, spaceAfter=8)
        h2 = ParagraphStyle("h2", parent=base, fontSize=12, leading=16, spaceBefore=10, spaceAfter=4)
        P = lambda v: Paragraph(escape(self._cell(v)[:300]), base)  # noqa: E731
        style = TableStyle([("GRID", (0, 0), (-1, -1), 0.4, colors.HexColor("#cccccc")),
                            ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#f2f2f2")), ("VALIGN", (0, 0), (-1, -1), "TOP")])
        page = landscape(A4)
        w = page[0] - 60
        story = [Paragraph("数据字典 / Data dictionary", h1)]
        widths = [w * x for x in (0.16, 0.1, 0.3, 0.08, 0.06, 0.3)]
        for t, g in df.groupby("表 table", sort=False):
            story.append(Paragraph(escape(f"{t}（{g['行数 rows'].iloc[0]} 行）{self._cell(g['表说明'].iloc[0])}"), h2))
            data = [[P(c) for c in self._DOC_COLS]] + [[P(v) for v in row] for row in g[self._DOC_COLS].itertuples(index=False)]
            story.append(Table(data, colWidths=widths, repeatRows=1, style=style))
        if rels:
            story += [Spacer(1, 8), Paragraph("表间关系 / Relations", h2)]
            data = [[P(c) for c in self._REL_COLS]] + [[P(r.get(c)) for c in self._REL_COLS] for r in rels]
            story.append(Table(data, colWidths=[w / len(self._REL_COLS)] * len(self._REL_COLS), repeatRows=1, style=style))
        SimpleDocTemplate(path, pagesize=page, leftMargin=30, rightMargin=30, topMargin=30, bottomMargin=30).build(story)

    def op_export_doc(self, req):
        """Data dictionary: tables, columns, types, null %, examples, catalog descriptions, relations."""
        fmt = (req.get("format") or "xlsx").lower()
        if fmt not in ("xlsx", "md", "html", "csv", "docx", "pdf"):
            raise ValueError("dictionary format must be xlsx | md | html | csv | docx | pdf")
        tables = req.get("tables") or [t["table"] for t in profile.list_tables(self.con)]
        desc = req.get("descriptions") or {}
        rows = []
        for t in tables:
            p = profile.profile_table(self.con, t)
            for c in p["columns"]:
                ex = c.get("values") or c.get("examples") or []
                rows.append({"表 table": t, "表说明": desc.get(t, {}).get("_table", ""), "行数 rows": p["rows"],
                             "列 column": c["name"], "类型 type": c["type"], "说明 description": desc.get(t, {}).get(c["name"], ""),
                             "空值% null": c.get("null_pct", 0), "唯一 unique": "Y" if c.get("unique") else "",
                             "不同值 distinct": c.get("distinct"), "示例 examples": ", ".join(str(v) for v in ex[:5])})
        df = pd.DataFrame(rows)
        rels = req.get("relations") or []
        path = self._unique_path(self._safe_name(req.get("filename") or "数据字典", fmt), fmt)
        if fmt == "xlsx":
            with pd.ExcelWriter(path, engine="xlsxwriter") as w:
                df.to_excel(w, index=False, sheet_name="columns")
                if rels:
                    pd.DataFrame(rels).to_excel(w, index=False, sheet_name="relations")
        elif fmt == "csv":
            df.to_csv(path, index=False, encoding="utf-8-sig")
        elif fmt == "docx":
            self._dict_docx(df, rels, path)
        elif fmt == "pdf":
            self._dict_pdf(df, rels, path)
        elif fmt == "md":
            with open(path, "w", encoding="utf-8") as f:
                f.write("# 数据字典 / Data dictionary\n\n")
                for t, g in df.groupby("表 table", sort=False):
                    f.write(f"## {t}\n\n" + _md_table(g.drop(columns=["表 table"])) + "\n\n")
                if rels:
                    f.write("## 表间关系 / Relations\n\n" + _md_table(pd.DataFrame(rels)) + "\n")
        else:
            html = df.to_html(index=False, escape=True)
            with open(path, "w", encoding="utf-8") as f:
                f.write("<!doctype html><meta charset='utf-8'><title>数据字典</title>"
                        "<style>body{font:13px -apple-system,sans-serif;margin:24px}table{border-collapse:collapse}"
                        "td,th{border:1px solid #ddd;padding:4px 8px}th{background:#f5f5f5}</style><h1>数据字典</h1>" + html)
        return {"path": path, "format": fmt, "rows": len(df), "tables": len(tables), "bytes": os.path.getsize(path)}

    def op_preview(self, req):
        t = req["table"]
        n = int(req.get("limit", 100))
        df = self.con.sql(f"SELECT * FROM {qident(t)} LIMIT {n}").df()
        return {"columns": [str(c) for c in df.columns], "dtypes": [str(x) for x in df.dtypes],
                "rows": json.loads(df.to_json(orient="values", date_format="iso", force_ascii=False))}

    def close(self):
        try:
            for n in list(self.nss):
                self.ns_name = n
                self._sync_dataframes()
            for c in list(self.ns_cons.values()):
                try:
                    c.close()
                except Exception:  # noqa: BLE001
                    pass
            self._main.close()
        except Exception:  # noqa: BLE001
            traceback.print_exc()
