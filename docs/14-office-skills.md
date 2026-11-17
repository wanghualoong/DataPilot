# 14 · Word / Excel / PPT / PDF

## 内置 skills
`Resources/skills/{docx,xlsx,pptx,pdf}` 来自 anthropics/skills（commit `2c5958e`，Apache-2.0，各目录保留 `LICENSE.txt`）。仅在 SKILL.md 开头加了一段“DataPilot environment”说明（按 Apache §4 标注已修改）：
- 优先用内置 Python 库：python-docx、python-pptx、openpyxl、XlsxWriter、pypdf、pdfplumber、pypdfium2、reportlab、lxml、defusedxml。
- node（docx-js / pptxgenjs）、LibreOffice（公式重算、格式转换）、pandoc、Poppler、markitdown **不打包**（GPL/MPL 或体积大）；本机装了才用，否则跳过并说明。
- 同名时内置 skill 优先于 `~/.claude/skills`、`~/.codex/skills` 中的外部 skill。

## 读取与导入
- `read_file` 直接读 .docx / .pptx / .pdf / .xlsx：内核 `extract_text` 操作（`Resources/kernel/dataagent/office.py`）转成 Markdown（标题、列表、表格；PPT 按页 `<!-- Slide N -->`；PDF 可用 `pages: "1-5"`）。
- `data_import` 可导入 .docx / .pdf / .pptx 中的表格（每个表格一张 DuckDB 表，`<文件名>_t<序号>`）。

## 体积
内置 Python 从 305MB 增至约 396MB（主要是 lxml、Pillow、cryptography、pypdfium2）。markitdown 因依赖 onnxruntime（约 60MB）未内置。

## 图表导出 PNG（chart_create save_png）
`chart_create` 的可选参数 `save_png: "文件名.png"`：用内置的 ECharts 渲染页（`Resources/web/index.html`）在 headless Chrome 里画同一张图，按 2 倍分辨率导出 PNG，保存到 `exports/`（项目模式下为 `outputs/`）。导出的图片可以直接插入 Word / PPT（docx / pptx skill）。测试：`DP_LIVE_CHROME=1 swift test --filter liveChartPNG`。
