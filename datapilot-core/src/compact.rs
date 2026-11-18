//! Context compaction (port of the Swift `Compactor`): tail split, microcompact, image pruning, summary call.
use crate::llm::{Client, LlmError};
use crate::paths::Paths;
use crate::text::{estimate_tokens, truncate_middle};
use crate::types::{ChatMessage, ModelRef, Role};

pub const SUMMARY_PREFIX: &str = "[上下文已压缩 / Context compacted] 另一个模型已经处理了本会话前面的部分，以下是交接摘要。请据此继续，不要重复已完成的工作。\nAnother model handled earlier parts of this session; below is its handoff summary.\n\n";

pub fn summary_message(s: &str) -> ChatMessage { let mut m = ChatMessage::user(format!("{SUMMARY_PREFIX}{s}")); m.is_meta = true; m }

fn msg_tokens(m: &ChatMessage) -> usize {
    estimate_tokens(&m.content) + m.tool_calls.iter().flatten().map(|c| estimate_tokens(&c.arguments)).sum::<usize>()
}

pub fn estimate_context(msgs: &[ChatMessage]) -> usize {
    2000 + msgs.iter().map(|m| msg_tokens(m) + m.images.as_ref().map(|i| i.len() * 1000).unwrap_or(0)).sum::<usize>()
}

/// (to summarise, keep). Tail: walk back until ≥ minTail tokens and ≥ 5 text messages, or maxTail; never start on a tool result.
pub fn split(msgs: &[ChatMessage], window: usize) -> (Vec<ChatMessage>, Vec<ChatMessage>) {
    let max_tail = 4000.max(40_000.min(window / 5));
    let min_tail = 2000.max(10_000.min(window / 12));
    let (mut tokens, mut texts, mut idx) = (0, 0, msgs.len());
    while idx > 0 {
        let m = &msgs[idx - 1];
        let t = msg_tokens(m);
        if tokens + t > max_tail { break; }
        tokens += t; idx -= 1;
        if (m.role == Role::User && !m.is_meta) || (m.role == Role::Assistant && !m.content.is_empty()) { texts += 1; }
        if tokens >= min_tail && texts >= 5 { break; }
    }
    while idx > 0 && idx < msgs.len() && msgs[idx].role == Role::Tool { idx -= 1; }
    if idx == 0 { return (vec![], msgs.to_vec()); }
    (msgs[..idx].to_vec(), msgs[idx..].to_vec())
}

pub fn microcompact(msgs: &mut [ChatMessage], keep_recent: usize, min_chars: usize) -> usize {
    let idx: Vec<usize> = msgs.iter().enumerate().filter(|(_, m)| m.role == Role::Tool).map(|(i, _)| i).collect();
    let mut saved = 0;
    for &i in idx.iter().take(idx.len().saturating_sub(keep_recent)) {
        let n = msgs[i].content.chars().count();
        if n > min_chars && !msgs[i].content.starts_with("[已清理") {
            saved += n;
            let head = crate::text::prefix(&msgs[i].content, 300);
            msgs[i].content = format!("[已清理的旧工具输出 / old tool output cleared] 开头：{head}…（需要时请重新查询）");
            msgs[i].images = None;
        }
    }
    saved
}

pub fn prune_images(msgs: &mut [ChatMessage], keep: usize) {
    let idx: Vec<usize> = msgs.iter().enumerate().filter(|(_, m)| m.images.as_ref().map(|i| !i.is_empty()).unwrap_or(false)).map(|(i, _)| i).collect();
    for &i in idx.iter().take(idx.len().saturating_sub(keep)) {
        msgs[i].images = None;
        msgs[i].content += "\n[旧截图已移除 / old screenshot removed]";
    }
}

pub fn render(msgs: &[ChatMessage]) -> String {
    msgs.iter().filter_map(|m| match m.role {
        Role::User => Some(format!("{}{}", if m.is_meta { "[context] " } else { "USER: " }, truncate_middle(&m.content, 4000))),
        Role::Assistant => {
            let mut s = format!("ASSISTANT: {}", truncate_middle(&m.content, 3000));
            for c in m.tool_calls.iter().flatten() { s += &format!("\n  → {}({})", c.name, truncate_middle(&c.arguments, 1500)); }
            Some(s)
        }
        Role::Tool => Some(format!("TOOL[{}]: {}", m.name.clone().unwrap_or_default(), truncate_middle(&m.content, 1500))),
        Role::System => None,
    }).collect::<Vec<_>>().join("\n\n")
}

pub fn extract_summary(text: &str) -> String {
    let mut t = text.to_string();
    if let (Some(a), Some(b)) = (t.find("<analysis>"), t.find("</analysis>")) { if a <= b { t.replace_range(a..b + 11, ""); } }
    // cut off inside the draft (output limit): no summary at all — a half-written <analysis> is not a handoff
    if t.contains("<analysis>") && !t.contains("<summary>") { return String::new(); }
    if let Some(a) = t.find("<summary>") {
        t = t[a + 9..].to_string();
        if let Some(b) = t.find("</summary>") { t.truncate(b); }
    }
    t.trim().to_string()
}

/// Token budget for the user's own words kept verbatim after compaction (Codex keeps ≤ 20k).
pub fn retain_budget(window: usize) -> usize { 20_000.min(window / 10) }

/// Codex-style: the user's own messages survive compaction verbatim — newest first within `budget` tokens, returned in
/// chronological order. Only real user messages (no injected context, previous summaries or images); a single message
/// larger than what is left is skipped rather than cut.
pub fn retained_user_messages(head: &[ChatMessage], budget: usize) -> Vec<ChatMessage> {
    let mut out = vec![];
    let mut left = budget;
    for m in head.iter().rev() {
        if left < 50 { break; }
        if m.role != Role::User || m.is_meta || m.content.trim().is_empty() { continue; }
        let t = msg_tokens(m);
        if t > left { continue; }
        left -= t;
        let mut c = m.clone(); c.images = None;
        out.push(c);
    }
    out.reverse();
    out
}

/// Drop the oldest messages (a previous summary at the front is kept) until the rendered text fits `limit` tokens.
/// Codex does the same when the compaction request itself overflows: trim from the start, keep the recent part.
pub fn fit_head(head: &[ChatMessage], limit: usize) -> Vec<ChatMessage> {
    let pinned = head.first().filter(|m| m.is_meta && m.content.starts_with(SUMMARY_PREFIX)).cloned();
    let rest = if pinned.is_some() { &head[1..] } else { head };
    let cost = |m: &ChatMessage| estimate_tokens(&render(std::slice::from_ref(m))) + 2;
    let mut total = pinned.as_ref().map(cost).unwrap_or(0) + rest.iter().map(cost).sum::<usize>();
    let mut start = 0;
    while total > limit && start + 1 < rest.len() { total -= cost(&rest[start]); start += 1; }
    pinned.into_iter().chain(rest[start..].iter().cloned()).collect()
}

/// Summary call. The conversation is fitted to the summarizer's own window (tokens, not characters); when the server
/// still reports "context too long", the budget shrinks and the oldest messages go first (up to 3 retries).
pub fn summarize(paths: &Paths, head: &[ChatMessage], notes: Option<&str>, client: &Client, model: &ModelRef) -> Result<String, String> {
    let mut m = model.clone();
    let mut caps = m.caps();
    caps.thinking_enabled = false;
    // room for the <analysis> draft and, on always-thinking models, the reasoning before it (glm-5.3-flash spent
    // all of 4096 on reasoning); non-thinking models stop by themselves, this is only a cap
    caps.max_output_tokens = 12_000.min(caps.context_window / 4).max(4096);
    let sys = crate::prompt::prompt(paths, "compact");
    let notes_block = notes.filter(|n| !n.is_empty()).map(|n| format!("\n\n当前会话笔记（口径以此为准）/ Session notes:\n{n}")).unwrap_or_default();
    let fixed = caps.max_output_tokens + estimate_tokens(&sys) + estimate_tokens(&notes_block) + 500;
    let mut limit = (caps.context_window.saturating_sub(fixed) * 85 / 100).max(2000);
    m.capabilities = Some(caps);
    let mut grown = false;
    for attempt in 0..5 {
        let h = fit_head(head, limit);
        let dropped = head.len() - h.len();
        let omitted = if dropped > 0 { format!("（最早的 {dropped} 条消息超出总结模型的上下文，已省略 / {dropped} oldest messages omitted）\n\n") } else { String::new() };
        let user = format!("以下是需要压缩的对话 / Conversation to compact:\n\n{omitted}{}{notes_block}", render(&h));
        match client.complete(&m, &[ChatMessage::system(sys.clone()), ChatMessage::user(user)]) {
            Ok(r) => {
                let truncated = r.finish.as_deref() == Some("length") && !r.content.contains("</summary>");
                // cut off by the output limit: once more with twice the room (input budget shrinks accordingly)
                if truncated && !grown && caps_out(&m) * 2 <= caps_window(&m) / 2 {
                    grown = true;
                    let mut c = m.caps(); c.max_output_tokens *= 2; let add = c.max_output_tokens / 2; m.capabilities = Some(c);
                    limit = limit.saturating_sub(add).max(2000);
                    continue;
                }
                let s = extract_summary(&r.content);
                if s.is_empty() {
                    return Err(if crate::llm::out_of_budget(&r) || truncated {
                        format!("模型的输出额度（{} tokens）被思考过程用完，没有写出摘要 / output limit reached before the summary", caps_out(&m))
                    } else { "模型没有返回摘要 / empty summary".into() });
                }
                return Ok(s);
            }
            Err(LlmError::ContextTooLong(b)) if attempt < 4 => {
                limit = crate::llm::context_limit_from_error(&b).map(|n| n.saturating_sub(fixed) * 85 / 100).filter(|&n| n < limit).unwrap_or(limit * 2 / 3).max(1000);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("压缩请求仍超出模型上下文 / compaction request still too long".into())
}

fn caps_out(m: &ModelRef) -> usize { m.caps().max_output_tokens }
fn caps_window(m: &ModelRef) -> usize { m.caps().context_window }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_keeps_pairs() {
        let mut v = vec![];
        for i in 0..40 {
            v.push(ChatMessage::user(format!("q{i} {}", "x".repeat(4000))));
            let mut a = ChatMessage::new(Role::Assistant, "");
            a.tool_calls = Some(vec![crate::types::ToolCall { id: format!("c{i}"), name: "sql_query".into(), arguments: "{}".into() }]);
            v.push(a);
            v.push(ChatMessage::tool("r".repeat(2000), &format!("c{i}"), "sql_query", "t"));
        }
        let (h, t) = split(&v, 32768);
        assert!(!h.is_empty() && t[0].role != Role::Tool);
        assert_eq!(extract_summary("<analysis>x</analysis><summary> ok </summary>"), "ok");
        assert_eq!(extract_summary("<analysis>按时间顺序梳理…（被截断"), "");
        assert_eq!(extract_summary("<analysis>a</analysis><summary>1. 目标…（被截断"), "1. 目标…（被截断");
    }

    #[test]
    fn user_words_retained_and_head_fitted() {
        let mut head = vec![summary_message("old summary")];
        for i in 0..30 {
            head.push(ChatMessage::user(format!("要求{i}：金额按订单号去重 {}", "口径".repeat(200))));
            head.push(ChatMessage::new(Role::Assistant, "y".repeat(8000)));
        }
        let mut meta = ChatMessage::user("<context>x</context>"); meta.is_meta = true; head.push(meta);
        // newest user messages first within budget, chronological order, no meta / summary
        let kept = retained_user_messages(&head, 1500);
        assert!(!kept.is_empty() && kept.len() < 30);
        assert!(kept.last().unwrap().content.starts_with("要求29"));
        assert!(kept.iter().all(|m| m.role == Role::User && !m.is_meta));
        let n: Vec<usize> = kept.iter().map(|m| m.content[6..8].trim_end_matches('：').parse().unwrap_or(0)).collect();
        assert!(n.windows(2).all(|w| w[0] < w[1]));
        // fitting drops the oldest messages but keeps the previous summary and the newest message
        let fit = fit_head(&head, 6000);
        assert!(fit.len() < head.len() && fit[0].content.starts_with(SUMMARY_PREFIX));
        assert_eq!(fit.last().unwrap().content, "<context>x</context>");
        assert!(estimate_tokens(&render(&fit)) <= 6000 + 2 * fit.len());
    }
}
