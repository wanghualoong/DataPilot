import { describe, expect, it } from "vitest";
import { addUser, answerCard, emptyChat, reduce, type ChatState } from "./reduce";
import type { AgentEvent } from "./protocol/rpc";

const run = (s: ChatState, evs: AgentEvent[]) => evs.reduce(reduce, s);

describe("reduce", () => {
  it("streams text, tool calls and finishes the turn", () => {
    let s = addUser(emptyChat(), "按大区汇总");
    s = run(s, [
      { type: "turn_started", turn_id: "t1" },
      { type: "reasoning_delta", text: "想" },
      { type: "assistant_delta", text: "我来" },
      { type: "assistant_delta", text: "查询" },
      { type: "assistant_message", message: { id: "a1", role: "assistant", content: "我来查询", is_meta: false, tool_calls: [{ id: "c1", name: "sql_query", arguments: "{}" }] } },
      { type: "tool_started", call_id: "c1", name: "sql_query", summary: "SQL: SELECT 1" },
      { type: "tool_finished", call_id: "c1", message: { id: "r1", role: "tool", content: "| v |", is_meta: false }, display: { kind: "sql" }, is_error: false, ms: 12 },
      { type: "assistant_delta", text: "华东最高" },
      { type: "assistant_message", message: { id: "a2", role: "assistant", content: "华东最高", is_meta: false } },
      { type: "turn_finished", turn_id: "t1", reason: "done" },
    ]);
    expect(s.running).toBe(false);
    expect(s.items.map((i) => i.kind)).toEqual(["user", "assistant", "tool", "assistant"]);
    expect((s.items[0] as any).turn_id).toBe("t1");
    expect((s.items[1] as any).reasoning).toBe("想");
    expect((s.items[2] as any).summary).toBe("SQL: SELECT 1");
    expect((s.items[2] as any).display.kind).toBe("sql");
    expect((s.items[3] as any).text).toBe("华东最高");
  });

  it("cards: answered, and closed when the turn ends", () => {
    let s = run(emptyChat(), [
      { type: "turn_started", turn_id: "t" },
      { type: "approval_required", request: { id: "ap1", tool: "web_fetch", summary: "x", risk: "network", args: {}, kind: "network", host: "a.com" } },
      { type: "question", call_id: "q1", question: "哪个口径？", options: ["含税", "不含税"] },
    ]);
    s = answerCard(s, "ap1", "allowOnce");
    expect((s.items[0] as any).answered).toBe("allowOnce");
    s = reduce(s, { type: "turn_finished", turn_id: "t", reason: "cancelled" });
    expect((s.items[1] as any).answered).toBe("—");
  });

  it("interjections and titles", () => {
    let s: ChatState = { ...emptyChat(), pending: [{ id: "i1", text: "顺便看下华北" }] };
    s = run(s, [{ type: "interjection_delivered", id: "i1", text: "顺便看下华北" }, { type: "title_changed", title: "大区销售" }]);
    expect(s.pending).toEqual([]);
    expect(s.items[0].kind).toBe("user");
    expect(s.title).toBe("大区销售");
  });
});
