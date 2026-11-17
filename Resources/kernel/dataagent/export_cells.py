"""Export cell-processor chain.

Adapted from Chat2DB Community v5.3.8 (LicenseRef-Chat2DB): `ExportCellProcessorChain` / `IExportCellProcessor` /
`ExportCell` / `ExportCellContext` (domain/core/impl/task/export). Each processor gets (context, cell) and returns a
new cell; the chain applies them in order. Re-implemented in Python for DataPilot's DuckDB export (`op_export` xlsx
writer); Chat2DB's JDBC metadata is replaced by DuckDB column types.
"""
import datetime as _dt
import decimal
import math
from dataclasses import dataclass, field


@dataclass
class Ctx:
    column: str
    db_type: str = ""
    fmt: str = "xlsx"


@dataclass
class Cell:
    value: object
    kind: str = "auto"          # auto | null | number | string | date | datetime | time | bool
    meta: dict = field(default_factory=dict)


class NullProcessor:
    """None / NaN / NaT → empty cell."""
    def process(self, ctx, cell):
        v = cell.value
        if v is None or (isinstance(v, float) and math.isnan(v)):
            return Cell(None, "null")
        return cell


class TemporalProcessor:
    """Dates and times become real Excel dates (tz dropped, Excel cannot store it)."""
    def process(self, ctx, cell):
        v = cell.value
        if isinstance(v, _dt.datetime):
            return Cell(v.replace(tzinfo=None) if v.tzinfo else v, "datetime")
        if isinstance(v, _dt.date):
            return Cell(v, "date")
        if isinstance(v, _dt.time):
            return Cell(v.replace(tzinfo=None), "time")
        return cell


class NumberProcessor:
    """Numbers stay numeric; integers beyond Excel's 15 significant digits (ids, card numbers) become text."""
    def process(self, ctx, cell):
        v = cell.value
        if cell.kind != "auto" or isinstance(v, bool):
            return Cell(v, "bool") if isinstance(v, bool) else cell
        if isinstance(v, int):
            return Cell(str(v), "string") if abs(v) >= 10 ** 15 else Cell(v, "number")
        if isinstance(v, decimal.Decimal):
            return Cell(float(v), "number") if v.adjusted() < 15 else Cell(str(v), "string")
        if isinstance(v, float):
            return Cell(None, "null") if math.isinf(v) else Cell(v, "number")
        return cell


class TextProcessor:
    """Everything else → text; Excel's 32,767-char cell limit; bytes as hex; nested values as JSON-ish text."""
    LIMIT = 32_767

    def process(self, ctx, cell):
        if cell.kind != "auto":
            return cell
        v = cell.value
        if isinstance(v, (bytes, bytearray, memoryview)):
            s = bytes(v).hex()
        elif isinstance(v, (list, tuple, dict)):
            import json
            s = json.dumps(v, ensure_ascii=False, default=str)
        else:
            s = str(v)
        if len(s) > self.LIMIT:
            return Cell(s[: self.LIMIT - 1] + "…", "string", {"truncated": True})
        return Cell(s, "string")


class Chain:
    def __init__(self, processors):
        self.processors = list(processors)

    def process(self, ctx, value):
        cell = Cell(value)
        for p in self.processors:
            cell = p.process(ctx, cell)
            if cell is None:
                raise ValueError(f"{type(p).__name__} returned no cell")
        return cell


CHAIN = Chain([NullProcessor(), TemporalProcessor(), NumberProcessor(), TextProcessor()])


class Formats:
    def __init__(self, wb):
        self.date = wb.add_format({"num_format": "yyyy-mm-dd"})
        self.datetime = wb.add_format({"num_format": "yyyy-mm-dd hh:mm:ss"})
        self.time = wb.add_format({"num_format": "hh:mm:ss"})


def write(ws, r, c, cell, fmts):
    k, v = cell.kind, cell.value
    if k == "null":
        return
    if k == "number":
        ws.write_number(r, c, v)
    elif k == "bool":
        ws.write_boolean(r, c, v)
    elif k == "datetime":
        ws.write_datetime(r, c, v, fmts.datetime)
    elif k == "date":
        ws.write_datetime(r, c, _dt.datetime(v.year, v.month, v.day), fmts.date)
    elif k == "time":
        ws.write_datetime(r, c, v, fmts.time)
    else:
        ws.write_string(r, c, v)
