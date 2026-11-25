"""Excel (xlsx) cells for `op_export`: one Python value → (kind, value) that XlsxWriter can store.

Rules: None / NaN / ±inf → empty cell; date-times lose their time zone (Excel stores none); integers with 15 or more
digits and decimals beyond Excel's 15 significant digits become text, so ids and card numbers keep every digit;
bytes → hex; lists / dicts → JSON text; text longer than Excel's 32,767-character cell limit is cut with "…".
"""
import datetime as _dt
import decimal
import json
import math

CELL_LIMIT = 32_767
_EXCEL_DIGITS = 15


def _text(s):
    return "string", (s if len(s) <= CELL_LIMIT else s[: CELL_LIMIT - 1] + "…")


def convert(v):
    """(kind, value); kind is one of null / bool / number / date / datetime / time / string."""
    if v is None:
        return "null", None
    if isinstance(v, bool):
        return "bool", v
    if isinstance(v, int):
        return ("number", v) if abs(v) < 10 ** _EXCEL_DIGITS else ("string", str(v))
    if isinstance(v, float):
        return ("null", None) if math.isnan(v) or math.isinf(v) else ("number", v)
    if isinstance(v, decimal.Decimal):
        if not v.is_finite():
            return "null", None
        return ("number", float(v)) if v.adjusted() < _EXCEL_DIGITS else _text(str(v))
    if isinstance(v, _dt.datetime):
        return "datetime", v.replace(tzinfo=None)
    if isinstance(v, _dt.date):
        return "date", _dt.datetime(v.year, v.month, v.day)
    if isinstance(v, _dt.time):
        return "time", v.replace(tzinfo=None)
    if isinstance(v, (bytes, bytearray, memoryview)):
        return _text(bytes(v).hex())
    if isinstance(v, (list, tuple, dict)):
        return _text(json.dumps(v, ensure_ascii=False, default=str))
    return _text(str(v))


class XlsxCells:
    """Writes converted values into a worksheet, with date / time number formats of one workbook."""

    def __init__(self, wb):
        self._fmt = {"date": wb.add_format({"num_format": "yyyy-mm-dd"}),
                     "datetime": wb.add_format({"num_format": "yyyy-mm-dd hh:mm:ss"}),
                     "time": wb.add_format({"num_format": "hh:mm:ss"})}

    def write(self, ws, r, c, v):
        kind, x = convert(v)
        if kind == "null":
            return
        if kind == "number":
            ws.write_number(r, c, x)
        elif kind == "bool":
            ws.write_boolean(r, c, x)
        elif kind in self._fmt:
            ws.write_datetime(r, c, x, self._fmt[kind])
        else:
            ws.write_string(r, c, x)
