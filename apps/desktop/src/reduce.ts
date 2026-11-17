// Pure event → display-item reducer (unit-tested; no React here).
import type { AgentEvent, HistoryItem } from "./protocol/rpc";

export type Card =
  | { kind: "approval"; id: string; tool: string; summary: string; risk: string; args: unknown; host: string; netKind: string; answered?: string }
  | { kind: "question"; id: string; question: string; options: string[]; answered?: string }
  | { kind: "plan"; id: string; plan: string; answered?: string };

export type Item =
  | (HistoryItem & { streaming?: boolean; running?: boolean; progress?: string; ms?: number; pending?: boolean })
  | (Card & { text?: undefined })
  | { kind: "btw"; id: string; text: string; output?: string; running?: boolean; is_error?: boolean };

export interface ChatState {
  items: Item[];
  running: boolean;
  turnId?: string;
  todos: any[];
  usage?: { context_tokens: number; context_window: number };
  title?: string;
  network: boolean;
  thinking?: boolean;
  /** typed while a turn ran, not yet delivered to the model: id → text */
  pending: { id: string; text: string }[];
}

export const emptyChat = (): ChatState => ({ items: [], running: false, todos: [], network: false, pending: [] });

let seq = 0;
const lid = (p: string) => `${p}-local-${++seq}`;

function upsert(items: Item[], id: string, patch: Partial<Item>, make: () => Item): Item[] {
  const i = items.findIndex((x) => x.id === id);
  if (i < 0) return [...items, { ...make(), ...patch } as Item];
  const out = items.slice();
  out[i] = { ...out[i], ...patch } as Item;
  return out;
}

function streamingIndex(items: Item[]) {
  const last = items.length - 1;
  return last >= 0 && items[last].kind === "assistant" && (items[last] as any).streaming ? last : -1;
}

export function fromHistory(h: { items: HistoryItem[]; todos: any[]; title?: string }): ChatState {
  return { ...emptyChat(), items: h.items, todos: h.todos ?? [], title: h.title };
}

/** The user's own message, shown at once (the core records it with the turn id). */
export function addUser(s: ChatState, text: string): ChatState {
  return { ...s, items: [...s.items, { id: lid("user"), kind: "user", text } as Item] };
}

export function reduce(s: ChatState, e: AgentEvent): ChatState {
  switch (e.type) {
    case "turn_started": {
      const items = s.items.slice();
      for (let i = items.length - 1; i >= 0; i--) {
        if (items[i].kind === "user") { if (!(items[i] as any).turn_id) items[i] = { ...items[i], turn_id: e.turn_id } as Item; break; }
      }
      return { ...s, items, running: true, turnId: e.turn_id };
    }
    case "assistant_delta":
    case "reasoning_delta": {
      const items = s.items.slice();
      let i = streamingIndex(items);
      if (i < 0) { items.push({ id: lid("asst"), kind: "assistant", text: "", reasoning: "", streaming: true } as Item); i = items.length - 1; }
      const it = items[i] as any;
      items[i] = e.type === "assistant_delta" ? { ...it, text: (it.text ?? "") + e.text } : { ...it, reasoning: (it.reasoning ?? "") + e.text };
      return { ...s, items };
    }
    case "assistant_message": {
      const m = e.message;
      let items = s.items.slice();
      const i = streamingIndex(items);
      const done = { id: m.id, kind: "assistant", text: m.content, reasoning: m.reasoning ?? (i >= 0 ? (items[i] as any).reasoning : ""), streaming: false } as Item;
      if (i >= 0) items[i] = done; else if (m.content || m.reasoning) items.push(done);
      if (i >= 0 && !m.content && !(done as any).reasoning) items.splice(i, 1);
      for (const c of m.tool_calls ?? [])
        items = upsert(items, c.id, {}, () => ({ id: c.id, kind: "tool", tool_name: c.name, summary: c.name, arguments: c.arguments, output: "", is_error: false, running: true } as Item));
      return { ...s, items };
    }
    case "tool_started":
      return { ...s, items: upsert(s.items, e.call_id, { summary: e.summary, running: true } as any,
        () => ({ id: e.call_id, kind: "tool", tool_name: e.name, output: "", is_error: false } as Item)) };
    case "tool_progress":
      return { ...s, items: upsert(s.items, e.call_id, { progress: e.text } as any, () => ({ id: e.call_id, kind: "tool", tool_name: "", output: "", is_error: false } as Item)) };
    case "tool_finished":
      return { ...s, items: upsert(s.items, e.call_id,
        { output: e.message.content, is_error: e.is_error, display: e.display as any, running: false, progress: undefined, ms: e.ms } as any,
        () => ({ id: e.call_id, kind: "tool", tool_name: e.message.name ?? "", summary: e.message.name ?? "" } as Item)) };
    case "approval_required": {
      const r = e.request;
      return { ...s, items: [...s.items, { kind: "approval", id: r.id, tool: r.tool, summary: r.summary, risk: r.risk, args: r.args, host: r.host, netKind: r.kind }] };
    }
    case "plan_proposed": return { ...s, items: [...s.items, { kind: "plan", id: e.call_id, plan: e.plan }] };
    case "question": return { ...s, items: [...s.items, { kind: "question", id: e.call_id, question: e.question, options: e.options }] };
    case "context_injected": return { ...s, items: [...s.items, { id: lid("ctx"), kind: "context", text: e.text } as Item] };
    case "todos_updated": return { ...s, todos: e.todos };
    case "compacted": return { ...s, items: [...s.items, { id: lid("cmp"), kind: "compacted", text: e.summary } as Item] };
    case "usage": return { ...s, usage: { context_tokens: e.context_tokens, context_window: e.context_window } };
    case "memory_saved":
    case "notice": return { ...s, items: [...s.items, { id: lid("note"), kind: "notice", text: e.text } as Item] };
    case "error": return { ...s, items: [...s.items, { id: lid("err"), kind: "error", text: e.text } as Item] };
    case "turn_finished": {
      // unanswered cards of this turn can no longer be answered
      const items = s.items.map((it) => {
        if ((it.kind === "approval" || it.kind === "question" || it.kind === "plan") && !(it as Card).answered) return { ...it, answered: "—" } as Item;
        if ((it as any).streaming || (it as any).running) return { ...it, streaming: false, running: false } as Item;
        return it;
      });
      return { ...s, items, running: false };
    }
    case "network_changed": return { ...s, network: e.enabled };
    case "interjection_delivered":
      return { ...s, pending: s.pending.filter((p) => p.id !== e.id), items: [...s.items, { id: lid("user"), kind: "user", text: e.text, turn_id: s.turnId } as Item] };
    case "title_changed": return { ...s, title: e.title };
    default: return s;
  }
}

export function answerCard(s: ChatState, id: string, answer: string): ChatState {
  return { ...s, items: s.items.map((it) => (it.id === id ? ({ ...it, answered: answer } as Item) : it)) };
}
