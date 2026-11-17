# ADR-0009 · 用网关网页登录（Duke NetID 等）选择 API Key

日期：2026-11-09　状态：已采纳

## 背景
用户不会去网关后台复制 API Key。DKU 网关为 New API v1.0.0-rc.37，开启了 OIDC（“Duke NetID”），但 OIDC 回调地址固定为网关网页的 `{server}/oauth/oidc`，App 无法注册自己的回调。

## 决策
- 在 App 内嵌 WKWebView 打开网关自己的 `/login` 页。网关提供的任何登录方式（账号密码 / Duke NetID / GitHub …）都可以用，MFA 也由网关页面处理。
- WKWebView 使用按网关主机区分的独立持久数据存储（`WKWebsiteDataStore(forIdentifier:)`）：与 Safari 和 Agent 浏览器隔离，30 天的 refresh cookie 在重启后仍有效。
- 检测到页面进入 `/console` 后，在**同源页面内**用 `fetch(..., {credentials:'include'})` 发请求：
  - `POST /api/user/auth/refresh` → `data.access_token`（15 分钟有效，只保存在内存中）
  - `GET /api/token/?p=1&size=100` → `data.items`（Key 已脱敏），用表格展示名称、状态、剩余额度、过期时间、模型限制、分组
  - 确认后 `POST /api/token/:id/key` → `data.key`，存入钥匙串，然后自动获取模型列表
  - 可选：“新建 DataPilot 专用 Key”（`POST /api/token/`，不限额度、永不过期，额度仍受账号余额约束）
- 请求在页面内发出，Origin 和 cookie 天然正确，可以通过 `SessionCookieOriginGuard`。
- 先读 `/api/status`，确认是 New API 并显示可用的登录方式，因此对任意 New API 实例通用。
- 兼容旧版：没有 `/auth/refresh` 时退回 cookie 会话加 `New-Api-User` 请求头；`/key` 不存在时使用列表中返回的完整 Key。

## 代价
依赖 New API 的控制台接口（非 OpenAI 标准接口）。接口变化时只需修改 `datapilot-core/src/newapi.rs`（REST 调用）和 `apps/desktop/src-tauri/src/newapi_login.rs`（登录窗口）。我们只调用其 HTTP API，不包含 New API（AGPL）的任何代码。
