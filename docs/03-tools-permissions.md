# 03 · 工具与权限

## 工具清单（注册顺序固定，利于前缀缓存）
| 类别 | 工具 | 风险 | 并发 |
|---|---|---|---|
| 数据 | `data_import` 导入文件（编码/分隔符/标题行/多 sheet） | sandboxed | 串行 |
| | `data_schema` M-Schema + 关系推断 + 语义层 | readOnly | 并行 |
| | `sql_query` 只读 SQL（守卫、预览、parquet 结果、Q-SQL 积累） | readOnly | 并行 |
| | `python` 持久内核执行 | sandboxed | 串行 |
| | `catalog_update` 语义层（描述/指标/关系/同义词） | sandboxed | 串行 |
| | `data_transform` 派生表/视图（原始表只读，见 16） · `data_export` 导出文件 | sandboxed | 串行 |
| | `relation_test` 测试关联键（只读） · `relation_define` 保存/确认/否决关系（见 17） | readOnly / sandboxed | |
| 数据库 | `db_list` / `db_schema` / `db_query`（远端只读实时查询） · `db_extract` / `db_refresh`（抽取到本地） · `query_save`（我的查询），见 19 | readOnly / sandboxed | |
| BI | `chart_create` 聊天内联图（可选 `save_png` 导出 PNG）· `dashboard_create` / `dashboard_update` | readOnly / sandboxed | |
| 记忆 | `memory_write` / `memory_search` / `memory_forget` / `history_search` | | |
| 规划 | `todo_write` / `ask_user` / `exit_plan_mode` / `agent`（子 agent，旧名 `subagent`，可并行，见 13-subagents.md） | | |
| Skills | `skill` 加载正文 · `save_skill` 保存流程 | readOnly / external | |
| 文件 | `list_files` / `read_file` / `grep` / `glob` / `write_file`（限工作区与项目） | | |
| 执行 | `bash`（沙箱 zsh，120s 超时） | sandboxed | 串行 |
| 网络 | `web_search`（本机 Chrome 搜 Google，Bing 兜底，可并行）· `web_fetch`（跟随系统代理、代理失败直连重试、HTML→文本、表格存 CSV，`render:true` 用 Chrome 渲染，可并行） · `browser` | network | |
| 扩展 | `mcp__<server>__<tool>`（MCP 服务器）· `computer`（默认关闭） | external | |

## 执行管线（executeOne）
未知工具 → JSON 解析 → schema 校验（错误回传模型自纠）→ 权限决策 → 审批（如需）→ 执行 → 事件日志 → 结果首尾截断（默认 8000 字符）→ 展示数据持久化。工具错误以错误结果回传，**不中断循环**。

## 并发
连续的 `isConcurrencySafe` 工具合为一批并行（上限 10），其余单独串行（Codex RwLock 思路 + 分批）。

## 权限模式
4 种审批模式（计划 / 询问 / 自动 / 完全访问）、`工具(说明符)` 规则和审批卡见下文「审批模式」一节。判定顺序：本地模式检查 → 联网总闸 → deny 规则 → 计划模式门禁 → ask / allow 规则与本会话允许 → 按风险与模式默认。

## 项目模式下的文件权限
见 `12-projects.md`：项目原始文件只读（READ_ROOT），`outputs/` 可写（WRITE_ROOT），`write_file` 默认写入 outputs/。

## 数据库端口（联网关闭时）
数据库连接不需要打开联网总闸：内核沙箱只放行 `connections.json` 里各连接的 TCP 端口和 DNS，其余网络仍被拒绝（见 19-db-connections.md）。新的计划模式权限：`db_list/db_schema/db_query/relation_test` 可用，`db_extract/db_refresh/query_save/data_transform/data_export/relation_define` 禁用；由 `PermissionMatrixTests` 覆盖。

## 联网总闸（0.2 起）
输入框下方的「联网」开关是**唯一**的联网闸门，默认关闭：
- 受控工具：`web_search`、`web_fetch`（含 `render:true`）、`browser` 的 navigate/click/type/select/back、HTTP 类 MCP、沙箱内 python/bash（Seatbelt `network.sbpl`）。工具通过 `needsNetwork(for:)` 声明。
- 开关关闭时调用这些工具 → 弹出蓝色“此操作需要联网”卡片：「开启联网并继续」等同于用户打开开关（输入框开关同步变为开启，事件 `networkChanged`）；「保持关闭」→ 工具返回明确的“联网已关闭”，模型如实告知用户。
- 调用模型 API 不受开关影响。
- 参考 Codex：`workspace-write` 沙箱默认 `network_access=false`，需要时申请批准。

## 审批模式（0.2 起，参考 Codex 预设 / Claude Code 权限模式）
| 模式 | 对应 | 只读 | 沙箱内写入/执行 | 联网工具（开关打开后） | 沙箱外（桌面控制、表单提交、MCP） |
|---|---|---|---|---|---|
| 计划 | Claude Code plan / Codex Read Only | 自动（含导入到沙箱库） | 禁止 | 询问（只读调研：web_search / web_fetch / 浏览器导航） | 禁止 |
| 询问（默认） | Codex Default（workspace-write + on-request） | 自动 | 自动 | 询问 | 询问 |
| 自动 | Claude Code acceptEdits+ | 自动 | 自动 | 自动 | 询问 |
| 完全访问 | Codex Full Access / bypassPermissions | 自动 | 自动 | 自动 | 自动（红色警示条，开启需确认） |

- 「开启联网」卡片只代替**普通联网调用**本身的审批；表单输入（browser type/select）等外部操作开启联网后仍会再弹一次审批卡（询问 / 自动模式）。
- 计划模式：`exit_plan_mode` → 「批准并执行」切到询问、「批准并自动执行」切到自动、「要求修改」留在计划模式。
- 测试：`permissionModeMatrix`（4 模式 × 13 类操作 × 联网开关开/关，走真实审批闸门）、`planModeApprovalFlow`、`computerUseGuards`。
- 输入框按 **⇧⇥** 循环：询问 → 自动 → 计划（完全访问不在循环中，只能从模式菜单或 `/full` 开启并确认）。新会话默认模式在 设置 → 通用。
- **规则**（设置 → 权限）：Claude Code 风格 `工具(说明符)`，优先级 拒绝 > 询问 > 允许。说明符：`domain:主机`（含子域名）、`前缀*`（命令/路径前缀）、子串。例：`web_fetch(domain:weather.com.cn)`、`bash(git status *)`、`mcp__playwright__*`。
- **审批卡**：允许 / 本会话允许（同域名或同命令前缀）/ 总是允许 `工具(说明符)`（写入规则，相当于 Codex 的 ExecpolicyAmendment / NetworkPolicyAmendment）/ 拒绝；也可“拒绝并反馈”，反馈文本作为工具结果交给模型。

## 本地模式
- 设置 → 模型服务 的“已添加的模型”中勾选“本地”（名称含 local/ollama、或地址为 localhost/内网 IP/.local 的自动勾选）。
- 输入框“🔒 本地”按钮、设置 → 通用 可开关。开启后：`AgentServices.client(for:)` 拒绝非本地模型（覆盖主循环、压缩、笔记、记忆抽取、标题、子 agent）；快速模型非本地时回退主模型；主模型非本地时自动换成第一个本地模型；模型菜单 / `/model` / 角色下拉只列本地模型；联网开关强制关闭，联网工具禁用。

## 联网搜索
`web_search`：本机 Chrome（DevTools Protocol）搜 Google，失败退到 Bing；risk=network，受联网开关和审批模式控制，可并行。详见 `15-web-search.md`。
