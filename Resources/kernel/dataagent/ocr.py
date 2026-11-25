"""Scanned PDF pages / images: text from document-parse results (MinerU structured_content.json, fetched by the core —
the kernel never goes online). The primary tier (default advanced: no invented text, reads seals) is merged with a
cross-check tier (default basic: keeps lines under red seals, may have character errors):

- lines only the check tier saw are added, flagged "需核对";
- a primary page much shorter than the check page (layout lost) takes the check tier's body, flagged;
- amounts / dates / long numbers found by only one tier are listed per page;
- seal text comes from the primary tier's image blocks; empty image regions are marked, signatures are never guessed.
"""
import html
import json
import os
import re
import unicodedata
from difflib import SequenceMatcher
from html.parser import HTMLParser

IMAGE_EXT = (".png", ".jpg", ".jpeg", ".webp", ".bmp", ".tif", ".tiff")
TEXT_TYPES = {"text", "list", "paragraph_title", "doc_title", "title", "header", "footer", "aside_text", "caption", "footnote"}


def is_image(path):
    return os.path.splitext(path)[1].lower() in IMAGE_EXT


def page_list(pages, n):
    """'1-5', '3', '1-3,7' → 0-based indices within [0, n)."""
    if not pages:
        return list(range(n))
    out = []
    for part in str(pages).replace("，", ",").split(","):
        part = part.strip()
        if not part:
            continue
        a, _, b = part.partition("-")
        try:
            lo, hi = int(a), int(b or a)
        except ValueError:
            continue
        out.extend(i for i in range(max(1, lo) - 1, min(n, hi)) if i not in out)
    return out


def range_str(nums):
    """[1,2,3,5] → '1-3,5' (1-based page numbers)."""
    nums = sorted(set(nums))
    out, i = [], 0
    while i < len(nums):
        j = i
        while j + 1 < len(nums) and nums[j + 1] == nums[j] + 1:
            j += 1
        out.append(str(nums[i]) if i == j else f"{nums[i]}-{nums[j]}")
        i = j + 1
    return ",".join(out)


def scan_pages(path, pages=None, min_chars=20):
    """Pages without a usable text layer (1-based). Images are one scanned page."""
    if is_image(path):
        return {"total": 1, "scanned": [1], "range": "1", "image": True}
    import pdfplumber
    scanned = []
    with pdfplumber.open(path) as pdf:
        n = len(pdf.pages)
        for i in page_list(pages, n):
            t = pdf.pages[i].extract_text() or ""
            good = sum(1 for ch in t if ch.isalnum())
            garbled = t.count("(cid:") > 5 or (t and t.count("�") / len(t) > 0.1)
            if good < min_chars or garbled:
                scanned.append(i + 1)
    return {"total": n, "scanned": scanned, "range": range_str(scanned)}


def render_page(path, page, out_dir, scale=2.0):
    """PNG of one page (original-page view); images are returned as they are."""
    if is_image(path):
        return {"path": path, "total": 1}
    import pypdfium2 as pdfium
    pdf = pdfium.PdfDocument(path)
    n = len(pdf)
    page = min(max(1, int(page)), n)
    os.makedirs(out_dir, exist_ok=True)
    import hashlib
    st = os.stat(path)
    key = hashlib.sha1(f"{os.path.realpath(path)}|{st.st_size}|{int(st.st_mtime)}|{scale}".encode()).hexdigest()[:16]
    out = os.path.join(out_dir, f"{key}_p{page}.png")
    if not os.path.exists(out):
        pdf[page - 1].render(scale=scale).to_pil().save(out)
    return {"path": out, "total": n, "page": page}


# ---------- result files ----------
def load_pages(p):
    """{page_idx: blocks} from the core (keys may be strings) or a structured_content.json path → {int page_idx (0-based): [blocks]}."""
    if isinstance(p, dict):
        return {int(k): v or [] for k, v in p.items()}
    if not p or not os.path.exists(p):
        return {}
    with open(p, encoding="utf-8") as f:
        d = json.load(f)
    return {int(pg.get("page_idx", i)): pg.get("blocks") or [] for i, pg in enumerate(d.get("pages") or [])}


# ---------- tables ----------
class _Table(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.rows, self.cell, self.depth = [], None, 0

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "table":
            self.depth += 1
            if self.depth > 1 and self.cell is not None:
                self.cell["t"].append(" ")
        elif self.depth == 1 and tag == "tr":
            self.rows.append([])
        elif self.depth == 1 and tag in ("td", "th"):
            def span(k):
                try:
                    return max(1, min(100, int(a.get(k) or 1)))
                except ValueError:
                    return 1
            self.cell = {"t": [], "rs": span("rowspan"), "cs": span("colspan")}
        elif self.cell is not None and tag in ("br", "td", "th", "p", "div"):
            self.cell["t"].append(" ")

    def handle_endtag(self, tag):
        if tag == "table":
            self.depth -= 1
        elif self.depth == 1 and tag in ("td", "th") and self.cell is not None:
            if not self.rows:
                self.rows.append([])
            self.rows[-1].append(self.cell)
            self.cell = None

    def handle_data(self, d):
        if self.cell is not None:
            self.cell["t"].append(d)


def html_grid(h):
    """HTML table → rectangular grid; merged cells repeat their text in every covered cell."""
    p = _Table()
    p.feed(h)
    grid = {}
    for r, row in enumerate(p.rows):
        c = 0
        for cell in row:
            while (r, c) in grid:
                c += 1
            txt = fix_text(" ".join("".join(cell["t"]).split()))
            for dr in range(cell["rs"]):
                for dc in range(cell["cs"]):
                    grid[(r + dr, c + dc)] = txt
            c += cell["cs"]
    if not grid:
        return []
    nr, nc = max(k[0] for k in grid) + 1, max(k[1] for k in grid) + 1
    return [[grid.get((r, c), "") for c in range(nc)] for r in range(nr)]


def md_grid(t):
    """Markdown pipe table → grid."""
    rows = []
    for line in t.strip().splitlines():
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [fix_text(x.strip()) for x in line.strip("|").split("|")]
        if all(re.fullmatch(r":?-{2,}:?", x or "--") for x in cells):
            continue
        rows.append(cells)
    return rows


def table_grid(content):
    c = content or ""
    return html_grid(c) if "<table" in c.lower() else md_grid(c)


def _md(rows):
    from .office import _md_table
    return _md_table(rows)


# ---------- text ----------
_EMAIL_GLUE = re.compile(r"(@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*?\.(?:com|cn|net|org|edu|gov))(?=[A-Za-z0-9_][A-Za-z0-9_.+-]*@)")


def fix_text(s):
    s = html.unescape(s or "")
    return _EMAIL_GLUE.sub(r"\1 ", s)


def norm(s):
    return re.sub(r"[\W_]+", "", unicodedata.normalize("NFKC", s or "")).lower()


def block_text(b):
    t, c = b.get("type"), b.get("content") or ""
    if t == "page_number":
        return ""
    if t == "table":
        g = table_grid(c)
        return _md(g) if g else fix_text(c)
    if t in ("image", "seal", "chart", "figure"):
        txt = " ".join(fix_text(c).split())
        return f"【印章 / 图片中的文字（OCR）：{txt}】" if txt else "【图片区域（印章 / 签名 / 插图），未识别出文字，请对照原页】"
    if t == "aside_text":
        return f"【页边文字：{' '.join(fix_text(c).split())}】" if c.strip() else ""
    if t == "doc_title":
        return "# " + fix_text(c).strip()
    if t in ("paragraph_title", "title"):
        return "## " + fix_text(c).strip()
    return fix_text(c).strip()


def plain(blocks):
    """Comparable text of a page (tables flattened, image text excluded: unreliable in the check tier)."""
    out = []
    for b in blocks:
        if b.get("type") in ("image", "seal", "chart", "figure", "page_number"):
            continue
        if b.get("type") == "table":
            out.extend(" ".join(r) for r in table_grid(b.get("content")))
        else:
            out.append(fix_text(b.get("content")))
    return "\n".join(out)


def _present(line_n, ref_n, ref_lines=()):
    """Is this check-tier line already in the primary page? (a variant of a primary line — e.g. one digit differs — counts
    as present: the differing value itself is reported by the key-field comparison)"""
    if len(line_n) < 4 or line_n in ref_n:
        return True
    m = SequenceMatcher(None, line_n, ref_n, autojunk=False).find_longest_match(0, len(line_n), 0, len(ref_n))
    if m.size >= 0.7 * len(line_n):
        return True
    return any(0.6 * len(line_n) <= len(r) <= 1.6 * len(line_n) and SequenceMatcher(None, line_n, r, autojunk=False).ratio() >= 0.8
               for r in ref_lines)


KEY_PATTERNS = [
    ("金额", re.compile(r"[¥￥$]\s*[:：]?\s*\d[\d,，]*(?:\.\d+)?|\d[\d,，]*(?:\.\d+)?\s*(?:元|万元|美元|USD|RMB)")),
    ("日期", re.compile(r"\d{4}\s*[年./-]\s*\d{1,2}\s*[月./-]\s*\d{1,2}\s*日?")),
    ("编号 / 账号", re.compile(r"(?<![0-9A-Za-z])(?:\d{8,}|[0-9][0-9A-Z]{17})(?![0-9A-Za-z])")),
]


def _alnum(s):
    return re.sub(r"[^0-9A-Za-z]", "", unicodedata.normalize("NFKC", s or "")).upper()


def key_fields(text):
    out = []
    for kind, rx in KEY_PATTERNS:
        for m in rx.finditer(unicodedata.normalize("NFKC", text or "")):
            v = m.group(0).strip()
            k = _alnum(v)
            if len(k) >= 3 and (kind, v) not in out:
                out.append((kind, v))
    return out


# ---------- merge ----------
def merge_page(primary, check, ptier="advanced", ctier="basic"):
    """One page → (markdown, review items)."""
    review, notes = [], []
    p_plain, c_plain = plain(primary), plain(check)
    pn, cn = norm(p_plain), norm(c_plain)
    body_blocks = primary
    if check and (not primary or (len(cn) > 200 and len(pn) < 0.7 * len(cn))):
        # primary lost much of the page (layout mix-up): body from the check tier; seal text still from the primary
        notes.append(f"（{ptier} 档本页结果不完整，正文采用 {ctier} 档识别，可能有个别错字）")
        body_blocks = [b for b in check if b.get("type") not in ("image", "seal")] + \
                      [b for b in primary if b.get("type") in ("image", "seal", "aside_text")]
        p_plain, c_plain = c_plain, p_plain
        pn, cn = cn, pn
        ptier, ctier = ctier, ptier
    parts = [t for t in (block_text(b) for b in body_blocks) if t]
    if check and body_blocks is primary:
        # lines only the check tier saw (often text under a red seal): added, flagged
        p_lines = [norm(x) for x in p_plain.splitlines() if x.strip()]
        for b in check:
            if b.get("type") not in TEXT_TYPES:
                continue
            t = " ".join(fix_text(b.get("content")).split())
            if t and not _present(norm(t), pn, p_lines):
                review.append(f"仅 {ctier} 档识别到（可能被印章遮挡，可能有错字）：{t}")
    if check:
        p_alnum, c_alnum = _alnum(p_plain), _alnum(c_plain)
        for kind, v in key_fields(p_plain):
            if _alnum(v) not in c_alnum:
                review.append(f"{kind}「{v}」只在 {ptier} 档中出现，{ctier} 档没有读到相同的值")
        for kind, v in key_fields(c_plain):
            if _alnum(v) not in p_alnum:
                review.append(f"{kind}「{v}」只在 {ctier} 档中出现")
    md = "\n\n".join(notes + parts)
    if review:
        md += "\n\n> ⚠ 需核对：\n" + "\n".join(f"> - {r}" for r in review)
    return md, review


def page_text(ocr, page_no):
    """Markdown of scanned page `page_no` (1-based) from the core's `ocr` dict; None when it is not a scanned page."""
    if not ocr or page_no not in (ocr.get("pages") or []):
        return None
    if ocr.get("unavailable"):
        return f"[第 {page_no} 页是扫描件，没有文字层；文档识别不可用：{ocr['unavailable']}]", []
    cache = ocr.setdefault("_cache", {})
    if "p" not in cache:
        cache["p"], cache["c"] = load_pages(ocr.get("primary")), load_pages(ocr.get("check"))
    idx = page_no - 1
    md, review = merge_page(cache["p"].get(idx, []), cache["c"].get(idx, []), ocr.get("primary_tier", "advanced"), ocr.get("check_tier", "basic"))
    return md or "[本页没有识别出文字]", review


def md_to_docx(md, out, title=None):
    """Markdown from extract_text (page markers, headings, pipe tables, ⚠ review quotes) → Word document."""
    import docx
    from docx.shared import Pt, RGBColor
    d = docx.Document()
    st = d.styles["Normal"]
    st.font.name, st.font.size = "Microsoft YaHei", Pt(10.5)
    st.element.get_or_add_rPr().get_or_add_rFonts().set(docx.oxml.ns.qn("w:eastAsia"), "Microsoft YaHei")
    if title:
        d.add_heading(title, 0)
    lines, i, first_page = (md or "").splitlines(), 0, True
    while i < len(lines):
        line = lines[i].rstrip()
        m = re.match(r"<!--\s*Page\s+(\d+)(.*?)-->", line)
        if m:
            if not first_page:
                d.add_page_break()
            first_page = False
            d.add_heading(f"第 {m.group(1)} 页" + ("（扫描件识别）" if "OCR" in m.group(2) else ""), 2)
        elif line.startswith("<!--"):
            p = d.add_paragraph(line.strip("<!-> ").strip())
            p.runs[0].italic = True
            p.runs[0].font.color.rgb = RGBColor(0x80, 0x80, 0x80)
        elif line.startswith("|"):
            block = []
            while i < len(lines) and lines[i].strip().startswith("|"):
                block.append(lines[i]); i += 1
            rows = md_grid("\n".join(block))
            if rows:
                w = max(len(r) for r in rows)
                t = d.add_table(rows=len(rows), cols=w)
                t.style = "Table Grid"
                for r, row in enumerate(rows):
                    for c in range(w):
                        t.cell(r, c).text = row[c] if c < len(row) else ""
                        if r == 0:
                            for run in t.cell(r, c).paragraphs[0].runs:
                                run.bold = True
            continue
        elif line.startswith(">"):
            txt = line.lstrip("> ").strip()
            if txt:
                p = d.add_paragraph(txt.lstrip("- "), style="List Bullet" if txt.startswith("- ") else None)
                for run in p.runs:
                    run.font.color.rgb = RGBColor(0xC0, 0x5A, 0x00)
                    run.bold = txt.startswith("⚠")
        elif line.startswith("#"):
            lvl = len(line) - len(line.lstrip("#"))
            d.add_heading(line.lstrip("# ").strip(), min(3, lvl + 1) if lvl > 0 else 3)
        elif line.strip():
            d.add_paragraph(line.strip())
        i += 1
    d.save(out)
    return out


def header(ocr, label="扫描件"):
    tiers = ocr.get("primary_tier", "advanced") + (f" 为主、{ocr.get('check_tier')} 核对" if ocr.get("check") else "")
    return f"<!-- {label} OCR：第 {range_str(ocr.get('pages') or [])} 页由文档识别服务（MinerU {ocr.get('server', '')}，{tiers}）识别 -->"


def tables(ocr, pages=None):
    """Tables of the scanned pages (primary tier, else check tier) as grids."""
    if not ocr or ocr.get("unavailable"):
        return []
    out = []
    p, c = load_pages(ocr.get("primary")), load_pages(ocr.get("check"))
    for n in ocr.get("pages") or []:
        if pages and n not in pages:
            continue
        blocks = p.get(n - 1) or c.get(n - 1) or []
        for b in blocks:
            if b.get("type") == "table":
                g = table_grid(b.get("content"))
                if len(g) >= 2:
                    out.append(g)
    return out
