"""Table relationships: persisted store (relations.json), key transforms, match testing, transform probing.

A relation joins child (`from`) key columns to parent (`to`) key columns, optionally after normalising the keys
with transform steps (trim / strip leading zeros / strip prefix …) or through a user-supplied mapping table.

relations.json:
  {"version": 1,
   "relations": [ {id, from:{table, columns[], steps[]?, expr?}, to:{...}, mapping?:{table, from_col, to_col},
                   cardinality, status: confirmed|rejected|suspect|inferred, match_rate, unmatched_samples[],
                   note, source: auto|user|model, updated} ],
   "inferred": {"stamp": "...", "items": [...]}}      # cache of automatic inference (never authoritative)
"""
import hashlib
import json
import os
import re
import time

import sqlglot
from sqlglot import exp

from .util import qident, replace_file

# ---------- key transforms ----------
NAMED_STEPS = {
    "trim": lambda s: f"trim({s})",
    "upper": lambda s: f"upper({s})",
    "lower": lambda s: f"lower({s})",
    "strip_zeros": lambda s: f"ltrim({s}, '0')",
    "num": lambda s: f"regexp_replace({s}, '\\.0+$', '')",
    "digits": lambda s: f"regexp_replace({s}, '[^0-9]', '', 'g')",
    "strip_prefix": lambda s: f"regexp_replace({s}, '^[A-Za-z]+[-_ ./]*', '')",
    "strip_spaces": lambda s: f"regexp_replace({s}, '\\s+', '', 'g')",
}
STEP_LABEL = {"trim": "去首尾空格", "upper": "转大写", "lower": "转小写", "strip_zeros": "去前导零", "num": "数值规范化(1.0→1)",
              "digits": "只保留数字", "strip_prefix": "去字母前缀", "strip_spaces": "去所有空格"}
# symmetric step sequences tried when the raw keys do not match well
PROBES = [["trim"], ["trim", "upper"], ["num"], ["trim", "strip_zeros"], ["trim", "num", "strip_zeros"],
          ["trim", "strip_prefix", "strip_zeros"], ["digits", "strip_zeros"], ["strip_spaces", "upper"]]

_ALLOWED_FUNCS = {"trim", "ltrim", "rtrim", "upper", "lower", "lpad", "rpad", "replace", "regexp_replace", "regexp_extract",
                  "substr", "substring", "left", "right", "concat", "concat_ws", "coalesce", "strftime", "strptime",
                  "lcase", "ucase", "reverse", "split_part", "length", "nullif"}


def _lit(v):
    return "'" + str(v).replace("'", "''") + "'"


class RelationError(ValueError):
    pass


def check_expr(expr):
    """User key expression with {col} placeholder: only whitelisted scalar functions, casts, literals, ||."""
    if "{col}" not in expr:
        raise RelationError("expr must reference the key column as {col}")
    s = expr.replace("{col}", "__k")
    if ";" in s:
        raise RelationError("expr must be a single expression")
    try:
        tree = sqlglot.parse_one(s, read="duckdb")
    except Exception as e:  # noqa: BLE001
        raise RelationError(f"cannot parse expr: {e}")
    for n in tree.walk():
        if isinstance(n, (exp.Select, exp.Subquery, exp.Table, exp.Command, exp.Query)):
            raise RelationError("expr may not contain queries")
        if isinstance(n, exp.Column) and n.name != "__k":
            raise RelationError(f"expr may only reference {{col}}, not {n.name}")
        if isinstance(n, exp.Func) and not isinstance(n, (exp.Cast, exp.TryCast, exp.DPipe)):
            name = (n.name if isinstance(n, exp.Anonymous) else n.sql_name()).lower()
            if name not in _ALLOWED_FUNCS and not any(name.startswith(a) for a in ("regexp_", "trim", "pad", "substr")):
                raise RelationError(f"function {name}() is not allowed in key expressions")
    return expr


def step_sql(step, inner):
    if isinstance(step, str):
        f = NAMED_STEPS.get(step)
        if not f:
            raise RelationError(f"unknown step '{step}' (allowed: {', '.join(NAMED_STEPS)}, lpad:N, replace, regex_extract, substr)")
        return f(inner)
    if isinstance(step, dict) and len(step) == 1:
        (k, v), = step.items()
        if k == "lpad":
            return f"lpad({inner}, {int(v)}, '0')"
        if k == "replace":
            a, b = v
            return f"replace({inner}, {_lit(a)}, {_lit(b)})"
        if k == "regex_extract":
            return f"regexp_extract({inner}, {_lit(v)})"
        if k == "regex_replace":
            a, b = v
            return f"regexp_replace({inner}, {_lit(a)}, {_lit(b)}, 'g')"
        if k == "substr":
            start, ln = (v + [None])[:2] if isinstance(v, list) else (v, None)
            return f"substr({inner}, {int(start)}" + (f", {int(ln)})" if ln else ")")
        if k == "prefix":
            return f"concat({_lit(v)}, {inner})"
    raise RelationError(f"bad step {step!r}")


def describe_steps(steps):
    out = []
    for s in steps or []:
        if isinstance(s, str):
            out.append(STEP_LABEL.get(s, s))
        elif isinstance(s, dict):
            (k, v), = s.items()
            out.append({"lpad": f"左补零到{v}位", "replace": f"替换{v}", "regex_extract": f"正则提取 {v}",
                        "regex_replace": f"正则替换 {v}", "substr": f"截取 {v}", "prefix": f"加前缀 {v}"}.get(k, f"{k}:{v}"))
    return " → ".join(out)


def key_sql(side, alias=None):
    """SQL expression for one side's (possibly composite) normalised key."""
    cols = side.get("columns") or ([side["column"]] if side.get("column") else [])
    if not cols:
        raise RelationError("each side needs columns")
    parts = []
    for c in cols:
        ref = (f"{alias}." if alias else "") + qident(c)
        e = f"CAST({ref} AS VARCHAR)"
        if side.get("expr"):
            e = check_expr(side["expr"]).replace("{col}", e)
        for st in side.get("steps") or []:
            e = step_sql(st, e)
        parts.append(e)
    return parts[0] if len(parts) == 1 else "concat_ws(chr(31), " + ", ".join(parts) + ")"


def _not_null(side, alias=None):
    cols = side.get("columns") or [side.get("column")]
    return " AND ".join(f"{(alias + '.') if alias else ''}{qident(c)} IS NOT NULL" for c in cols)


# ---------- testing ----------
def test(con, frm, to, mapping=None, sample=200_000, samples=6):
    """Match quality of child keys against parent keys after transforms (optionally through a mapping table)."""
    fk, tk = key_sql(frm), key_sql(to)
    child = f"SELECT {fk} AS k FROM {qident(frm['table'])} WHERE {_not_null(frm)} LIMIT {int(sample)}"
    if mapping:
        m = mapping
        child = (f"SELECT CAST(m.{qident(m['to_col'])} AS VARCHAR) AS k, c.k AS raw FROM ({child}) c "
                 f"LEFT JOIN {qident(m['table'])} m ON c.k = CAST(m.{qident(m['from_col'])} AS VARCHAR)")
    else:
        child = f"SELECT k, k AS raw FROM ({child}) c0"
    parent = f"SELECT DISTINCT {tk} AS k FROM {qident(to['table'])} WHERE {_not_null(to)}"
    n, hit, nd, hitd = con.execute(
        f"WITH c AS ({child}), p AS ({parent}) SELECT count(*), count(p.k), count(DISTINCT c.raw), "
        f"count(DISTINCT CASE WHEN p.k IS NOT NULL THEN c.raw END) FROM c LEFT JOIN p ON c.k = p.k").fetchall()[0]
    unmatched = [r[0] for r in con.execute(
        f"WITH c AS ({child}), p AS ({parent}) SELECT DISTINCT c.raw FROM c LEFT JOIN p ON c.k = p.k "
        f"WHERE p.k IS NULL LIMIT {samples}").fetchall()]
    raw_child = [r[0] for r in con.execute(
        f"SELECT DISTINCT CAST({qident((frm.get('columns') or [frm.get('column')])[0])} AS VARCHAR) FROM {qident(frm['table'])} "
        f"WHERE {_not_null(frm)} LIMIT {samples}").fetchall()]
    raw_parent = [r[0] for r in con.execute(
        f"SELECT DISTINCT CAST({qident((to.get('columns') or [to.get('column')])[0])} AS VARCHAR) FROM {qident(to['table'])} "
        f"WHERE {_not_null(to)} LIMIT {samples}").fetchall()]
    pu = con.execute(f"SELECT count(*) = count(DISTINCT k) FROM (SELECT {tk} AS k FROM {qident(to['table'])} WHERE {_not_null(to)})").fetchall()[0][0]
    cu = con.execute(f"SELECT count(*) = count(DISTINCT k) FROM (SELECT {fk} AS k FROM {qident(frm['table'])} WHERE {_not_null(frm)})").fetchall()[0][0]
    rate = (hit / n) if n else 0.0
    return {"match_rate": round(rate, 4), "distinct_match_rate": round(hitd / nd, 4) if nd else 0.0, "rows_checked": int(n),
            "unmatched_samples": unmatched, "from_samples": raw_child, "to_samples": raw_parent,
            "parent_unique": bool(pu), "cardinality": ("1:1" if cu else "N:1") if pu else "N:N",
            "join_sql": join_snippet(frm, to, mapping)}


def probe(con, frm, to, base_rate=None, min_gain=0.1):
    """Try symmetric transform sequences; return the best one if it clearly beats the raw match rate."""
    if base_rate is None:
        base_rate = test(con, frm, to)["match_rate"]
    best = None
    for steps in PROBES:
        f2 = dict(frm, steps=(frm.get("steps") or []) + steps)
        t2 = dict(to, steps=(to.get("steps") or []) + steps)
        try:
            r = test(con, f2, t2, samples=3)
        except Exception:  # noqa: BLE001
            continue
        if r["match_rate"] >= base_rate + min_gain and (best is None or r["match_rate"] > best["match_rate"] + 1e-9):
            best = {"steps": steps, "label": describe_steps(steps), "match_rate": r["match_rate"]}
            if r["match_rate"] >= 0.999:
                break
    return best


def join_snippet(frm, to, mapping=None, fa="a", ta="b"):
    fk, tk = key_sql(frm, fa), key_sql(to, ta)
    if mapping:
        return (f"FROM {qident(frm['table'])} {fa} JOIN {qident(mapping['table'])} m ON {fk} = CAST(m.{qident(mapping['from_col'])} AS VARCHAR) "
                f"JOIN {qident(to['table'])} {ta} ON CAST(m.{qident(mapping['to_col'])} AS VARCHAR) = {tk}")
    plain = not (frm.get("steps") or frm.get("expr") or to.get("steps") or to.get("expr")) and len(frm.get("columns") or [1]) == 1
    if plain:
        return f"FROM {qident(frm['table'])} {fa} JOIN {qident(to['table'])} {ta} ON {fa}.{qident(frm['columns'][0])} = {ta}.{qident(to['columns'][0])}"
    return f"FROM {qident(frm['table'])} {fa} JOIN {qident(to['table'])} {ta} ON {fk} = {tk}"


# ---------- store ----------
def rel_id(frm, to):
    k = f"{frm['table']}.{','.join(frm.get('columns') or [])}>{to['table']}.{','.join(to.get('columns') or [])}".lower()
    return "r_" + hashlib.sha1(k.encode()).hexdigest()[:10]


def normalize(r):
    """Accept the legacy flat form {from_table, from_column, to_table, to_column} too."""
    r = dict(r)
    if "from" not in r and r.get("from_table"):
        r["from"] = {"table": r["from_table"], "columns": [r["from_column"]]}
        r["to"] = {"table": r["to_table"], "columns": [r["to_column"]]}
    for s in ("from", "to"):
        side = dict(r[s])
        if side.get("column") and not side.get("columns"):
            side["columns"] = [side.pop("column")]
        r[s] = side
    r.setdefault("id", rel_id(r["from"], r["to"]))
    return r


def flat(r):
    """Flat fields used by older renderers / UI."""
    out = dict(r)
    out.update(from_table=r["from"]["table"], from_column=",".join(r["from"]["columns"]),
               to_table=r["to"]["table"], to_column=",".join(r["to"]["columns"]))
    tr = []
    if r["from"].get("steps") or r["from"].get("expr"):
        tr.append(f"{r['from']['table']}: " + (r["from"].get("expr") or describe_steps(r["from"]["steps"])))
    if r["to"].get("steps") or r["to"].get("expr"):
        tr.append(f"{r['to']['table']}: " + (r["to"].get("expr") or describe_steps(r["to"]["steps"])))
    if r.get("mapping"):
        tr.append(f"对照表 {r['mapping']['table']}")
    out["transform"] = "；".join(tr)
    return out


class Store:
    def __init__(self, ws, lock):
        self.path = os.path.join(ws, "relations.json")
        self.ws = ws
        self.lock = lock

    def load(self):
        try:
            with open(self.path, encoding="utf-8") as f:
                return json.load(f)
        except Exception:  # noqa: BLE001
            d = {"version": 1, "relations": [], "inferred": {}}
            try:   # migrate confirmed relations from the old catalog.json
                with open(os.path.join(self.ws, "catalog.json"), encoding="utf-8") as f:
                    for r in json.load(f).get("relations") or []:
                        n = normalize(r)
                        n.update(status="confirmed", source="model", updated=time.time())
                        d["relations"].append(n)
            except Exception:  # noqa: BLE001
                pass
            return d

    def save(self, d):
        tmp = self.path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(d, f, ensure_ascii=False, indent=1, default=str)
        replace_file(tmp, self.path)

    def upsert(self, r):
        r = normalize(r)
        r["updated"] = time.time()
        with self.lock:
            d = self.load()
            d["relations"] = [x for x in d["relations"] if x["id"] != r["id"]] + [r]
            self.save(d)
        return r

    def set_status(self, rid, status):
        with self.lock:
            d = self.load()
            hit = [x for x in d["relations"] if x["id"] == rid]
            if not hit:   # promote an inferred one
                hit = [dict(x) for x in (d.get("inferred") or {}).get("items", []) if x["id"] == rid]
                d["relations"] += hit
            for x in hit:
                x["status"], x["updated"] = status, time.time()
                x["source"] = x.get("source") if x.get("source") in ("user", "model") else "user"
            self.save(d)
            return hit[0] if hit else None

    def delete(self, rid):
        with self.lock:
            d = self.load()
            d["relations"] = [x for x in d["relations"] if x["id"] != rid]
            self.save(d)

    def effective(self, tables=None):
        """Confirmed + user/model relations + cached inferred ones that were not rejected / overridden."""
        d = self.load()
        stored = {x["id"]: x for x in d["relations"]}
        out = [x for x in d["relations"] if x.get("status") != "rejected"]
        for x in (d.get("inferred") or {}).get("items", []):
            if x["id"] not in stored:
                out.append(x)
        if tables is not None:
            ts = {t.lower() for t in tables}
            out = [x for x in out if x["from"]["table"].lower() in ts and x["to"]["table"].lower() in ts]
        order = {"confirmed": 0, "inferred": 1, "suspect": 2}
        return sorted(out, key=lambda x: (order.get(x.get("status"), 3), -(x.get("match_rate") or 0)))

    def cache_inferred(self, stamp, items):
        with self.lock:
            d = self.load()
            d["inferred"] = {"stamp": stamp, "items": items, "at": time.time()}
            self.save(d)

    def cached_stamp(self):
        return (self.load().get("inferred") or {}).get("stamp")


def tables_stamp(table_list):
    s = "|".join(f"{t['table']}:{t['rows']}:{','.join(c['name'] + c['type'] for c in t['columns'])}" for t in table_list)
    return hashlib.sha1(s.encode()).hexdigest()[:16]


# ---------- automatic inference (cached; never authoritative) ----------
def infer(con, profiles, limit_tables=60):
    """Inferred + suspect relations in the store format. Partial matches are probed with key transforms."""
    from . import profile as P
    if len(profiles) > limit_tables:
        profiles = profiles[:limit_tables]
    weak = []
    out = []
    for r in P.infer_relationships(con, profiles, weak=weak):
        n = normalize(r)
        n.update(status="inferred", source="auto", match_rate=r["inclusion"], confidence=r["confidence"])
        out.append(n)
    seen = {(x["from"]["table"], x["from"]["columns"][0]) for x in out}
    # also: same-named key-like columns whose values do not match raw (e.g. 'C-0012' vs 12) — no uniqueness needed
    for w in weak[:40]:
        if (w["from_table"], w["from_column"]) in seen:
            continue
        frm = {"table": w["from_table"], "columns": [w["from_column"]]}
        to = {"table": w["to_table"], "columns": [w["to_column"]]}
        try:
            best = probe(con, frm, to, base_rate=w["inclusion"])
        except Exception:  # noqa: BLE001
            best = None
        n = normalize({"from": frm, "to": to})
        if best and best["match_rate"] >= 0.95:
            n["from"]["steps"] = list(best["steps"]); n["to"]["steps"] = list(best["steps"])
            n.update(status="inferred", source="auto", match_rate=best["match_rate"], note="自动探测到需要转换：" + best["label"])
        elif w["inclusion"] >= 0.3 or (best and best["match_rate"] >= 0.3):
            rate = best["match_rate"] if best else w["inclusion"]
            if best:
                n["suggested_steps"] = best["steps"]
            n.update(status="suspect", source="auto", match_rate=rate,
                     note="字段名相近但值只部分匹配，需要确认对应规则" + (f"（建议：{best['label']}）" if best else ""))
        else:
            continue
        try:
            t = test(con, n["from"], n["to"], samples=4)
            n.update(unmatched_samples=t["unmatched_samples"], from_samples=t["from_samples"], to_samples=t["to_samples"],
                     cardinality=t["cardinality"])
        except Exception:  # noqa: BLE001
            pass
        seen.add((w["from_table"], w["from_column"]))
        out.append(n)
    return out


def render(rels):
    """Text block for M-Schema: status, transform and a ready-to-use JOIN snippet."""
    if not rels:
        return ""
    lines = ["【Relations / 表间关系】（confirmed=已确认，inferred=自动推断，suspect=疑似，需向用户确认）"]
    for r in rels:
        f = flat(r)
        rate = r.get("match_rate")
        lines.append(f"- [{r.get('status', 'inferred')}] {f['from_table']}.{f['from_column']} → {f['to_table']}.{f['to_column']}"
                     f" ({r.get('cardinality', 'N:1')}" + (f", match {rate:.0%}" if isinstance(rate, (int, float)) else "") + ")"
                     + (f" 转换: {f['transform']}" if f["transform"] else "") + (f" 备注: {r['note']}" if r.get("note") else ""))
        if r.get("status") != "suspect":
            try:
                lines.append("    " + join_snippet(r["from"], r["to"], r.get("mapping")))
            except Exception:  # noqa: BLE001
                pass
    return "\n".join(lines)
