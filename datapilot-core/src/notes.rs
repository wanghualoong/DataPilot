//! Session notes (port of Swift `SessionNotes`): fixed-section Markdown, rewritten by the fast model on token
//! thresholds; used in the turn context and as a cheap handoff summary at compaction.
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::compact;
use crate::llm::Client;
use crate::paths::Paths;
use crate::text::estimate_tokens;
use crate::types::{ChatMessage, ModelRef};

pub const SECTION_CAP_CHARS: usize = 8000;
pub const INIT_THRESHOLD: usize = 10_000;
pub const UPDATE_DELTA: usize = 5_000;
pub const MAX_TOKENS: usize = 12_000;
pub const NOTES_SUMMARY_PREFIX: &str = "（以下为会话笔记，覆盖到压缩时刻；最近的消息原样保留在后面）\n\n";

#[derive(Default)]
struct State { last_update_tokens: usize, tool_calls: usize, updating_since: Option<Instant> }

pub struct SessionNotes { pub path: PathBuf, template: String, state: Mutex<State> }

fn squash(s: &str) -> String { s.chars().filter(|c| !c.is_whitespace()).collect() }

impl SessionNotes {
    pub fn new(path: PathBuf, paths: &Paths) -> Self {
        SessionNotes { path, template: crate::prompt::prompt(paths, "notes_template"), state: Mutex::new(State::default()) }
    }
    pub fn text(&self) -> Option<String> { std::fs::read_to_string(&self.path).ok().filter(|s| !s.is_empty()) }
    pub fn is_effectively_empty(&self) -> bool { self.text().map(|t| squash(&t) == squash(&self.template)).unwrap_or(true) }
    pub fn add_tool_calls(&self, n: usize) { self.state.lock().unwrap().tool_calls += n; }

    pub fn should_update(&self, context_tokens: usize, last_turn_had_tools: bool) -> bool {
        let s = self.state.lock().unwrap();
        if context_tokens < INIT_THRESHOLD { return false; }
        context_tokens.saturating_sub(s.last_update_tokens) >= UPDATE_DELTA && (s.tool_calls >= 3 || !last_turn_had_tools)
    }

    /// Rewrite the notes with the fast model (headers and italic hints kept). Returns true when the file was written.
    pub fn update(&self, paths: &Paths, recent: &[ChatMessage], client: &Client, model: &ModelRef, context_tokens: usize) -> bool {
        self.state.lock().unwrap().updating_since = Some(Instant::now());
        let ok = self.do_update(paths, recent, client, model, context_tokens);
        self.state.lock().unwrap().updating_since = None;
        ok
    }

    fn do_update(&self, paths: &Paths, recent: &[ChatMessage], client: &Client, model: &ModelRef, context_tokens: usize) -> bool {
        let current = self.text().unwrap_or_else(|| self.template.clone());
        let mut m = model.clone();
        let mut caps = m.caps();
        caps.thinking_enabled = false;
        caps.max_output_tokens = caps.max_output_tokens.min(3000);
        m.capabilities = Some(caps);
        let tail = &recent[recent.len().saturating_sub(40)..];
        let hint = if estimate_tokens(&current) > MAX_TOKENS { format!("\n笔记已超过 {MAX_TOKENS} tokens，必须压缩较长的章节。") } else { String::new() };
        let sys = crate::prompt::prompt(paths, "notes_update").replace("{{SIZE_HINT}}", &hint);
        let user = format!("当前笔记：\n{current}\n\n最近对话：\n{}", compact::render(tail));
        let Ok(r) = client.complete(&m, &[ChatMessage::system(sys), ChatMessage::user(user)]) else { return false };
        let mut out = r.content.trim().to_string();
        if out.starts_with("```") {
            out = out.lines().skip(1).collect::<Vec<_>>().join("\n");
            if let Some(s) = out.strip_suffix("```") { out = s.to_string(); }
        }
        if !out.contains("# ") { return false; }
        if let Some(p) = self.path.parent() { let _ = std::fs::create_dir_all(p); }
        if std::fs::write(&self.path, &out).is_err() { return false; }
        let mut s = self.state.lock().unwrap();
        s.last_update_tokens = context_tokens;
        s.tool_calls = 0;
        true
    }

    /// Wait for an in-flight update: at most `max_wait`; one running longer than `stale_after` counts as stuck.
    pub fn wait_if_updating(&self, max_wait: Duration, stale_after: Duration) {
        let t0 = Instant::now();
        loop {
            let since = self.state.lock().unwrap().updating_since;
            match since {
                Some(s) if s.elapsed() < stale_after && t0.elapsed() < max_wait => std::thread::sleep(Duration::from_millis(200)),
                _ => return,
            }
        }
    }
}

/// Deterministic per-section cap applied before the notes replace a summary at compaction.
pub fn capped_sections(text: &str, cap: usize) -> String {
    let mut out: Vec<String> = vec![];
    let mut cur: Vec<&str> = vec![];
    let flush = |cur: &mut Vec<&str>, out: &mut Vec<String>| {
        if cur.is_empty() { return; }
        let sec = cur.join("\n");
        out.push(if sec.chars().count() > cap { crate::text::prefix(&sec, cap) + "\n…（本节过长，已截断）" } else { sec });
        cur.clear();
    };
    for line in text.split('\n') {
        if line.starts_with("# ") { flush(&mut cur, &mut out); }
        cur.push(line);
    }
    flush(&mut cur, &mut out);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caps_and_thresholds() {
        let t = format!("# A\n{}\n# B\nok", "x".repeat(50));
        let c = capped_sections(&t, 20);
        assert!(c.contains("本节过长") && c.ends_with("# B\nok"));
        let paths = Paths::dev(std::env::temp_dir());
        let n = SessionNotes::new(std::env::temp_dir().join(format!("notes-{}.md", crate::text::short_id(6))), &paths);
        assert!(n.is_effectively_empty());
        assert!(!n.should_update(9_000, false));
        assert!(n.should_update(12_000, false));
        assert!(!n.should_update(12_000, true));
        n.add_tool_calls(3);
        assert!(n.should_update(12_000, true));
    }
}
