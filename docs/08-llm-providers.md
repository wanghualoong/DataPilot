# 08 · 模型接入（New API / Qwen / DeepSeek / 本地模型）

## 接口
只使用 OpenAI 兼容接口：`POST /v1/chat/completions`（SSE 流式）、`GET /v1/models`。不使用 Embedding / Rerank（检索走关键词 + grep，见 ADR-0006）。Base URL 可写到 `/v1`，也可只写主机（自动补 `/v1`）。API Key 存 macOS 钥匙串（服务名 `io.github.datapilot.apikeys`；旧版 `edu.duke.datapilot.apikeys` 首次读取时自动迁移）。

## 代理
- 全局代理（设置 → 通用 → 网络代理）：**跟随系统（默认，含 PAC 自动配置）** / 直连 / 自定义（主机 + 端口）。代码中不再写死任何代理地址。
- 每个 Provider 可选：跟随全局 / 直连 / 自定义地址。局域网/本机模型选“直连”。
- 代理层错误（CFNetwork 306/310/311/312）自动直连重试一次；错误信息转成可读文字（如“代理拒绝建立连接”）。
- 构建脚本的下载代理只从环境变量 `DP_PROXY` 读取，默认不使用代理。

## 模型能力配置（设置 → 模型角色 → 能力…）
| 项 | 说明 |
|---|---|
| 上下文窗口 / 最大输出 | 决定压缩阈值与注入预算 |
| 工具调用方式 | `native`（OpenAI tools）或 `hermes`（系统提示描述工具，模型输出 `<tool_call>{...}</tool_call>`，由客户端解析） |
| 并行工具调用 / 发送 `parallel_tool_calls` | 有的后端不接受该参数，默认不发送 |
| 思考开关参数 | `chat_template_kwargs.enable_thinking`（Qwen3 on vLLM）、`chat_template_kwargs.thinking`（DeepSeek on vLLM）、`enable_thinking`（DashScope）、`thinking:{type}`（DeepSeek 官方 API） |
| 推理内容回传 | `never`（R1 类）/ `currentTurn`（DeepSeek V3.1+：当前回合工具循环内回传 `reasoning_content`） |
| 视觉 | 是否发送 `image_url` |
| 额外请求参数 | 任意 JSON 合并到请求体，如 `{"top_p":0.8,"repetition_penalty":1.05}` |

按模型名有预设（`ModelCapabilities.preset`）：qwen3 → 128k、思考参数 Qwen 风格；deepseek → 128k、V3 回传当前回合推理、R1 用 Hermes。**“测试”按钮**会发一次带工具的流式请求：返回了原生工具调用 → native，否则自动切到 Hermes；同时报告是否返回推理内容与 token 用量。

## 流式健壮性（已有单元测试）
- 工具调用按 `index` 累积，`id`/`name` 只在首个 delta，参数字符串拼接后解析；缺 id 自动生成。
- `finish_reason` 为 `stop` 但带工具调用也能处理。
- 同时读取 `reasoning_content` 与 `reasoning`（新版 vLLM 改名）；`<think>…</think>` 内联推理（含跨 chunk 拆分的标签）自动分离。
- 忽略 `: keepalive` 注释，处理 `[DONE]`、流中 `{"error":...}`、无空行分隔的网关。
- 参数 JSON 修复（空参数、重复对象、缺右括号）。
- 429/5xx/超时指数退避重试 3 次；上下文过长 → 强制压缩后重试。

## 部署建议
**Qwen3（vLLM）**
```bash
vllm serve Qwen/Qwen3-32B --enable-auto-tool-choice --tool-call-parser hermes \
  --reasoning-parser qwen3 --max-model-len 131072
```
数据分析建议关闭思考或仅在复杂问题时开启（输入框“思考”开关）。

**DeepSeek（vLLM）**：V3.1+ 在非思考模式下工具调用最稳定；思考模式用 `chat_template_kwargs.thinking`。官方 API 的思考模式工具调用规则请以 https://api-docs.deepseek.com 为准（调研时无法访问，未核实）。


## 模型角色
主对话（必需）、快速（标题/记忆抽取/压缩/笔记，未配置则用主模型）、视觉（主模型不支持图片时才需要）。首次启动向导会按名称自动推荐：优先名称结尾为 local 的模型（如 `qwen3.8-27b-local`），其次 qwen3（非 VL）或 deepseek → 主模型；7b/8b/flash/turbo → 快速。旧配置中的 Embedding / Rerank 角色在加载时自动清除。

## 图片输入（视觉）
实测（2026-11-08，红/蓝两色图片识别）：`qwen3.8-27b-local` ✅ 0.58s、`qwen3.8-flash` ✅ 0.70s、`DeepSeek-V4.1-Flash` ✅ 1.1s。预设已对 qwen3.5+ / qwen3.8 / deepseek-v4 打开 `supportsVision`；“模型角色 → 测试”会实际发一张图片探测并自动写入能力配置。开启后浏览器截图、Computer Use 视觉模式可直接用主模型。

## 实测：DKU New API 网关（2026-11-08）
Base URL：`https://aichat.dukekunshan.edu.cn:3443/v1`（走 `no_proxy` 直连；TLS 证书链由系统钥匙串信任，URLSession 正常，Python 需用系统 CA）。

| 模型 | 原生工具调用 | 推理内容 | 关闭思考的有效参数 |
|---|---|---|---|
| `qwen3.8-27b-local`（本地 xinference） | ✅（流式，按 index 累积） | `reasoning_content` | `chat_template_kwargs.enable_thinking=false` ✅ |
| `qwen3.8-max` | ✅ | `reasoning_content` | 同上 ✅ |
| `deepseek-v4-pro` | ✅ | `reasoning_content` | 只有 `thinking:{type:"disabled"}` 或 `reasoning_effort:"none"` 有效；`chat_template_kwargs` 无效 |
| `DeepSeek-V4.1-Flash` | ✅ | 默认不思考 | `thinking:{type}` 控制 |

- 工具循环中回传或不回传 `reasoning_content`，三个模型都能正常继续（不会 400）。
- **坑**：`DeepSeek-V4.1-Flash` 在部分参数下返回的 JSON 字符串中含未转义的控制字符（换行），严格 JSON 解析会失败。客户端已加 `JSONSanitizer` 兜底。
- 预设已据此更新：qwen3* → `qwenChatTemplate`；deepseek（非 R1）→ `thinkingObject`。

## 添加模型（自动获取 + 勾选）
设置 → 模型服务 → 选择 Provider → “获取并添加模型…”：调用 `/v1/models` 拉取全部模型，弹出勾选列表（可搜索、按 全部/对话/其他 过滤；默认保留已添加的模型并补勾一个结尾为 local 的模型；“推荐”按钮可一键追加 Qwen/DeepSeek 对话模型）。只有**已添加**的模型出现在模型切换菜单、`/model` 选择器和角色下拉框中。首次启动向导完成后只添加所选的主模型 / 快速模型，以及一个结尾为 local 的本地模型；“登录并选择 Key”后弹出的勾选列表默认只勾选这个 local 模型，可在列表顶部修改服务名称。

## API Key 配置
Key 不写入代码或 config.json，统一保存在 macOS 钥匙串（服务名 `io.github.datapilot.apikeys`，自动从旧名 `edu.duke.datapilot.apikeys` 迁移），在“首次启动向导”或“设置 → 模型服务”中填写/修改。

### 登录并选择 Key（New API 网关，如 DKU 的 Duke NetID）
向导选择 DKU 预设（或自定义），或者在 设置 → 模型服务 中点 **“登录并选择 Key…”**：
1. 弹出窗口打开网关登录页，用 Duke NetID（或网关支持的其他方式）登录；
2. 自动列出你账号下的 Key（名称、状态、剩余额度、过期时间、可用模型、分组；Key 已脱敏）；
3. 选中后点“使用所选 Key”（或双击）：取回完整 Key 存入钥匙串，然后自动获取模型列表；
4. 没有 Key 时可点“新建 DataPilot 专用 Key”。
登录凭据（access token）只保存在内存中；网页登录状态保存在 App 专用的 cookie 存储里（与 Safari 隔离），可点“切换账号”清除。设计见 [ADR-0009](adr/0009-newapi-login-key-picker.md)。

## 本地模式
设置 → 通用 → “本地模式”打开后，只会使用标为「本地」的模型（设置 → 模型服务 → 已添加的模型，点右侧的「本地 / 云端」标签切换）；云端模型、联网工具都会被拒绝。名称含 local / ollama，或服务地址是 localhost / 内网 IP 的模型默认标为本地。添加模型（向导、登录选 Key、“获取并添加模型…”）时，会默认勾选一个名称结尾为 local 的对话模型（优先 qwen3），并明确标为「本地」；没有设置主模型时用它做主模型。见 [ADR-0008](adr/0008-approval-modes-local-only.md)。
回归测试用黄金样本（`tests/golden/`，模拟模型服务，不需要联网）：`cd datapilot-core && cargo test --release --test golden`。
