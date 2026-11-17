//! Project / agent / user topic memories: `<dir>/MEMORY.md` index + one markdown file per topic with frontmatter.
//! Same file format as the Swift `ProjectMemory` (both cores read and write the same folders).
use std::path::{Path, PathBuf};

pub const TYPES: &[&str] = &["user", "feedback", "project", "reference"];
pub const INDEX_MAX_LINES: usize = 200;
pub const INDEX_MAX_CHARS: usize = 25_000;

#[derive(Clone, Debug)]
pub struct Topic { pub file: String, pub name: String, pub description: String, pub kind: String, pub body: String, pub modified: f64 }

pub fn project_dir(root: &Path) -> PathBuf { root.join(".datapilot/memory") }
pub fn user_dir(app_support: &Path) -> PathBuf { app_support.join("memory") }

pub fn parse(text: &str, file: &str, modified: f64) -> Option<Topic> {
    if !text.starts_with("---") { return None; }
    let lines: Vec<&str> = text.split('\n').collect();
    let end = lines.iter().skip(1).position(|l| l.trim() == "---")? + 1;
    let mut meta = std::collections::HashMap::new();
    for l in &lines[1..end] {
        if let Some((k, v)) = l.split_once(':') {
            let mut v = v.trim().to_string();
            if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) { v = v[1..v.len() - 1].to_string(); }
            if !k.trim().is_empty() && !v.is_empty() { meta.insert(k.trim().to_string(), v); }
        }
    }
    let body = lines[end + 1..].join("\n").trim().to_string();
    let kind = meta.get("type").filter(|t| TYPES.contains(&t.as_str())).cloned().unwrap_or_else(|| "project".into());
    Some(Topic { file: file.into(), name: meta.get("name").cloned().unwrap_or_else(|| file.trim_end_matches(".md").into()),
                 description: meta.get("description").cloned().unwrap_or_default(), kind, body, modified })
}

pub fn render(t: &Topic) -> String {
    let q = |s: &str| if s.contains(':') || s.contains('#') { format!("\"{}\"", s.replace('"', "'")) } else { s.to_string() };
    format!("---\nname: {}\ndescription: {}\ntype: {}\n---\n\n{}\n", q(&t.name), q(&t.description.replace('\n', " ")), t.kind, t.body)
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for ch in s.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' { out.push(ch) } else if !out.ends_with('-') { out.push('-') }
    }
    let out = out.trim_matches('-').to_string();
    let out = if out.is_empty() { format!("memory-{}", crate::text::short_id(6)) } else { out };
    out.chars().take(60).collect()
}

fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Topics, newest first.
pub fn topics(d: &Path, limit: usize) -> Vec<Topic> {
    let mut out: Vec<Topic> = std::fs::read_dir(d).into_iter().flatten().flatten().filter_map(|e| {
        let p = e.path();
        let f = p.file_name()?.to_str()?.to_string();
        if !f.ends_with(".md") || f == "MEMORY.md" || f.starts_with('.') { return None; }
        let t = std::fs::read_to_string(&p).ok()?;
        let m = mtime(&p);
        Some(parse(&t, &f, m).unwrap_or(Topic { file: f.clone(), name: f.trim_end_matches(".md").into(), description: crate::text::prefix(&t, 80),
                                                 kind: "project".into(), body: t, modified: m }))
    }).collect();
    out.sort_by(|a, b| b.modified.partial_cmp(&a.modified).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(limit);
    out
}

/// MEMORY.md as loaded into the prompt: 200 lines / 25k chars, with a note when cut (the file itself is untouched).
pub fn load_index(d: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(d.join("MEMORY.md")).ok()?;
    let text = raw.trim();
    if text.is_empty() { return None; }
    let mut lines: Vec<&str> = text.split('\n').collect();
    let mut cut = false;
    if lines.len() > INDEX_MAX_LINES { lines.truncate(INDEX_MAX_LINES); cut = true; }
    let mut s = lines.join("\n");
    if s.chars().count() > INDEX_MAX_CHARS { s = s.chars().take(INDEX_MAX_CHARS).collect(); cut = true; }
    if cut { s += &format!("\n\n> 注意：MEMORY.md 超过 {INDEX_MAX_LINES} 行 / {INDEX_MAX_CHARS} 字，只加载了前面部分。请合并或精简索引条目（正文放主题文件）。"); }
    Some(s)
}

fn set_index_line(d: &Path, file: &str, line: Option<&str>) -> std::io::Result<()> {
    let u = d.join("MEMORY.md");
    let cur = std::fs::read_to_string(&u).unwrap_or_default();
    let mut lines: Vec<String> = if cur.is_empty() { vec![] } else { cur.split('\n').map(String::from).collect() };
    let key = format!("]({file})");
    match lines.iter().position(|l| l.contains(&key)) {
        Some(i) => { if let Some(l) = line { lines[i] = l.into() } else { lines.remove(i); } }
        None => { if let Some(l) = line { lines.push(l.into()) } }
    }
    std::fs::write(&u, lines.join("\n").trim_matches('\n').to_string() + "\n")
}

pub fn write(d: &Path, name: &str, description: &str, kind: &str, body: &str, file: Option<&str>) -> Result<Topic, String> {
    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    let name = redact(name.trim());
    if name.is_empty() { return Err("memory name is required".into()); }
    let mut f = file.map(|f| Path::new(f).file_name().and_then(|x| x.to_str()).unwrap_or(f).to_string()).unwrap_or_else(|| slug(&name) + ".md");
    if !f.ends_with(".md") { f += ".md"; }
    if f == "MEMORY.md" { f = "memory-index-topic.md".into(); }
    let desc = if description.is_empty() { crate::text::prefix(body, 80) } else { description.to_string() };
    let t = Topic { file: f.clone(), name, description: redact(&desc), kind: if TYPES.contains(&kind) { kind.into() } else { "project".into() },
                    body: redact(body.trim()), modified: crate::text::now() };
    std::fs::write(d.join(&f), render(&t)).map_err(|e| e.to_string())?;
    let line = format!("- [{}]({}) — {}", t.name, f, crate::text::prefix(&t.description.replace('\n', " "), 150));
    set_index_line(d, &f, Some(&line)).map_err(|e| e.to_string())?;
    Ok(t)
}

pub fn delete(d: &Path, file: &str) -> Result<(), String> {
    let f = Path::new(file).file_name().and_then(|x| x.to_str()).unwrap_or(file).to_string();
    let _ = std::fs::remove_file(d.join(&f));
    set_index_line(d, &f, None).map_err(|e| e.to_string())
}

/// Replace secrets before anything is persisted (same intent as the Swift `SecretRedactor`).
pub fn redact(s: &str) -> String {
    use std::sync::OnceLock;
    static RES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    let res = RES.get_or_init(|| vec![
        (regex::Regex::new(r"\bsk-[A-Za-z0-9_\-]{16,}").unwrap(), "[REDACTED_SECRET]"),
        (regex::Regex::new(r"\b(ghp|gho|xox[abp])_?[A-Za-z0-9_\-]{16,}").unwrap(), "[REDACTED_SECRET]"),
        (regex::Regex::new(r"(?i)\b(password|passwd|pwd|token|api[_-]?key|secret)(\s*[:=：]\s*)\S+").unwrap(), "${1}${2}[REDACTED_SECRET]"),
        (regex::Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}").unwrap(), "Bearer [REDACTED_SECRET]"),
    ]);
    let mut out = s.to_string();
    for (re, rep) in res { out = re.replace_all(&out, *rep).into_owned(); }
    out
}

/// Character bigrams without whitespace / punctuation (Swift `Retriever.bigrams`).
pub fn bigrams(s: &str) -> std::collections::HashSet<String> {
    let c: Vec<char> = s.to_lowercase().chars().filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !"，。、；：？！“”‘’（）【】《》—…".contains(*c)).collect();
    if c.len() < 2 { return c.iter().map(|x| x.to_string()).collect(); }
    c.windows(2).map(|w| w.iter().collect()).collect()
}

/// |a ∩ b| / |a|
pub fn overlap(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() { return 0.0; }
    a.intersection(b).count() as f64 / a.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let d = std::env::temp_dir().join(format!("dpmem-{}", crate::text::short_id(6)));
        let t = write(&d, "GMV 口径", "只算已支付", "project", "规则 password=abc123 sk-abcdefghijklmnopqrstu", None).unwrap();
        let back = topics(&d, 10);
        assert_eq!(back[0].name, "GMV 口径");
        assert!(back[0].body.contains("[REDACTED_SECRET]") && !back[0].body.contains("abc123"));
        assert!(load_index(&d).unwrap().contains(&t.file));
        delete(&d, &t.file).unwrap();
        assert!(topics(&d, 10).is_empty());
    }
}
