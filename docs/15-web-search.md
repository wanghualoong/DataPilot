# 15 · 联网搜索（Chrome + DevTools Protocol + Google）

无需任何 API Key。

## 组成
| 文件 | 作用 |
|---|---|
| `Net/ChromeLocator.swift` | 查找 Chrome / Chromium / Edge / App 自己安装的 Chrome for Testing；缺失时从官方索引下载 CfT（stable，mac-arm64 / mac-x64），只接受 `storage.googleapis.com/chrome-for-testing-public/` 的 HTTPS 下载地址；解压、去隔离属性、核对 bundle id（`com.google.chrome.for.testing`）后放到 `AppSupport/DataPilot/chrome/` |
| `Net/ChromeCDP.swift` | 启动 headless Chrome（独立持久 profile `chrome-profile/`、`--remote-debugging-port=0`、真实 UA、`--disable-blink-features=AutomationControlled`），用 `URLSessionWebSocketTask` 直连 DevTools Protocol（不依赖 node / puppeteer / chrome-devtools-mcp）。空闲 10 分钟退出，App 退出时关闭 |
| `Net/WebSearch.swift` | Google 结果解析（`a h3` 标题、`cite` 站点、`.VwiC3b` 摘要；`/url?q=` 直接取参数，`/goto?url=` 读 302 Location 解析真实链接）；验证码识别；cn.bing.com 解析（兜底） |
| `Net/WebSearchTool.swift` | `web_search(query, num)` 工具：联网开关 + 审批模式控制，本地模式禁用；可并行；结果缓存 15 分钟；每轮最多 8 次 |

## 流程
1. 联网开关关闭 → 先弹“开启联网”卡片。
2. 找 Chrome；找不到 → 弹“下载 Chrome for Testing（约 150MB）”卡片（也可在 设置 → 通用 → 网络代理 中安装）；拒绝或失败 → 改用 Bing。
3. Chrome 打开 Google 搜索页，轮询直到出现结果或验证码（≤15s），解析结果、并行解析跳转链接。
4. 遇到验证码 → 用同一 profile 打开**有界面**的 Chrome 窗口让用户通过一次（cookie 保留），本次改用 Bing。
5. 返回编号结果列表（`<untrusted_web_content>`），聊天中显示可点击的结果卡片；系统提示要求模型再并行 `web_fetch` 2–3 个来源并注明出处。

## 代理
Chrome 默认跟随系统代理（含 PAC）；全局代理设为“直连”传 `--no-proxy-server`，“自定义”传 `--proxy-server=`。

## 实测（2026-11-09，单位网络）
- 不改 UA 的 headless Chrome 会被 Google 判为机器人；带真实 UA + 关闭 AutomationControlled 后正常返回 10 条结果。
- `DP_LIVE_CHROME=1 swift test --filter liveChromeSearch`：Google 8 条（首次启动约 15s），Bing 8 条。
- 已知限制：部分 `/goto` 链接解析失败时退化为站点首页（cite 域名）；Google 改版时只需改 `WebSearch.googleExtractJS` / `parseGoogleHTML`。

## web_fetch 的 Chrome 渲染（render:true）
`web_fetch` 默认用 URLSession 直接抓取。页面正文很少（依赖 JavaScript 渲染）时，工具结果会提示模型改用 `render:true`：用同一个 headless Chrome（CDP）打开页面，等待 `document.readyState === 'complete'` 后取 outerHTML，再走同样的正文提取和表格另存流程。仍受「联网」开关、审批模式和本地模式约束；没有 Chrome 时走与搜索相同的 Chrome for Testing 安装卡片。

> 注：Chrome for Testing 官方包是 ad-hoc 签名（没有 Developer ID），`codesign --verify` 必然失败（"code has no resources but signature indicates they must be present"），所以完整性依赖 Google 官方 HTTPS 下载源 + bundle id 核对。实测（2026-11-09）：CfT 157 下载约 187 MB，安装后 CDP 可正常启动。测试：`DP_LIVE_CFT=1 swift test --filter liveChromeForTestingInstall`。
