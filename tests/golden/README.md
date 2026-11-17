# 黄金样本回放测试

**目的**：保证 Swift 核心和以后的 Rust `datapilot-core` 在同样的输入下，产出相同的工具调用顺序、工具成败和结束原因。迁移核心时不靠人工逐一对比。

- `cases/*.json`：每个样本包含
  - 用户消息；
  - 权限模式；
  - 是否准备测试数据（`fixtures`）；
  - 可选 `interact`：脚本化用户对卡片的回答，`plan`（approve / approve_auto / 意见）、`question`（回答文字）、`approval`（allowOnce / allowSession / allowAlways / deny）；
  - 模型的脚本化回复 `model[]`：第 n 次流式请求返回第 n 项；子 Agent 的对话（系统提示以 `Resources/prompts/subagent.md` 开头）改用 `subagent_model[]`；
  - `model[]` 的某一步可写 `prompt_tokens`，模拟上下文用量（用来触发压缩）；
  - 期望结果 `expect`：
    - 每轮的工具调用名；
    - 每个工具结果是否出错，按工具名依次匹配；
    - 结束原因；
    - 回答中必须包含的文字；
    - 可选 `compactions`：本轮压缩次数。
- `scripts/golden_llm_server.py`：脚本化的 OpenAI 兼容模型服务，任何语言写的核心都能用它。
- 回放器：
  - Rust：`datapilot-core/tests/golden.rs`，读同一批样本（`cargo test --test golden`，输出写到 `out/<样本>.rust.json`）。
- 每次回放都会把规范化后的结果写到 `tests/golden/out/<样本>.json`（不纳入版本控制），方便对比前后两次的输出。
- 回放时，每个事件的 wire 形式都会按 `protocol/agent-event.schema.json` 检查必填字段。

**新增样本**：复制一个现有样本，改写其中的 `model` 和 `expect`，然后运行 `swift test --filter golden`。
