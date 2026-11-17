//! Background memory work after a turn (port of Swift `MemoryExtractor` + `ProjectMemory.consolidateIfDue`).
//! Prompt shared with Swift: Resources/prompts/memory_extract.md. Consolidation ("dreaming") lives in dream.rs.
//! Storage difference (M1): the Swift core keeps user/dataset memories in a global SQLite store; the Rust core writes
//! them as topic files under AppSupport/memory/ (same format as project memory).
use serde_json::Value;
use std::path::Path;

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

/// Returns true on success (the caller may advance its cursor), false on failure (retry next time).
pub fn extract(paths: &Paths, msgs: &[ChatMessage], client: &Client, model: &ModelRef, project_dir: Option<&Path>) -> bool {
    let convo = msgs.iter().filter(|m| (m.role == Role::User && !m.is_meta) || (m.role == Role::Assistant && !m.content.is_empty()))
        .map(|m| format!("{}: {}", if m.role == Role::User { "USER" } else { "ASSISTANT" }, truncate_middle(&memory::redact(&m.content), 1500)))
        .collect::<Vec<_>>().join("\n\n");
    if convo.is_empty() { return true; }
    let user_dir = memory::user_dir(&paths.app_support);
    let mut lines: Vec<String> = memory::topics(&user_dir, 200).iter().map(|t| format!("- [user] {} — {}", t.name, t.description)).collect();
    if let Some(d) = project_dir { lines.extend(memory::topics(d, 200).iter().map(|t| format!("- [project] {} — {}", t.name, t.description))); }
    let existing = if lines.is_empty() { "（无）".to_string() } else { truncate_middle(&lines.join("\n"), 8000) };
    let user = format!("已有记忆（避免重复）：\n{existing}\n\n新增对话：\n{convo}");
    let sys = crate::prompt::prompt(paths, "memory_extract");
    let Ok(r) = client.complete(&quiet(model, 1000), &[ChatMessage::system(sys), ChatMessage::user(user)]) else { return false };
    let Some(items) = extract_json(&r.content).and_then(|j| j["memories"].as_array().cloned()) else { return false };
    for it in items {
        let Some(raw) = it["content"].as_str().filter(|s| !s.is_empty()) else { continue };
        let content = memory::redact(raw);
        let kind = it["kind"].as_str().unwrap_or("preference");
        let source = it["source"].as_str().unwrap_or("extracted");
        let mut scope = it["scope"].as_str().unwrap_or("user").to_string();
        // in a project, business definitions / rules / traps belong to the project memory
        if project_dir.is_some() && matches!(kind, "definition" | "rule" | "trap") { scope = "project".into(); }
        let name = it["name"].as_str().map(String::from).unwrap_or_else(|| crate::text::prefix(&content, 20));
        let ty = if source == "correction" || kind == "trap" { "feedback" } else if scope == "project" { "project" } else { "user" };
        let dir = match (scope.as_str(), project_dir) { ("project", Some(d)) => d.to_path_buf(), _ => user_dir.clone() };
        let body = entry_body(&content, &it);
        // `file` = update an existing topic (only if it really exists in that folder)
        let file = it["file"].as_str().map(|f| Path::new(f).file_name().and_then(|x| x.to_str()).unwrap_or(f).to_string())
            .filter(|f| dir.join(f).is_file());
        let _ = memory::write(&dir, &name, &content, ty, &body, file.as_deref());
    }
    true
}

/// Entry format: rule (user wording kept) + **依据** + **状态** + **How to apply** (old prompts: **Why**).
pub fn entry_body(content: &str, it: &Value) -> String {
    let f = |k: &str| it[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(memory::redact);
    let mut b = content.trim().to_string();
    if let Some(e) = f("evidence") { b += &format!("\n\n**依据：** {e}"); }
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
    }
}
