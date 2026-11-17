# 01 · 架构

```
┌──── 界面：Tauri 2（apps/desktop，React + TS；macOS WKWebView / Windows WebView2）────┐
│  侧栏(项目/会话/搜索) │ 聊天(流式/工具卡/审批/计划/提问) │ 检查器 │ 设置               │
├──────────── stdio JSON-RPC（protocol/rpc.schema.json + agent-event.schema.json）────┤
│ datapilot-core（Rust）：agent 循环、工具、权限、会话 / 压缩 / 记忆、MCP、Chrome CDP、 │
│   SSH 隧道、UI 数据层（ui.rs：app.sqlite 索引、config、看板、skill、文件）             │
│   platform/（macOS / Windows 差异）  sandbox/（Seatbelt / AppContainer + Job）        │
│   secrets（macOS 钥匙串 / Windows 凭据管理器）                                       │
├──────── 沙箱（只能写工作区 / tmp / outputs，默认无网络）──────────────────────────────┤
│ 内置 Python 3.12（python-build-standalone）+ pandas/duckdb/pyarrow…                 │
│ Resources/kernel/kernel.py：JSON-lines 持久内核（dataagent 包，请求并发）            │
└───────────────────────────────────────────────────────────────────────────────────────┘
图表 / 看板 / ER 图：iframe 加载 Resources/web（本地 ECharts），host.js 走 postMessage
```

## 目录结构
```
datapilot-core/             Rust 核心（cargo test；Windows 交叉检查 cargo xwin check）
apps/desktop/               Tauri 界面：src/（React）、src-tauri/（外壳，启动核心）、e2e/、scripts/build_mac.sh、build_win.sh
protocol/                   RPC / 事件 / 工具契约 schema
Resources/kernel/           Python 内核：kernel.py + dataagent/{engine,importer,profile,sqlguard,util}
Resources/sandbox/          Seatbelt 策略 base.sbpl / network.sbpl
Resources/web/              dashboard.js / dashboard.css / index.html / vendor/echarts.min.js
Resources/skills/           内置 Skills（SKILL.md）
scripts/                    fetch_vendor / build_venv / build_dmg（Mac DMG，基于 Tauri 版）/ golden_llm_server / mock_llm_server
tests/python/               内核端到端测试（真实沙箱）
tests/eval/                 Text-to-SQL 评测集
docs/                       本文档
```

## 运行时数据
`~/Library/Application Support/DataPilot/`（Windows：`%APPDATA%\\DataPilot\\`）
| 路径 | 内容 |
|---|---|
| `config.json` | Provider、模型角色、设置、权限规则（API Key 在钥匙串 / 凭据管理器） |
| `app.sqlite` | 会话索引、消息全文索引、记忆、事件、情景、Q-SQL、数据集指纹 |
| `sessions/<id>/transcript.jsonl` | 追加式会话记录（消息、压缩点、展示数据、任务） |
| `sessions/<id>/workspace/` | 沙箱工作区：uploads/ data.duckdb artifacts/ checkpoints/ dashboards/ exports/ notes.md catalog.json cells.jsonl |
| `sessions/<id>/snapshots/` | 每轮工作区快照（APFS clone，用于回退） |
| `skills/` | 用户 Skills |

## 关键设计选择
见 [adr/](adr/)，跨平台见 ADR-0012。要点：Rust 核心（单一实现，多个界面）+ 沙箱内 Python 数据内核（复用生态、隔离）；DuckDB 作为嵌入式分析库；看板用语义 Spec 而不是让模型直接写 ECharts。


## 项目（v0.1 增补）
核心根据会话所属项目解析路径（项目会话在 `<项目>/.datapilot/`），按项目共享 `kernel::KernelProcess`，`KernelClient` 是带 namespace 的会话句柄。见 `12-projects.md`、ADR 0005。
