# 00 · 项目概览

DataPilot 是一个 Mac / Windows 桌面数据分析 Agent 应用（Tauri 2 界面 + Rust 核心 + Python 内核），定位类似 Codex / Claude Code 的桌面版，但专注数据分析：

- 用自然语言分析 Excel / CSV / Parquet，多表自动推断关系，用 DuckDB 做 Text-to-SQL；
- 生成可交互 BI 看板（筛选、维度/指标/图表类型切换、联动、导出离线 HTML）；
- 支持 New API 中转的 Qwen / DeepSeek 等 OpenAI 兼容模型（含本地 vLLM / Ollama）；检索与 Codex / Claude Code 一样只用关键词 + grep；
- Agent Loop + 工具 + 权限 + Skills + 短期/长期记忆 + Plan 模式；
- 所有代码在 macOS Seatbelt 沙箱内执行，每个会话一个持久工作区，重开会话秒级恢复。

## 文档索引
| 文档 | 内容 |
|---|---|
| [01-architecture](01-architecture.md) | 总体架构、目录结构、运行时数据位置 |
| [02-agent-loop](02-agent-loop.md) | Agent 循环、事件流、上下文组装与压缩 |
| [03-tools-permissions](03-tools-permissions.md) | 工具清单、并发调度、权限模式与规则 |
| [04-memory](04-memory.md) | 四层记忆、习惯学习、短期与长期结合 |
| [05-sandbox-resume](05-sandbox-resume.md) | Seatbelt 沙箱、持久内核、会话恢复与快照 |
| [06-data-engine](06-data-engine.md) | 导入、探查、关系推断、Text-to-SQL、阈值 |
| [07-bi-dashboard-spec](07-bi-dashboard-spec.md) | DashboardSpec 规格、渲染器、导出 |
| [08-llm-providers](08-llm-providers.md) | New API / Qwen / DeepSeek / vLLM 配置与坑 |
| [09-build-release](09-build-release.md) | 构建、打包 DMG、签名与公证 |
| [10-references](10-references.md) | 参考项目与设计借鉴 |
| [11-browser-computer-use](11-browser-computer-use.md) | 浏览器、MCP、Computer Use |
| [12-projects](12-projects.md) | 项目（文件夹）、共享数据库、只读原始文件 |
| [13-subagents](13-subagents.md) | 子 agent（并行、权限冒泡） |
| [14-office-skills](14-office-skills.md) | Word / Excel / PPT / PDF skills |
| [15-web-search](15-web-search.md) | Chrome + CDP 联网搜索 |
| [user-guide](user-guide.md) | 用户使用手册 |
| [TODO](TODO.md) | 暂缓 / 待办事项（Computer Use、模型速度） |
| [adr/](adr/) | 关键架构决策记录 |
