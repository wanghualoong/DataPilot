# 18 · 跨平台开发规范（从现在起的新代码都遵守）

目标：以后迁到 Rust 核心 + Tauri 外壳（ADR-0012）时，新代码尽量不用重写。

## 1. 逻辑放在哪一层
| 内容 | 放在 | 例子 |
|---|---|---|
| 数据计算、导出、关系推断、转换探测、数据字典 | Python 内核 `op_*` | `op_transform`、`op_export`、`op_relation_test` |
| 交互界面（图表、看板、ER 图、表单） | `Resources/web/*.js` | `dashboard.js`、`er.js`、`expr.js` |
| agent 循环、工具调度、权限、记忆 | Rust `datapilot-core`（见 20-rust-core.md） | 保持薄；复杂逻辑尽量下沉到内核；黄金样本兜底 |
| 窗口、菜单、原生对话框 | Tauri（`apps/desktop`） | 只做托管和转发 |

Swift 工具的 `run()` 理想状态是：校验参数 → 调内核 op → 包装结果。

## 2. Web ↔ 外壳只走 `host.js`
- 网页里**禁止**直接写 `window.webkit`、`chrome.webview`、`__TAURI__`。统一用：
  - `DPHost.post({type, ...})`：发事件给外壳；
  - `DPHost.call(op, params)`：请外壳代调一个**白名单**内核 op，返回 Promise；
  - `DPHost.available()`：是否在 App 内（导出的离线 HTML 中为 false）。
- 外壳 → 网页：调用网页暴露的全局函数（`DP.renderDashboard`、`DPER.load`）。
- 宿主实现（Mac / Windows 共用）：`apps/desktop/src/components/WebFrame.tsx`（iframe + postMessage）。

## 3. 先定协议
- `protocol/agent-event.schema.json`：核心 → UI 的事件流；由 Rust `datapilot-core/src/events.rs` 生成，前端类型由 `apps/desktop/scripts/gen-protocol.mjs` 从 schema 生成。
- `protocol/tool-contract.schema.json` + `tools.snapshot.json`：工具名、风险、参数 schema。改工具参数要**有意识地**更新快照：`DP_UPDATE_SNAPSHOT=1 swift test --filter toolContract`。
- 字段用 snake_case；加可选字段兼容，改名/删除算破坏性变更。

## 4. 黄金样本
- `tests/golden/cases/*.json` + `scripts/golden_llm_server.py`（脚本化模型）。新增会改变工具调用流程的功能时，加一个样本。
- Rust 核心（`datapilot-core/tests/golden.rs`）读同一批样本，结果必须一致：`cd datapilot-core && cargo test`。
- 提示词只写在 `Resources/prompts/*.md`（两个核心共用），不要再写进 Swift 或 Rust 源码。

## 5. 平台相关代码
- 只出现在适配层：`Sandbox/`、`Extensions/ComputerUse.swift`、`BrowserController.swift`、钥匙串。业务代码里不写 `#if os(...)`。
- 路径用 `URL` 拼接，不手写 `/`；内核里用 `os.path.join`；临时文件放工作区内，不写 `/tmp`。
- 文本统一 UTF-8；CSV 导出带 BOM（Excel 兼容）。

## 6. 资源目录不分叉
`Resources/`（kernel、web、skills、sandbox、about）两个平台共用；平台差异用配置表达。

## 7. 检查清单（提交前）
- [ ] 新逻辑能放内核/web 的没有写进 Swift
- [ ] 网页没有直接引用平台全局对象（`tests/js/test_er.js` 有静态检查示例）
- [ ] 工具参数改动已更新 `protocol/tools.snapshot.json`
- [ ] 改变工具流程的功能有黄金样本
