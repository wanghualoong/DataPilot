//! Core → UI events in their wire form (protocol/agent-event.schema.json). The Rust core emits wire JSON directly.
use serde_json::{json, Value};

use crate::types::{ChatMessage, Usage};

pub type Emit = std::sync::Arc<dyn Fn(Value) + Send + Sync>;

pub fn turn_started(t: &str) -> Value { json!({"type": "turn_started", "turn_id": t}) }
pub fn assistant_delta(s: &str) -> Value { json!({"type": "assistant_delta", "text": s}) }
pub fn reasoning_delta(s: &str) -> Value { json!({"type": "reasoning_delta", "text": s}) }
pub fn tool_call_streaming(i: usize, n: &str) -> Value { json!({"type": "tool_call_streaming", "index": i, "name": n}) }
pub fn assistant_message(m: &ChatMessage) -> Value { json!({"type": "assistant_message", "message": m.wire()}) }
pub fn tool_started(c: &str, n: &str, s: &str) -> Value { json!({"type": "tool_started", "call_id": c, "name": n, "summary": s}) }
pub fn tool_progress(c: &str, t: &str) -> Value { json!({"type": "tool_progress", "call_id": c, "text": t}) }
pub fn tool_finished(c: &str, m: &ChatMessage, display: Option<&Value>, err: bool, ms: i64) -> Value {
    let mut v = json!({"type": "tool_finished", "call_id": c, "message": m.wire(), "is_error": err, "ms": ms});
    if let Some(d) = display { v["display"] = d.clone(); }
    v
}
pub fn approval_required(id: &str, tool: &str, summary: &str, risk: &str, args: &Value, kind: &str, host: &str) -> Value {
    json!({"type": "approval_required", "request": {"id": id, "tool": tool, "summary": summary, "risk": risk, "args": args, "kind": kind, "host": host}})
}
pub fn plan_proposed(c: &str, p: &str) -> Value { json!({"type": "plan_proposed", "call_id": c, "plan": p}) }
pub fn todos_updated(t: &Value) -> Value { json!({"type": "todos_updated", "todos": t}) }
pub fn question(c: &str, q: &str, o: &[String]) -> Value { json!({"type": "question", "call_id": c, "question": q, "options": o}) }
pub fn context_injected(s: &str) -> Value { json!({"type": "context_injected", "text": s}) }
pub fn compacted(s: &str, b: usize, a: usize) -> Value { json!({"type": "compacted", "summary": s, "before_tokens": b, "after_tokens": a}) }
pub fn usage(u: &Usage, ct: usize, cw: usize) -> Value {
    json!({"type": "usage", "usage": {"prompt_tokens": u.prompt_tokens, "completion_tokens": u.completion_tokens}, "context_tokens": ct, "context_window": cw})
}
pub fn interjection_delivered(id: &str, t: &str) -> Value { json!({"type": "interjection_delivered", "id": id, "text": t}) }
pub fn title_changed(t: &str) -> Value { json!({"type": "title_changed", "title": t}) }
pub fn notice(s: &str) -> Value { json!({"type": "notice", "text": s}) }
pub fn error(s: &str) -> Value { json!({"type": "error", "text": s}) }
pub fn turn_finished(t: &str, r: &str) -> Value { json!({"type": "turn_finished", "turn_id": t, "reason": r}) }
pub fn network_changed(on: bool) -> Value { json!({"type": "network_changed", "enabled": on}) }
pub fn memory_saved(s: &str) -> Value { json!({"type": "memory_saved", "text": s}) }
