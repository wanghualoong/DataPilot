"""Read-only SQL guard: allow only SELECT/WITH/SUMMARIZE/DESCRIBE/SHOW/EXPLAIN/PIVOT; add outer LIMIT."""
import re

import sqlglot
from sqlglot import exp

ALLOWED_PREFIX = re.compile(r"^\s*(select|with|summarize|describe|show|explain|pivot|unpivot|from|values|\()", re.I)
FORBIDDEN = (exp.Insert, exp.Update, exp.Delete, exp.Drop, exp.Create, exp.Alter, exp.Command, exp.Merge,
             exp.TruncateTable)


class SQLGuardError(ValueError):
    pass


def strip_comments(sql):
    sql = re.sub(r"/\*.*?\*/", " ", sql, flags=re.S)
    return "\n".join(l.split("--")[0] if "--" in l and "'" not in l else l for l in sql.splitlines()).strip()


def check_readonly(sql):
    s = sql.strip().rstrip(";").strip()
    if not s:
        raise SQLGuardError("empty SQL")
    if ";" in strip_comments(s):
        try:
            parts = [p for p in sqlglot.parse(s, read="duckdb") if p is not None]
        except Exception:  # noqa: BLE001
            parts = [None, None]
        if len(parts) > 1:
            raise SQLGuardError("only one statement allowed")
    if not ALLOWED_PREFIX.match(strip_comments(s)):
        raise SQLGuardError("only read-only queries (SELECT/WITH/SUMMARIZE/DESCRIBE) are allowed in sql_query; "
                            "use python tool for transformations")
    try:
        tree = sqlglot.parse_one(s, read="duckdb")
    except Exception:  # noqa: BLE001
        return s  # let DuckDB report syntax error
    for node in tree.walk():
        if isinstance(node, FORBIDDEN):
            raise SQLGuardError(f"forbidden statement: {type(node).__name__}")
    return s


def tables_in(sql):
    try:
        return sorted({t.name for t in sqlglot.parse_one(sql, read="duckdb").find_all(exp.Table) if t.name})
    except Exception:  # noqa: BLE001
        return []


# ---------- controlled write SQL for data_transform (derived tables / views only) ----------
_FILE_FUNCS = re.compile(r"\b(read_csv|read_csv_auto|read_parquet|read_json|read_json_auto|read_text|read_blob|"
                         r"parquet_scan|csv_scan|glob|sniff_csv|read_xlsx|st_read)\s*\(", re.I)


def _target(tree):
    t = tree.this if isinstance(tree, (exp.Create, exp.Alter, exp.Update)) else None
    if isinstance(tree, exp.Drop):
        ts = tree.args.get("tables") or ([tree.this] if tree.this is not None else [])
        if len(ts) != 1:
            return None
        t = ts[0]
    if isinstance(t, exp.Schema):
        t = t.this
    return t.name if isinstance(t, exp.Table) else None


def check_transform(sql, base_tables, derived_tables):
    """Allow: CREATE [OR REPLACE] TABLE|VIEW x AS SELECT …; ALTER TABLE <derived> ADD COLUMN …;
    UPDATE <derived> SET …; DROP TABLE|VIEW <derived>. Imported (base) tables are never modified.
    Returns (normalized_sql, kind, target, replaces_existing)."""
    s = sql.strip().rstrip(";").strip()
    if not s:
        raise SQLGuardError("empty SQL")
    try:
        parts = [p for p in sqlglot.parse(s, read="duckdb") if p is not None]
    except Exception as e:  # noqa: BLE001
        raise SQLGuardError(f"cannot parse SQL: {e}")
    if len(parts) != 1:
        raise SQLGuardError("exactly one statement per data_transform call")
    tree = parts[0]
    if _FILE_FUNCS.search(strip_comments(s)):
        raise SQLGuardError("file-reading functions are not allowed here; import files with data_import first")
    bad = tuple(getattr(exp, n) for n in ("Command", "Copy", "Attach", "Detach", "Install", "Insert", "Delete",
                                           "Merge", "TruncateTable", "Pragma", "Set") if hasattr(exp, n))
    for node in tree.walk():
        if isinstance(node, bad):
            raise SQLGuardError(f"forbidden statement: {type(node).__name__}")
    base = {b.lower() for b in base_tables}
    derived = {d.lower() for d in derived_tables}
    target = _target(tree)
    if not target:
        raise SQLGuardError("could not determine the target table")
    tl = target.lower()
    if isinstance(tree, exp.Create):
        kind = (tree.args.get("kind") or "").upper()
        if kind not in ("TABLE", "VIEW"):
            raise SQLGuardError("only CREATE TABLE / CREATE VIEW … AS SELECT are allowed")
        if tree.expression is None:
            raise SQLGuardError("CREATE needs AS SELECT …")
        if tl in base:
            raise SQLGuardError(f"'{target}' is an imported source table and cannot be replaced; choose another name")
        exists = tl in derived
        if exists and not tree.args.get("replace"):
            raise SQLGuardError(f"'{target}' already exists; use CREATE OR REPLACE to overwrite it")
        return s, "create_" + kind.lower(), target, exists
    if isinstance(tree, (exp.Alter, exp.Update, exp.Drop)):
        if tl not in derived:
            raise SQLGuardError(f"'{target}' is not a derived table (only tables created by data_transform can be "
                                f"altered / updated / dropped)")
        kind = {exp.Alter: "alter", exp.Update: "update", exp.Drop: "drop"}[type(tree)]
        return s, kind, target, True
    raise SQLGuardError("only CREATE … AS SELECT, ALTER TABLE … ADD COLUMN, UPDATE and DROP of derived tables are allowed")
