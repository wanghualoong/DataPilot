import datetime as _dt
import decimal
import math
import re

import numpy as np


def clip(s, n):
    if s is None:
        return ""
    if len(s) <= n:
        return s
    head = int(n * 0.6)
    return s[:head] + f"\n…[{len(s) - n} chars omitted]…\n" + s[-(n - head):]


def json_default(o):
    if isinstance(o, (np.integer,)):
        return int(o)
    if isinstance(o, (np.floating,)):
        f = float(o)
        return None if math.isnan(f) or math.isinf(f) else f
    if isinstance(o, (np.bool_,)):
        return bool(o)
    if isinstance(o, (_dt.datetime, _dt.date, _dt.time)):
        return o.isoformat()
    if isinstance(o, decimal.Decimal):
        return float(o)
    if isinstance(o, (bytes, bytearray)):
        return o.hex()
    if isinstance(o, np.ndarray):
        return o.tolist()
    try:
        import pandas as pd
        if o is pd.NaT:
            return None
        if isinstance(o, pd.Timestamp):
            return o.isoformat()
        if isinstance(o, pd.Timedelta):
            return str(o)
    except Exception:  # noqa: BLE001
        pass
    return str(o)


def clean_value(v):
    if v is None:
        return None
    if isinstance(v, float) and (math.isnan(v) or math.isinf(v)):
        return None
    return v


_ws = re.compile(r"\s+")


def normalize_col(name, idx):
    s = str(name) if name is not None else ""
    s = _ws.sub("_", s.strip()).strip("_")
    s = s.replace('"', "").replace("\n", "_")
    if not s or s.lower().startswith("unnamed") or s.lower() == "nan":
        s = f"col_{idx + 1}"
    return s[:63]


def dedupe(names):
    seen = {}
    out = []
    for n in names:
        k = n.lower()
        if k in seen:
            seen[k] += 1
            out.append(f"{n}_{seen[k]}")
        else:
            seen[k] = 0
            out.append(n)
    return out


def table_name_from(stem):
    s = re.sub(r"[^\w一-鿿]+", "_", stem).strip("_")
    if not s:
        s = "table"
    if s[0].isdigit():
        s = "t_" + s
    return s[:50]


def qident(name):
    return '"' + str(name).replace('"', '""') + '"'


def is_within(root, path):
    """True when `path` is `root` or inside it. Case-insensitive on Windows (os.path.normcase), symlinks resolved."""
    import os
    r = os.path.normcase(os.path.realpath(root))
    p = os.path.normcase(os.path.realpath(path))
    return p == r or p.startswith(r.rstrip(os.sep) + os.sep)


def replace_file(tmp, dst, tries=20):
    """os.replace with retries: on Windows the target may be briefly open in another process (antivirus, indexer)."""
    import os
    import time
    for i in range(tries):
        try:
            os.replace(tmp, dst)
            return
        except PermissionError:
            if os.name != "nt" or i == tries - 1:
                raise
            time.sleep(0.05 * (i + 1))
