"""Text / table extraction for Office and PDF files (python-docx, python-pptx, openpyxl, pdfplumber/pypdf)."""
import os


def extract_text(path, max_chars=60000, pages=None):
    ext = os.path.splitext(path)[1].lower()
    if ext == ".docx":
        out = _docx(path)
    elif ext == ".pptx":
        out = _pptx(path)
    elif ext in (".xlsx", ".xlsm"):
        out = _xlsx(path)
    elif ext == ".pdf":
        out = _pdf(path, pages)
    else:
        raise ValueError(f"unsupported file type for text extraction: {ext}")
    truncated = len(out) > max_chars
    return {"text": out[:max_chars], "chars": len(out), "truncated": truncated, "type": ext.lstrip(".")}


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


def _pdf(path, pages=None):
    import pdfplumber
    parts = []
    with pdfplumber.open(path) as pdf:
        idx = range(len(pdf.pages))
        if pages:
            a, _, b = str(pages).partition("-")
            idx = range(max(0, int(a) - 1), min(len(pdf.pages), int(b or a)))
        for i in idx:
            p = pdf.pages[i]
            parts.append(f"<!-- Page {i + 1} -->\n" + (p.extract_text() or ""))
    return "\n\n".join(parts)


def tables(path):
    """Tables from .docx / .pdf / .pptx as lists of rows (first row = header)."""
    ext = os.path.splitext(path)[1].lower()
    out = []
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
        with pdfplumber.open(path) as pdf:
            for p in pdf.pages:
                for t in p.extract_tables() or []:
                    out.append([[("" if c is None else str(c)).strip() for c in r] for r in t])
    return [t for t in out if len(t) >= 2]
