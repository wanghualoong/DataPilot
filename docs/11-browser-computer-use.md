# 11 · 浏览器、MCP 与 Computer Use

原则：**优先结构化（文本）控制，截图+坐标为补充**。DeepSeek / Qwen 文本模型没有视觉能力，因此浏览器与桌面都提供“元素树 + ref”路径；Qwen-VL 等视觉模型可额外使用截图坐标。

## 1. 网页获取 `web_fetch`
- 跟随全局代理设置（设置 → 通用，默认跟随系统/PAC，代理失败自动直连重试）抓取；受「联网」开关控制；需要 JS 渲染的页面可加 `render:true` 用本机 Chrome 渲染（见 15-web-search.md）；HTML → 文本，包裹在 `<untrusted_web_content>` 中（提示注入防护）。
- 页面 `<table>` 自动保存为 `uploads/web_table_*.csv`；链接若是 csv/xlsx/json/parquet 直接下载到 uploads/。之后 `data_import`。
- 风险级别 network：询问模式下需审批，可设“总是允许”或按域名规则允许。

## 2. 内置浏览器 `browser`
- 实现：`datapilot-core/src/tools/browser.rs`，通过 CDP 驱动本机无头 Chrome（独立用户数据目录，与日常浏览器隔离，保留登录态）；每个会话一个标签页；检查器“浏览器”页显示最新截图。
- 交互方式借鉴 playwright-mcp 的 **snapshot + ref**：注入 JS（snapshot 脚本）枚举可见的可交互元素，打上 `data-dp-ref="eN"`，返回“`[e12] button "下一页"`”式列表 + 可见文本。模型用 `click(ref)` / `type(ref,text,submit)` / `select(ref,value)` 操作，无需坐标。
- 动作：`navigate snapshot click type select scroll back extract_tables screenshot`。
- 风险分级：snapshot/scroll/back/extract/screenshot 只读；navigate/click 为 network；type/select 为 external（可能提交表单）→ 默认需审批。
- 域名允许列表：设置 → 扩展。
- `extract_tables` 把页面表格存为 CSV 到 uploads/。

## 3. MCP 客户端
- 实现：`Extensions/MCP.swift`（自研最小 JSON-RPC 客户端：initialize → tools/list → tools/call；stdio 与 Streamable HTTP）。
- 设置 → 扩展 → “添加 Playwright MCP（本机 Chrome）”一键添加 `npx @playwright/mcp@latest`；也可添加任意 stdio/HTTP 服务器。
- 工具注册为 `mcp__<server>__<tool>`，风险 external（MCP 服务器在沙箱外运行）→ 默认逐次审批，可设“总是允许”。输出包裹 `<untrusted_tool_output>`。

## 4. Computer Use `computer`（默认关闭）
- 实现：`Extensions/ComputerUse.swift`（App 进程内运行，不在沙箱中）。
- 开启：设置 → 扩展 → 允许控制电脑，并授予“辅助功能”与“屏幕录制”权限。
- **AX 结构化模式（所有模型）**：`ax_snapshot` 读取前台应用焦点窗口与菜单栏控件树（AXUIElement），分配 `aN` ref；`ax_press` / `ax_set_value` / `open_app`。
- **视觉模式（Qwen-VL 等）**：`screenshot`（ScreenCaptureKit，排除本 App 窗口）+ 坐标动作 `left_click double_click right_click mouse_move left_click_drag type key scroll wait terminate`，动作名与 Qwen 官方 cookbook `computer_use` 一致。坐标模式在模型能力中配置：`relative1000`（Qwen3-VL，0–1000）或 `absolutePixels`（Qwen2.5-VL，截图像素）。
- 每个动作后自动回传新截图（视觉模型）或新控件树（文本模型）；上下文中只保留最近 3 张截图。
- 安全：
  - 除 ax_snapshot/screenshot/wait 外每步 external → 审批；
  - 每轮改变状态的电脑操作有上限（设置 `computerMaxStepsPerTurn`，默认 40；截图 / 控件树 / 等待不计数），到达上限返回错误并提示模型向用户汇报；
  - 计划模式下禁止；子 agent 不能使用 computer；
  - 执行期间屏幕边缘红框 + 提示条；
  - **⌃⌥⎋ 急停**（全局监听需辅助功能权限，App 内始终有效）：置停止标志、取消当前回合；
  - 黑名单：钥匙串、系统设置、Passwords、1Password、Bitwarden（前台为这些应用时拒绝任何输入动作）；
  - ad-hoc 签名重建后需重新授权（Developer ID 签名可解决）。
