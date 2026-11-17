"""File import: CSV/TSV/TXT (encoding + delimiter + header detection), Excel (multi-sheet, title rows), Parquet, JSON."""
import csv
import io
import os

import pandas as pd

from .util import dedupe, normalize_col, qident, table_name_from

SMALL_ROWS_DEFAULT = 5000


def detect_encoding(path, sample_bytes=200_000):
    with open(path, "rb") as f:
        raw = f.read(sample_bytes)
    if raw.startswith(b"\xef\xbb\xbf"):
        return "utf-8-sig"
    try:
        raw.decode("utf-8")
        return "utf-8"
    except UnicodeDecodeError:
        pass
    try:
        from charset_normalizer import from_bytes
        best = from_bytes(raw).best()
        if best and best.encoding:
            enc = best.encoding.lower()
            # charset-normalizer often reports gb2312/big5 subsets — widen to superset
            if enc in ("gb2312", "gbk", "hz"):
                return "gb18030"
            return enc
    except Exception:  # noqa: BLE001
        pass
    return "gb18030"


def detect_delimiter(text):
    try:
        return csv.Sniffer().sniff(text[:20000], delimiters=",\t;|").delimiter
    except Exception:  # noqa: BLE001
        counts = {d: text[:5000].count(d) for d in [",", "\t", ";", "|"]}
        return max(counts, key=counts.get)


def _header_row_guess(df_raw, max_scan=15):
    """Find the first row that looks like a header: most non-null cells, mostly strings, followed by data."""
    best, best_score = 0, -1
    ncols = df_raw.shape[1]
    for i in range(min(max_scan, len(df_raw))):
        row = df_raw.iloc[i]
        nonnull = row.notna().sum()
        if ncols and nonnull / ncols < 0.5:
            continue
        str_ratio = sum(isinstance(v, str) for v in row if pd.notna(v)) / max(nonnull, 1)
        score = nonnull + str_ratio * ncols
        if score > best_score + 0.5:
            best, best_score = i, score
        if nonnull == ncols and str_ratio > 0.8:
            return i
    return best


def _finalize_df(df, keep_text=()):
    """keep_text: columns forced to text by the caller (no numeric / date inference, so '001' stays '001')."""
    df = df.dropna(how="all").dropna(axis=1, how="all")
    cols = dedupe([normalize_col(c, i) for i, c in enumerate(df.columns)])
    df.columns = cols
    # strip strings, convert numeric-like object columns
    for c in df.columns:
        if df[c].dtype == object or pd.api.types.is_string_dtype(df[c].dtype):
            s = df[c].astype("string").str.strip()
            s = s.replace({"": pd.NA})
            if c in keep_text:
                df[c] = s
                continue
            num = pd.to_numeric(s.str.replace(",", "", regex=False).str.replace("%", "", regex=False), errors="coerce")
            if s.notna().sum() > 0 and num.notna().sum() / s.notna().sum() >= 0.98 and not s.str.contains("%", na=False).any():
                df[c] = num
                continue
            if s.notna().sum() > 0 and s.str.match(r"^\d{4}[-/年.]\d{1,2}([-/月.]\d{1,2})?").mean() > 0.9:
                dt = pd.to_datetime(s.str.replace("年", "-").str.replace("月", "-").str.replace("日", ""), errors="coerce")
                if dt.notna().sum() / s.notna().sum() >= 0.95:
                    df[c] = dt
                    continue
            df[c] = s
    for c in df.columns:
        if isinstance(df[c].dtype, pd.DatetimeTZDtype):
            df[c] = df[c].dt.tz_convert(None)
    return df.reset_index(drop=True)


def _formula_columns(path, sheet, header_row, cols, max_rows=300):
    """Header names of `cols` whose cells hold formulas without a cached value (file saved by a tool that does not
    calculate, e.g. openpyxl / some exporters). Only the first rows are scanned (read-only, fast)."""
    try:
        import openpyxl
        wb = openpyxl.load_workbook(path, read_only=True, data_only=False)
        try:
            ws = wb[sheet]
            hits = set()
            for r_i, row in enumerate(ws.iter_rows(min_row=header_row + 2, max_row=header_row + 1 + max_rows, values_only=True)):
                for j in cols:
                    if j < len(row) and isinstance(row[j], str) and row[j].startswith("="):
                        hits.add(j)
            return hits
        finally:
            wb.close()
    except Exception:  # noqa: BLE001
        return set()


def read_excel_sheets(path, header_row=None, sheet=None, keep_text=()):
    ext = os.path.splitext(path)[1].lower()
    engine = "calamine"
    try:
        xl = pd.ExcelFile(path, engine=engine)
    except Exception:  # noqa: BLE001
        engine = "openpyxl" if ext in (".xlsx", ".xlsm") else "xlrd"
        xl = pd.ExcelFile(path, engine=engine)
    out, warnings = {}, {}
    names = [sheet] if sheet else xl.sheet_names
    for i, sn in enumerate(names):
        _progress(f"读取 sheet {i + 1}/{len(names)}：{sn}")
        raw = xl.parse(sn, header=None)
        if raw.dropna(how="all").empty:
            continue
        h = header_row if header_row is not None else _header_row_guess(raw)
        header = raw.iloc[h].tolist()
        body = raw.iloc[h + 1:].copy()
        body.columns = header
        # formulas: calamine / openpyxl return the value Excel cached at save time. Columns that have a header but no
        # values at all usually mean formulas that were never calculated → warn instead of silently importing blanks.
        empty = [j for j in range(body.shape[1]) if pd.notna(header[j]) and body.iloc[:, j].isna().all()] if len(body) else []
        if empty and ext in (".xlsx", ".xlsm"):
            f = _formula_columns(path, sn, h, empty)
            if f:
                warnings[sn] = [str(header[j]) for j in sorted(f)]
        _progress(f"整理列类型：{sn}（{len(body):,} 行）")
        out[sn] = _finalize_df(body, keep_text)
    read_excel_sheets.warnings = warnings
    return out


def _progress(msg):
    """Progress line for the kernel task list (set by the engine for the running request)."""
    f = getattr(_progress, "hook", None)
    if f:
        try:
            f(msg)
        except Exception:  # noqa: BLE001
            pass


def read_csv_df(path, header_row=None, delimiter=None, encoding=None, keep_text=()):
    enc = encoding or detect_encoding(path)
    with open(path, "r", encoding=enc, errors="replace") as f:
        head = f.read(100_000)
    delim = delimiter or detect_delimiter(head)
    if header_row is None:
        sample = pd.read_csv(io.StringIO(head), sep=delim, header=None, nrows=20, dtype=str,
                             engine="python", on_bad_lines="skip")
        header_row = _header_row_guess(sample) if len(sample) else 0
    df = pd.read_csv(path, sep=delim, header=header_row, encoding=enc, encoding_errors="replace",
                     dtype=str, keep_default_na=True, on_bad_lines="warn", low_memory=False)
    return _finalize_df(df, keep_text), {"encoding": enc, "delimiter": delim, "header_row": header_row}


def apply_mapping(df, columns=None, types=None):
    """Column mapping before writing. `types` = {原列名: number|date|text|bool} is applied first (names as in the
    file), then `columns` = {old: new} (rename; "__keep_only__": true drops the others) or [names] (select)."""
    for c, t in (types or {}).items():
        if c not in df.columns:
            raise ValueError(f"type mapping: column {c!r} not found; available: {list(df.columns)}")
        t = str(t).lower()
        s = df[c]
        if t in ("number", "numeric", "float", "int", "integer", "数值", "数字"):
            df[c] = pd.to_numeric(s.astype("string").str.replace(",", "", regex=False).str.strip(), errors="coerce")
        elif t in ("date", "datetime", "timestamp", "日期", "时间"):
            ss = s.astype("string").str.replace("年", "-").str.replace("月", "-").str.replace("日", "").str.strip()
            df[c] = pd.to_datetime(ss, errors="coerce")
        elif t in ("text", "string", "str", "varchar", "文本", "字符串"):
            df[c] = s.astype("string")
        elif t in ("bool", "boolean", "布尔"):
            df[c] = s.astype("string").str.lower().map({"true": True, "1": True, "yes": True, "是": True,
                                                         "false": False, "0": False, "no": False, "否": False})
        else:
            raise ValueError(f"unknown type {t!r} for {c!r} (number | date | text | bool)")
    if isinstance(columns, list) and columns:
        missing = [c for c in columns if c not in df.columns]
        if missing:
            raise ValueError(f"columns not found: {missing}; available: {list(df.columns)}")
        df = df[columns]
    elif isinstance(columns, dict) and columns:
        keep_only = bool(columns.pop("__keep_only__", False))
        missing = [c for c in columns if c not in df.columns]
        if missing:
            raise ValueError(f"columns not found: {missing}; available: {list(df.columns)}")
        if keep_only:
            df = df[list(columns)]
        df = df.rename(columns={k: v for k, v in columns.items() if v})
        df.columns = dedupe([str(c) for c in df.columns])
    return df


def import_file(con, path, table=None, header_row=None, sheet=None, mode="replace", small_rows=SMALL_ROWS_DEFAULT,
                columns=None, types=None, dry_run=False):
    """Import file into DuckDB tables. Returns list of table infos. dry_run: parse + map only (preview)."""
    keep_text = {c for c, t in (types or {}).items() if str(t).lower() in ("text", "string", "str", "varchar", "文本", "字符串")}
    ext = os.path.splitext(path)[1].lower()
    stem = table_name_from(os.path.splitext(os.path.basename(path))[0])
    frames = {}
    meta = {}
    if ext in (".xlsx", ".xlsm", ".xls", ".ods"):
        sheets = read_excel_sheets(path, header_row=header_row, sheet=sheet, keep_text=keep_text)
        warn = getattr(read_excel_sheets, "warnings", {})
        for sn, df in sheets.items():
            name = table or stem if len(sheets) == 1 else f"{table or stem}_{table_name_from(sn)}"
            m = {"sheet": sn}
            if warn.get(sn):
                m["formula_warning"] = (f"这些列是公式但文件里没有计算结果（导入为空）：{', '.join(warn[sn])}。"
                                        "请在 Excel / WPS 中打开该文件、保存一次后重新导入。")
            frames[name] = (df, m)
    elif ext in (".csv", ".tsv", ".txt"):
        df, m = read_csv_df(path, header_row=header_row, keep_text=keep_text)
        frames[table or stem] = (df, m)
    elif ext == ".parquet":
        frames[table or stem] = (pd.read_parquet(path), {})
    elif ext in (".json", ".jsonl", ".ndjson"):
        df = pd.read_json(path, lines=ext != ".json")
        frames[table or stem] = (_finalize_df(df), {})
    elif ext in (".docx", ".pdf", ".pptx"):
        from . import office
        ts = office.tables(path)
        if not ts:
            raise ValueError(f"No tables found in {os.path.basename(path)}. Use read_file to read its text instead.")
        for i, t in enumerate(ts, 1):
            w = max(len(r) for r in t)
            t = [r + [None] * (w - len(r)) for r in t]
            df = pd.DataFrame(t[1:], columns=t[0])
            name = (table or stem) if len(ts) == 1 else f"{table or stem}_t{i}"
            frames[name] = (_finalize_df(df), {"source_table": i})
    else:
        raise ValueError(f"Unsupported file type: {ext}")

    if columns or types:
        frames = {n: (apply_mapping(df, dict(columns) if isinstance(columns, dict) else columns, types), m) for n, (df, m) in frames.items()}
    if dry_run:
        out = []
        for name, (df, m) in frames.items():
            head = df.head(10)
            out.append({"table": name, "rows": int(len(df)), "columns": [str(c) for c in df.columns],
                        "dtypes": [str(t) for t in df.dtypes], "sample": head.astype(object).where(head.notna(), None).astype(str).where(head.notna(), None).values.tolist(),
                        "source": os.path.basename(path), **{k: v for k, v in m.items() if k in ("sheet", "encoding", "delimiter", "header_row", "formula_warning")}})
        return out, frames
    results = []
    for name, (df, m) in frames.items():
        _progress(f"写入表 {name}（{len(df):,} 行）")
        if mode == "append":
            con.register("_dp_import", df)
            con.execute(f"INSERT INTO {qident(name)} SELECT * FROM _dp_import")
            con.unregister("_dp_import")
        else:
            con.register("_dp_import", df)
            con.execute(f"CREATE OR REPLACE TABLE {qident(name)} AS SELECT * FROM _dp_import")
            con.unregister("_dp_import")
        n = con.execute(f"SELECT count(*) FROM {qident(name)}").fetchall()[0][0]
        results.append({
            "table": name, "rows": int(n), "columns": len(df.columns), "source": os.path.basename(path), **m,
            "strategy": "pandas+sql" if n <= small_rows else "sql",
        })
        meta[name] = m
    return results, frames
