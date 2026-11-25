//! Background memory work after a turn (port of Swift `MemoryExtractor` + `ProjectMemory.consolidateIfDue`).
//! Prompt shared with Swift: Resources/prompts/memory_extract.md. Consolidation (\"dreaming\") lives in dream.rs.
//! Storage difference (M1): the Swift core keeps user/dataset memories in a global SQLite store; the Rust core writes
//! them as topic files under AppSupport/memory/ (same format as project memory).
//! Trust rule (0.1.4): the extractor may *claim* `confirmed`, but the core only keeps it when the evidence (or the
//! rule) quotes something the user actually wrote in this part of the conversation; otherwise it is stored as
//! `pending` with origin \"助手推断\". Outside a project, inferred business rules are not written to the user layer.
//! Every run is logged to stderr (core.log): counts, file names and skip reasons, never memory bodies.
use serde_json::Value;
use std::path::Path;

use crate::events::{self, Emit};
use crate::llm::Client;
use crate::memory;
use crate::paths::Paths;
use crate::recall::extract_json;
use crate::text::truncate_middle;
use crate::types::{ChatMessage, ModelRef, Role};

fn quiet(model: &ModelRef, max_out: usize) -> ModelRef {
    let mut m = model.clone();
    let mut caps = m.caps();
    caps.thinking_enabled = false;
    caps.max_output_tokens = max_out;
    m.capabilities = Some(caps);
    m
}

/// User wording that signals something to remember (explicit request, standing preference or a correction).
pub const CUES: &[&str] = &["记住", "记一下", "以后", "默认", "每次", "所有项目", "一律", "不对", "错了", "应该是", "不要再", "别再",
                            "remember", "from now on", "always", "never again"];

pub fn has_cue(text: &str) -> bool { let l = text.to_lowercase(); CUES.iter().any(|c| l.contains(c)) }

/// Letters / digits / CJK only, lower-case (quotes and punctuation differ between the user and the extractor).
fn norm(s: &str) -> String { s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect() }

/// Quoted segments (「」 “” "") of `s`, or the whole text when nothing is quoted.
fn segments(s: &str) -> Vec<String> {
    let mut out = vec![];
    for (a, b) in [('「', '」'), ('“', '”'), ('"', '"'), ('『', '』')] {
        let mut rest = s;
        while let Some(i) = rest.find(a) {
            let after = &rest[i + a.len_utf8()..];
            let Some(j) = after.find(b) else { break };
            out.push(after[..j].to_string());
            rest = &after[j + b.len_utf8()..];
        }
    }
    if out.is_empty() { out.push(s.to_string()); }
    out
}

/// True when some quoted part of `evidence` really comes from the user's messages: the whole segment (≥ 4 chars after
/// normalising) or any 6-char window of it appears in a user message.
pub fn grounded(evidence: &str, users: &[String]) -> bool {
    let users: Vec<String> = users.iter().map(|u| norm(u)).collect();
    segments(evidence).iter().any(|seg| {
        let n: Vec<char> = norm(seg).chars().collect();
        if n.len() < 4 { return false; }
        let whole: String = n.iter().collect();
        if users.iter().any(|u| u.contains(&whole)) { return true; }
        n.len() >= 6 && n.windows(6).any(|w| { let w: String = w.iter().collect(); users.iter().any(|u| u.contains(&w)) })
    })
}

/// Returns true on success (the caller may advance its cursor), false on failure (retry next time).
/// `emit` (when given) gets one `memory_saved` event per topic written, so the chat says what was remembered.
pub fn extract(paths: &Paths, msgs: &[ChatMessage], client: &Client, model: &ModelRef, project_dir: Option<&Path>, emit: Option<&Emit>) -> bool {
    let users: Vec<String> = msgs.iter().filter(|m| m.role == Role::User && !m.is_meta).map(|m| m.content.clone()).collect();
    let convo = msgs.iter().filter(|m| (m.role == Role::User && !m.is_meta) || (m.role == Role::Assistant && !m.content.is_empty()))
        .map(|m| format!("{}: {}", if m.role == Role::User { "USER" } else { "ASSISTANT" }, truncate_middle(&memory::redact(&m.content), 1500)))
        .collect::<Vec<_>>().join("\n\n");
    if convo.is_empty() { return true; }
    let user_dir = memory::user_dir(&paths.app_support);
    let mut lines: Vec<String> = memory::topics(&user_dir, 200).iter().map(|t| format!("- [user] {} ({}) — {}", t.name, t.file, t.description)).collect();
    if let Some(d) = project_dir { lines.extend(memory::topics(d, 200).iter().map(|t| format!("- [project] {} ({}) — {}", t.name, t.file, t.description))); }
    let existing = if lines.is_empty() { "（无）".to_string() } else { truncate_middle(&lines.join("\n"), 8000) };
    let cue = users.iter().any(|u| has_cue(u));
    let hint = if cue { "\n\n注意：用户本轮出现了“记住 / 以后 / 默认 / 纠正”类说法，请认真判断是否需要保存或更新。" } else { "" };
    let where_ = if project_dir.is_some() { "当前对话属于一个项目" } else { "当前是普通对话（不属于任何项目）" };
    let user = format!("{where_}。\n已有记忆（避免重复）：\n{existing}\n\n新增对话：\n{convo}{hint}");
    let sys = crate::prompt::prompt(paths, "memory_extract");
    let r = match client.complete(&quiet(model, 1000), &[ChatMessage::system(sys), ChatMessage::user(user)]) {
        Ok(r) => r,
        Err(e) => { eprintln!("memory-extract: failed (model call): {}", crate::text::prefix(&e.to_string(), 200)); return false; }
    };
    let Some(items) = extract_json(&r.content).and_then(|j| j["memories"].as_array().cloned()) else {
        eprintln!("memory-extract: failed (reply is not the expected JSON, {} chars)", r.content.chars().count());
        return false;
    };
    let (mut saved, mut skipped) = (vec![], vec![]);
    for it in items {
        let Some(raw) = it["content"].as_str().filter(|s| !s.trim().is_empty()) else { skipped.push("空内容".to_string()); continue };
        let mut it = it.clone();
        let content = memory::redact(raw);
        let kind = it["kind"].as_str().unwrap_or("preference").to_string();
        let source = it["source"].as_str().unwrap_or("extracted").to_string();
        let mut scope = it["scope"].as_str().unwrap_or("user").to_string();
        // in a project, business definitions / rules / traps belong to the project memory
        let business = matches!(kind.as_str(), "definition" | "rule" | "trap");
        if project_dir.is_some() && business { scope = "project".into(); }
        // trust: confirmed only when the user's own words back it (evidence or the rule itself quotes them)
        let claimed = it["status"].as_str() == Some("confirmed") || (it["status"].as_str().is_none() && matches!(source.as_str(), "explicit" | "correction"));
        let from_user = grounded(it["evidence"].as_str().unwrap_or(""), &users) || grounded(&content, &users);
        let confirmed = claimed && from_user;
        let name = it["name"].as_str().map(String::from).unwrap_or_else(|| crate::text::prefix(&content, 20));
        // outside a project an inferred business rule would leak into every project: keep only what the user said
        if project_dir.is_none() && business && !confirmed {
            skipped.push(format!("{name}：普通对话里推断的业务口径，不写入用户记忆"));
            continue;
        }
        it["status"] = Value::from(if confirmed { "confirmed" } else { "pending" });
        it["origin"] = Value::from(if confirmed { "用户原话" } else if claimed { "助手推断（标为已确认，但找不到用户原话，已改为待确认）" } else { "助手推断（待用户确认）" });
        let ty = if source == "correction" || kind == "trap" { "feedback" } else if scope == "project" { "project" } else { "user" };
        let (dir, label) = match (scope.as_str(), project_dir) { ("project", Some(d)) => (d.to_path_buf(), "项目"), _ => (user_dir.clone(), "用户") };
        let body = entry_body(&content, &it);
        // `file` = update an existing topic (only if it really exists in that folder)
        let file = it["file"].as_str().map(|f| Path::new(f).file_name().and_then(|x| x.to_str()).unwrap_or(f).to_string())
            .filter(|f| dir.join(f).is_file());
        match memory::write(&dir, &name, &content, ty, &body, file.as_deref()) {
            Ok(t) => {
                if let Some(e) = emit {
                    e(events::memory_saved(&format!("🧠 已记住：{}（{label}记忆 · {}）— 不对可在右侧「记忆」面板修改或删除",
                                                    t.name, if confirmed { "你说的" } else { "推断，待确认" })));
                }
                saved.push(format!("{label}:{}{}", t.file, if confirmed { "" } else { "(pending)" }));
            }
            Err(e) => skipped.push(format!("{name}：写入失败 {e}")),
        }
    }
    eprintln!("memory-extract: ok, saved {} [{}], skipped {} [{}]{}", saved.len(), saved.join(", "), skipped.len(), skipped.join("; "),
              if cue { ", cue in user message" } else { "" });
    true
}

/// Entry format: rule (user wording kept) + **依据** + **来源** + **状态** + **How to apply** (old prompts: **Why**).
pub fn entry_body(content: &str, it: &Value) -> String {
    let f = |k: &str| it[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(memory::redact);
    let mut b = content.trim().to_string();
    if let Some(e) = f("evidence") { b += &format!("\n\n**依据：** {e}"); }
    if let Some(o) = f("origin") { b += &format!("\n\n**来源：** {o}"); }
    if let Some(w) = f("why") { b += &format!("\n\n**Why:** {w}"); }
    let status = f("status").filter(|s| s == "confirmed" || s == "pending")
        .or_else(|| it["source"].as_str().filter(|s| *s == "explicit" || *s == "correction").map(|_| "confirmed".to_string()));
    if let Some(s) = status { b += &format!("\n\n**状态：** {s}"); }
    if let Some(h) = f("how") { b += &format!("\n\n**How to apply:** {h}"); }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_format() {
        let it = serde_json::json!({"evidence":"用户：「C 级是测试账号」","status":"confirmed","how":"GMV 类问题"});
        let b = entry_body("GMV 不含 C 级客户", &it);
        assert!(b.starts_with("GMV 不含 C 级客户\n\n**依据：** 用户"));
        assert!(b.contains("**状态：** confirmed") && b.ends_with("**How to apply:** GMV 类问题"));
        let b = entry_body("x", &serde_json::json!({"source":"correction"}));
        assert_eq!(b, "x\n\n**状态：** confirmed");
        let b = entry_body("x", &serde_json::json!({"origin":"用户原话","status":"confirmed"}));
        assert_eq!(b, "x\n\n**来源：** 用户原话\n\n**状态：** confirmed");
    }

    #[test]
    fn grounding() {
        let users = vec!["以后金额都用万元展示，结论放最前面".to_string(), "对".to_string()];
        assert!(grounded("用户：「以后金额都用万元展示」", &users));
        assert!(grounded("用户说“金额都用万元”", &users));          // 6-char window
        assert!(!grounded("助手：「销售额含运费」，用户未纠正", &users)); // the assistant's words
        assert!(!grounded("用户回复「对」", &users));                  // too short to prove anything
        assert!(!grounded("", &users));
        assert!(has_cue("以后都这样") && has_cue("Remember this") && !has_cue("看下销售额"));
    }
}
