//! Selective recall of project memory topics (port of Swift `MemoryRecall`; design from the memory-mechanism analysis
//! docs, no code): newest 200 topic frontmatters → fast model picks ≤ 5 (no call when ≤ 5 candidates; keyword fallback),
//! each capped at 200 lines / 4 KB, 60 KB per session, topics already in the live context skipped.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::llm::Client;
use crate::memory::{self, Topic};
use crate::paths::Paths;
use crate::types::{ChatMessage, ModelRef};

pub const MAX_CANDIDATES: usize = 200;
pub const MAX_SELECTED: usize = 5;
pub const PER_FILE_LINES: usize = 200;
pub const PER_FILE_BYTES: usize = 4096;
pub const SESSION_BYTES: usize = 60 * 1024;

#[derive(Clone, Debug)]
pub struct Picked { pub file: String, pub name: String, pub text: String }

/// Memory layers searched by recall: project topics keep their bare file name as key, user-layer topics get `user:`.
pub const USER_PREFIX: &str = "user:";

pub struct Layers { pub project: Option<PathBuf>, pub user: PathBuf }

impl Layers {
    /// (folder, file name) for a recall key such as `gmv.md` or `user:units.md`.
    pub fn route(&self, key: &str) -> Option<(PathBuf, String)> {
        match key.strip_prefix(USER_PREFIX) {
            Some(f) => Some((self.user.clone(), f.to_string())),
            None => self.project.clone().map(|d| (d, key.to_string())),
        }
    }
    pub fn any_exists(&self) -> bool { self.user.exists() || self.project.as_ref().is_some_and(|d| d.exists()) }
    /// Newest topics across layers, keys prefixed per layer.
    fn candidates(&self) -> Vec<(Topic, PathBuf)> {
        let mut v: Vec<(Topic, PathBuf)> = vec![];
        if let Some(d) = &self.project {
            v.extend(memory::topics(d, MAX_CANDIDATES).into_iter().map(|t| { let p = d.join(&t.file); (t, p) }));
        }
        v.extend(memory::topics(&self.user, MAX_CANDIDATES).into_iter().map(|mut t| {
            let p = self.user.join(&t.file);
            t.file = format!("{USER_PREFIX}{}", t.file);
            (t, p)
        }));
        v.sort_by(|a, b| b.0.modified.partial_cmp(&a.0.modified).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(MAX_CANDIDATES);
        v
    }
}

pub fn select(paths: &Paths, question: &str, layers: &Layers, exclude: &HashSet<String>, budget_left: usize,
              fast: Option<(&Client, &ModelRef)>) -> Vec<Picked> {
    if budget_left == 0 || question.trim().is_empty() { return vec![]; }
    let all: Vec<(Topic, PathBuf)> = layers.candidates().into_iter().filter(|(t, _)| !exclude.contains(&t.file)).collect();
    if all.is_empty() { return vec![]; }
    let cands: Vec<Topic> = all.iter().map(|x| x.0.clone()).collect();
    let chosen = if cands.len() <= MAX_SELECTED { cands }
        else if let Some(p) = fast.and_then(|(c, m)| pick(paths, question, &cands, c, m)) { p }
        else { keyword(question, &cands) };
    let where_: HashMap<&str, &PathBuf> = all.iter().map(|(t, p)| (t.file.as_str(), p)).collect();
    let mut out = vec![];
    let mut used = 0;
    for t in chosen.into_iter().take(MAX_SELECTED) {
        if used >= budget_left { break; } // checked before adding: the last one may cross the budget
        let hint = where_.get(t.file.as_str()).map(|p| p.display().to_string()).unwrap_or_default();
        let text = render(&t, &hint);
        used += text.len();
        out.push(Picked { file: t.file.clone(), name: t.name.clone(), text });
    }
    out
}

pub fn extract_json(s: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str(s.trim()) { return Some(v); }
    let (a, b) = (s.find('{')?, s.rfind('}')?);
    if a < b { serde_json::from_str(&s[a..=b]).ok() } else { None }
}

fn ymd(secs: f64) -> String { let (y, m, d, ..) = crate::text::local_time(secs); format!("{y:04}-{m:02}-{d:02}") }

fn pick(paths: &Paths, q: &str, cands: &[Topic], client: &Client, model: &ModelRef) -> Option<Vec<Topic>> {
    let list = cands.iter().map(|t| format!("- {} [{}{}] {} — {} ({})", t.file, if t.file.starts_with(USER_PREFIX) { "用户偏好 / " } else { "" }, t.kind, t.name, t.description, ymd(t.modified))).collect::<Vec<_>>().join("\n");
    let sys = crate::prompt::prompt(paths, "memory_recall").replace("{{MAX}}", &MAX_SELECTED.to_string());
    let mut m = model.clone();
    let mut caps = m.caps();
    caps.thinking_enabled = false;
    caps.max_output_tokens = 200;
    m.capabilities = Some(caps);
    let r = client.complete(&m, &[ChatMessage::system(sys), ChatMessage::user(format!("问题：{q}\n\n清单：\n{list}"))]).ok()?;
    let files = crate::tools::strs(&extract_json(&r.content)?["files"]);
    let by: HashMap<&str, &Topic> = cands.iter().map(|t| (t.file.as_str(), t)).collect();
    Some(files.iter().filter_map(|f| { let f = f.trim(); by.get(f).or_else(|| by.get(Path::new(f).file_name()?.to_str()?)) }.map(|t| (*t).clone())).collect())
}

/// Fallback without a model: character-bigram overlap on name + description.
pub fn keyword(q: &str, cands: &[Topic]) -> Vec<Topic> {
    let qg = memory::bigrams(q);
    let mut v: Vec<(f64, &Topic)> = cands.iter().map(|t| (memory::overlap(&qg, &memory::bigrams(&format!("{} {}", t.name, t.description))), t))
        .filter(|x| x.0 >= 0.2).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    v.into_iter().take(MAX_SELECTED).map(|x| x.1.clone()).collect()
}

pub fn age_note(modified: f64) -> String {
    let days = ((crate::text::now() - modified) / 86400.0) as i64;
    if days > 30 { format!("{days} 天前记录，可能已过期，使用前先核实") } else if days >= 2 { format!("{days} 天前") } else { String::new() }
}

/// One topic as injected: header with type + age, body capped at 200 lines / 4 KB (UTF-8). `path` = where to read the rest.
pub fn render(t: &Topic, path: &str) -> String {
    let lines: Vec<&str> = t.body.split('\n').collect();
    let mut cut = lines.len() > PER_FILE_LINES;
    let mut body = lines.into_iter().take(PER_FILE_LINES).collect::<Vec<_>>().join("\n");
    if body.len() > PER_FILE_BYTES {
        let mut end = PER_FILE_BYTES;
        while !body.is_char_boundary(end) { end -= 1; }
        body.truncate(end);
        cut = true;
    }
    if cut { body += &format!("\n（已截断；完整内容：read_file {path}）"); }
    let age = age_note(t.modified);
    format!("### [mem:{}] {}（{}{}）\n{body}", t.file, t.name, t.kind, if age.is_empty() { String::new() } else { format!("，{age}") })
}

pub fn block(picked: &[Picked]) -> String {
    format!("<relevant_memories>\n以下是与当前问题相关的记忆：[mem:xxx.md] 为本项目记忆，[mem:user:xxx.md] 为跨项目的用户偏好（两者冲突时以项目记忆为准）。都是某个时间点的记录；用到表名、列名、口径前先核实，与当前数据冲突时以数据为准。引用时写 [mem:文件名]（照抄方括号里的名字）。\n\n{}\n</relevant_memories>",
            picked.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n\n"))
}

/// `[mem:file.md]` markers in an answer.
pub fn citations(text: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\[mem:([^\]\s]+)\]").unwrap()).captures_iter(text).map(|c| c[1].to_string()).collect()
}

/// Usage counters in <dir>/.usage.json (same layout as Swift: {file: {recalled?, cited?, count}}). Loaded ≠ used.
fn mark_dir(dir: &Path, files: &[String], kind: &str) {
    if files.is_empty() || !dir.exists() { return; }
    let u = dir.join(".usage.json");
    let mut v: Value = std::fs::read_to_string(&u).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    if !v.is_object() { v = json!({}); }
    let now = crate::text::now();
    let set: HashSet<&String> = files.iter().collect();
    for f in set {
        if !dir.join(f).exists() { continue; }
        let e = v.as_object_mut().unwrap().entry(f.clone()).or_insert(json!({"count": 0}));
        e[if kind == "cited" { "cited" } else { "recalled" }] = json!(now);
        e["count"] = json!(e["count"].as_i64().unwrap_or(0) + 1);
    }
    let _ = std::fs::write(&u, v.to_string());
}

/// Route recall keys (`x.md`, `user:x.md`) to their layer's `.usage.json`.
pub fn mark_usage(layers: &Layers, keys: &[String], kind: &str) {
    let mut by: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for k in keys { if let Some((d, f)) = layers.route(k) { by.entry(d).or_default().push(f); } }
    for (d, fs) in by { mark_dir(&d, &fs, kind); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recall_without_model() {
        let d = std::env::temp_dir().join(format!("dprecall-{}", crate::text::short_id(6)));
        let u = std::env::temp_dir().join(format!("dprecall-u-{}", crate::text::short_id(6)));
        memory::write(&d, "GMV 口径", "GMV 只算已支付订单", "project", "规则：GMV = 已支付金额", None).unwrap();
        memory::write(&u, "金额单位", "金额默认用万元展示", "user", "金额一律用万元", None).unwrap();
        let layers = Layers { project: Some(d.clone()), user: u.clone() };
        let paths = Paths::dev(std::env::temp_dir());
        let p = select(&paths, "GMV 怎么算", &layers, &HashSet::new(), SESSION_BYTES, None);
        assert_eq!(p.len(), 2);
        let b = block(&p);
        assert!(b.contains("[mem:gmv-口径.md]") && b.contains("[mem:user:金额单位.md]"), "{b}");
        assert_eq!(citations("见 [mem:gmv-口径.md] 和 [mem:user:金额单位.md]"), vec!["gmv-口径.md", "user:金额单位.md"]);
        mark_usage(&layers, &["gmv-口径.md".into(), "user:金额单位.md".into()], "cited");
        let pu: Value = serde_json::from_str(&std::fs::read_to_string(d.join(".usage.json")).unwrap()).unwrap();
        let uu: Value = serde_json::from_str(&std::fs::read_to_string(u.join(".usage.json")).unwrap()).unwrap();
        assert_eq!((pu["gmv-口径.md"]["count"].as_i64(), uu["金额单位.md"]["count"].as_i64()), (Some(1), Some(1)));
        let ex: HashSet<String> = ["gmv-口径.md".to_string(), "user:金额单位.md".to_string()].into_iter().collect();
        assert!(select(&paths, "GMV", &layers, &ex, SESSION_BYTES, None).is_empty());
        // no project: user layer alone still recalls
        let only_user = Layers { project: None, user: u };
        assert_eq!(select(&paths, "金额单位", &only_user, &HashSet::new(), SESSION_BYTES, None).len(), 1);
        assert!(only_user.route("x.md").is_none());
    }
}
