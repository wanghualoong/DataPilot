//! "Dreaming" — offline consolidation of a project memory folder (memory plan B3).
//! Inputs: the topic files (SQL cited as evidence is re-run read-only and the result attached), and excerpts of the
//! sessions touched since the last run. The fast model merges duplicates, resolves conflicts, records rules the user
//! gave but nobody saved, and maintains a short always-loaded "core" block at the top of MEMORY.md.
//! Safety: lock file, snapshot → restore on any failure or on a suspicious mass delete; the pre-dream state is kept in
//! `<dir>.prev` so `/dream undo` can roll back. Triggers: after a turn (≥ 24 h + ≥ 5 sessions), idle (server, ≥ 6 h and
//! memory changed), manual `/dream`.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::kernel::KernelClient;
use crate::llm::Client;
use crate::memory;
use crate::paths::Paths;
use crate::recall::extract_json;
use crate::text::{now, prefix, truncate_middle};
use crate::types::{ChatMessage, ModelRef};

pub const CORE_START: &str = "<!-- core:start 核心口径（整理时自动维护，始终加载） -->";
pub const CORE_END: &str = "<!-- core:end -->";
const CORE_MAX: usize = 8;
const MAX_NEW: usize = 3;
const MAX_SQL: usize = 10;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Trigger { Due, Idle, Manual }

/// Everything a dream needs, detached from the agent (so it can run without holding the session lock).
#[derive(Clone)]
pub struct Ctx {
    pub paths: Paths,
    pub dir: PathBuf,
    pub sessions_dir: PathBuf,
    pub current: String,
    pub kernel: Option<KernelClient>,
    pub client: Arc<Client>,
    pub model: ModelRef,
}

#[derive(Default, Debug, Clone)]
pub struct Report {
    pub updated: Vec<String>,
    pub created: Vec<String>,
    pub deleted: Vec<String>,
    pub core: usize,
    pub sql_ok: usize,
    pub sql_failed: usize,
    pub sessions: usize,
    pub skipped: Option<String>,
}

impl Report {
    pub fn changed(&self) -> bool { !self.updated.is_empty() || !self.created.is_empty() || !self.deleted.is_empty() }
    pub fn text(&self) -> String {
        if let Some(s) = &self.skipped { return format!("🌙 记忆整理未执行：{s}"); }
        let mut s = format!("🌙 记忆整理完成：参考了 {} 个最近会话，复核 SQL {} 条（{} 条无法执行）", self.sessions, self.sql_ok + self.sql_failed, self.sql_failed);
        let list = |v: &Vec<String>| v.join("、");
        if !self.updated.is_empty() { s += &format!("；更新 {}", list(&self.updated)); }
        if !self.created.is_empty() { s += &format!("；新增 {}", list(&self.created)); }
        if !self.deleted.is_empty() { s += &format!("；删除 {}", list(&self.deleted)); }
        if !self.changed() { s += "；没有需要修改的条目"; }
        s += &format!("；核心口径 {} 条。可用 /dream undo 撤销。", self.core);
        s
    }
}

fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn last_run(d: &Path) -> f64 {
    std::fs::read_to_string(d.join(".last_consolidation")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0.0)
}

/// Session folders (other than `current`) whose transcript changed after `since`, newest first.
fn sessions_since(sessions_dir: &Path, current: &str, since: f64) -> Vec<(f64, PathBuf)> {
    let mut v: Vec<(f64, PathBuf)> = std::fs::read_dir(sessions_dir).into_iter().flatten().flatten().filter_map(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with('.') || n == current || !e.path().is_dir() { return None; }
        let t = e.path().join("transcript.jsonl");
        let m = if t.exists() { mtime(&t) } else { mtime(&e.path()) };
        (m > since).then_some((m, e.path()))
    }).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// After-turn gate: ≥ 24 h since the last run and ≥ 5 other sessions active since then.
pub fn due(d: &Path, sessions_dir: &Path, current: &str, now: f64) -> bool {
    let last = last_run(d);
    now - last >= 24.0 * 3600.0 && sessions_since(sessions_dir, current, last).len() >= 5
}

/// Idle gate: ≥ 6 h since the last run and some memory topic changed since then.
pub fn idle_due(d: &Path, now: f64) -> bool {
    let last = last_run(d);
    now - last >= 6.0 * 3600.0 && memory::topics(d, 200).iter().any(|t| t.modified > last)
}

static LAST_SCAN: Mutex<Vec<(PathBuf, f64)>> = Mutex::new(Vec::new());

/// The after-turn gate scans the sessions folder at most every 10 minutes per memory folder.
fn claim_scan(d: &Path, now: f64) -> bool {
    let mut g = LAST_SCAN.lock().unwrap();
    if let Some(e) = g.iter_mut().find(|e| e.0 == d) { if now - e.1 < 600.0 { return false; } e.1 = now; } else { g.push((d.to_path_buf(), now)); }
    true
}

/// Recent conversation excerpts (user + assistant text, secrets redacted): (sessions used, text).
pub fn excerpts(sessions_dir: &Path, current: &str, since: f64, max_sessions: usize, budget: usize) -> (usize, String) {
    let since = since.max(now() - 30.0 * 86400.0);
    let mut blocks = vec![];
    for (m, dir) in sessions_since(sessions_dir, current, since).into_iter().take(max_sessions) {
        let Ok(text) = std::fs::read_to_string(dir.join("transcript.jsonl")) else { continue };
        let lines: Vec<String> = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|r| r["type"] == "message").filter_map(|r| {
            let msg = &r["message"];
            let c = msg["content"].as_str().unwrap_or("").trim();
            if c.is_empty() || msg["isMeta"].as_bool().unwrap_or(false) { return None; }
            match msg["role"].as_str()? { "user" => Some(format!("U: {}", prefix(c, 400))), "assistant" => Some(format!("A: {}", prefix(c, 300))), _ => None }
        }).collect();
        if lines.is_empty() { continue; }
        let (y, mo, d, ..) = crate::text::local_time(m);
        blocks.push(format!("### 会话 {y:04}-{mo:02}-{d:02}\n{}", truncate_middle(&memory::redact(&lines.join("\n")), 2500)));
    }
    (blocks.len(), truncate_middle(&blocks.join("\n\n"), budget))
}

/// Complete SQL statements cited in a memory body (```sql fences and inline `SELECT …`), max 2 per topic.
pub fn sqls(body: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<(regex::Regex, regex::Regex)> = OnceLock::new();
    let (fence, inline) = RE.get_or_init(|| (regex::Regex::new(r"(?s)```(?:sql|SQL)?\s*\n(.*?)```").unwrap(),
                                              regex::Regex::new(r"(?i)`((?:SELECT|WITH)\b[^`]{10,})`").unwrap()));
    let mut out: Vec<String> = vec![];
    for c in fence.captures_iter(body).chain(inline.captures_iter(body)) {
        let s = c[1].trim().trim_end_matches(';').trim().to_string();
        let head = s.trim_start().to_uppercase();
        if (head.starts_with("SELECT") || head.starts_with("WITH")) && !out.contains(&s) { out.push(s); }
        if out.len() >= 2 { break; }
    }
    out
}

fn check_sql(k: &KernelClient, sql: &str) -> Result<String, String> {
    let r = k.call("sql", json!({"sql": sql, "limit": 3, "save": false}), 30)?.result;
    Ok(format!("{} 行；前几行 {}", r["row_count"].as_i64().unwrap_or(0), prefix(&r["rows"].to_string(), 240)))
}

/// Replace the always-loaded core block at the top of MEMORY.md (lines sanitised so they never look like index lines).
pub fn set_core(d: &Path, lines: &[String]) -> std::io::Result<()> {
    let u = d.join("MEMORY.md");
    let cur = std::fs::read_to_string(&u).unwrap_or_default();
    let rest = match (cur.find(CORE_START), cur.find(CORE_END)) {
        (Some(a), Some(b)) if a < b => format!("{}{}", &cur[..a], &cur[b + CORE_END.len()..]),
        _ => cur,
    };
    let rest = rest.trim_matches('\n');
    let clean: Vec<String> = lines.iter().map(|l| prefix(&memory::redact(l.trim().trim_start_matches(['-', '*', ' '])), 120).replace("](", "] (").replace('\n', " "))
        .filter(|l| !l.is_empty()).take(CORE_MAX).collect();
    let mut out = String::new();
    if !clean.is_empty() {
        out += &format!("{CORE_START}\n## 核心口径（每次分析都要遵守；细节见下面对应条目）\n{}\n{CORE_END}\n\n", clean.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"));
    }
    out += rest;
    std::fs::write(&u, out.trim_matches('\n').to_string() + "\n")
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        if e.path().is_file() { std::fs::copy(e.path(), to.join(e.file_name()))?; }
    }
    Ok(())
}

fn restore(d: &Path, from: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        if e.path().is_file() && !e.file_name().to_string_lossy().starts_with(".consolidate") { let _ = std::fs::remove_file(e.path()); }
    }
    copy_dir(from, d)
}

fn sibling(d: &Path, suffix: &str) -> PathBuf {
    d.with_file_name(format!("{}.{suffix}", d.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| "memory".into())))
}

/// Roll back to the state before the last dream. Returns the number of topic files restored.
pub fn rollback(d: &Path) -> Result<usize, String> {
    let prev = sibling(d, "prev");
    if !prev.is_dir() { return Err("没有可撤销的整理 / nothing to undo".into()); }
    restore(d, &prev).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&prev);
    Ok(memory::topics(d, 1000).len())
}

fn quiet(model: &ModelRef, max_out: usize) -> ModelRef {
    let mut m = model.clone();
    let mut caps = m.caps();
    caps.thinking_enabled = false;
    caps.max_output_tokens = max_out;
    m.capabilities = Some(caps);
    m
}

pub fn run(ctx: &Ctx, trigger: Trigger) -> Report {
    let d = &ctx.dir;
    let t0 = now();
    let skip = |s: &str| Report { skipped: Some(s.into()), ..Default::default() };
    match trigger {
        Trigger::Due => if !claim_scan(d, t0) || !due(d, &ctx.sessions_dir, &ctx.current, t0) { return skip("未到时间"); },
        Trigger::Idle => if !idle_due(d, t0) { return skip("未到时间"); },
        Trigger::Manual => {}
    }
    let ts = memory::topics(d, 200);
    if ts.is_empty() || (ts.len() < 2 && trigger != Trigger::Manual) { return skip("还没有足够的项目记忆"); }
    let lock = d.join(".consolidate.lock");
    if let Some(t) = std::fs::read_to_string(&lock).ok().and_then(|l| l.split(' ').last().and_then(|x| x.trim().parse::<f64>().ok())) {
        if t0 - t < 3600.0 { return skip("另一个整理正在进行"); }
    }
    let _ = std::fs::write(&lock, format!("{} {t0}", std::process::id()));
    let snap = sibling(d, "snapshot");
    let _ = std::fs::remove_dir_all(&snap);
    if copy_dir(d, &snap).is_err() { let _ = std::fs::remove_file(&lock); return skip("无法创建快照"); }

    let mut rep = Report::default();
    let mut sql_budget = MAX_SQL;
    let listing = ts.iter().map(|t| {
        let (y, m, dd, ..) = crate::text::local_time(t.modified);
        let mut s = format!("### {} [{}] 修改于 {y:04}-{m:02}-{dd:02}\n{} — {}\n{}", t.file, t.kind, t.name, t.description, prefix(&t.body, 1500));
        if let Some(k) = &ctx.kernel {
            for q in sqls(&t.body) {
                if sql_budget == 0 { break; }
                sql_budget -= 1;
                match check_sql(k, &q) {
                    Ok(r) => { rep.sql_ok += 1; s += &format!("\n> SQL 复核（现在重新执行）：{r}"); }
                    Err(e) => { rep.sql_failed += 1; s += &format!("\n> SQL 复核：无法执行（{}）", prefix(&e.replace('\n', " "), 150)); }
                }
            }
        }
        s
    }).collect::<Vec<_>>().join("\n\n");
    let (n, ex) = excerpts(&ctx.sessions_dir, &ctx.current, last_run(d), 10, 12_000);
    rep.sessions = n;
    let user = format!("## 记忆条目（{} 个）\n{listing}\n\n## 最近会话摘录（{n} 个会话）\n{}", ts.len(), if ex.is_empty() { "（无）".into() } else { ex });
    let (y, m, dd, ..) = crate::text::local_time(t0);
    let sys = crate::prompt::prompt(&ctx.paths, "memory_consolidate").replace("{{TODAY}}", &format!("{y:04}-{m:02}-{dd:02}"));
    let existing: Vec<String> = ts.iter().map(|t| t.file.clone()).collect();

    let apply = |rep: &mut Report| -> Result<(), String> {
        let r = ctx.client.complete(&quiet(&ctx.model, 4000), &[ChatMessage::system(sys.clone()), ChatMessage::user(user.clone())]).map_err(|e| e.to_string())?;
        let j: Value = extract_json(&r.content).ok_or("整理结果不是 JSON / bad json")?;
        let deletes: Vec<String> = crate::tools::strs(&j["delete"]).into_iter()
            .map(|f| Path::new(&f).file_name().and_then(|x| x.to_str()).unwrap_or(&f).to_string()).filter(|f| existing.contains(f)).collect();
        if trigger != Trigger::Manual && deletes.len() * 2 > existing.len() { return Err(format!("整理要删除 {} / {} 条记忆，已放弃", deletes.len(), existing.len())); }
        for u in j["update"].as_array().into_iter().flatten() {
            let (Some(f), Some(body)) = (u["file"].as_str(), u["body"].as_str().filter(|b| !b.trim().is_empty())) else { continue };
            let f = Path::new(f).file_name().and_then(|x| x.to_str()).unwrap_or(f).to_string();
            let is_new = !existing.contains(&f) && !f.ends_with("MEMORY.md");
            if is_new && rep.created.len() >= MAX_NEW { continue; }
            if deletes.contains(&f) { continue; }
            // file name no longer matches the content (e.g. "gmv-旧口径.md" now holding the new rule) → rename
            let rename = u["rename"].as_str().map(|r| Path::new(r).file_name().and_then(|x| x.to_str()).unwrap_or(r).to_string())
                .filter(|r| !is_new && r.ends_with(".md") && r != &f && r != "MEMORY.md" && !existing.contains(r) && !d.join(r).exists());
            let target = rename.clone().unwrap_or_else(|| f.clone());
            let t = memory::write(d, u["name"].as_str().unwrap_or(&f), u["description"].as_str().unwrap_or(""), u["type"].as_str().unwrap_or("project"), body, Some(&target))?;
            if rename.is_some() { memory::delete(d, &f)?; rep.updated.push(format!("{f}→{}", t.file)); }
            else if is_new { rep.created.push(t.file) } else { rep.updated.push(t.file) }
        }
        for f in &deletes { memory::delete(d, f)?; rep.deleted.push(f.clone()); }
        let core = crate::tools::strs(&j["core"]);
        if !core.is_empty() || j.get("core").is_some() {
            set_core(d, &core).map_err(|e| e.to_string())?;
            rep.core = core.len().min(CORE_MAX);
        } else if let Ok(t) = std::fs::read_to_string(d.join("MEMORY.md")) {
            rep.core = t.find(CORE_START).map(|a| t[a..].split(CORE_END).next().unwrap_or("").lines().filter(|l| l.starts_with("- ")).count()).unwrap_or(0);
        }
        std::fs::write(d.join(".last_consolidation"), now().to_string()).map_err(|e| e.to_string())?;
        let log = json!({"time": t0, "trigger": format!("{trigger:?}"), "updated": rep.updated, "created": rep.created, "deleted": rep.deleted,
                         "core": rep.core, "sql_ok": rep.sql_ok, "sql_failed": rep.sql_failed, "sessions": rep.sessions});
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(d.join(".dreams.jsonl")) { let _ = writeln!(f, "{log}"); }
        Ok(())
    };
    match apply(&mut rep) {
        Ok(()) => {
            let prev = sibling(d, "prev");
            let _ = std::fs::remove_dir_all(&prev);
            if std::fs::rename(&snap, &prev).is_err() { let _ = std::fs::remove_dir_all(&snap); }
        }
        Err(e) => {
            let _ = restore(d, &snap);
            let _ = std::fs::remove_dir_all(&snap);
            rep = Report { skipped: Some(format!("失败，已恢复原状：{e}")), ..Default::default() };
        }
    }
    let _ = std::fs::remove_file(&lock);
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf { std::env::temp_dir().join(format!("dpdream-{tag}-{}", crate::text::short_id(6))) }

    #[test]
    fn gate() {
        let base = tmp("gate");
        let (d, s) = (base.join("memory"), base.join("sessions"));
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..4 { std::fs::create_dir_all(s.join(format!("s{i}"))).unwrap(); }
        assert!(!due(&d, &s, "s0", now()));
        std::fs::create_dir_all(s.join("s4")).unwrap();
        std::fs::create_dir_all(s.join("s5")).unwrap();
        assert!(due(&d, &s, "s0", now()));
        std::fs::write(d.join(".last_consolidation"), now().to_string()).unwrap();
        assert!(!due(&d, &s, "s0", now()));
        // idle: needs 6 h and a changed topic
        std::fs::write(d.join(".last_consolidation"), (now() - 7.0 * 3600.0).to_string()).unwrap();
        assert!(!idle_due(&d, now()));
        memory::write(&d, "GMV 口径", "x", "project", "GMV 只算已支付", None).unwrap();
        assert!(idle_due(&d, now()));
    }

    #[test]
    fn sql_extraction() {
        let body = "规则\n```sql\nSELECT count(*) FROM orders WHERE 状态='paid';\n```\n或 `SELECT sum(x) FROM t WHERE y=1` 以及片段 `WHERE a=1`";
        let v = sqls(body);
        assert_eq!(v, vec!["SELECT count(*) FROM orders WHERE 状态='paid'".to_string(), "SELECT sum(x) FROM t WHERE y=1".to_string()]);
        assert!(sqls("没有 SQL").is_empty());
    }

    #[test]
    fn core_block() {
        let d = tmp("core");
        memory::write(&d, "GMV 口径", "GMV 只算已支付", "project", "x", None).unwrap();
        set_core(&d, &["GMV = paid 且不含 C 级".into(), "- 目标表单位是万元".into(), "坏链接 [a](b.md)".into()]).unwrap();
        let t = std::fs::read_to_string(d.join("MEMORY.md")).unwrap();
        assert!(t.starts_with(CORE_START) && t.contains("- 目标表单位是万元") && t.contains("[a] (b.md)"));
        // index maintenance keeps the block; a second set_core replaces it
        memory::write(&d, "活跃客户", "≥25 笔", "project", "y", None).unwrap();
        set_core(&d, &["只有一条".into()]).unwrap();
        let t = std::fs::read_to_string(d.join("MEMORY.md")).unwrap();
        assert_eq!(t.matches(CORE_START).count(), 1);
        assert!(t.contains("只有一条") && !t.contains("万元") && t.contains("](gmv-口径.md)") && t.contains("](活跃客户.md)"));
        set_core(&d, &[]).unwrap();
        assert!(!std::fs::read_to_string(d.join("MEMORY.md")).unwrap().contains(CORE_START));
    }

    #[test]
    fn transcript_excerpts() {
        let s = tmp("ex");
        let d = s.join("a1");
        std::fs::create_dir_all(&d).unwrap();
        let lines = [
            json!({"type": "message", "message": {"id": "1", "role": "user", "content": "<context>…</context>", "isMeta": true}}),
            json!({"type": "message", "message": {"id": "2", "role": "user", "content": "GMV 不含 C 级，token=abcdef123456"}}),
            json!({"type": "message", "message": {"id": "3", "role": "assistant", "content": "好的"}}),
            json!({"type": "message", "message": {"id": "4", "role": "tool", "content": "row_count=3"}}),
        ];
        std::fs::write(d.join("transcript.jsonl"), lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
        let (n, t) = excerpts(&s, "other", 0.0, 10, 5000);
        assert_eq!(n, 1);
        assert!(t.contains("U: GMV 不含 C 级") && t.contains("A: 好的") && !t.contains("<context>") && !t.contains("row_count") && !t.contains("abcdef123456"));
        assert_eq!(excerpts(&s, "a1", 0.0, 10, 5000).0, 0);
    }

    #[test]
    fn undo() {
        let d = tmp("undo").join("memory");
        memory::write(&d, "A", "a", "project", "old", None).unwrap();
        let prev = sibling(&d, "prev");
        copy_dir(&d, &prev).unwrap();
        memory::write(&d, "A", "a", "project", "new", Some("a.md")).unwrap();
        memory::write(&d, "B", "b", "project", "b", None).unwrap();
        assert_eq!(rollback(&d).unwrap(), 1);
        assert!(std::fs::read_to_string(d.join("a.md")).unwrap().contains("old") && !d.join("b.md").exists());
        assert!(rollback(&d).is_err());
    }
}
