//! Planning / file / memory tools (ports of the Swift AgentTools used by the data workflow).
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use super::{imp, plain, strs, sv, Impl, ToolCtx, ToolResult};
use crate::permissions::Mode;
use crate::types::arr;
use crate::{events, memory, prompt, text};

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("memory_write", imp(memory_write, |a| format!("记住：{}", a["name"].as_str().or(a["content"].as_str()).unwrap_or(""))));
    m.insert("memory_search", imp(memory_search, |a| format!("检索记忆 {}", sv(a, "query"))));
    m.insert("memory_forget", imp(memory_forget, |a| format!("删除记忆 {}", a["file"].as_str().map(String::from).unwrap_or_else(|| a["id"].to_string()))));
    m.insert("todo_write", plain(todo_write));
    m.insert("ask_user", plain(ask_user));
    m.insert("exit_plan_mode", plain(exit_plan_mode));
    m.insert("skill", plain(skill));
    m.insert("list_files", plain(list_files));
    m.insert("read_file", plain(read_file));
    m.insert("grep", imp(grep, |a| format!("grep {}{}", sv(a, "pattern"), a["glob"].as_str().map(|g| format!(" ({g})")).unwrap_or_default())));
    m.insert("glob", imp(glob, |a| format!("glob {}", sv(a, "pattern"))));
    m.insert("write_file", imp(write_file, |a| format!("写入 {}", sv(a, "path"))));
    // the sub-agent loop needs the session (model, registry, permission gate): executed by Agent::run_subagent
    m.insert("agent", imp(|_, _| Ok(ToolResult::err("agent tool is run by the agent loop")), crate::subagent::summary));
    m.insert("save_skill", imp(save_skill, |a| format!("保存 Skill：{}", sv(a, "name"))));
    m.insert("history_search", plain(history_search));
    m.insert("bash", imp(bash, |a| format!("$ {}", text::prefix(&sv(a, "command"), 120))));
}

// ---------- paths ----------
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c { Component::ParentDir => { out.pop(); } Component::CurDir => {} c => out.push(c) }
    }
    out
}

/// Canonical form of a possibly non-existing path (canonicalise the existing parent).
pub fn canon(p: &Path) -> PathBuf {
    if let Ok(c) = crate::platform::canonicalize(p) { return c; }
    let n = normalize(p);
    match (n.parent(), n.file_name()) {
        (Some(par), Some(f)) => crate::platform::canonicalize(par).map(|c| c.join(f)).unwrap_or(n.clone()),
        _ => n,
    }
}

/// Same rule as the Swift `resolveIn`: the path must stay inside one of the roots.
pub fn resolve_in(p: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    for (i, ws) in roots.iter().enumerate() {
        let u = if Path::new(p).is_absolute() { PathBuf::from(p) } else { ws.join(p) };
        let (sp, root) = (canon(&u), canon(ws));
        if !(sp == root || sp.starts_with(&root)) { continue; }
        if Path::new(p).is_absolute() || i == roots.len() - 1 || sp.exists() { return Ok(sp); }
    }
    Err(format!("路径必须在工作区/项目内 / path must be inside the workspace or project: {p}"))
}

pub fn write_url(c: &ToolCtx, p: &str) -> Result<PathBuf, String> {
    let ws = c.workspace().to_path_buf();
    let (Some(root), Some(out)) = (c.shared.project_root.clone(), c.shared.outputs_dir()) else { return resolve_in(p, &[ws]) };
    if p == "DATAPILOT.md" || p == "/DATAPILOT.md" { return Ok(root.join("DATAPILOT.md")); }
    if Path::new(p).is_absolute() { return resolve_in(p, &[out, ws]); }
    let rel = p.strip_prefix("exports/").or(p.strip_prefix("outputs/")).unwrap_or(p);
    let _ = std::fs::create_dir_all(&out);
    resolve_in(rel, &[out])
}

fn size_str(b: u64) -> String {
    let b = b as f64;
    if b < 1000.0 { format!("{b:.0} bytes") } else if b < 1e6 { format!("{:.0} KB", b / 1e3) } else if b < 1e9 { format!("{:.1} MB", b / 1e6) } else { format!("{:.2} GB", b / 1e9) }
}

// ---------- memory ----------
fn memory_write(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    if !c.shared.settings.memory_enabled { return Ok(ToolResult::err("记忆功能已关闭 / memory disabled")); }
    let content = memory::redact(&sv(a, "content"));
    if content.trim().is_empty() { return Ok(ToolResult::err("empty memory")); }
    // M1: user / dataset scope go to topic files in AppSupport/memory (the Swift core keeps them in SQLite).
    let (dir, label) = match (&c.agent_memory_dir, &c.shared.project_root) {
        (Some(d), _) if a["scope"] == "agent" => (d.clone(), "子 Agent"),
        (_, Some(r)) if a["scope"] != "user" && a["scope"] != "dataset" => (memory::project_dir(r), "项目"),
        _ => (memory::user_dir(&c.shared.paths.app_support), "用户"),
    };
    let name = a["name"].as_str().map(String::from).unwrap_or_else(|| text::prefix(&content, 24));
    let kind = a["type"].as_str().map(String::from).unwrap_or_else(|| {
        if a["source"] == "correction" { "feedback".into() } else if label == "项目" { "project".into() } else { "user".into() }
    });
    let desc = a["description"].as_str().map(String::from).unwrap_or_else(|| text::prefix(&content, 100));
    let t = memory::write(&dir, &name, &desc, &kind, &content, a["file"].as_str())?;
    (c.emit)(events::memory_saved(&format!("{}（{label}记忆 {}）", t.name, t.file)));
    Ok(ToolResult::ok(format!("已保存{label}记忆 {}（{}），索引已更新", t.file, t.kind)))
}

fn bigrams(s: &str) -> HashSet<String> {
    let cs: Vec<char> = s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    cs.windows(2).map(|w| w.iter().collect()).collect()
}

fn memory_dirs(c: &ToolCtx) -> Vec<(PathBuf, &'static str)> {
    let mut v = vec![];
    if let Some(r) = &c.shared.project_root { v.push((memory::project_dir(r), "project")); }
    v.push((memory::user_dir(&c.shared.paths.app_support), "user"));
    v
}

fn memory_search(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let q = sv(a, "query");
    let qg = bigrams(&q);
    let mut hits: Vec<(f64, String)> = vec![];
    for (d, scope) in memory_dirs(c) {
        for t in memory::topics(&d, 200) {
            let all = format!("{} {} {}", t.name, t.description, t.body);
            let g = bigrams(&all);
            let mut score = if qg.is_empty() { 0.0 } else { qg.iter().filter(|x| g.contains(*x)).count() as f64 / qg.len() as f64 };
            if !q.is_empty() && all.to_lowercase().contains(&q.to_lowercase()) { score += 1.0; }
            if score >= 0.3 {
                hits.push((score, format!("- [{scope}] {}（{}，{}）：{}\n  {}", t.name, t.file, t.kind, t.description, text::prefix(&t.body.replace('\n', " "), 300))));
            }
        }
    }
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    if hits.is_empty() { return Ok(ToolResult::ok("没有找到相关记忆。")); }
    Ok(ToolResult::ok(hits.into_iter().take(8).map(|h| h.1).collect::<Vec<_>>().join("\n")))
}

fn memory_forget(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let Some(f) = a["file"].as_str() else {
        return Ok(ToolResult::err("Rust 核心暂不支持按 id 删除全局记忆，请用 file 指定主题文件"));
    };
    for (d, _) in memory_dirs(c) {
        if d.join(Path::new(f).file_name().unwrap_or_default()).exists() {
            memory::delete(&d, f)?;
            return Ok(ToolResult::ok(format!("已删除记忆 {f}")));
        }
    }
    Ok(ToolResult::err(format!("没有找到记忆文件 {f}")))
}

// ---------- planning ----------
fn todo_write(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let items: Vec<Value> = arr(a, "todos").iter().enumerate().map(|(i, t)| json!({
        "id": t["id"].as_str().map(String::from).unwrap_or_else(|| (i + 1).to_string()),
        "content": sv(t, "content"), "status": t["status"].as_str().unwrap_or("pending")})).collect();
    let v = json!(items);
    *c.shared.todos.lock().unwrap() = v.clone();
    c.shared.record(json!({"type": "todos", "payload": v}));
    (c.emit)(events::todos_updated(&v));
    let done = items.iter().filter(|t| t["status"] == "completed").count();
    Ok(ToolResult::ok(format!("任务清单已更新（{done}/{} 完成）", items.len())))
}

fn ask_user(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let id = format!("ask_{}", text::short_id(8));
    (c.emit)(events::question(&id, &sv(a, "question"), &strs(&a["options"])));
    let ans = c.shared.broker.wait_answer(&id);
    Ok(ToolResult::ok(format!("用户回答：{ans}")))
}

fn exit_plan_mode(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let plan = sv(a, "plan");
    let id = format!("plan_{}", text::short_id(8));
    (c.emit)(events::plan_proposed(&id, &plan));
    let ans = c.shared.broker.wait_answer(&id);
    if ans == "approve" || ans == "approve_auto" {
        c.shared.perms.set_mode(if ans == "approve_auto" { Mode::Auto } else { Mode::Ask });
        return Ok(ToolResult::with("[plan approved] 用户已批准计划，已切换到执行模式。请立即按计划开始执行，并用 todo_write 跟踪进度。",
                                   json!({"kind": "plan", "plan": plan, "status": "approved"})));
    }
    Ok(ToolResult::with(format!("[plan rejected] 用户未批准。反馈：{ans}。请根据反馈修改计划后再次调用 exit_plan_mode。"),
                        json!({"kind": "plan", "plan": plan, "status": "rejected"})))
}

fn skill(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let n = sv(a, "name");
    let all = prompt::skills_for(&c.shared.paths, &c.shared.settings);
    match all.iter().find(|s| s.name == n) {
        Some(s) => Ok(ToolResult::ok(format!("# Skill: {}\nBase directory: {}\n\n{}", s.name, s.dir.display(), s.body))),
        None => Ok(ToolResult::err(format!("unknown skill '{n}'. Available: {}", all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")))),
    }
}

fn save_skill(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let re = regex::Regex::new("[^a-z0-9-]+").unwrap();
    let safe = re.replace_all(&a["name"].as_str().unwrap_or("skill").to_lowercase(), "-").to_string();
    let dir = c.shared.paths.app_support.join("skills").join(&safe);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = format!("---\nname: {safe}\ndescription: {}\n---\n\n{}\n", sv(a, "description").replace('\n', " "), sv(a, "instructions"));
    std::fs::write(dir.join("SKILL.md"), text).map_err(|e| e.to_string())?;
    Ok(ToolResult::ok(format!("Skill 已保存到 {}", dir.display())))
}

/// Full-text search over past conversations (index.sqlite, FTS5 trigram).
fn history_search(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let q = sv(a, "query");
    if q.trim().is_empty() { return Ok(ToolResult::err("empty query")); }
    let hits = c.shared.index.search_messages(&q, &c.shared.id, 15);
    if hits.is_empty() { return Ok(ToolResult::ok("无匹配")); }
    Ok(ToolResult::ok(hits.iter().map(|(_, t, snip)| format!("- 《{t}》 {}", snip.replace('\n', " "))).collect::<Vec<_>>().join("\n")))
}

// ---------- files ----------
fn list_files(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let dir = resolve_in(&sv(a, "path"), &c.read_roots())?;
    fn walk(d: &Path, base: &Path, out: &mut Vec<String>) {
        let mut es: Vec<PathBuf> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).collect();
        es.sort();
        for p in es {
            if out.len() > 300 { return; }
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name.starts_with('.') { continue; }
            let rel = p.strip_prefix(base).map(|r| r.to_string_lossy().to_string()).unwrap_or_default();
            if rel.starts_with("checkpoints") || rel.ends_with(".wal") || rel.split('/').count() > 3 { continue; }
            if p.is_dir() { out.push(format!("{rel}/")); walk(&p, base, out); }
            else { out.push(format!("{rel}  ({})", size_str(std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)))); }
        }
    }
    let mut lines = vec![];
    walk(&dir, &dir, &mut lines);
    lines.sort();
    Ok(ToolResult::ok(if lines.is_empty() { "(empty)".into() } else { lines.join("\n") }))
}

fn read_file(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let u = resolve_in(&sv(a, "path"), &c.read_roots())?;
    let ext = u.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let off = (a["offset"].as_i64().unwrap_or(1).max(1)) as usize;
    let fname = u.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    if ["docx", "pptx", "pdf", "xlsx", "xlsm"].contains(&ext.as_str()) || crate::docparse::is_image(&ext) {
        let mut p = json!({"path": u.display().to_string()});
        if let Some(pg) = a["pages"].as_str() { p["pages"] = json!(pg); }
        // scanned pages / images: text from the document-parse service (only pages without a text layer are sent)
        let ocr = if crate::docparse::handles(&ext) { crate::docparse::for_tool(c, &u, a["pages"].as_str())? } else { None };
        if let Some(o) = &ocr { p["ocr"] = o.clone(); }
        let r = c.kernel("extract_text", p, 300)?;
        let t = sv(&r, "text");
        let lines: Vec<&str> = t.split('\n').collect();
        let lim = a["limit"].as_i64().unwrap_or(600) as usize;
        let slice = lines.iter().skip(off - 1).take(lim).cloned().collect::<Vec<_>>().join("\n");
        let mut out = format!("[{fname} → Markdown，{} 字符]\n{slice}", r["chars"].as_i64().unwrap_or(t.chars().count() as i64));
        if lines.len() > off - 1 + lim || r["truncated"] == true { out += &format!("\n…({} 行；用 offset/limit 或 pages 继续读取)", lines.len()); }
        if let Some(o) = &ocr {
            out = format!("{}\n{out}", crate::docparse::describe(o));
            if let Some(n) = crate::docparse::review_note(&r["ocr_review"]) { out += &format!("\n\n{n}"); }
            return Ok(ToolResult::with(out, json!({"kind": "ocr", "ocr": crate::docparse::card(&u, o, &r["ocr_review"])})));
        }
        return Ok(ToolResult::ok(out));
    }
    let Ok(d) = std::fs::read(&u) else { return Ok(ToolResult::err(format!("cannot read {fname}"))) };
    let t = String::from_utf8_lossy(&d);
    let lines: Vec<&str> = t.split('\n').collect();
    let lim = a["limit"].as_i64().unwrap_or(400) as usize;
    let body = lines.iter().enumerate().skip(off - 1).take(lim).map(|(i, l)| format!("{}\t{l}", i + 1)).collect::<Vec<_>>().join("\n");
    let more = if lines.len() > off - 1 + lim { format!("\n…({} lines total)", lines.len()) } else { String::new() };
    Ok(ToolResult::ok(body + &more))
}

const BINARY_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "pdf", "xlsx", "xls", "xlsm", "docx", "pptx", "zip", "gz", "parquet", "duckdb", "sqlite", "db", "wal", "feather", "arrow"];

/// Glob → regex on relative paths: `**/` any depth, `*` within a segment, `?`, `{a,b}`.
pub fn glob_regex(pat: &str) -> Option<regex::Regex> {
    let cs: Vec<char> = pat.chars().collect();
    let (mut r, mut i, mut brace) = (String::from("^"), 0, false);
    while i < cs.len() {
        match cs[i] {
            '*' if cs.get(i + 1) == Some(&'*') => {
                if cs.get(i + 2) == Some(&'/') { r += "(?:.*/)?"; i += 3; } else { r += ".*"; i += 2; }
                continue;
            }
            '*' => r += "[^/]*",
            '?' => r += "[^/]",
            '{' => { r += "(?:"; brace = true; }
            '}' if brace => { r += ")"; brace = false; }
            ',' if brace => r += "|",
            ch => r += &regex::escape(&ch.to_string()),
        }
        i += 1;
    }
    regex::Regex::new(&(r + "$")).ok()
}

fn glob_match(re: &regex::Regex, pat: &str, rel: &str) -> bool {
    re.is_match(rel) || (!pat.contains('/') && re.is_match(rel.rsplit('/').next().unwrap_or(rel)))
}

fn search_roots(a: &Value, c: &ToolCtx) -> Result<Vec<PathBuf>, String> {
    match a["path"].as_str().filter(|p| !p.is_empty()) {
        Some(p) => Ok(vec![resolve_in(p, &c.read_roots())?]),
        None => Ok(c.read_roots()),
    }
}

fn files_of(root: &Path) -> Vec<(String, PathBuf)> {
    if root.is_dir() { prompt::files(root, 5000).into_iter().map(|r| (r.clone(), root.join(r))).collect() }
    else { vec![(root.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), root.to_path_buf())] }
}

fn grep(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let pat = sv(a, "pattern");
    let Ok(re) = regex::RegexBuilder::new(&pat).case_insensitive(a["case_sensitive"] != true).build() else {
        return Ok(ToolResult::err(format!("invalid regex: {pat}")));
    };
    let mode = a["output_mode"].as_str().unwrap_or("content");
    let ctxn = a["context"].as_i64().unwrap_or(0).clamp(0, 5) as usize;
    let head = a["head_limit"].as_i64().unwrap_or(200).max(1) as usize;
    let gpat = sv(a, "glob");
    let filter = if gpat.is_empty() { None } else { glob_regex(&gpat) };
    let roots = search_roots(a, c)?;
    let (mut out, mut files_hit, mut truncated) = (Vec::<String>::new(), 0, false);
    'outer: for root in &roots {
        let prefix = if roots.len() > 1 && root == &c.shared.workspace && c.shared.project_root.is_some() { ".datapilot-data/" } else { "" };
        for (rel, u) in files_of(root) {
            if c.shared.cancelled() { break 'outer; }
            let ext = u.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if BINARY_EXT.contains(&ext.as_str()) { continue; }
            if let Some(f) = &filter { if !glob_match(f, &gpat, &rel) { continue; } }
            if std::fs::metadata(&u).map(|m| m.len() >= 20_000_000).unwrap_or(true) { continue; }
            let Ok(d) = std::fs::read(&u) else { continue };
            if d.iter().take(4096).any(|b| *b == 0) { continue; }
            let t = String::from_utf8_lossy(&d);
            let lines: Vec<&str> = t.split('\n').collect();
            let hits: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| re.is_match(l)).map(|(i, _)| i).collect();
            if hits.is_empty() { continue; }
            files_hit += 1;
            match mode {
                "files_with_matches" => out.push(format!("{prefix}{rel}")),
                "count" => out.push(format!("{prefix}{rel}:{}", hits.len())),
                _ => {
                    let mut shown = HashSet::new();
                    for &h in &hits {
                        for i in h.saturating_sub(ctxn)..=(h + ctxn).min(lines.len() - 1) {
                            if !shown.insert(i) { continue; }
                            let l = if lines[i].chars().count() > 400 { text::prefix(lines[i], 400) + "…" } else { lines[i].to_string() };
                            out.push(format!("{prefix}{rel}:{}{}{l}", i + 1, if i == h { ":" } else { "-" }));
                        }
                    }
                }
            }
            if out.len() >= head { truncated = true; break 'outer; }
        }
    }
    if out.is_empty() { return Ok(ToolResult::ok("No matches.")); }
    out.truncate(head);
    Ok(ToolResult::ok(format!("{}{}\n[{files_hit} file(s) matched]", out.join("\n"),
        if truncated { format!("\n…(truncated at {head} lines; narrow with path/glob)") } else { String::new() })))
}

fn glob(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let pat = a["pattern"].as_str().unwrap_or("*").to_string();
    let Some(re) = glob_regex(&pat) else { return Ok(ToolResult::err("invalid pattern")) };
    let mut hits: Vec<(String, std::time::SystemTime, u64)> = vec![];
    for root in search_roots(a, c)? {
        for (rel, u) in files_of(&root) {
            if glob_match(&re, &pat, &rel) {
                let m = std::fs::metadata(&u).ok();
                hits.push((rel, m.as_ref().and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH), m.map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    if hits.is_empty() { return Ok(ToolResult::ok("No files found.")); }
    hits.sort_by(|a, b| b.1.cmp(&a.1));
    let n = hits.len();
    let lines: Vec<String> = hits.into_iter().take(200).map(|h| format!("{}  ({})", h.0, size_str(h.2))).collect();
    Ok(ToolResult::ok(lines.join("\n") + &if n > 200 { format!("\n…({n} total)") } else { String::new() }))
}

fn write_file(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let u = write_url(c, &sv(a, "path"))?;
    if let Some(p) = u.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }
    std::fs::write(&u, sv(a, "content")).map_err(|e| e.to_string())?;
    let base = format!("{}/", c.shared.project_root.as_deref().unwrap_or(c.workspace()).display());
    Ok(ToolResult::with(format!("已写入 {}", u.display().to_string().replace(&base, "")), json!({"kind": "file", "path": u.display().to_string()})))
}

fn bash(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let s = &c.shared;
    let tmp = if s.project_root.is_some() { s.workspace.join("tmp") } else { s.workspace.parent().unwrap_or(&s.workspace).join("tmp") };
    let _ = std::fs::create_dir_all(&tmp);
    let net = s.network();
    let policy = crate::sandbox::Policy { workspace: s.workspace.clone(), tmp, paths: s.paths.clone(), network: net,
        read_roots: s.project_root.iter().cloned().collect(), write_roots: s.outputs_dir().into_iter().collect(), db_ports: vec![] };
    let secs = a["timeout"].as_u64().unwrap_or(120).min(600);
    let command = sv(a, "command");
    let env = policy.env();
    let sp = crate::sandbox::spawn(&policy, &crate::sandbox::shell(&command), &env, s.project_root.as_ref().unwrap_or(&s.workspace), false)?;
    let mut child = sp.proc;
    let (mut so, mut se) = (sp.stdout, sp.stderr);
    let ho = std::thread::spawn(move || { let mut b = Vec::new(); let _ = so.read_to_end(&mut b); b });
    let he = std::thread::spawn(move || { let mut b = Vec::new(); let _ = se.read_to_end(&mut b); b });
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut timed_out = false;
    let status = loop {
        if let Some(code) = child.try_wait() { break code; }
        if Instant::now() > deadline || s.cancelled() {
            child.kill(); timed_out = !s.cancelled();
            break child.wait();
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(&ho.join().unwrap_or_default()).to_string();
    let stderr = String::from_utf8_lossy(&he.join().unwrap_or_default()).to_string();
    let mut out = stdout;
    if !stderr.is_empty() { if !out.is_empty() { out.push('\n'); } out += &format!("stderr:\n{stderr}"); }
    if timed_out { out += &format!("\n[timed out after {secs}s]"); }
    out += &format!("\n[exit {status}]");
    if crate::sandbox::denied(&stderr) || (!net && crate::sandbox::offline(&stderr)) {
        out += if net { "\n提示：沙箱拒绝了该操作（只能写工作区与 outputs/）。" }
               else { "\n提示：沙箱拒绝了该操作。当前「联网」开关是关闭的，沙箱内无网络；如需联网，请让用户打开输入框下方的「联网」开关。" };
    }
    Ok(ToolResult { content: out.clone(), is_error: status != 0, display: Some(json!({"kind": "bash", "command": command, "output": out})), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn globs() {
        let r = glob_regex("**/*.{csv,xlsx}").unwrap();
        assert!(r.is_match("a/b/c.csv") && r.is_match("x.xlsx") && !r.is_match("a.md"));
        let r = glob_regex("sales_*.xlsx").unwrap();
        assert!(glob_match(&r, "sales_*.xlsx", "data/sales_2024.xlsx"));
    }
}
