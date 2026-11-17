# 待办（TODO）

## Computer Use（暂缓，2026-11-10 列入）
现状：已实现 `computer` 工具（AX 控件树 + 截图坐标），默认关闭；询问 / 自动模式下每一步都要审批，计划模式禁止，子 agent 不可用；⌃⌥⎋ 急停；敏感 App 黑名单；每轮步数上限 `computerMaxStepsPerTurn` 已生效。详见 `11-browser-computer-use.md`。

待做：
- [ ] **真机验证**：授予「辅助功能」+「屏幕录制」后，用备忘录、Finder、Excel/Numbers 各走一遍（打开 App → 读控件树 → 点击/输入 → 截图回传）。
- [ ] **签名**：换成 Developer ID 签名。现在是临时签名，每次重新打包后系统都要重新授权。
- [ ] **控件树范围**：目前只读前台 App 的焦点窗口和菜单栏；支持指定 App / 窗口、多窗口和弹出层。
- [ ] **多显示器**：截图和坐标换算目前只按主显示器处理。
- [ ] **与 Codex 对齐**：对比 Codex / Claude computer use 的动作集和提示词（如窗口切换、按 App 名聚焦、剪贴板读写），补齐缺的动作。
- [ ] **审批体验**：同一 App 内的连续低风险动作（滚动、聚焦）支持「本 App 本轮允许」，减少逐步点批准。
- [ ] **端到端测试**：用测试 App 或 AX mock 做自动化测试，覆盖 open_app → ax_press → ax_set_value。
- [ ] **实测反馈（2026-11-12，Rust 核心）**：授权后截图、读控件树可用；整体操作效果一般，尤其是 Excel（单元格定位、输入）。
- [ ] **急停后卡忙**：⌃⌥⎋ 急停后界面一直显示“忙”，过一会才恢复；需查 turn/cancel 在模型流式输出 / computer 动作 / 审批等待时的退出路径。
- [x] **签名（本机）**：build_app.sh 无 Developer ID 时自动用 Apple Development 证书签名（App + datapilot-core），重新打包后授权不再丢失；“授权”按钮直接打开对应的系统设置页（2026-11-12）

## 模型速度（暂缓）
- [x] qwen3.8-27b-local 慢：网关已开启前缀缓存（2026-11-16 实测：34k 提示重复前缀命中 31k，首字 17–21 s → 2–3 s；输出约 20 tok/s 不变）。如仍嫌慢，可再做本地模型精简模式（少带工具、压短提示）。

## 第三轮遗留（2026-11-10）
- [x] 看板卡片「编辑」面板（按 Chat2DB ChartSchema：维度 / 度量 / 颜色分组 / 排序 / 显示开关）——计划在 web 层实现，Tauri 版直接复用
- [x] 看板 spec `version` 字段与迁移函数
- [x] `data_import` 先预览、后映射（列改名、选列、指定类型）
- [x] 项目记忆、关系图、导出在真实项目 + DeepSeek 上端到端实测（2026-11-10，`LiveRound3Tests`；修复了模型把关系一侧以 JSON 字符串传入的问题）
- [x] 数据库直连（PG/MySQL/SQL Server/Oracle/ClickHouse/SQLite/DuckDB，只读；抽取/刷新/实时查询；端口级沙箱放行），见 19-db-connections.md
- [x] 我的查询、SQL 格式化、执行计划、结果分页查看
- [x] 数据库工具真实模型实测（2026-11-10，`LiveDBTools`，DeepSeek：db_list → db_schema → db_query/db_extract，结果与真值一致；补了“画图前先抽取到本地”的提示）
- [ ] 数据库直连：在真实 PG / MySQL / SQL Server / Oracle / ClickHouse 服务器上端到端验证（本机无服务器，目前只测了 SQLite / DuckDB 文件）
- [x] SSH 隧道（2026-11-12，核心在沙箱外建隧道，口令走钥匙串 + SSH_ASKPASS；见 19-db-connections.md）
- [ ] 达梦驱动
- [x] M1 核心：Rust `datapilot-core`（agent 循环、权限、内核、工具、会话、压缩、app-server），3 个黄金样本与 Swift 结果一致（2026-11-11），见 20-rust-core.md
- [x] M1 补齐（第一批）：看板工具、history_search、save_skill、web_fetch（2026-11-11）
- [x] M1 补齐（第二批）：子 Agent（explore/analyst/researcher + 自定义类型 + 子 Agent 记忆；黄金样本 05 两核心一致）（2026-11-11）
- [x] M1 补齐（第三批）：会话笔记（阈值更新、压缩时直接用笔记代替总结调用）、项目记忆选择式召回与引用计数（2026-11-11）
- [x] M1 补齐（第四批）：后台记忆抽取、项目记忆每日整合（快照 + 失败回滚），提示词共用（2026-11-11）
- [x] M1 补齐（第五批）：web_search（Chrome → Google，失败回落 Bing）、web_fetch 的 Chrome 渲染、chart_create 存 PNG（自写最小 CDP/WebSocket 客户端，已用本机 Chrome 实测）（2026-11-11）
- [x] M1 遗留：browser（C5）、computer（C7）、Chrome for Testing 自动下载（C4）、看板表达式预检（C1）已移植；全局记忆存储已统一（M2-3）；Google 人机验证窗口已移植（Google 要求验证时用同一配置目录打开可见 Chrome 窗口，验证期间搜索走 Bing，窗口关闭后恢复 Google；App 退出时一并关闭）（2026-11-12）
- [x] M1 验证：真实模型（DeepSeek-V4.1-Flash，NetID 网关）经 app-server 端到端：导入 → SQL → 回答，结果正确（2026-11-11）
- [x] M1 补测：黄金样本 04（计划卡批准 → 提问卡回答 → 审批卡拒绝），两个核心结果一致（2026-11-11）
- [x] M1 补测：黄金样本 06（超阈值压缩后继续）、07（联网卡被拒），两个核心结果一致（2026-11-11）
- [x] 记忆 A1/A2：陷阱题 4 道（agent_tasks.json）；跨会话学习评估 `run_memory_eval.py`（8 步系列 + 10 条无关记忆 + 1 条 75 天前的过期冲突口径；`--prompts` 做提示词 A/B；输出记忆卫生指标：同主题文件数、仍主张旧口径的文件）（2026-11-12）
- [x] 记忆 B1/B2：提示词说明“记忆是干什么的”并加弱模型指引（先看相关记忆、pending/过期先核实、规则变了就改写旧条目）；条目格式 = 规则（保留用户原话）+ **依据** + **状态**（confirmed/pending）+ **How to apply**；新增 trap 类；抽取可用 `file` 更新已有条目。A/B：正确率持平（基线 6/8×3、v1 6/8×2，上限 6/8；记忆关 5/8——模型用 catalog_update 绕过了记忆开关，评估区分度不足，待加难）；GMV 同主题文件 基线 4/4/1 → v1 2/3（2026-11-12）
- [x] 记忆 B3 做梦整合（`datapilot-core/src/dream.rs`）：Due（≥24h 且 ≥5 个其他会话）/ Idle（≥6h 且记忆有变，server 每 15 分钟）/ Manual（`/dream`、RPC `memory/dream`）；MEMORY.md 顶部 core 块（≤8 行）；依据 SQL 只读重跑核对（≤10 条）；会话摘录（≤10 会话 / 12k 字，抹密钥）；每次新增 ≤3 个主题；非手动删除过半则放弃；快照回滚 + `<dir>.prev` 供 `/dream undo`；`.dreams.jsonl` 记录（2026-11-12）
- [x] 记忆 B4 用户层进召回（`recall.rs` `Layers`）：项目 + `AppSupport/memory/` 两层候选合并按修改时间取 200；用户层键为 `user:文件名`，召回、引用计数各写回本层 `.usage.json`；无项目时也能召回用户偏好；提示词说明冲突以项目记忆为准（2026-11-12）
- [x] C1 看板表达式预检（Rust）：`tools/bi.rs` `check_exprs` 用 rquickjs 运行 `Resources/web/expr.js` 的 `DPExpr.validate`（64MB 内存上限），dashboard_create / update 在字段检查前报出表达式错误（2026-11-12）
- [x] C2 项目源文件变化检测（Rust）：`source_sync.rs` + `Agent::sync_sources`，移植自 Swift `AgentSession+SourceSync`，两个核心读写同一组状态文件；修改的文件自动重导，并重建派生表、复核关系；新增和删除的文件通过提问卡询问（2026-11-12）
- [x] C3 `turn/interject`（Rust）：`Shared.interjections` 队列 + `turn/withdraw` / `turn/take_pending`；工具结果之后、或模型给出最终回答时送达（后者继续同一回合），行为与 Swift `deliverInterjections` 一致（2026-11-12）
- [x] C4 Chrome for Testing 自动下载（Rust）：`chrome::install` + `web_search` 审批卡；官方地址白名单、下载完整性与 bundle id 校验、走用户代理设置；拒绝或失败回落 Bing（2026-11-12）
- [x] C5 browser 工具（Rust）：`tools/browser.rs`，无头 Chrome + CDP 代替 WKWebView（不依赖外壳）；navigate/snapshot/click/type/select/scroll/back/extract_tables/screenshot；按动作分风险；`browserAllowedDomains`；会话关闭时关标签页；Chrome 闲置退出后按上次 URL 重开（2026-11-12）
- [x] C6 MCP 客户端（Rust）：`mcp.rs`，stdio + streamable HTTP，分页 tools/list、图片结果、取消、进程级管理（`mcp/sync`、`mcp/status`，启动时后台连接）；测试含本地 stdio 假服务器与 HTTP/SSE 假服务器（2026-11-12）
- [x] C7 Computer Use（Rust）：`tools/computer.rs`，AX / CGEvent FFI + screencapture；守卫与 Swift 版一致（默认关、每轮步数上限、黑名单、计划模式禁止、子 Agent 不可用）；单测覆盖按键映射、坐标换算、上限与只读系统查询，**未做真机点击测试**（2026-11-12）
- [x] 记忆评估加难：`--isolate-catalog`（清空语义层，口径只能靠记忆）+ 可学题计分；记忆关 0/8、记忆开 5/8 → 修流式卡住后复跑 6/8（可学 6/6，无过期口径残留）（2026-11-12）
- [x] 卫生指标 topic:* 改为只按主题名（name）计数，不再把描述里提到 GMV 的其他口径算进去；iso2 复算 topic:GMV = 1（2026-11-12）
- [x] `/dream` SQL 复核与会话摘录真实模型实测：2 个历史会话 + 2 条依据 SQL（1 条引用不存在的表）→ 按会话里的用户纠正改写退款率口径、新增金额单位条目、核心口径 3 条（2026-11-12）
- [x] 整合时顺带改名：update 可带 `rename`，文件名与内容不符时改名（如 `gmv-旧口径.md`→`gmv-口径.md`），已用真实模型实测，`/dream undo` 可恢复（2026-11-12）
- [x] `/dream` 进界面：设置 coreEngine=rust 后 `/dream`、`/dream undo` 可用（Swift 核心下提示需要 Rust 核心）
- [x] `/dream` 真实模型实测：3 条记忆 → 合并重复 GMV 条目、生成 2 行 core 块；`/dream undo` 恢复 3 条（2026-11-12）
- [x] 模型流卡住：流式请求单独用 90 s 读空闲超时（`llm::STREAM_IDLE`），超时按网络错误重试；非流式（快速模型、整合）仍为 300 s（2026-11-12）
- [x] M2-1 Rust 协议补齐：kernel/*、session/rewind（+ 工作区快照）、session/btw、model/set、config/reload、llm/fast、qsql/feedback、project/close、title_changed 事件、空闲内核回收（2026-11-12）
- [x] M2-2 Swift 客户端（CoreProcess / RemoteSession / RemoteKernel、AgentEvent wire 解码；ChatSessionAPI / KernelAPI 协议让界面同时支持两个核心；CoreClientTests 用真二进制 + 黄金模拟模型）（2026-11-12）
- [x] M2-3 App 接入（2026-11-12）：隐藏开关 `settings.coreEngine`、`/dream`、保存设置后 `config/reload`、看板取数走 `dashboard/query`（只执行 spec 里的 SQL）、「始终允许」规则由 Rust 写回 config.json（App 改为重读，不覆盖）、浏览器页显示无头 Chrome 截图、全局记忆一次性导出到 `AppSupport/memory/` +「用户记忆」分区、build_app.sh 打包并签名 Rust 二进制和 protocol/
  - [x] 打包实测（真实 DeepSeek）：新会话标题、插话/撤回、/btw /compact /dream /dream undo、回退、看板显示与筛选、数据面板（ER 图、关系、我的查询、数据库连接、停止长任务）、浏览器截图、始终允许跨重启、退出无残留进程
  - [x] 实测中修复：无快速模型时不生成标题（改用主模型）；只剩上次摘要时重复压缩（跳过）；App 保存设置覆盖 Rust 写入的规则；打包缺 protocol/
  - [x] Computer Use 读控件树、截图：授权后可用；Rust 子进程沿用 App 的授权（2026-11-12）
- [x] 记忆面板 Q-SQL / 数据集改走 Rust 索引（`memory/qsql`、`memory/datasets` RPC）；首次启动一次性把 app.sqlite 的 Q-SQL（105 条）和数据集指纹导入 index.sqlite（`Index::import_legacy`，`PRAGMA user_version` 防重复）（2026-11-12）
- [x] 记忆面板在 Rust 核心下隐藏「长期记忆 / 历史会话 / 操作记录 / 使用统计」（这些是 Swift 核心的 app.sqlite 数据，Rust 不读写）；顶部统计改为 主题记忆 / 数据集 / Q-SQL（2026-11-12）
- [x] M2-4 删除 Swift agent 核心（2026-11-13）：AgentSession、工具、上下文/压缩、记忆抽取/召回/SQLite 记忆库、WebSearch/ChromeCDP、内核进程与沙箱、WKWebView 浏览器均已删除；`coreEngine` 开关移除，App 只走 Rust 核心。AgentCore 只保留客户端（CoreProcess/RemoteSession/RemoteKernel）、数据模型、SessionStore/AppDatabase（侧栏与搜索）、ProjectMemory（记忆面板读写主题文件）、看板取数缓存、SkillLibrary（斜杠菜单）、LLMClient（设置页拉模型列表/探测能力）。删除前备份：build/backup/pre-swift-core-removal-20261113.tgz
  - [x] docs/01、02、04、12 已改为指向 datapilot-core / Tauri（2026-11-13；02、04 中保留了 Swift 时期的行为描述和历史设计）
- [x] M4-A 核心补 UI 接口（2026-11-13）：`server_ui.rs` 新增 43 个方法（会话/项目索引与搜索、历史重建、导出 md/html/jsonl、config/get|set（合并写回、保留未知字段、校验后原子写）、secret/set|has（只写，`security -i` 走 stdin）、provider/models|probe、net/test、chrome/status|install、newapi/*、memory/topics、dashboard/*、skill/*、files/list、computer/permissions|request）；核心直接读写 app.sqlite（标题、消息索引与 SwiftUI 互通）；外部 skill 目录（Codex / Claude Code）进 Rust；数据字典导出自动带 catalog.json
- [x] M3 准备：`platform/{mod,macos,windows,other}.rs`（app 数据目录、python 路径、ssh 路径、系统代理、时区、废纸篓、目录复制）；`core-foundation` 只在 macOS 引入；computer 工具只在 macOS 编译
  - [x] Windows 实现（2026-11-13，M3 阶段 C）：`platform/windows.rs`（WinHTTP IE 代理 + PAC/WPAD、SHFileOperation 回收站、GetTimeZoneInformation、核心进程 Job Object）；`secrets::WinCred` 凭据管理器（target `io.github.datapilot.*/<id>`）；`sandbox/`（Seatbelt / AppContainer `DataPilot.Kernel` + ACL + Job Object，内核与 bash 共用）；bash 在 Windows 走 PowerShell `-EncodedCommand`；MCP 走 `cmd /D /S /C`；Chrome 找 Chrome/Edge、CfT win64 用 zip crate 解压；ssh 无控制台窗口；安装时 `datapilot-core --loopback-exempt` 放行回环
  - [x] Windows 运行时与安装包：`scripts/fetch_vendor.py`（python-build-standalone win + uv 装 win_amd64 wheel，`scripts/requirements-win.lock`）；`apps/desktop/scripts/build_win.sh`（cargo-xwin + makensis，Mac 上交叉编译）→ `DataPilot_0.1.0_x64-setup.exe`（85 MB，未签名）；备用 `scripts/windows/build.ps1`
  - [ ] 用户在 Windows 上按 `docs/21-windows-test-checklist.md` 实测
- [x] M4-B Tauri 界面（`apps/desktop/`）
  - [x] B1 骨架（2026-11-13）：Tauri 2 外壳（`src-tauri/src/core.rs` 移植 CoreProcess：按 id 配对、崩溃后自动重启并重开会话、退出时 shutdown；只暴露 `core_request` / `core-event`；CSP + 最小权限）；`protocol/rpc.schema.json` + `gen-protocol.mjs` 生成 TS 类型，Rust 测试校验方法表；React 侧栏（项目/会话/搜索/重命名/置顶/归档/删除）、聊天流（流式文本、思考过程、工具卡片 + 虚拟表格、审批/提问/计划卡、插话/撤回、停止、回退、斜杠命令 + skill）；`dev-bridge.mjs` + `e2e/run.mjs`（黄金模拟模型 + 真核心 + 系统 Chrome）7/7 用例通过；vitest 3/3
  - [x] B2 聊天其余（2026-11-13）：模型菜单 + 思考开关、SQL 卡片（导出 CSV/xlsx/parquet、解释、格式化、执行计划、👍）、图表卡片（iframe 复用 Resources/web，host.js 增加 parent.postMessage 通道）、加入看板、拖入/选择文件（核心 `files/import`）、导出会话、斜杠命令全集；核心新增 `files/import`、`files/save`；e2e `b2-chart-sql` 通过
  - [x] B3 检查器（2026-11-13）：数据（表/预览/导出/数据字典、派生表、关系确认/否决/重新推断、ER 图 iframe、我的查询、数据库连接含 SSH 表单与抽取、内核任务停止）、看板（iframe + `dashboard/query inline` 细粒度数据、卡片编辑写回、HTML/PNG/JSON/数据集明细导出）、浏览器截图（`files/image`）、记忆（用户/项目主题、数据集、Q-SQL）、任务、文件；e2e 覆盖
  - [x] B4 设置（2026-11-13）：模型服务（拉模型、选择添加、本地标记、删除）、角色（测试写回能力、能力编辑）、通用/代理测试/Chrome 安装/数据位置、权限规则、扩展（Skill 目录、MCP 增删改+状态、浏览器域名、Computer Use 授权）、NewAPI 登录（账号密码或访问令牌；完整 Key 只在核心里写入钥匙串，核心新增 `newapi/login`）、关于；config 修改走 `config/set` 合并写回；e2e 覆盖
  - [x] Tauri 打包（2026-11-13）：`apps/desktop/scripts/build_mac.sh` → `src-tauri/target/release/bundle/macos/DataPilot.app`（核心 sidecar + Python + Resources，嵌套代码签名）；启动 / 退出后无残留进程；确认对话框改用原生 `ask`
  - [x] 用户在 Mac 上试用 Tauri 版（2026-11-15 通过）：已装到 `/Applications/DataPilot.app`（2026-11-15 起改名为 DataPilot，旧的「DataPilot Tauri」已删除）
- [x] M4-D 切换（2026-11-15）：`scripts/build_dmg.sh` 改打 Tauri 版；删除 `App/`、`Packages/AgentCore`、`project.yml`、Xcode 工程、`build_app.sh`（备份 `build/backup/pre-swiftui-removal-20261115.tgz`）
- [x] 深色 / 浅色手动切换（跟随系统 / 浅色 / 深色）与新增斜杠命令 /context /fork /review /doctor /remember /copy /permissions /settings /todos（2026-11-17；核心新增 `session/context`、`session/fork`、`app/doctor`）
- [x] 压缩改进（2026-11-17，参照 Codex 源码）：保留用户原话（≤20k）、总结请求超长时删旧重试、压缩上限 `compactTokenLimit`、摘要模型可选 `compactModel`、上下文窗口从 /models 与报错中自动识别；token 估算按网关实测校准、真实用量跨重启保留
