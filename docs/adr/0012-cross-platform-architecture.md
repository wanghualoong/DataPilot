# ADR-0012：跨平台（macOS + Windows）架构

- 状态：已采纳方向，分阶段实施（2026-11-10）；M1、M2、M4 完成：Mac 改用 Tauri 版，SwiftUI 已删除（2026-11-15）；M3 Windows 安装包已出、待实测
- 相关：ADR-0001（Swift 原生 + Python 内核）、ADR-0002（Seatbelt）、docs/18-cross-platform-dev-guide.md

## 背景
DataPilot 目前只有 macOS 版。各层代码量（2026-11-10 统计）：

| 层 | 语言 | 行数 | 能否在 Windows 上用 |
|---|---|---|---|
| L0 数据内核 `Resources/kernel` | Python + DuckDB | ~2,100 | 能（无 POSIX 专用调用） |
| L1 Web 资产 `Resources/web` | HTML/JS | ~1,500 | 能 |
| L2 Agent 核心 `Packages/AgentCore` | Swift | ~7,500 | 不能（Swift 在 Windows 上生态弱；依赖 Seatbelt、钥匙串、AX、WebKit） |
| L3 外壳 `App/` | SwiftUI | ~5,800 | 不能 |

需要 Windows 版，同时尽量共享代码、减少两套 UI 的维护量。

## 参考：Codex
openai/codex 最早用 TypeScript/Node 写，后来把核心整体重写为 Rust（`codex-rs`）：
- TUI、桌面 App、IDE 插件都是同一个核心的客户端，走 `app-server` 的 stdio JSON-RPC 协议（`app-server-protocol` 定义类型）；
- Windows 沙箱 `windows-sandbox-rs` 用 Rust 直接调 Win32：受限令牌（`token.rs`）、工作区 ACL（`acl.rs`/`workspace_acl.rs`）、专用沙箱用户（`identity`/`setup*`）、WFP 断网（`wfp.rs`）、AppContainer、ConPTY、DPAPI。Apache-2.0，可改编。

## 决策
1. **L0、L1 原样保留并作为共享主体**：新功能的逻辑优先写进 Python 内核 `op_*`，界面优先写进 `Resources/web`。
2. **L2 用 Rust 重写为 `datapilot-core`**，独立进程，stdio JSON-RPC（app-server 模式）。事件、工具契约以 `protocol/*.schema.json` 为准。
3. **L3 外壳用 Tauri 2**（Rust 外壳 + TypeScript 前端），Mac 用 WKWebView、Windows 用 WebView2；现有 web 资产直接嵌入。过渡期 SwiftUI 版改为 `datapilot-core` 的客户端，Tauri 版功能追平后 SwiftUI 只维护不加功能。
4. **平台差异只放在 Rust trait 里**：`SandboxBackend`（Seatbelt / Windows 受限令牌+ACL+WFP / Linux bwrap）、`SecretStore`（钥匙串 / 凭据管理器+DPAPI）、`DesktopControl`（AX / UI Automation）。

### 不选的方案
| 方案 | 不选的原因 |
|---|---|
| 继续 Swift，Windows 另写 | SwiftUI 不跨平台，UI 仍要重写；Windows 安全 API 调用别扭；两套核心 |
| 全部 Electron + TypeScript | 安装包 ~100MB；进程控制、沙箱能力弱；Codex 正是从这条路迁出 |
| .NET MAUI / Flutter | 需要重写全部 web 资产或桥接；沙箱仍需原生代码 |

## 迁移路线（每步都可发布）
| 阶段 | 内容 | 工作量 |
|---|---|---|
| M0（已完成） | 本 ADR、开发规范；`protocol/` schema + 编解码测试；`tests/golden/` 回放；`host.js` 统一 web↔外壳通信 | — |
| M1（核心已完成 2026-11-11） | Rust `datapilot-core`：模型提供商、agent 循环、工具调度、权限、会话存储、压缩、记忆；先跑通黄金样本。现状见 `docs/20-rust-core.md` | 4–6 周 |
| M2 | Mac SwiftUI 改为调用 `datapilot-core`；Seatbelt 适配移到 Rust | 2–3 周 |
| M3 | Windows：沙箱（下节）、凭据管理器、python-build-standalone Windows 版、WebView2 | 3–4 周 |
| M4 | Tauri 统一界面，Mac/Windows 同时出包 | 4–6 周 |

## Windows 沙箱映射（实施前细读 windows-sandbox-rs 源码再定稿）
| DataPilot 需求 | Windows 实现 |
|---|---|
| 内核只能写工作区 / outputs | 沙箱用户或受限令牌 + 对工作区、outputs 加 ACL 允许，其余拒绝写 |
| 联网开关 | WFP 按沙箱身份断网；开关打开时放行到本地代理端口 |
| 项目原始文件只读 | 只读 ACL |
| API Key | 凭据管理器 + DPAPI，绝不传入内核 |
| 一次性管理员授权 | 安装时创建沙箱用户（与 Codex 提升模式相同）；非提升模式退化为受限令牌 + AppContainer |

## 后果
- 好处：一套核心、一套 UI；web 资产和 Python 内核零改动复用；协议让 CLI、批处理、MCP 服务都只是另一个客户端。
- 代价：M1–M4 约 13–19 周；过渡期两个核心并存，靠黄金样本保证一致。

## 实施记录（2026-11-13）
- Windows 沙箱第一版选了 **AppContainer + Job Object**，没有用沙箱用户 + WFP（不需要创建系统账户，安装更简单）：
  - 容器 `DataPilot.Kernel`；工作区、tmp、outputs 加完全控制 ACL，项目目录、Python、Resources 加只读 ACL；
  - 网络：默认无能力；联网开关打开或工作区有数据库连接时加 `internetClient` + `privateNetworkClientServer`（比 Mac 只放行数据库端口粗）；
  - AppContainer 默认不能连 127.0.0.1，SSH 隧道依赖它：安装包（管理员）执行 `datapilot-core --loopback-exempt`；
  - 每个沙箱进程树一个 `KILL_ON_JOB_CLOSE` Job；核心自身也放进 Job，核心崩溃时 ssh / Chrome / MCP 一起结束。
- 代码位置：`datapilot-core/src/sandbox/{mod,seatbelt,appcontainer}.rs`、`platform/windows.rs`、`secrets.rs`（`WinCred`）。
