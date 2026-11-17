# 20 · Rust 核心 `datapilot-core`（ADR-0012 M1）

纯 Rust 实现的 agent 核心，独立进程，通过 stdio JSON-RPC 与外壳通信。Swift 版核心（M2）与 SwiftUI 界面（M4-D）都已删除，行为由黄金样本回归保证。

## 构建与测试
```bash
cd datapilot-core
cargo test            # 单元测试 + 黄金样本回放（需要 build/vendor/python）
cargo build --release # target/release/datapilot-core（约 3.4 MB）
```
依赖只有 serde / serde_json / ureq（native-tls，走系统钥匙串的证书）/ regex，没有异步运行时：每个回合一个线程，并行工具用 `std::thread::scope`。

## 与 Swift 共用的东西（单一来源）
| 内容 | 位置 |
|---|---|
| 系统提示词、计划模式、压缩、项目记忆说明、子 Agent 通用提示、会话笔记模板与更新指令、记忆召回 / 抽取 / 整合指令 | `Resources/prompts/*.md` |
| 工具名、描述、参数、风险、并发与计划模式标记 | `protocol/tools.snapshot.json`（Rust 启动时读取，只实现执行函数） |
| 事件格式 | `protocol/agent-event.schema.json`（Rust 直接输出 wire JSON） |
| Python 数据内核、沙箱策略 | `Resources/kernel/`、`Resources/sandbox/*.sbpl` |
| 会话目录、`transcript.jsonl`、项目记忆、`connections.json`、`catalog.json`、`config.json` | 文件格式完全相同，两个核心可以互相打开对方的会话 |
| 密钥 | macOS 钥匙串同一服务名（`io.github.datapilot.apikeys` / `dbpasswords`） |

## 模块
| 文件 | 内容 |
|---|---|
| `llm.rs` | OpenAI 兼容客户端：请求体（思考参数 5 种风格、reasoning 回放、hermes 文本工具调用）、SSE 流、`<think>` 拆分、工具调用累积与 JSON 修复、重试（流式请求 90 s 无数据视为卡住并重试）、代理 |
| `agent.rs` | 回合循环：`<context>` 注入替换、skill 显式调用、计划模式工具过滤、重复调用提示、并行批次（最多 10）、参数校验、权限门（本地模式 → 联网卡 → 模式/规则 → 审批卡）、输出截断、压缩 |
| `permissions.rs` | 4 种模式 × 4 种风险、allow/ask/deny 规则（`domain:`、前缀、子串）、计划模式拦截卡 |
| `kernel.rs` | sandbox-exec 启动内核、JSON lines、超时先取消后重启、按会话 `ns` 取消、数据库端口规则与按需重启 |
| `tools/` | bi 的 calcFields / measures / params 预检在 QuickJS（rquickjs）里跑 `Resources/web/expr.js`，与渲染端、Swift 版同一份引擎；data（导入/结构/SQL/python/派生表/导出/关系/语义层/图表）、bi（看板创建/修改，校验与自动排版）、db（6 个数据库工具，密码按次注入）、browser（CDP 驱动无头 Chrome，每会话一个标签页、页面脚本与 Swift BrowserJS 相同、域名白名单、截图仅视觉模型附图）、web（web_fetch：代理/PAC、正文提取、表格存 CSV）、agent（记忆、todo、提问、计划、skill、save_skill、history_search、文件、grep/glob、bash） |
| `session.rs` / `compact.rs` / `memory.rs` / `prompt.rs` | 审批与提问的等待、transcript 读写与压缩恢复、尾部切分与微压缩、主题记忆文件、DATAPILOT 分层指令与 skills 目录 |
| `subagent.rs` | 子 Agent：内置 explore / analyst / researcher 与自定义 Markdown 类型、模型选择、独立 Python 命名空间、全局并发上限、子 Agent 记忆；每次工具调用都走主会话的权限门 |
| `notes.rs` / `recall.rs` | 会话笔记（快速模型按阈值改写；压缩时笔记有效就代替总结调用）；记忆选择式召回：项目层 + 用户层（`AppSupport/memory/`，键前缀 `user:`，冲突时项目优先），与主模型并行、≤5 条、60KB/会话、`[mem:文件]` 引用按层计数 |
| `extract.rs` | 回合后后台记忆抽取（与主模型写记忆互斥，条目格式：规则 + 依据 + 状态 + How to apply）；后台任务一次只跑一个，新请求替换待处理的 |
| `dream.rs` | 项目记忆「做梦」整合：三种触发（Due：≥24 小时且 ≥5 个其他会话，回合后检查；Idle：≥6 小时且记忆有变，server 每 15 分钟检查；Manual：`/dream`）；锁文件 + 快照，失败回滚，保留 `<dir>.prev` 供 `/dream undo`；`MEMORY.md` 顶部 core 块（≤8 行）；依据中的 SQL 只读重跑核对（每次 ≤10 条）；会话摘录（≤10 个会话 / 12k 字，抹掉密钥）；非手动触发时删除过半则放弃；每次运行记入 `.dreams.jsonl` |
| `tools/computer.rs` | Computer Use（移植自 Swift `ComputerTool`，不依赖外壳）：AX 控件树 + ref（ApplicationServices FFI）、CGEvent 键鼠、`screencapture` + `sips` 截图（主显示器，≤1280 宽）、前台应用用 `lsappinfo`；默认关闭（`computerUseEnabled` 为假时不提供给模型）；改变状态的动作为 external 风险，每轮上限 `computerMaxStepsPerTurn`；敏感 App 黑名单；计划模式与子 Agent 不可用；坐标模式 relative1000 / absolutePixels（qwen2.5-vl） |
| `mcp.rs` | MCP 客户端（与 Swift `MCPClient` 同一配置 `settings.mcpServers`）：stdio（JSON lines，应答服务端 ping，拒绝 sampling/roots，取消时发 `notifications/cancelled`）与 streamable HTTP（JSON 或 SSE 应答、`Mcp-Session-Id`、走用户代理）；initialize → tools/list（分页）→ tools/call；工具名 `mcp__<server>__<tool>`，一律 external 风险（ask 模式需审批，规则可写 `mcp__server__*`），HTTP 服务器还要过联网开关；输出包在 `<untrusted_tool_output>`；启动时后台连接 |
| `source_sync.rs` | 项目源文件变化检测（与 Swift `SourceSync` 同一套 `sources.json` / `source_state.json`）：每个回合开始时检查；修改的文件按原导入参数自动重导，并重建派生表（`rebuild_derived`）、复核已确认关系（`relation_revalidate`）；新增文件和删除文件用提问卡问用户；追加导入的文件只提示；同一项目同时只跑一次；中途停止则下回合再问 |
| `chrome.rs` | Chrome for Testing 自动下载（web_search 找不到 Chrome 时弹审批卡 `chrome_install`；只接受 Google 官方索引 + `storage.googleapis.com/chrome-for-testing-public/` 的 HTTPS 地址，校验大小与 bundle id，装到 `AppSupport/chrome`，与 Swift 版同目录）；无头 Chrome（DevTools 协议，标准库 TcpStream 上的最小 WebSocket 客户端）：web_search 的 Google 结果、web_fetch 渲染、chart_create 的 PNG；独立配置目录，闲置 10 分钟关闭 |
| `server.rs` / `main.rs` | app-server |

## app-server 协议（每行一个 JSON-RPC 2.0 对象）
请求：`initialize`、`session/open {id?, project_root?, mode?}`、`session/close`、`turn/start {session, text, skill?}`、`turn/cancel`、`approval/respond {session, id, choice: allowOnce|allowSession|allowAlways|deny, feedback?}`、`question/answer {session, id, answer}`（计划卡回答 `approve` / `approve_auto` / 修改意见）、`turn/interject {session, text, id?}`（回合进行中补充消息：排队，在下一次模型调用前作为用户消息送达并发 `interjection_delivered`）、`turn/withdraw {session, id}`（撤回未送达的）、`turn/take_pending {session}`（回合先结束时取回未送达的，由界面作为新回合发送）、`mcp/sync {servers?}`（重连 MCP 服务器，返回各服务器状态与工具列表）、`mcp/status`、`mode/set`、`network/set`、`compact`、`memory/dream {session, undo?}`（立即整合项目记忆 / 撤销上次整合，返回 `{text}`；回合进行中时报错）、`shutdown`。

M2 新增（界面接入用）：`kernel/call {session, op, params?, timeout?}`（只放行界面用到的 op 白名单 `server::UI_KERNEL_OPS`，不含 python / sql；db_* 由核心从钥匙串注入密码）、`kernel/tasks`、`kernel/cancel_task {id}`、`kernel/restart`、`kernel/status`、`session/rewind {session, turn_id}`（非项目会话同时恢复工作区快照，`snapshots.rs`）、`session/btw {session, question}`（回合进行中也可用，读磁盘上的 transcript）、`model/set {session, model?, thinking?}`、`config/reload`（重读 config.json、重连 MCP、空闲会话按同一 id 重开）、`llm/fast {system, user}`、`qsql/feedback {session, question, sql, up}`、`project/close {root}`。`memory/qsql {action: list|update|delete}`、`memory/datasets {action: list|rename|delete}`（记忆面板，无需 session）；`dashboard/query {session, id, dataset, cap?}`（看板面板取数：SQL 取自看板 spec 文件，不接受界面传入的 SQL；行数据写临时文件 `rows_file`）。新事件 `title_changed {title}`。审批卡选「始终允许」时，规则由核心追加写回 `config.json` 的 `rules`（`permissions::persist_rule`）。browser 工具每步后存一张截图到 `<workspace>/browser/latest.jpg`，路径放在 display 的 `screenshot` 字段，供界面浏览器页显示。可能阻塞的请求（模型调用、长内核操作）在工作线程里应答，读循环不被卡住。空闲检查每 5 分钟一次：按 `kernelIdleSeconds` 关闭没有回合在跑的内核，并做 Idle 做梦检查。聊天里输入 `/dream`、`/dream undo` 效果相同，不经过模型。

通知：`agent/event {session, event}`，`event` 即 schema 中的事件。

环境变量：`DP_RESOURCES`、`DP_PYTHON_ROOT`（不设时用仓库开发目录）、`DP_APP_SUPPORT`（默认 `~/Library/Application Support/DataPilot`）、`DP_API_KEY`（测试用；默认读钥匙串）。

## 代理
与 Swift 版一致：模型服务的代理设为「跟随全局」且全局为「系统」时，按 macOS 系统设置解析——先读 PAC（`scutil --proxy` 的自动配置 URL，支持 `shExpMatch` / `dnsDomainIs` / `host ==` 分支），再退回 HTTPS 代理与例外列表。DKU 网络下 PAC 对 `aichat.dukekunshan.edu.cn` 返回 DIRECT，直接用 HTTPS 代理反而连不上。

## 尚未移植（见 TODO）
Google 人机验证窗口、全局 SQLite 记忆（Rust 写 `AppSupport/memory/` 主题文件）。history_search 用扫描 transcript 代替 SQLite 全文索引。未移植的工具在 Rust 版不注册，模型看不到。


## 界面客户端
M2 期间的 SwiftUI 客户端（CoreProcess / RemoteSession）已在 M4-D 删除（2026-11-15，备份 `build/backup/pre-swiftui-removal-20261115.tgz`）。现在唯一的界面是 `apps/desktop`（Tauri）：`src-tauri/src/core.rs` 启动核心、按 id 配对请求、崩溃后自动重启并重开会话；会话 / 项目索引由核心的 `session/*`、`project/*` RPC 直接读写 `app.sqlite`。

## 平台层（M3）
- `platform/`：数据目录、python / ssh 路径、系统代理（macOS scutil+PAC；Windows WinHTTP IE 设置 + PAC/WPAD）、时区、废纸篓/回收站、目录复制、`canonicalize`（去掉 Windows `\\?\` 前缀）、`contain_children`（Windows Job Object）。
- `sandbox/`：Seatbelt（macOS）与 AppContainer + Job（Windows），见 docs/05。
- `secrets.rs`：`system_store()` → macOS 钥匙串 / Windows 凭据管理器。
- Windows 交叉检查：`cargo xwin check --release --target x86_64-pc-windows-msvc`。
