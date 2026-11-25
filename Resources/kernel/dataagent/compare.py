"""数据对比 / Data compare — helpers behind the `data-compare` skill, called from the `python` tool:

    from dataagent.compare import compare, compare_many, reconcile, suggest_keys, schema_diff
    r = compare(con, "orders_a", "orders_b", keys=["订单号"], tol=0.01)
    print(r["text"])

* compare       two tables (or SELECT queries) row by row on a key: only-left / only-right rows, which column changed
                from what to what, duplicate / empty keys, columns present on one side only.
* compare_many  several tables: each against a base table (default the first) or in sequence (chain=True), plus
                a column-structure matrix.
* reconcile     totals by dimension (count + sums) — for big database tables, compare aggregates first.
* suggest_keys  candidate key columns (unique on both sides and matching), single columns or pairs.
* schema_diff   which columns / types each table has.

Everything runs in DuckDB SQL (large tables never go through pandas, except the capped Excel report). Before
comparing, keys and values are normalised: text is trimmed (empty = NULL); integer-valued floats lose ".0" in keys
(Excel IDs); numbers stored as text ("1,234.50", "¥12") compare as numbers when the other side is numeric; dates /
timestamps compare as timestamps; numeric differences within `tol` count as equal. Results are tables named
`cmp_<name>_*` (browse with sql_query, export with data_export) and an Excel report in OUTPUT_DIR.
"""
import itertools
import os
import re
import time

from .util import qident

_INT = {"TINYINT", "SMALLINT", "INTEGER", "BIGINT", "HUGEINT", "UTINYINT", "USMALLINT", "UINTEGER", "UBIGINT", "UHUGEINT"}
_FLOAT = {"FLOAT", "DOUBLE", "REAL", "DECIMAL", "NUMERIC"}
_TEXT = {"VARCHAR", "TEXT", "STRING", "CHAR", "BPCHAR"}
_HINT = re.compile(r"(id|no|code|key|编号|编码|代码|单号|号码|账号|工号|学号|序号|号)$|^(id|编号|编码)", re.I)
_WS = r"[\s\x{3000}\x{00A0}]"
XLSX_MAX = 1_048_575


def _base(t):
    return str(t).upper().split("(")[0].strip()


def _is_int(t):
    return _base(t) in _INT


def _is_float(t):
    return _base(t) in _FLOAT


def _is_num(t):
    return _is_int(t) or _is_float(t)


def _is_time(t):
    b = _base(t)
    return b in ("DATE", "TIME") or b.startswith("TIMESTAMP")


def _is_text(t):
    return _base(t) in _TEXT


def _lit(s):
    return "'" + str(s).replace("'", "''") + "'"


def _is_query(t):
    return bool(re.match(r"(?is)^\s*(select|with|from)\b", str(t)))


def _src(t):
    return f"({t}) _q" if _is_query(t) else qident(str(t).strip())


def _label(t):
    return "查询" if _is_query(t) else str(t).strip()


def _safe(s):
    return re.sub(r"[^0-9A-Za-z_\u4e00-\u9fff]+", "_", str(s)).strip("_")[:48] or "x"


def _nn(name):
    """Column name for matching: case, spaces, underscores and brackets ignored."""
    return re.sub(r"[\s_\-./()（）·:：]+", "", str(name)).lower()


def _cols(con, t):
    return [(r[0], str(r[1])) for r in con.execute(f"DESCRIBE SELECT * FROM {_src(t)}").fetchall()]


def _trim(ref):
    return f"regexp_replace(CAST({ref} AS VARCHAR), '^{_WS}+|{_WS}+$', '', 'g')"


def _num_txt(ref):
    return f"TRY_CAST(NULLIF(regexp_replace(CAST({ref} AS VARCHAR), '[,，¥￥$]|{_WS}', '', 'g'), '') AS DOUBLE)"


def _align(lc, rc, columns=None):
    """Pairs (left, right, ltype, rtype) by exact name, then by normalised name; `columns` = {left: right} overrides."""
    lt, rt = dict(lc), dict(rc)
    m = dict(columns or {})
    for a, b in m.items():
        if a not in lt or b not in rt:
            raise ValueError(f"列映射 {a} → {b}：列不存在。左表列：{list(lt)}；右表列：{list(rt)}")
    pairs, used = [], set()
    targets = set(m.values())
    for a, _ in lc:
        b = m.get(a)
        if b is None:
            if a in rt and a not in targets and a not in used:
                b = a
            else:
                b = next((x for x, _ in rc if x not in used and x not in targets and _nn(x) == _nn(a)), None)
        if b is not None and b not in used:
            pairs.append((a, b, lt[a], rt[b]))
            used.add(b)
    paired = {p[0] for p in pairs}
    return pairs, [a for a, _ in lc if a not in paired], [b for b, _ in rc if b not in used]


def _resolve(spec, pairs, lc, rc, what="对比键"):
    """keys / by / measures → pairs. Accepts [name], ["左列=右列"], {左列: 右列}."""
    if not spec:
        return []
    lt, rt = dict(lc), dict(rc)
    items = list(spec.items()) if isinstance(spec, dict) else [tuple(s.split("=", 1)) if isinstance(s, str) and "=" in s else (s, None) for s in spec]
    out = []
    for a, b in items:
        a = str(a).strip()
        if b is not None:
            b = str(b).strip()
            if a not in lt or b not in rt:
                raise ValueError(f"{what} {a} = {b}：列不存在。左表列：{list(lt)}；右表列：{list(rt)}")
            out.append((a, b, lt[a], rt[b]))
            continue
        p = next((p for p in pairs if p[0] == a), None) or next((p for p in pairs if p[1] == a), None) \
            or next((p for p in pairs if _nn(p[0]) == _nn(a)), None)
        if p is None:
            raise ValueError(f"{what}「{a}」在两边没有对应的列（列名不同请写成 \"左列=右列\"）。左表列：{list(lt)}；右表列：{list(rt)}")
        out.append(p)
    return out


def _kexpr(ref, t, other, steps=()):
    """Key value as normalised text, so 1 / 1.0 / ' 1' / '1.0' meet."""
    if _is_time(t) or _is_time(other):
        tgt = "DATE" if "DATE" in (_base(t), _base(other)) else "TIMESTAMP"
        e = f"CAST(TRY_CAST({ref} AS {tgt}) AS VARCHAR)" if not _is_text(t) else f"CAST(TRY_CAST(NULLIF({_trim(ref)}, '') AS {tgt}) AS VARCHAR)"
    elif _is_int(t):
        e = f"CAST({ref} AS VARCHAR)"
    elif _is_float(t):
        e = (f"CASE WHEN {ref} = trunc({ref}) AND abs({ref}) < 1e15 THEN CAST(CAST({ref} AS HUGEINT) AS VARCHAR) "
             f"ELSE CAST(CAST({ref} AS DOUBLE) AS VARCHAR) END")
    else:
        e = f"regexp_replace({_trim(ref)}, '^(-?[0-9]+)\\.0+$', '\\1')"
    for s in steps or ():
        if s in ("upper", "ignore_case"):
            e = f"upper({e})"
        elif s == "strip_zeros":
            e = f"regexp_replace({e}, '^0+([0-9])', '\\1')"
        elif s == "no_space":
            e = f"regexp_replace({e}, '{_WS}+', '', 'g')"
    return f"NULLIF({e}, '')"


def _cast_stats(con, src, cols):
    """Text columns → (non-empty, castable to number, castable to timestamp)."""
    if not cols:
        return {}
    parts = []
    for c in cols:
        t = _trim(qident(c))
        parts += [f"count(NULLIF({t}, ''))", f"count({_num_txt(qident(c))})", f"count(TRY_CAST(NULLIF({t}, '') AS TIMESTAMP))"]
    row = con.execute(f"SELECT {', '.join(parts)} FROM {src}").fetchone()
    return {c: tuple(int(x or 0) for x in row[3 * i:3 * i + 3]) for i, c in enumerate(cols)}


def _kind(lt, rt, sl, sr):
    def num_ok(t, s):
        return _is_num(t) or (s is not None and (s[0] == 0 or s[1] >= 0.98 * s[0]))

    def time_ok(t, s):
        return _is_time(t) or (s is not None and (s[0] == 0 or s[2] >= 0.98 * s[0]))

    if (_is_num(lt) or _is_num(rt)) and num_ok(lt, sl) and num_ok(rt, sr):
        return "num"
    if (_is_time(lt) or _is_time(rt)) and time_ok(lt, sl) and time_ok(rt, sr):
        return "time"
    return "text"


def _vexpr(ref, t, kind, ignore_case=False):
    if kind == "num":
        return f"CAST({ref} AS DOUBLE)" if _is_num(t) else _num_txt(ref)
    if kind == "time":
        return f"TRY_CAST({ref} AS TIMESTAMP)" if not _is_text(t) else f"TRY_CAST(NULLIF({_trim(ref)}, '') AS TIMESTAMP)"
    e = f"NULLIF({_trim(ref)}, '')"
    return f"upper({e})" if ignore_case else e


def _differs(a, b, kind, tol):
    cmp = f"abs(({a}) - ({b})) > {float(tol)!r}" if kind == "num" else f"({a}) <> ({b})"
    return f"(CASE WHEN ({a}) IS NULL AND ({b}) IS NULL THEN false WHEN ({a}) IS NULL OR ({b}) IS NULL THEN true ELSE {cmp} END)"


def _tol(tol, col):
    if isinstance(tol, dict):
        return tol.get(col, tol.get("*", 1e-6))
    return tol if tol is not None else 1e-6


def _df_table(con, name, df):
    con.register("_cmp_frame", df)
    try:
        con.execute(f"CREATE OR REPLACE TABLE {qident(name)} AS SELECT * FROM _cmp_frame")
    finally:
        con.unregister("_cmp_frame")


def _count(con, sql):
    return int(con.execute(sql).fetchone()[0] or 0)


def _out_path(out_dir, title):
    d = out_dir or os.environ.get("DP_OUTPUT_DIR") or os.path.join(os.environ.get("DP_WORKSPACE") or os.getcwd(), "exports")
    os.makedirs(d, exist_ok=True)
    base = re.sub(r'[\\/:*?"<>|\x00-\x1f]', "_", title)[:100]
    p = os.path.join(d, f"{base}_{time.strftime('%Y%m%d_%H%M%S')}.xlsx")
    i = 2
    while os.path.exists(p):
        p = os.path.join(d, f"{base}_{time.strftime('%Y%m%d_%H%M%S')}_{i}.xlsx")
        i += 1
    return p


def _write_report(con, path, sheets):
    """sheets: [(sheet name, table name / DataFrame)]. Rows beyond Excel's limit are cut (noted in the sheet list)."""
    import pandas as pd
    cut = []
    with pd.ExcelWriter(path, engine="xlsxwriter", engine_kwargs={"options": {"strings_to_formulas": False, "strings_to_urls": False}}) as w:
        for sheet, src in sheets:
            sheet = re.sub(r"[\[\]:*?/\\]", "_", sheet)[:31]
            if isinstance(src, pd.DataFrame):
                df = src
            else:
                n = _count(con, f"SELECT count(*) FROM {qident(src)}")
                df = con.execute(f"SELECT * FROM {qident(src)} LIMIT {XLSX_MAX}").df()
                if n > XLSX_MAX:
                    cut.append(f"{sheet}：共 {n} 行，报告只放前 {XLSX_MAX} 行，完整结果见表 {src}")
            df = df.copy()
            for c in df.columns:
                s = df[c]
                if getattr(s.dtype, "tz", None) is not None:
                    df[c] = s.dt.tz_localize(None)
                elif s.dtype == object:
                    df[c] = s.map(lambda v: str(v) if isinstance(v, (list, dict, tuple, set, bytes)) else v)
            df.to_excel(w, sheet_name=sheet, index=False)
            ws = w.sheets[sheet]
            ws.freeze_panes(1, 0)
            for j, c in enumerate(df.columns):
                ws.set_column(j, j, min(40, max(8, len(str(c)) * 2 + 2)))
    return cut


def _md(df, n=20):
    if df is None or not len(df):
        return ""
    df = df.head(n)
    cols = [str(c) for c in df.columns]
    esc = lambda v: ("" if v is None or (isinstance(v, float) and v != v) else str(v)).replace("|", "\\|").replace("\n", " ")[:60]
    lines = ["| " + " | ".join(cols) + " |", "|" + "---|" * len(cols)]
    lines += ["| " + " | ".join(esc(v) for v in row) + " |" for row in df.itertuples(index=False)]
    return "\n".join(lines)


# ---------------------------------------------------------------- keys

def suggest_keys(con, left, right, columns=None, key_steps=None, top=5):
    """Candidate keys: unique on both sides (≥ 95 %) and how many distinct keys meet. Best first."""
    lc, rc = _cols(con, left), _cols(con, right)
    pairs, _, _ = _align(lc, rc, columns)
    ls, rs = _src(left), _src(right)
    cand = [p for p in pairs if not ((_is_float(p[2]) or _is_float(p[3]) or _is_time(p[2]) or _is_time(p[3])) and not _HINT.search(str(p[0])))]
    if not cand:
        return []
    kl = {p: _kexpr(qident(p[0]), p[2], p[3], key_steps) for p in cand}
    kr = {p: _kexpr(qident(p[1]), p[3], p[2], key_steps) for p in cand}

    def uniq(src, exprs):
        row = con.execute(f"SELECT count(*), {', '.join(f'count(DISTINCT {e})' for e in exprs)} FROM {src}").fetchone()
        n = max(1, int(row[0] or 0))
        return [int(x or 0) / n for x in row[1:]], [int(x or 0) for x in row[1:]]

    def match(el, er, dl, dr):
        hit = _count(con, f"SELECT count(*) FROM (SELECT DISTINCT {el} AS k FROM {ls}) a JOIN (SELECT DISTINCT {er} AS k FROM {rs}) b ON a.k = b.k")
        return hit / max(1, min(dl, dr))

    ul, dl = uniq(ls, [kl[p] for p in cand])
    ur, dr = uniq(rs, [kr[p] for p in cand])
    out = []
    for i, p in enumerate(cand):
        if ul[i] >= 0.95 and ur[i] >= 0.95:
            out.append({"keys": [p[0]], "right_keys": [p[1]], "unique_left": ul[i], "unique_right": ur[i],
                        "match_rate": match(kl[p], kr[p], dl[i], dr[i]), "hint": bool(_HINT.search(str(p[0])))})
    if not any(min(c["unique_left"], c["unique_right"]) >= 0.999 for c in out):
        # no single unique column: try pairs among the most distinct columns
        order = sorted(range(len(cand)), key=lambda i: -min(ul[i], ur[i]))[:8]
        combos = list(itertools.combinations(order, 2))
        if combos:
            cat = lambda ex: "concat_ws(chr(31), " + ", ".join(f"coalesce({e}, '∅')" for e in ex) + ")"
            el = [cat([kl[cand[i]], kl[cand[j]]]) for i, j in combos]
            er = [cat([kr[cand[i]], kr[cand[j]]]) for i, j in combos]
            cul, cdl = uniq(ls, el)
            cur, cdr = uniq(rs, er)
            for n, (i, j) in enumerate(combos):
                if cul[n] >= 0.99 and cur[n] >= 0.99:
                    out.append({"keys": [cand[i][0], cand[j][0]], "right_keys": [cand[i][1], cand[j][1]],
                                "unique_left": cul[n], "unique_right": cur[n], "match_rate": match(el[n], er[n], cdl[n], cdr[n]),
                                "hint": bool(_HINT.search(str(cand[i][0])) or _HINT.search(str(cand[j][0])))})
    out.sort(key=lambda c: (min(c["unique_left"], c["unique_right"]) >= 0.999, round(c["match_rate"], 2), c["hint"],
                            -len(c["keys"]), min(c["unique_left"], c["unique_right"])), reverse=True)
    for c in out:
        for k in ("unique_left", "unique_right", "match_rate"):
            c[k] = round(c[k], 4)
    return out[:top]


def _keys_text(kp):
    return "、".join(a if a == b else f"{a}={b}" for a, b, *_ in kp)


# ---------------------------------------------------------------- compare

def compare(con, left, right, keys=None, columns=None, ignore=None, tol=1e-6, ignore_case=False, key_steps=None,
            name=None, report=True, out_dir=None, sample=20):
    """Compare two tables / queries.

    keys: ["订单号"] | ["订单号=order_id"] | {"订单号": "order_id"}; None = pick automatically (asks via warnings),
          [] = whole-row comparison (only shows rows missing on either side).
    columns: {左列: 右列} for value columns whose names differ.   ignore: columns to skip.
    tol: numeric tolerance (number, or {列: tol, "*": default}).   ignore_case: text / keys case-insensitive.
    key_steps: extra key normalisation, e.g. ["strip_zeros"] ("007" = "7"), ["no_space"].
    """
    lc, rc = _cols(con, left), _cols(con, right)
    pairs, lonly, ronly = _align(lc, rc, columns)
    ign = set(ignore or [])
    pairs = [p for p in pairs if p[0] not in ign and p[1] not in ign]
    lonly, ronly = [c for c in lonly if c not in ign], [c for c in ronly if c not in ign]
    steps = list(key_steps or [])
    if ignore_case and "upper" not in steps:
        steps.append("upper")
    warnings = []
    if keys is None:
        cands = suggest_keys(con, left, right, columns, steps)
        best = cands[0] if cands else None
        if best and min(best["unique_left"], best["unique_right"]) >= 0.99 and best["match_rate"] >= 0.3:
            keys = dict(zip(best["keys"], best["right_keys"]))
            warnings.append(f"未指定对比键，自动选择「{'、'.join(best['keys'])}」（左表唯一率 {best['unique_left']:.1%}，右表 {best['unique_right']:.1%}，"
                            f"键匹配率 {best['match_rate']:.1%}），请向用户确认这个键是否正确")
        else:
            keys = []
            warnings.append("没有找到可靠的对比键（两边都唯一且能对上的列），已改为整行对比：只能看出哪些行只在一边出现，看不出具体哪一列变了。"
                            "请让用户指定键（如订单号、客户编码）" + (f"。候选：{[c['keys'] for c in cands]}" if cands else ""))
    kp = _resolve(keys, pairs, lc, rc)
    kl_names = {p[0] for p in kp}
    # keys given as "左列=右列" are not in the name-aligned pairs: they are not one-sided columns
    lonly = [c for c in lonly if c not in kl_names]
    ronly = [c for c in ronly if c not in {q[1] for q in kp}]
    vp = [p for p in pairs if p[0] not in kl_names and p[1] not in {q[1] for q in kp}]
    p = "cmp_" + _safe(name or f"{_label(left)}_vs_{_label(right)}")
    T = {k: f"{p}_{k}" for k in ("summary", "columns", "diff", "left_only", "right_only", "bad_keys")}
    ls, rs = _src(left), _src(right)
    nl, nr = _count(con, f"SELECT count(*) FROM {ls}"), _count(con, f"SELECT count(*) FROM {rs}")
    # value comparison mode per column
    sl = _cast_stats(con, ls, [a for a, b, at, bt in vp if _is_text(at)])
    sr = _cast_stats(con, rs, [b for a, b, at, bt in vp if _is_text(bt)])
    kinds = [_kind(at, bt, sl.get(a), sr.get(b)) for a, b, at, bt in vp]
    for (a, b, at, bt), k in zip(vp, kinds):
        if _is_num(at) != _is_num(bt) and k == "text":
            warnings.append(f"列「{a}」一边是数字、一边是文本且无法全部转成数字，按文本比较")
    res = {"left": _label(left), "right": _label(right), "keys": [q[0] for q in kp], "right_keys": [q[1] for q in kp],
           "rows_left": nl, "rows_right": nr, "left_only_columns": lonly, "right_only_columns": ronly, "tables": T, "warnings": warnings}
    tmp = [f"_{p}_l", f"_{p}_r", f"_{p}_j"]
    try:
        if kp:
            _keyed(con, res, kp, vp, kinds, ls, rs, T, tmp, steps, tol, ignore_case)
        else:
            _rowwise(con, res, vp, kinds, ls, rs, T, ignore_case)
    finally:
        for t in tmp:
            con.execute(f"DROP TABLE IF EXISTS {qident(t)}")
    # columns sheet
    import pandas as pd
    colrows = [[a, b, at, bt, "对比键", None, ""] for a, b, at, bt in kp]
    labels = {"num": "数值" + ("" if tol in (None, 0) else ""), "time": "日期时间", "text": "文本"}
    for (a, b, at, bt), k in zip(vp, kinds):
        colrows.append([a, b, at, bt, labels[k] + (f"（容差 {_tol(tol, a)}）" if k == "num" else "") if kp else labels[k],
                        res.get("diff_by_column", {}).get(a if a == b else f"{a} ↔ {b}") if kp else None, ""])
    colrows += [[a, "", dict(lc)[a], "", "", None, "仅左表有"] for a in lonly]
    colrows += [["", b, "", dict(rc)[b], "", None, "仅右表有"] for b in ronly]
    _df_table(con, T["columns"], pd.DataFrame(colrows, columns=["左列", "右列", "左类型", "右类型", "比较方式", "不同行数", "说明"]))
    srows = [("左表", res["left"]), ("右表", res["right"]), ("对比键", _keys_text(kp) or "（无，整行对比）"),
             ("左表行数", nl), ("右表行数", nr)]
    if kp:
        srows += [("两边都有的键", res["matched_keys"]), ("参与逐值比较的行", res["compared"]), ("值完全一致的行", res["same"]),
                  ("有差异的行", res["diff_rows"]), ("仅在左表的行", res["left_only"]), ("仅在右表的行", res["right_only"]),
                  ("重复键（左/右）", f"{res['dup_keys_left']} / {res['dup_keys_right']}"),
                  ("空键行（左/右）", f"{res['null_keys_left']} / {res['null_keys_right']}")]
    else:
        srows += [("仅在左表的行", res["left_only"]), ("仅在右表的行", res["right_only"])]
    srows += [("仅左表有的列", "、".join(lonly) or "—"), ("仅右表有的列", "、".join(ronly) or "—"), ("提示", "；".join(warnings) or "—")]
    _df_table(con, T["summary"], pd.DataFrame([(a, str(b)) for a, b in srows], columns=["指标", "值"]))
    if report:
        path = _out_path(out_dir, f"数据对比_{_safe(res['left'])}_vs_{_safe(res['right'])}")
        sheets = [("概要", T["summary"]), ("列对照", T["columns"])]
        if kp:
            sheets.append(("值不同", T["diff"]))
        sheets += [("仅在左表", T["left_only"]), ("仅在右表", T["right_only"])]
        if kp:
            sheets.append(("重复或空键", T["bad_keys"]))
        res["report_cut"] = _write_report(con, path, sheets)
        res["report"] = path
    if not kp:
        for k in ("diff", "bad_keys"):
            T.pop(k, None)
    res["samples"] = con.execute(f"SELECT * FROM {qident(T['diff'])} LIMIT {int(sample)}").df() if kp and res["diff_rows"] else None
    res["text"] = _text(res)
    return res


def _keyed(con, res, kp, vp, kinds, ls, rs, T, tmp, steps, tol, ignore_case):
    L, R, J = tmp
    K = [f"__k{i}" for i in range(len(kp))]
    kl = [_kexpr(qident(a), at, bt, steps) for a, b, at, bt in kp]
    kr = [_kexpr(qident(b), bt, at, steps) for a, b, at, bt in kp]
    for t, src, ex in ((L, ls, kl), (R, rs, kr)):
        sel = ", ".join(f"{e} AS {qident(k)}" for e, k in zip(ex, K))
        con.execute(f"CREATE OR REPLACE TEMP TABLE {qident(t)} AS SELECT *, {sel}, count(*) OVER (PARTITION BY {', '.join(ex)}) AS __cmp_n FROM {src}")
    kq = ", ".join(qident(k) for k in K)
    notnull = lambda a: " AND ".join(f"{a}.{qident(k)} IS NOT NULL" for k in K)
    anynull = " OR ".join(f"{qident(k)} IS NULL" for k in K)
    on = " AND ".join(f"l.{qident(k)} = r.{qident(k)}" for k in K)
    excl = f"EXCLUDE ({kq}, __cmp_n)"
    con.execute(f"CREATE OR REPLACE TABLE {qident(T['left_only'])} AS SELECT l.* {excl} FROM {qident(L)} l WHERE {notnull('l')} "
                f"AND NOT EXISTS (SELECT 1 FROM {qident(R)} r WHERE {on})")
    con.execute(f"CREATE OR REPLACE TABLE {qident(T['right_only'])} AS SELECT r.* {excl} FROM {qident(R)} r WHERE {notnull('r')} "
                f"AND NOT EXISTS (SELECT 1 FROM {qident(L)} l WHERE {on})")
    res["left_only"] = _count(con, f"SELECT count(*) FROM {qident(T['left_only'])}")
    res["right_only"] = _count(con, f"SELECT count(*) FROM {qident(T['right_only'])}")
    res["matched_keys"] = _count(con, f"SELECT count(*) FROM (SELECT DISTINCT {kq} FROM {qident(L)} l WHERE {notnull('l')}) l "
                                      f"JOIN (SELECT DISTINCT {kq} FROM {qident(R)} r WHERE {notnull('r')}) r ON {on}")
    for side, t in (("left", L), ("right", R)):
        res[f"dup_keys_{side}"] = _count(con, f"SELECT count(*) FROM (SELECT DISTINCT {kq} FROM {qident(t)} x WHERE {notnull('x')} AND __cmp_n > 1)")
        res[f"null_keys_{side}"] = _count(con, f"SELECT count(*) FROM {qident(t)} WHERE {anynull}")
    knames = [qident(a) for a, *_ in kp]
    bad = []
    for lab, t in (("左表", L), ("右表", R)):
        bad.append(f"SELECT {_lit(lab)} AS 来源, '重复键' AS 原因, {', '.join(f'{qident(k)} AS {n}' for k, n in zip(K, knames))}, count(*) AS 行数 "
                   f"FROM {qident(t)} x WHERE {notnull('x')} GROUP BY {kq} HAVING count(*) > 1")
        bad.append(f"SELECT {_lit(lab)}, '键为空', {', '.join('CAST(NULL AS VARCHAR)' for _ in K)}, count(*) FROM {qident(t)} WHERE {anynull} HAVING count(*) > 0")
    con.execute(f"CREATE OR REPLACE TABLE {qident(T['bad_keys'])} AS {' UNION ALL '.join(bad)}")
    # rows whose key is unique on both sides: value by value
    vsel = "".join(f", l.{qident(a)} AS {qident(f'__l{i}')}, r.{qident(b)} AS {qident(f'__r{i}')}" for i, (a, b, at, bt) in enumerate(vp))
    con.execute(f"CREATE OR REPLACE TEMP TABLE {qident(J)} AS SELECT {', '.join(f'l.{qident(k)}' for k in K)}{vsel} "
                f"FROM {qident(L)} l JOIN {qident(R)} r ON {on} WHERE l.__cmp_n = 1 AND r.__cmp_n = 1")
    conds, parts, labels = [], [], []
    for i, ((a, b, at, bt), kind) in enumerate(zip(vp, kinds)):
        le = _vexpr(qident(f"__l{i}"), at, kind, ignore_case)
        re_ = _vexpr(qident(f"__r{i}"), bt, kind, ignore_case)
        d = _differs(le, re_, kind, _tol(tol, a))
        conds.append(d)
        lab = a if a == b else f"{a} ↔ {b}"
        labels.append(lab)
        delta = f"({re_}) - ({le})" if kind == "num" else "CAST(NULL AS DOUBLE)"
        parts.append(f"SELECT {', '.join(f'{qident(k)} AS {n}' for k, n in zip(K, knames))}, {_lit(lab)} AS 列, "
                     f"CAST({qident(f'__l{i}')} AS VARCHAR) AS 左值, CAST({qident(f'__r{i}')} AS VARCHAR) AS 右值, {delta} AS 差额 FROM {qident(J)} WHERE {d}")
    if not parts:
        parts = [f"SELECT {', '.join(f'{qident(k)} AS {n}' for k, n in zip(K, knames))}, CAST(NULL AS VARCHAR) AS 列, CAST(NULL AS VARCHAR) AS 左值, "
                 f"CAST(NULL AS VARCHAR) AS 右值, CAST(NULL AS DOUBLE) AS 差额 FROM {qident(J)} WHERE false"]
    con.execute(f"CREATE OR REPLACE TABLE {qident(T['diff'])} AS {' UNION ALL '.join(parts)}")
    agg = ", ".join([f"count_if({' OR '.join(conds)})" if conds else "0"] + [f"count_if({c})" for c in conds])
    row = con.execute(f"SELECT count(*), {agg} FROM {qident(J)}").fetchone()
    res["compared"], res["diff_rows"] = int(row[0]), int(row[1] or 0)
    res["same"] = res["compared"] - res["diff_rows"]
    res["dup_matched_keys"] = res["matched_keys"] - res["compared"]
    res["diff_by_column"] = {lab: int(n or 0) for lab, n in zip(labels, row[2:])}
    if res["dup_matched_keys"]:
        res["warnings"].append(f"{res['dup_matched_keys']} 个键在一边或两边重复出现，没有参与逐值比较（见表 {T['bad_keys']}）；如果重复是正常的（如明细行），换更细的键或先用 reconcile 按键汇总再比")
    if res["matched_keys"] == 0 and (res["rows_left"] and res["rows_right"]):
        res["warnings"].append("两边的键一个都对不上：检查键是否选错，或格式不同（前导零、前缀、大小写），可试 key_steps=['strip_zeros'] / ignore_case=True")


def _rowwise(con, res, vp, kinds, ls, rs, T, ignore_case):
    if not vp:
        raise ValueError("两边没有同名（或可对应）的列，无法整行对比；请用 columns={左列: 右列} 指定对应关系")
    la = ", ".join(f"{_vexpr(qident(a), at, k, ignore_case)} AS {qident(a)}" for (a, b, at, bt), k in zip(vp, kinds))
    ra = ", ".join(f"{_vexpr(qident(b), bt, k, ignore_case)} AS {qident(a)}" for (a, b, at, bt), k in zip(vp, kinds))
    con.execute(f"CREATE OR REPLACE TABLE {qident(T['left_only'])} AS SELECT {la} FROM {ls} EXCEPT ALL SELECT {ra} FROM {rs}")
    con.execute(f"CREATE OR REPLACE TABLE {qident(T['right_only'])} AS SELECT {ra} FROM {rs} EXCEPT ALL SELECT {la} FROM {ls}")
    res["left_only"] = _count(con, f"SELECT count(*) FROM {qident(T['left_only'])}")
    res["right_only"] = _count(con, f"SELECT count(*) FROM {qident(T['right_only'])}")
    res["same"] = res["rows_left"] - res["left_only"]


def _text(r):
    T = r["tables"]
    kp = list(zip(r["keys"], r["right_keys"]))
    out = [f"**数据对比：{r['left']} ↔ {r['right']}**（对比键：{_keys_text([(a, b) for a, b in kp]) or '无，整行对比'}）",
           f"- 左表 {r['rows_left']:,} 行，右表 {r['rows_right']:,} 行"]
    if r["keys"]:
        cols = [f"{k} {v:,}" for k, v in sorted(r["diff_by_column"].items(), key=lambda x: -x[1]) if v]
        out += [f"- 两边都有的键 {r['matched_keys']:,} 个；逐值比较 {r['compared']:,} 行：完全一致 {r['same']:,} 行，有差异 {r['diff_rows']:,} 行"
                + (f"（按列：{'，'.join(cols)}）" if cols else ""),
                f"- 仅在左表 {r['left_only']:,} 行；仅在右表 {r['right_only']:,} 行",
                f"- 重复键 左 {r['dup_keys_left']} / 右 {r['dup_keys_right']}；空键行 左 {r['null_keys_left']} / 右 {r['null_keys_right']}"]
    else:
        out += [f"- 两边完全相同的行 {r['same']:,}；仅在左表 {r['left_only']:,} 行；仅在右表 {r['right_only']:,} 行（整行对比，看不出具体哪一列变了）"]
    if r["left_only_columns"] or r["right_only_columns"]:
        out.append(f"- 列：仅左表有 {r['left_only_columns'] or '—'}；仅右表有 {r['right_only_columns'] or '—'}")
    out.append("- 结果表：" + "、".join(f"{v}" for v in T.values()))
    if r.get("report"):
        out.append(f"- Excel 报告：{r['report']}")
    for c in r.get("report_cut") or []:
        out.append(f"- ⚠ {c}")
    for w in r["warnings"]:
        out.append(f"- ⚠ {w}")
    if r.get("samples") is not None:
        out += ["", f"差异样例（前 {len(r['samples'])} 条，完整见 {T['diff']}）：", _md(r["samples"], len(r["samples"]))]
    return "\n".join(out)


# ---------------------------------------------------------------- several tables

def schema_diff(con, tables, name=None):
    """Column-structure matrix: one row per column (matched by normalised name), one column per table (type or —)."""
    import pandas as pd
    cols = {t: _cols(con, t) for t in tables}
    order, label = [], {}
    for t in tables:
        for c, _ in cols[t]:
            k = _nn(c)
            if k not in label:
                label[k] = c
                order.append(k)
    rows = []
    for k in order:
        row = [label[k]]
        for t in tables:
            hit = next(((c, ty) for c, ty in cols[t] if _nn(c) == k), None)
            row.append("—" if not hit else (hit[1] if hit[0] == label[k] else f"{hit[0]}（{hit[1]}）"))
        row.append("全部都有" if all(x != "—" for x in row[1:]) else "部分缺失")
        rows.append(row)
    df = pd.DataFrame(rows, columns=["列"] + [_label(t) for t in tables] + ["说明"])
    tname = "cmp_schema_" + _safe(name or "_".join(_label(t) for t in tables))
    _df_table(con, tname, df)
    miss = df[df["说明"] != "全部都有"]
    text = f"列结构：共 {len(df)} 列，{len(df) - len(miss)} 列各表都有" + (f"；部分表缺少：{list(miss['列'])}" if len(miss) else "") + f"（表 {tname}）"
    return {"table": tname, "df": df, "text": text}


def compare_many(con, tables, keys=None, base=None, chain=False, name=None, report=True, out_dir=None, **kw):
    """Several tables: each vs `base` (default the first), or consecutive pairs with chain=True (e.g. monthly versions)."""
    import pandas as pd
    tables = list(tables)
    if len(tables) < 2:
        raise ValueError("至少需要两个表")
    if chain:
        pairs = list(zip(tables, tables[1:]))
    else:
        b0 = base or tables[0]
        pairs = [(b0, t) for t in tables if t != b0]
    results = [compare(con, a, b, keys=keys, name=f"{_label(a)}_vs_{_label(b)}", report=False, **kw) for a, b in pairs]
    sch = schema_diff(con, tables, name=name)
    rows = []
    for r in results:
        top = sorted((r.get("diff_by_column") or {}).items(), key=lambda x: -x[1])
        rows.append([r["left"], r["right"], "、".join(r["keys"]) or "整行", r["rows_left"], r["rows_right"], r.get("same"),
                     r.get("diff_rows"), r["left_only"], r["right_only"], "、".join(f"{k}({v})" for k, v in top[:3] if v) or "—",
                     "；".join(r["warnings"]) or ""])
    df = pd.DataFrame(rows, columns=["左表", "右表", "对比键", "左行数", "右行数", "一致行", "有差异行", "仅左表", "仅右表", "差异最多的列", "提示"])
    sname = "cmp_many_" + _safe(name or "_".join(_label(t) for t in tables)) + "_summary"
    _df_table(con, sname, df)
    out = {"pairs": results, "summary_table": sname, "schema": sch, "summary": df}
    if report:
        path = _out_path(out_dir, "数据对比_多表_" + _safe(name or "_".join(_label(t) for t in tables))[:40])
        sheets = [("汇总", df), ("列结构", sch["df"])]
        for i, r in enumerate(results, 1):
            T = r["tables"]
            if "diff" in T:
                sheets.append((f"{i}_值不同", T["diff"]))
            sheets += [(f"{i}_仅在{_safe(r['left'])}"[:31], T["left_only"]), (f"{i}_仅在{_safe(r['right'])}"[:31], T["right_only"])]
        out["report_cut"] = _write_report(con, path, sheets)
        out["report"] = path
    lines = [f"**多表对比**（{'依次相邻对比' if chain else '以 ' + _label(pairs[0][0]) + ' 为基准'}，{len(results)} 组）", _md(df.drop(columns=["提示"]), 50), "", sch["text"],
             f"汇总表：{sname}；每组明细见各自的 cmp_<左>_vs_<右>_* 表"]
    if out.get("report"):
        lines.append(f"Excel 报告：{out['report']}")
    for r in results:
        for w in r["warnings"]:
            lines.append(f"- ⚠ {r['left']} ↔ {r['right']}：{w}")
    out["text"] = "\n".join(lines)
    return out


# ---------------------------------------------------------------- aggregates

def reconcile(con, left, right, by=None, measures=None, columns=None, tol=0.01, counts=True, key_steps=None,
              name=None, report=True, out_dir=None, sample=20):
    """Totals by dimension: row count + sum of each measure on both sides, joined on `by` (None/[] = grand total).

    Use for big tables (pull `SELECT dim, count(*), sum(x) ... GROUP BY dim` from the database with db_extract and
    reconcile against the local table), or when keys repeat (detail lines). Measures default to the numeric columns
    both sides share. Pre-aggregated inputs: pass counts=False and list the count column as a measure.
    """
    import pandas as pd
    lc, rc = _cols(con, left), _cols(con, right)
    pairs, _, _ = _align(lc, rc, columns)
    bp = _resolve(by or [], pairs, lc, rc, "分组列")
    if measures is None:
        used = {q[0] for q in bp}
        mp = [q for q in pairs if q[0] not in used and (_is_num(q[2]) or _is_num(q[3]))]
    else:
        mp = _resolve(measures, pairs, lc, rc, "指标列")
    if not mp and not counts:
        raise ValueError("没有可汇总的指标列")
    ls, rs = _src(left), _src(right)
    G = [f"__g{i}" for i in range(len(bp))]

    def agg(src, side):
        sel = [f"{_kexpr(qident(q[0] if side == 'l' else q[1]), q[2] if side == 'l' else q[3], q[3] if side == 'l' else q[2], key_steps)} AS {qident(g)}"
               for q, g in zip(bp, G)]
        sel.append("1 AS __in")
        if counts:
            sel.append("count(*) AS __cnt")
        for i, q in enumerate(mp):
            ref, t = (q[0], q[2]) if side == "l" else (q[1], q[3])
            sel.append(f"sum({_vexpr(qident(ref), t, 'num')}) AS {qident(f'__m{i}')}")
        return f"SELECT {', '.join(sel)} FROM {src}" + (f" GROUP BY {', '.join(str(i + 1) for i in range(len(G)))}" if G else "")

    on = " AND ".join(f"a.{qident(g)} IS NOT DISTINCT FROM b.{qident(g)}" for g in G) or "true"
    outc = [f"coalesce(a.{qident(g)}, b.{qident(g)}) AS {qident(q[0])}" for q, g in zip(bp, G)]
    diffs = []
    if counts:
        outc += ["a.__cnt AS 左_行数", "b.__cnt AS 右_行数", "coalesce(b.__cnt, 0) - coalesce(a.__cnt, 0) AS 行数差"]
        diffs.append("coalesce(a.__cnt, 0) <> coalesce(b.__cnt, 0)")
    for i, q in enumerate(mp):
        m = qident(f"__m{i}")
        lab = q[0]
        outc += [f"a.{m} AS {qident('左_' + lab)}", f"b.{m} AS {qident('右_' + lab)}", f"coalesce(b.{m}, 0) - coalesce(a.{m}, 0) AS {qident(lab + '_差额')}"]
        diffs.append(f"abs(coalesce(b.{m}, 0) - coalesce(a.{m}, 0)) > {float(_tol(tol, lab))!r}")
    status = (f"CASE WHEN a.__in IS NULL THEN '仅右表' WHEN b.__in IS NULL THEN '仅左表' "
              f"WHEN {' OR '.join(diffs) or 'false'} THEN '不一致' ELSE '一致' END")
    p = "cmp_" + _safe(name or f"{_label(left)}_vs_{_label(right)}")
    tname = f"{p}_reconcile"
    con.execute(f"CREATE OR REPLACE TABLE {qident(tname)} AS SELECT * FROM (SELECT {', '.join(outc)}, {status} AS 状态 "
                f"FROM ({agg(ls, 'l')}) a FULL OUTER JOIN ({agg(rs, 'r')}) b ON {on}) "
                f"ORDER BY CASE 状态 WHEN '不一致' THEN 0 WHEN '仅左表' THEN 1 WHEN '仅右表' THEN 2 ELSE 3 END")
    st = dict(con.execute(f"SELECT 状态, count(*) FROM {qident(tname)} GROUP BY 1").fetchall())
    tot = []
    if counts:
        a, b = con.execute(f"SELECT sum(左_行数), sum(右_行数) FROM {qident(tname)}").fetchone()
        tot.append(("行数", a, b))
    for q in mp:
        a, b = con.execute(f"SELECT sum({qident('左_' + q[0])}), sum({qident('右_' + q[0])}) FROM {qident(tname)}").fetchone()
        tot.append((q[0], a, b))
    tdf = pd.DataFrame([(k, a, b, (b or 0) - (a or 0)) for k, a, b in tot], columns=["指标", "左表合计", "右表合计", "差额"])
    res = {"left": _label(left), "right": _label(right), "by": [q[0] for q in bp], "measures": [q[0] for q in mp], "table": tname,
           "groups": sum(st.values()), "status": st, "totals": tdf}
    if report:
        path = _out_path(out_dir, f"数据对账_{_safe(res['left'])}_vs_{_safe(res['right'])}")
        res["report_cut"] = _write_report(con, path, [("合计", tdf), ("分组对账", tname)])
        res["report"] = path
    bad = con.execute(f"SELECT * FROM {qident(tname)} WHERE 状态 <> '一致' LIMIT {int(sample)}").df()
    lines = [f"**汇总对账：{res['left']} ↔ {res['right']}**（分组：{'、'.join(res['by']) or '总计'}；指标：{'、'.join(res['measures']) or '仅行数'}；容差 {tol}）",
             f"- 共 {res['groups']} 组：一致 {st.get('一致', 0)}，不一致 {st.get('不一致', 0)}，仅左表 {st.get('仅左表', 0)}，仅右表 {st.get('仅右表', 0)}",
             "", _md(tdf), "", f"结果表：{tname}"]
    if res.get("report"):
        lines.append(f"Excel 报告：{res['report']}")
    if len(bad):
        lines += ["", f"不一致的组（前 {len(bad)} 个）：", _md(bad, len(bad))]
    res["text"] = "\n".join(lines)
    return res
