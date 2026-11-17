"""Column profiling → Schema Card (M-Schema style) and relationship inference."""
import hashlib
import re

from .util import clean_value, qident

LOW_CARD = 50


def list_tables(con):
    rows = con.execute(
        "SELECT table_name, table_type FROM information_schema.tables WHERE table_schema='main' ORDER BY table_name").fetchall()
    out = []
    for name, ttype in rows:
        if name.startswith("_dp_"):
            continue
        n = con.execute(f"SELECT count(*) FROM {qident(name)}").fetchall()[0][0]
        cols = con.execute(
            "SELECT column_name, data_type FROM information_schema.columns WHERE table_schema='main' AND table_name=? ORDER BY ordinal_position",
            [name]).fetchall()
        out.append({"table": name, "type": "view" if "VIEW" in ttype else "table", "rows": int(n),
                    "columns": [{"name": c, "type": t} for c, t in cols]})
    return out


def profile_table(con, table, max_examples=5):
    t = qident(table)
    total = con.execute(f"SELECT count(*) FROM {t}").fetchall()[0][0]
    cols = con.execute(
        "SELECT column_name, data_type FROM information_schema.columns WHERE table_schema='main' AND table_name=? ORDER BY ordinal_position",
        [table]).fetchall()
    out = []
    for name, dtype in cols:
        c = qident(name)
        nulls, distinct = con.execute(f"SELECT count(*) - count({c}), approx_count_distinct({c}) FROM {t}").fetchall()[0]
        info = {"name": name, "type": dtype, "null_pct": round(100.0 * nulls / total, 2) if total else 0.0,
                "distinct": int(distinct)}
        is_num = any(k in dtype.upper() for k in ("INT", "DOUBLE", "FLOAT", "DECIMAL", "REAL", "NUMERIC", "HUGEINT"))
        is_time = any(k in dtype.upper() for k in ("DATE", "TIME"))
        if is_num or is_time:
            mn, mx = con.execute(f"SELECT min({c}), max({c}) FROM {t}").fetchall()[0]
            info["min"], info["max"] = clean_value(mn), clean_value(mx)
            if is_num:
                avg = con.execute(f"SELECT avg({c}) FROM {t}").fetchall()[0][0]
                info["avg"] = round(float(avg), 4) if avg is not None else None
        if distinct <= LOW_CARD and not is_num:
            vals = con.execute(
                f"SELECT {c}, count(*) n FROM {t} WHERE {c} IS NOT NULL GROUP BY 1 ORDER BY n DESC LIMIT {LOW_CARD}").fetchall()
            info["values"] = [clean_value(v) for v, _ in vals]
            info["low_cardinality"] = True
        else:
            ex = con.execute(
                f"SELECT DISTINCT {c} FROM (SELECT {c} FROM {t} WHERE {c} IS NOT NULL LIMIT 2000) LIMIT {max_examples}").fetchall()
            info["examples"] = [clean_value(v[0]) for v in ex]
        info["unique"] = total > 0 and distinct >= total * 0.99 and nulls == 0
        out.append(info)
    return {"table": table, "rows": int(total), "columns": out}


def _fmt(v, n=40):
    s = str(v)
    return s if len(s) <= n else s[:n] + "…"


def m_schema(profiles, relations=None, descriptions=None):
    """Render XiYan-style M-Schema text for prompts."""
    descriptions = descriptions or {}
    lines = ["【DB_ID】 workspace (DuckDB)", "【Schema】"]
    for p in profiles:
        tdesc = descriptions.get(p["table"], {}).get("_table", "")
        lines.append(f"# Table: {p['table']}" + (f", {tdesc}" if tdesc else "") + f"  ({p['rows']} rows)")
        lines.append("[")
        for c in p["columns"]:
            parts = [f"({c['name']}:{c['type']}"]
            d = descriptions.get(p["table"], {}).get(c["name"])
            if d:
                parts.append(d)
            if c.get("unique"):
                parts.append("Unique")
            if c.get("null_pct"):
                parts.append(f"Null {c['null_pct']}%")
            if "min" in c and c.get("min") is not None:
                parts.append(f"Range [{_fmt(c['min'])} ~ {_fmt(c['max'])}]")
            if c.get("values") is not None:
                vals = c["values"][:20]
                more = f" …(+{len(c['values']) - 20})" if len(c["values"]) > 20 else ""
                parts.append("Values: [" + ", ".join(_fmt(v, 30) for v in vals) + "]" + more)
            elif c.get("examples"):
                parts.append("Examples: [" + ", ".join(_fmt(v, 30) for v in c["examples"][:3]) + "]")
            lines.append(", ".join(parts) + "),")
        lines.append("]")
    if relations:
        lines.append("【Foreign keys】")
        for r in relations:
            lines.append(f"{r['from_table']}.{r['from_column']} = {r['to_table']}.{r['to_column']}"
                         f"  ({r.get('cardinality', 'N:1')}, confidence {r.get('confidence', 0):.2f})")
    return "\n".join(lines)


_id_re = re.compile(r"(_id|id|编号|代码|编码|号|_code|code|_no|no)$", re.I)


def _norm(name):
    return re.sub(r"[_\s]", "", name.lower())


def _compatible(t1, t2):
    def k(t):
        t = t.upper()
        if any(x in t for x in ("INT", "DECIMAL", "NUMERIC", "HUGEINT")):
            return "int"
        if "VARCHAR" in t or "STRING" in t or "TEXT" in t:
            return "str"
        if "DATE" in t or "TIME" in t:
            return "time"
        if "DOUBLE" in t or "FLOAT" in t:
            return "float"
        return t
    a, b = k(t1), k(t2)
    return a == b or {a, b} <= {"int", "str", "float"}


def infer_relationships(con, profiles, min_inclusion=0.95, sample=200_000, weak=None, max_pairs=400):
    """Candidate FK pairs by name similarity + type compatibility, verified by value inclusion and parent uniqueness.
    `weak` (list): candidates whose values only partly match are appended there (for transform probing)."""
    cands = []
    by_table = {p["table"]: p for p in profiles}
    for parent in profiles:
        pcols = [c for c in parent["columns"] if c.get("unique") and c["distinct"] > 1]
        for pc in pcols:
            for child in profiles:
                if child["table"] == parent["table"]:
                    continue
                for cc in child["columns"]:
                    if not _compatible(cc["type"], pc["type"]):
                        continue
                    n_c, n_p = _norm(cc["name"]), _norm(pc["name"])
                    ptab = _norm(parent["table"]).rstrip("s")
                    name_score = 0.0
                    if n_c == n_p and _id_re.search(cc["name"]):
                        name_score = 1.0
                    elif n_p in ("id", "编号") and (n_c == ptab + "id" or n_c == ptab + n_p or n_c.endswith(ptab + "id")):
                        name_score = 0.95
                    elif n_c == n_p:
                        name_score = 0.6
                    elif _id_re.search(cc["name"]) and (n_c.endswith(n_p) or n_p.endswith(n_c)) and min(len(n_c), len(n_p)) >= 3:
                        name_score = 0.5
                    if name_score == 0:
                        continue
                    cands.append((child["table"], cc, parent["table"], pc, name_score))
    rels = []
    cands.sort(key=lambda c: -c[4])
    for ct, cc, pt, pc, ns in cands[:max_pairs]:
        c_col, p_col = qident(cc["name"]), qident(pc["name"])
        q = (f"SELECT count(*) AS n, count(p.k) AS hit FROM (SELECT {c_col} AS v FROM {qident(ct)} WHERE {c_col} IS NOT NULL LIMIT {sample}) c "
             f"LEFT JOIN (SELECT DISTINCT CAST({p_col} AS VARCHAR) AS k FROM {qident(pt)}) p ON CAST(c.v AS VARCHAR) = p.k")
        n, hit = con.execute(q).fetchall()[0]
        if not n:
            continue
        inc = hit / n
        if inc < min_inclusion and not (ns >= 0.95 and inc >= 0.8):
            if weak is not None and ns >= 0.6:
                weak.append({"from_table": ct, "from_column": cc["name"], "to_table": pt, "to_column": pc["name"],
                             "inclusion": round(inc, 4), "name_score": ns})
            continue
        child_unique = cc.get("unique", False)
        conf = round(0.6 * inc + 0.4 * ns, 3)
        rels.append({"from_table": ct, "from_column": cc["name"], "to_table": pt, "to_column": pc["name"],
                     "inclusion": round(inc, 4), "name_score": ns, "confidence": conf,
                     "cardinality": "1:1" if child_unique else "N:1", "status": "inferred"})
    # keep best per (child table, child column)
    best = {}
    for r in sorted(rels, key=lambda r: -r["confidence"]):
        best.setdefault((r["from_table"], r["from_column"]), r)
    return sorted(best.values(), key=lambda r: -r["confidence"])


def schema_fingerprint(profile):
    cols = sorted(f"{c['name'].lower()}:{c['type'].split('(')[0].upper()}" for c in profile["columns"])
    return hashlib.sha1("|".join(cols).encode()).hexdigest()[:16], [c.split(":")[0] for c in cols]
