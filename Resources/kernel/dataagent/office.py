"""Text / table extraction for Office and PDF files (python-docx, python-pptx, openpyxl, pdfplumber/pypdf)."""
import os


def extract_text(path, max_chars=60000, pages=None, ocr=None):
    """`ocr`: scanned-page results from the core (see ocr.py); those pages are read from it instead of the text layer."""
    from . import ocr as ocrmod
    ext = os.path.splitext(path)[1].lower()
    review = {}
    if ocrmod.is_image(path):
        r = ocrmod.page_text(ocr, 1) if ocr else None
        if r is None:
            raise ValueError("图片需要文档识别服务（设置 → 通用 → 文档识别）才能读出文字 / image text needs the document-parse service")
        out, rv = r
        out = ocrmod.header(ocr, "图片") + "\n" + out
        if rv:
            review["1"] = rv
        truncated = len(out) > max_chars
        return {"text": out[:max_chars], "chars": len(out), "truncated": truncated, "type": ext.lstrip("."), "ocr_review": review}
    if ext == ".docx":
        out = _docx(path)
    elif ext == ".pptx":
        out = _pptx(path)
    elif ext in (".xlsx", ".xlsm"):
        out = _xlsx(path)
    elif ext == ".pdf":
        out = _pdf(path, pages, ocr, review)
    else:
        raise ValueError(f"unsupported file type for text extraction: {ext}")
    truncated = len(out) > max_chars
    return {"text": out[:max_chars], "chars": len(out), "truncated": truncated, "type": ext.lstrip("."), "ocr_review": review}


def _md_table(rows):
    rows = [[("" if c is None else str(c)).replace("\n", " ").replace("|", "\\|").strip() for c in r] for r in rows if r]
    if not rows:
        return ""
    w = max(len(r) for r in rows)
    rows = [r + [""] * (w - len(r)) for r in rows]
    lines = ["| " + " | ".join(rows[0]) + " |", "|" + "---|" * w]
    lines += ["| " + " | ".join(r) + " |" for r in rows[1:]]
    return "\n".join(lines)


def _docx(path):
    import docx
    from docx.table import Table
    from docx.text.paragraph import Paragraph
    d = docx.Document(path)
    parts = []
    for child in d.element.body.iterchildren():
        tag = child.tag.rsplit("}", 1)[-1]
        if tag == "p":
            p = Paragraph(child, d)
            t = p.text.strip()
            if not t:
                continue
            style = (p.style.name or "") if p.style is not None else ""
            if style.lower().startswith("heading"):
                lvl = "".join(ch for ch in style if ch.isdigit()) or "1"
                parts.append("#" * min(int(lvl), 4) + " " + t)
            elif "list" in style.lower():
                parts.append("- " + t)
            else:
                parts.append(t)
        elif tag == "tbl":
            parts.append(_md_table([[c.text for c in r.cells] for r in Table(child, d).rows]))
    return "\n\n".join(parts)


def _pptx(path):
    from pptx import Presentation
    prs = Presentation(path)
    parts = []
    for i, s in enumerate(prs.slides, 1):
        parts.append(f"<!-- Slide {i} -->")
        for sh in s.shapes:
            if sh.has_text_frame and sh.text_frame.text.strip():
                parts.append(sh.text_frame.text.strip())
            if getattr(sh, "has_table", False) and sh.has_table:
                parts.append(_md_table([[c.text for c in r.cells] for r in sh.table.rows]))
        if s.has_notes_slide and s.notes_slide.notes_text_frame.text.strip():
            parts.append("Notes: " + s.notes_slide.notes_text_frame.text.strip())
    return "\n\n".join(parts)


def _xlsx(path):
    import openpyxl
    wb = openpyxl.load_workbook(path, read_only=True, data_only=True)
    parts = []
    for ws in wb.worksheets:
        rows = []
        for r in ws.iter_rows(values_only=True):
            if any(v is not None for v in r):
                rows.append(list(r))
            if len(rows) >= 200:
                break
        parts.append(f"## Sheet: {ws.title}\n" + _md_table(rows))
    return "\n\n".join(parts)


def _pdf(path, pages=None, ocr=None, review=None):
    import pdfplumber
    from . import ocr as ocrmod
    parts = [ocrmod.header(ocr)] if ocr and ocr.get("pages") else []
    with pdfplumber.open(path) as pdf:
        for i in ocrmod.page_list(pages, len(pdf.pages)):
            r = ocrmod.page_text(ocr, i + 1)
            if r is not None:
                body, rv = r
                if rv and review is not None:
                    review[str(i + 1)] = rv
                parts.append(f"<!-- Page {i + 1}（扫描件 OCR） -->\n" + body)
            else:
                parts.append(f"<!-- Page {i + 1} -->\n" + (pdf.pages[i].extract_text() or ""))
    return "\n\n".join(parts)


def tables(path, ocr=None):
    """Tables from .docx / .pdf / .pptx (and scanned pages / images via `ocr`) as lists of rows (first row = header)."""
    from . import ocr as ocrmod
    ext = os.path.splitext(path)[1].lower()
    out = []
    scanned = set((ocr or {}).get("pages") or [])
    if ocrmod.is_image(path):
        return ocrmod.tables(ocr)
    if ext == ".docx":
        import docx
        for t in docx.Document(path).tables:
            out.append([[c.text.strip() for c in r.cells] for r in t.rows])
    elif ext == ".pptx":
        from pptx import Presentation
        for s in Presentation(path).slides:
            for sh in s.shapes:
                if getattr(sh, "has_table", False) and sh.has_table:
                    out.append([[c.text.strip() for c in r.cells] for r in sh.table.rows])
    elif ext == ".pdf":
        import pdfplumber
        ocr_tables = {}
        if scanned:
            for n in sorted(scanned):
                ocr_tables[n] = ocrmod.tables(ocr, [n])
        with pdfplumber.open(path) as pdf:
            for i, p in enumerate(pdf.pages, 1):
                if i in scanned:
                    out.extend(ocr_tables.get(i) or [])
                    continue
                for t in p.extract_tables() or []:
                    out.append([[("" if c is None else str(c)).strip() for c in r] for r in t])
    return [t for t in out if len(t) >= 2]
