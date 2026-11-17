# ADR-0010 · 联网搜索用本机 Chrome（DevTools Protocol）搜 Google

日期：2026-11-09　状态：已采纳

## 背景
用户不会配置搜索 API Key（Tavily / Brave / SerpAPI 等）。本机测试：curl 直接请求 Google 无结果，DuckDuckGo / Jina 超时，cn.bing.com 可用。

## 决策
- 主后端：本机 Chrome headless + CDP 访问 Google，Swift 直接连 WebSocket，不依赖 node。
- 保证可用：没有 Chrome 时一键下载 Google 官方 Chrome for Testing（不打包进 DMG，避免体积与再分发问题）。
- 兜底：cn.bing.com HTML；验证码时弹出有界面的 Chrome 让用户通过一次。

## 代价
抓取结果页依赖页面结构，Google 改版需更新选择器；首次搜索要启动 Chrome（数秒）。

## 补充（2026-11-09）
- Chrome for Testing 官方包为 ad-hoc 签名，`codesign --verify` 无法通过；改为：只接受 `storage.googleapis.com/chrome-for-testing-public/` 的 HTTPS 地址 + 核对 bundle id `com.google.chrome.for.testing`。实测下载安装、CDP 启动均正常。
- 同一个 headless Chrome 也用于 `web_fetch render:true`（JS 渲染页面）和 `chart_create save_png`（图表导出 PNG）。
