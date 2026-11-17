# protocol/ — 核心与 UI 之间的契约

| 文件 | 内容 | 生成 / 校验 |
|---|---|---|
| `agent-event.schema.json` | 核心 → UI 事件流（以后是 JSON-RPC 通知 `agent/event` 的 params），按 `type` 区分 | Swift `AgentEvent.wire`；`GoldenTests.goldenReplay` 逐事件检查必填字段 |
| `tool-contract.schema.json` | 单个工具契约：name、risk、concurrency_safe、plan_mode、parameters | `GoldenTests.toolContractSnapshot` |
| `tools.snapshot.json` | 当前全部工具的契约快照 | `DP_UPDATE_SNAPSHOT=1 swift test --filter toolContract` 重新生成 |

以后由 schema 生成 Rust（typify）和 TypeScript（json-schema-to-typescript）类型。见 ADR-0012 和 docs/18。
