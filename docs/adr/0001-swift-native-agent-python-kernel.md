# ADR 0001：Swift 原生 Agent 核心 + 沙箱内 Python 数据内核
- 状态：已采纳（2026-11）
- 背景：需要原生 macOS 体验、DMG 分发、可靠沙箱与强数据处理能力。
- 决策：Agent loop / 工具 / 记忆 / 持久化用 Swift（AgentCore 包）；数据处理用内置 python-build-standalone + pandas + DuckDB，作为 sandbox-exec 下的持久 JSON-lines 内核。
- 备选：Electron + Node（体积大、非原生）；全部 Python 后端（多一层进程管理与打包复杂度）；Swift 直接链接 DuckDB C API（失去 pandas 生态）。
- 后果：两种语言；内核协议需维护；App 体积约 300MB。
