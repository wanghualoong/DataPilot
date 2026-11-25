"""Scanned pages: scan detection, merging the primary (advanced) and check (basic) tiers, tables, kernel ops."""
import os, sys, tempfile
sys.path.insert(0, os.path.dirname(__file__))
from test_kernel import Kernel, PY  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "Resources", "kernel"))
from dataagent import ocr  # noqa: E402

TABLE = "<table><tr><td>项目</td><td>金额</td></tr><tr><td rowspan='2'>服务费</td><td>¥12,000.00</td></tr><tr><td>¥3,000.00</td></tr></table>"
PRIMARY = {0: [
    {"type": "doc_title", "content": "技术服务合同"},
    {"type": "text", "content": "甲方：示例科技有限公司 合同编号 HT20260316"},
    {"type": "text", "content": "签订日期 2026年3月16日，总价 15000 元"},
    {"type": "table", "content": TABLE},
    {"type": "image", "content": "示例科技有限公司 合同专用章"},
    {"type": "image", "content": ""},
    {"type": "page_number", "content": "1"},
]}
CHECK = {0: [
    {"type": "title", "content": "技术服务合同"},
    {"type": "text", "content": "甲方：示例科技有限公司 合同编号 HT20260316"},
    {"type": "text", "content": "签订日期 2026年3月18日，总价 15000 元"},
    {"type": "text", "content": "地址：示例大道8号 甲方（盖章）"},
    {"type": "table", "content": TABLE},
]}


def main():
    fails = []

    def check(c, m):
        print(("PASS " if c else "FAIL ") + m)
        if not c:
            fails.append(m)

    check(ocr.page_list("1-3,7", 5) == [0, 1, 2] and ocr.page_list(None, 2) == [0, 1], "page_list")
    check(ocr.range_str([1, 2, 3, 5, 7, 8]) == "1-3,5,7-8", "range_str")
    g = ocr.html_grid(TABLE)
    check(g == [["项目", "金额"], ["服务费", "¥12,000.00"], ["服务费", "¥3,000.00"]], f"rowspan expanded: {g}")
    check(ocr.fix_text("a@x.comb@y.com") == "a@x.com b@y.com" and ocr.fix_text("A &amp; B") == "A & B", "fix_text")

    md, review = ocr.merge_page(PRIMARY[0], CHECK[0])
    check("# 技术服务合同" in md and "| 服务费 | ¥12,000.00 |" in md, "title + table markdown")
    check("【印章 / 图片中的文字（OCR）：示例科技有限公司 合同专用章】" in md and "未识别出文字，请对照原页" in md, "seal text and empty image region marked")
    check(any("示例大道8号" in r and "仅 basic" in r for r in review), f"line only in check tier flagged: {review}")
    check(any("2026年3月16日" in r for r in review) and any("2026年3月18日" in r for r in review), "differing dates flagged both ways")
    check(not any("15000" in r or "HT20260316" in r for r in review), "agreeing fields not flagged")
    check("> ⚠ 需核对：" in md and "\n1\n" not in md, "review list appended, page number dropped")

    # primary lost most of the page → body from check tier, seal still from primary
    long_check = [{"type": "text", "content": "第一条 " + "服务内容说明" * 40}, {"type": "image", "content": "basic印章"}]
    md2, _ = ocr.merge_page([{"type": "text", "content": "第一条"}, {"type": "image", "content": "advanced印章"}], long_check)
    check("正文采用 basic 档识别" in md2 and "advanced印章" in md2 and "basic印章" not in md2, "fallback to check-tier body")

    o = {"pages": [1], "primary": PRIMARY, "check": CHECK, "primary_tier": "advanced", "check_tier": "basic", "server": "4.2.3"}
    t, rv = ocr.page_text(o, 1)
    check(t.startswith("# 技术服务合同") and rv and ocr.page_text(o, 2) is None, "page_text")
    check(ocr.page_text({"pages": [1], "unavailable": "连不上"}, 1)[0].startswith("[第 1 页是扫描件"), "unavailable message")
    check(ocr.tables(o) == [g], "tables from scanned pages")

    # kernel: image-only PDF detected as scanned, extract_text / import / render use the core's OCR results
    ws = os.path.realpath(tempfile.mkdtemp(prefix="dpws"))
    from PIL import Image, ImageDraw
    img = Image.new("RGB", (600, 800), "white")
    ImageDraw.Draw(img).rectangle([50, 50, 550, 200], outline="black")
    img.save(os.path.join(ws, "scan.pdf"))
    img.save(os.path.join(ws, "photo.png"))
    k = Kernel(ws)
    r = k.call("ocr_scan", path="scan.pdf")
    check(r["ok"] and r["result"]["scanned"] == [1] and r["result"]["range"] == "1", f"ocr_scan: {r}")
    r = k.call("ocr_scan", path="photo.png")
    check(r["ok"] and r["result"].get("image"), "image = one scanned page")
    wire = {"pages": [1], "primary": {"0": PRIMARY[0]}, "check": {"0": CHECK[0]}, "primary_tier": "advanced", "check_tier": "basic", "server": "4.2.3"}
    r = k.call("extract_text", path="scan.pdf", ocr=wire)
    res = r.get("result") or {}
    check(r["ok"] and "扫描件 OCR" in res.get("text", "") and "技术服务合同" in res["text"] and res["ocr_review"].get("1"), f"extract_text via OCR: {r.get('error')}")
    r = k.call("extract_text", path="photo.png", ocr=wire)
    check(r["ok"] and "合同专用章" in r["result"]["text"], f"image extract_text: {r.get('error')}")
    r = k.call("extract_text", path="photo.png")
    check(not r["ok"] and "文档识别" in (r.get("error") or ""), "image without OCR → clear error")
    r = k.call("import", path="scan.pdf", ocr=wire, table="fee")
    check(r["ok"] and r["result"]["tables"][0]["rows"] == 2, f"import OCR table: {r.get('error')}")
    r = k.call("import", path="scan.pdf", ocr={"pages": [1], "unavailable": "连不上 VPN"})
    check(not r["ok"] and "连不上 VPN" in (r.get("error") or ""), "import with OCR unavailable → message")
    r = k.call("ocr_render", path="scan.pdf", page=1)
    check(r["ok"] and os.path.exists(r["result"]["path"]) and r["result"]["total"] == 1, f"ocr_render: {r.get('error')}")
    r = k.call("ocr_export", path="scan.pdf", ocr=wire, format="docx")
    ok = r["ok"] and os.path.exists(r["result"]["path"]) and r["result"]["path"].endswith(".docx")
    if ok:
        import docx
        d = docx.Document(r["result"]["path"])
        txt = "\n".join(p.text for p in d.paragraphs)
        ok = "第 1 页" in txt and "技术服务合同" in txt and "需核对" in txt and len(d.tables) == 1 and d.tables[0].cell(1, 1).text == "¥12,000.00"
    check(ok, f"ocr_export docx: {r.get('error')}")
    r = k.call("ocr_export", path="scan.pdf", ocr=wire, format="md")
    check(r["ok"] and open(r["result"]["path"], encoding="utf-8").read().count("技术服务合同") >= 1, "ocr_export md")
    r = k.call("ocr_merge", ocr=wire)
    check(r["ok"] and r["result"]["pages"][0]["review"], "ocr_merge")
    k.close()

    print(f"\n{'ALL PASS' if not fails else f'{len(fails)} FAIL'}")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
