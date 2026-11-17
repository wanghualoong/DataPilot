//! Project source-file change detection (port of Swift `SourceSync`; same files, both cores interoperate).
//! - `<data>/sources.json` (written by the kernel on every import): project file → tables, mtime, size, import options.
//! - `<data>/source_state.json` (written here): files already seen (only *newly appearing* files are offered) and
//!   deleted sources whose tables the user chose to keep.
//! Policy: a modified file is re-imported automatically; added / deleted files are asked about.
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

pub const IMPORTABLE: &[&str] = &["csv", "tsv", "xlsx", "xls", "xlsm", "ods", "parquet", "json", "jsonl", "ndjson"];
const SKIP_DIRS: &[&str] = &[".datapilot", ".git", "node_modules", ".venv", "venv", "__pycache__", ".build", "build", "DerivedData"];

#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub path: String, pub tables: Vec<String>, pub params: Map<String, Value>, pub mode: String, mtime: f64, size: f64 }

#[derive(Default, Debug)]
pub struct Changes { pub modified: Vec<Entry>, pub modified_append: Vec<String>, pub deleted: Vec<Entry>, pub added: Vec<String> }
impl Changes { pub fn is_empty(&self) -> bool { self.modified.is_empty() && self.modified_append.is_empty() && self.deleted.is_empty() && self.added.is_empty() } }

#[derive(Default)]
struct State { known: Vec<String>, ignored_deleted: Vec<String> }

fn read_json(p: &Path) -> Option<Value> { serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok() }

fn entries(data: &Path) -> Vec<Entry> {
    let Some(v) = read_json(&data.join("sources.json")) else { return vec![] };
    v["imports"].as_array().into_iter().flatten().filter_map(|e| Some(Entry {
        path: e["path"].as_str()?.to_string(),
        tables: crate::tools::strs(&e["tables"]),
        params: e["params"].as_object().cloned().unwrap_or_default(),
        mode: e["mode"].as_str().unwrap_or("replace").to_string(),
        mtime: e["mtime"].as_f64().unwrap_or(0.0), size: e["size"].as_f64().unwrap_or(0.0),
    })).collect()
}

fn load_state(data: &Path) -> Option<State> {
    let v = read_json(&data.join("source_state.json"))?;
    Some(State { known: crate::tools::strs(&v["known"]), ignored_deleted: crate::tools::strs(&v["ignoredDeleted"]) })
}

fn save_state(data: &Path, s: &State) {
    let tmp = data.join("source_state.json.tmp");
    if std::fs::write(&tmp, json!({"known": s.known, "ignoredDeleted": s.ignored_deleted}).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, data.join("source_state.json"));
    }
}

/// Importable data files in the project folder (depth ≤ 4, hidden / heavy folders and outputs/ skipped), sorted.
pub fn current_files(root: &Path) -> Vec<String> {
    fn walk(d: &Path, base: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > 4 || out.len() >= 4000 { return; }
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') { continue; }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !(depth == 0 && name == "outputs") { walk(&p, base, depth + 1, out); }
            } else if ft.is_file() {
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
                if IMPORTABLE.contains(&ext.as_str()) {
                    if let Ok(rel) = p.strip_prefix(base) { out.push(rel.to_string_lossy().to_string()); }
                }
            }
        }
    }
    let mut out = vec![];
    walk(root, root, 0, &mut out);
    out.sort();
    out
}

fn stat(p: &Path) -> Option<(f64, f64)> {
    let m = std::fs::metadata(p).ok()?;
    let mt = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
    Some((mt, m.len() as f64))
}

pub fn detect(root: &Path, data: &Path) -> Changes {
    let es = entries(data);
    let files = current_files(root);
    if es.is_empty() {
        // nothing imported from the folder yet: the files present now become the baseline
        if load_state(data).is_none() && data.exists() { save_state(data, &State { known: files, ..Default::default() }); }
        return Changes::default();
    }
    let state = load_state(data).unwrap_or_else(|| {
        let s = State { known: files.clone(), ..Default::default() };
        save_state(data, &s);
        s
    });
    let mut c = Changes::default();
    let mut seen_append = HashSet::new();
    for e in &es {
        match stat(&root.join(&e.path)) {
            Some((mt, sz)) => if (mt - e.mtime).abs() > 0.01 || sz != e.size {
                if e.mode == "append" { if seen_append.insert(e.path.clone()) { c.modified_append.push(e.path.clone()); } }
                else { c.modified.push(e.clone()); }
            },
            None => if !state.ignored_deleted.contains(&e.path) { c.deleted.push(e.clone()); },
        }
    }
    let imported: HashSet<&str> = es.iter().map(|e| e.path.as_str()).collect();
    let known: HashSet<&str> = state.known.iter().map(|s| s.as_str()).collect();
    c.added = files.into_iter().filter(|f| !known.contains(f.as_str()) && !imported.contains(f.as_str())).collect();
    c
}

/// After handling: everything present now is "known"; remember deleted sources whose tables the user kept.
pub fn mark_handled(root: &Path, data: &Path, kept_deleted: &[String]) {
    let mut s = load_state(data).unwrap_or_default();
    s.known = current_files(root);
    let mut ig: Vec<String> = s.ignored_deleted.into_iter().chain(kept_deleted.iter().cloned()).collect::<HashSet<_>>().into_iter().collect();
    ig.sort();
    s.ignored_deleted = ig;
    save_state(data, &s);
}

/// One sync per project at a time (two chats of a project may start a turn simultaneously).
static BUSY: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub struct Gate(String);
pub fn begin(root: &Path) -> Option<Gate> {
    let k = root.to_string_lossy().to_string();
    let mut b = BUSY.lock().unwrap();
    if b.contains(&k) { return None; }
    b.push(k.clone());
    Some(Gate(k))
}
impl Drop for Gate { fn drop(&mut self) { BUSY.lock().unwrap().retain(|x| x != &self.0); } }

/// Free-text answers to the yes/no cards.
pub fn is_yes(answer: &str, yes: &str) -> bool {
    let a = answer.trim().to_lowercase();
    a == yes.to_lowercase() || ["是", "好", "好的", "可以", "导入", "删除", "yes", "y", "ok"].contains(&a.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detect_changes() {
        let root = std::env::temp_dir().join(format!("dpsrc-{}", crate::text::short_id(6)));
        let data = root.join(".datapilot/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(root.join("outputs")).unwrap();
        for f in ["a.csv", "b.csv", "gone.csv", "outputs/x.csv", "notes.txt"] { std::fs::write(root.join(f), "k\n1\n").unwrap(); }
        assert!(detect(&root, &data).is_empty()); // nothing imported: baseline only
        let (am, asz) = stat(&root.join("a.csv")).unwrap();
        let (bm, bsz) = stat(&root.join("b.csv")).unwrap();
        std::fs::write(data.join("sources.json"), json!({"imports": [
            {"path": "a.csv", "tables": ["a"], "params": {}, "mode": "replace", "mtime": am, "size": asz},
            {"path": "b.csv", "tables": ["b"], "params": {"sheet": "S"}, "mode": "replace", "mtime": bm - 100.0, "size": bsz},
            {"path": "gone.csv", "tables": ["g"], "params": {}, "mode": "append", "mtime": 0, "size": 0}]}).to_string()).unwrap();
        std::fs::remove_file(root.join("gone.csv")).unwrap();
        std::fs::write(root.join("new.xlsx"), "x").unwrap();
        let c = detect(&root, &data);
        assert_eq!(c.modified.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["b.csv"]);
        assert_eq!(c.modified[0].params["sheet"], "S");
        assert_eq!(c.deleted.len(), 1);
        assert_eq!(c.added, vec!["new.xlsx"]);
        mark_handled(&root, &data, &["gone.csv".into()]);
        let c = detect(&root, &data);
        assert!(c.deleted.is_empty() && c.added.is_empty() && c.modified.len() == 1);
        let g = begin(&root).unwrap();
        assert!(begin(&root).is_none());
        drop(g);
        assert!(begin(&root).is_some());
        assert!(is_yes(" 好的 ", "导入新文件") && !is_yes("暂不导入", "导入新文件"));
    }
}
