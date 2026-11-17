# Third-party notices

DataPilot is licensed under the Apache License 2.0 (see `LICENSE`). It includes or adapts the following third-party work:

| Component | Where | License |
|---|---|---|
| openai/codex — `seatbelt_base_policy.sbpl` (adapted) | `Resources/sandbox/*.sbpl` | Apache-2.0 |
| OtterMind/Chat2DB Community v5.3.8 — chart display options (`handleOption.ts`), export cell-processor chain, ER node sizing / stored layout with virtual edges, database connection fields / catalog queries / read-only execution policy (adapted; files marked "Adapted from Chat2DB") | `Resources/web/dashboard.js`, `Resources/web/er.js`, `Resources/kernel/dataagent/engine.py (op_export)`, `Resources/kernel/dataagent/connectors.py`, `Resources/kernel/dataagent/dbops.py` | LicenseRef-Chat2DB (`Resources/about/CHAT2DB_LICENSE`): internal / educational self-hosted use only; no external binary distribution |
| Apache ECharts | `Resources/web/vendor/echarts.min.js` (`ECHARTS_LICENSE`) | Apache-2.0 |
| python-build-standalone (CPython) | bundled `python/` runtime | PSF-2.0 / various (see runtime) |
| anthropics/skills — `docx`, `xlsx`, `pptx`, `pdf` (commit 2c5958ecac7cee2b583967a2c4981d0f4a75c579; SKILL.md modified with a DataPilot environment note) | `Resources/skills/{docx,xlsx,pptx,pdf}` (each keeps its `LICENSE.txt`) | Apache-2.0 |
| python-docx, python-pptx, pypdf, pdfplumber, pypdfium2, reportlab, XlsxWriter, lxml, defusedxml, Pillow | bundled Python site-packages | MIT / BSD / Apache-2.0 / HPND |
| pg8000 (BSD-3), PyMySQL (MIT), pymssql (LGPL-2.1, dynamically loaded; bundles FreeTDS, LGPL), python-oracledb (Apache-2.0 / UPL-1.0), clickhouse-connect (Apache-2.0) | bundled Python site-packages (database drivers) | as listed |
| pandas, numpy, pyarrow, duckdb, openpyxl, sqlglot, charset-normalizer, python-calamine, xlrd, python-dateutil, tzdata | bundled Python site-packages | BSD-3 / Apache-2.0 / MIT / MPL-2.0 (tzdata: Apache-2.0) |

Chrome for Testing is **not** bundled; when no Chromium-based browser is installed, DataPilot can download it on demand from Google's official index (googlechromelabs.github.io/chrome-for-testing) into the user's Application Support folder, under Google's own terms.

Concepts (not code) were taken from public documentation of Claude Code, independent analyses of its memory mechanism (claude-code-architecture-guide, the `docs/` of claude-code-memory-brief — design conclusions and thresholds only, no source), the Codex `codex-rs/memories` design (Apache-2.0), the open-source projects vanna, SQLBot, WrenAI, DB-GPT, microsoft/playwright-mcp and QwenLM/Qwen3-VL cookbooks; see `docs/10-references.md`.
