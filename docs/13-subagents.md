# 13 · 子 Agent（agent 工具）

参考 Claude Code 的 Agent 工具 / subagents 与 Codex 的 multi_agents（`max_threads=6`、`max_depth=1`）。

## 用法
- 工具名 `agent`（旧名 `subagent` 仍可用）。参数：`description`（界面标签）、`prompt`（自包含任务）、`subagent_type`、`max_turns`。
- 主 Agent 在**同一条消息**里发出多个 `agent` 调用即并行执行；全局同时最多 `subagentMaxConcurrent`（默认 4，照顾本地 GPU），多余的排队。
- 深度 1：子 Agent 不能再启动子 Agent，也不能 ask_user / exit_plan_mode / todo_write / computer。
- 子 Agent 只把最终结论返回主上下文；聊天中显示子 Agent 卡片（实时进度：轮次、当前工具；完成后显示轮数与用过的工具）。

## 内置类型
| 类型 | 工具 | 模型 |
|---|---|---|
| explore | data_schema、sql_query、list_files、read_file、grep、glob、memory_search、history_search、skill | 快速模型 |
| analyst | explore 的工具 + python（独立变量空间 `<ns>_sub_<id>`，共享 DuckDB）、chart_create、write_file | 会话模型 |
| researcher | web_search、web_fetch、read_file、memory_search | 会话模型 |

## 自定义类型
`~/Library/Application Support/DataPilot/agents/*.md` 或项目 `.datapilot/agents/*.md`，frontmatter 与 Claude Code 一致：
```markdown
---
name: auditor
description: 核对数据质量与口径
tools: data_schema, sql_query, grep
model: fast        # fast | inherit | <模型 id>
maxTurns: 10
---
额外说明（追加到子 Agent 系统提示）。
```

## 权限
子 Agent 的每次工具调用都经过父会话的 `AgentSession.authorize`：审批模式与规则、联网总闸、本地模式全部生效；审批卡标注 `[子 Agent 类型: 描述]`。本地模式下只用本地模型。
