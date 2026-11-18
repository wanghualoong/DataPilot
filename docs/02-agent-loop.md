# 02 · Agent Loop

实现：`datapilot-core/src/agent.rs`（回合循环）、`session.rs`（审批 / 提问等待、transcript）、`compact.rs`、`notes.rs`、`extract.rs`。下文沿用 Swift 时期的名字（AgentSession、ContextAssembler 等）描述行为，Rust 版行为相同。

## 一轮对话（runTurn）
1. **注入动态上下文**（`ContextAssembler.turnContext`）：日期、用户画像/习惯、数据集口径、该数据集历史分析、工作区状态（表/变量）、会话笔记、相关历史会话、已验证 Q-SQL。总预算 ≈ 窗口的 10%；**内容不变就不重复注入**（避免 system-reminder 膨胀问题）。注入块在 UI 中可开关查看（工具栏 👁）。
2. 追加用户消息；第一条消息后台生成标题；检测纠正信号（“不对/应该是/要剔除/记住”）记入事件。
3. 循环：
   - `maybeCompact`：>60% 先做微压缩（清理旧工具输出，无 LLM 调用）；>80%（可配）自动压缩。
   - 组装系统提示（稳定部分在前：基础规则 → 语言 → 阈值 → Skills 目录 → Plan 模式说明）。
   - 流式调用模型：`assistantDelta` / `reasoningDelta` / `toolCallStreaming` 事件实时推给 UI。
   - 无工具调用 → 结束；有工具调用 → 分批执行（见 03）→ 结果追加 → 继续。
4. 保护：`maxTurns`（默认 60）、取消（Task 取消 + 只取消本对话在内核里的请求，同项目其他对话不受影响；10 s 内停不下来才重启内核，见 12-projects「内核内的并发」）、同参数重复调用 3 次注入提醒、上下文过长错误 → 强制压缩重试一次、压缩连续失败 3 次熔断。
5. 结束后后台执行（不阻塞用户）：会话笔记更新 → 记忆抽取 → 保存情景摘要。

## 中途插话（参考 Claude Code）
- Agent 运行时输入框照常可用：回车即插话。消息进入 `AgentSession.interjections` 队列，界面显示虚线气泡「排队中」。
- 送达时机：每批工具执行完、下一次调用模型前（`deliverInterjections`），作为普通用户消息加入上下文，并附一条 meta 提示“用户在任务进行中补充了以上内容，请据此调整”；送达后发 `interjectionDelivered` 事件，气泡变为正常并移到当前位置。
- 模型本轮已给出最终回答（无工具调用）时，若队列中有插话，则继续同一轮；轮次结束后才到的插话，界面自动用它开始下一轮。
- 送达前可撤回：气泡上的 ✕，或输入框为空时按 ↑ 取回编辑。运行中不能带附件。子 agent 不接收插话。
- 测试：`interjectionDeliveredMidTurn`。

## /btw 顺便问
`/btw 问题`：用当前对话（工具调用压平成文字，约占窗口 1/3）作上下文，**不给工具**单次调用主模型（`AgentSession.btw`）。问题和回答都不写入上下文和对话记录，运行中也能用、不打断主任务；本地模式下同样只走本地模型。界面显示为紫色「旁问 · 不计入对话」卡片，可关闭；导出对话时不包含。

## 事件（AgentEvent）
`turnStarted / assistantDelta / reasoningDelta / toolCallStreaming / assistantMessage / toolStarted / toolFinished / approvalRequired / planProposed / todosUpdated / question / contextInjected / compacted / usage / memorySaved / notice / error / turnFinished`

事件经 `agent/event` 通知发给界面（Tauri：`apps/desktop/src/reduce.ts` 映射为聊天条目）；审批、计划、提问通过 `approval/respond`、`question/answer` 回传给循环。

## 上下文压缩
- **微压缩**：保留最近 6 个工具结果，更早且 >1500 字符的替换为“已清理”占位。
- **自动压缩**（`Compactor`）：
  - 尾部保留：从末尾往前累积，≥ min(10k, 窗口/12) token 且 ≥3 条文本消息即停，最多 min(40k, 窗口/5)；工具调用/结果成对保留。
  - 先刷新会话笔记，再用快速模型生成交接摘要（`<analysis>` 草稿 + `<summary>` 8 节：目标/数据集/口径/结论/产物/错误/进行中/下一步），草稿丢弃，开头强调不得调用工具。
  - 写入 `compacted` 记录（摘要 + 保留消息 id），重开会话时据此重建上下文。
  - **保留用户原话**（参照 Codex `COMPACT_USER_MESSAGE_MAX_TOKENS`）：压缩后按从新到旧保留用户自己说过的话，预算 min(20k, 窗口/10) token，放在摘要之后、尾部之前；口径、更正不再只靠摘要转述。
  - **总结请求超长**：按 token 把待总结部分装进总结模型的窗口（保留上次摘要，从最早的消息删起）；服务端仍报“上下文过长”时缩小预算重试，最多 3 次（参照 Codex 的删旧重试）。
  - **触发点**：取模型报告的真实用量和估算中较大者（真实用量写入 transcript 的 `usage` 记录，重开会话后仍有效；估算计入系统提示与工具定义）；阈值 = 窗口 × `compactThreshold`（默认 80%），设置了 `compactTokenLimit` 时取两者较小值。
  - **摘要模型**：`compactModel` = `fast`（默认，快速模型）或 `main`（主模型，质量更好）。
  - **上下文窗口识别**：「测试」按钮读取服务端 `/models` 报告的上下文长度（vLLM `max_model_len`、LM Studio / OpenRouter `context_length`）并写入能力配置；New API 网关不报告时用内置预设；对话中服务端报“超出上下文”并给出上限时，自动按该值调小并记住。
- 手动压缩：⌘⇧K 或工具栏按钮。Agent 运行中点击不会报错，而是排队：在本轮下一次调用模型之前（或本轮结束时）执行。
- **总结请求走流式**：所有一次性调用（总结、笔记、标题、记忆抽取、/btw）都用 SSE 流式请求、内部收齐再返回。网关（nginx `proxy_read_timeout`，DKU 网关为 60 s）只要求首个 token 在时限内到达；非流式请求必须在 60 s 内生成完整答复，长对话的总结会被切断成 HTTP 504。实测 qwen 27b：同一份输入非流式 60 s 后 504，流式 14 s 出首字、77 s 正常完成。超长输入首次预填充超过 60 s 时仍可能 504，重试时服务器已缓存前缀（实测重试 6 s 出首字），`with_retry` 会自动重试。
- **失败兜底**：总结调用最终失败时，如果会话笔记不为空，就用会话笔记完成压缩（并提示），不再只报“压缩失败”。网关返回空体或 HTML 错误页时，错误信息显示为可读的中文原因（如“网关超时”）。

## 推理内容回传
DeepSeek V3.1+ 思考模式：当前用户回合内的工具循环回传 `reasoning_content`，历史回合剥离；R1 类从不回传。由 `ModelCapabilities.reasoningReplay` 控制。
