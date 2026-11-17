# 21 · Windows 实测清单（M3）

安装包：`apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/DataPilot_0.1.0_x64-setup.exe`（Mac 上交叉编译，未签名）。
重新打包：Mac 上 `apps/desktop/scripts/build_win.sh`；在 Windows 上打包用 `scripts\windows\build.ps1`。

## 安装
- [ ] 双击安装。SmartScreen 提示「已保护你的电脑」时，点「更多信息 → 仍要运行」。安装需要管理员权限（UAC），因为要做回环豁免。
- [ ] 安装完成后，在管理员 PowerShell 里执行 `CheckNetIsolation LoopbackExempt -s`，列表中应有 DataPilot.Kernel 对应的 SID（`S-1-15-2-…`）。
- [ ] 开始菜单能启动 DataPilot，启动时不弹黑色控制台窗口。

## 基本功能（与 Mac 打包实测对应）
- [ ] 引导页：填写模型服务和 API Key → 保存。打开「凭据管理器 → Windows 凭据 → 普通凭据」，应能看到 `io.github.datapilot.apikeys/<id>`。
- [ ] 新会话发一条消息，能正常流式回复，并自动生成标题。
- [ ] 「登录并选择 Key」：弹出网关登录窗口，登录（NetID / 账号密码）后自动列出 Key；选一个后模型列表弹出，主模型自动设好。
- [ ] 模型回复中途点停止（⌘. / Ctrl+.），应在 1 秒内停下。
- [ ] 设置 → 通用 → 界面语言切到 English，重新加载后界面为英文；切回简体中文。
- [ ] 导入表格后，检查器「数据」页签马上出现新表（不用等本轮结束）。
- [ ] 窗口拉窄 / 打开检查器时，输入框下面的控制条不换行、不变形。
- [ ] 拖入 / 选择 CSV 或 xlsx 导入 → SQL 查询 → 结果表格 → 导出 CSV。
- [ ] 图表卡片显示正常 → 加入看板 → 看板筛选。
- [ ] 打开一个项目文件夹（含中文路径和空格）→ 导入 `data\*.xlsx` → 生成文件写到 `outputs\`。
- [ ] 插话 / 撤回、停止、回退（rewind）。
- [ ] `/compact`、`/btw`、`/dream`。
- [ ] 设置：拉取模型列表、能力测试、代理测试（系统代理与 PAC 都试）、权限规则「始终允许」重启后仍生效。
- [ ] 让模型用 bash 工具执行 `Get-ChildItem`，输出为 UTF-8、中文不乱码。
- [ ] MCP：添加一个 `npx` 启动的 MCP 服务器（如 `npx -y @modelcontextprotocol/server-everything`），状态为已连接。
- [ ] 浏览器 / 网页搜索：本机有 Chrome 或 Edge 时直接使用；都没有时在「通用 → Chrome」点安装（下载 CfT win64）。
- [ ] 数据库连接（PostgreSQL / MySQL 任一）：测试连接、看表结构、查询。
- [ ] SSH 隧道连接：需要 Windows 自带的 OpenSSH 客户端（设置 → 可选功能）。
- [ ] 删除项目数据 → 文件进入回收站（不是直接删除）。
- [ ] 会话时间显示为本地时区。

## 沙箱越权检查（重点）
在聊天里让模型用 python 工具执行以下代码，每一项都应失败（PermissionError / 拒绝访问 / 无法解析主机）：
- [ ] 写工作区外：`open(r"C:\Users\<你>\Desktop\x.txt","w").write("x")`
- [ ] 读其他用户文件：`open(r"C:\Users\<你>\Documents\<任意文件>").read()`（项目文件夹以外）
- [ ] 联网开关关闭时：`import urllib.request; urllib.request.urlopen("https://example.com")`
- [ ] 打开联网开关后，同一段代码应能成功。
- [ ] bash 工具：`New-Item $env:USERPROFILE\..\x.txt` 应失败；`New-Item test.txt` 在工作区内应成功。

## 进程清理
- [ ] 正常退出 DataPilot 后，任务管理器中没有残留的 `datapilot-core.exe`、`python.exe`、`ssh.exe`、`chrome.exe`（DataPilot 启动的那个）。
- [ ] 在任务管理器里强制结束 `datapilot-core.exe`：它的子进程（python、ssh、MCP）应一起消失（Job Object）；再发消息时界面会自动重启核心。

## 已知限制（第一版）
- 没有 Computer Use（设置里隐藏）。
- 数据库连接需要的网络权限比 Mac 粗：只要工作区存在数据库连接，内核就能访问网络（Mac 只放行对应端口）。
- 安装包未签名，首次运行 SmartScreen 会提示。

## 出问题时请提供
`%APPDATA%\DataPilot\core.log`，以及出错的那一步截图。
