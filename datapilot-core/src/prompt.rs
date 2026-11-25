//! System-prompt pieces shared with the Swift core: Resources/prompts/*.md, skills catalog, layered DATAPILOT
//! instruction files, project-memory index.
use std::path::{Path, PathBuf};

use crate::paths::Paths;

pub fn prompt(paths: &Paths, name: &str) -> String {
    match std::fs::read_to_string(paths.prompts().join(format!("{name}.md"))) {
        Ok(t) => t.strip_suffix('\n').unwrap_or(&t).to_string(),
        Err(_) => format!("（缺少提示词文件 prompts/{name}.md / missing prompt file）"),
    }
}

#[derive(Clone, Debug)]
pub struct Skill { pub name: String, pub description: String, pub dir: PathBuf, pub body: String, pub source: String }

pub fn parse_skill(text: &str, dir: &Path, source: &str) -> Option<Skill> {
    if !text.starts_with("---") { return None; }
    let rest = &text[3..];
    let end = rest.find("\n---")?;
    let front = &rest[..end];
    let body = rest[end + 4..].trim().to_string();
    let mut name = None; let mut desc = None;
    for l in front.lines() {
        if let Some((k, v)) = l.split_once(':') {
            let mut v = v.trim().to_string();
            if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') { v = v[1..v.len() - 1].to_string(); }
            match k.trim() { "name" => name = Some(v), "description" => desc = Some(v), _ => {} }
        }
    }
    let name = name.filter(|n| !n.is_empty())?;
    Some(Skill { name, description: desc?, dir: dir.to_path_buf(), body, source: source.into() })
}

/// External skill folders from the settings (`~` expanded), when enabled.
pub fn external_dirs(s: &crate::types::Settings) -> Vec<PathBuf> {
    if !s.external_skills_enabled { return vec![]; }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    s.external_skill_dirs.iter().map(|d| match d.strip_prefix("~/") { Some(r) => Path::new(&home).join(r), None => PathBuf::from(d) }).collect()
}

fn external_label(d: &Path) -> &'static str {
    let p = d.to_string_lossy().replace('\\', "/");
    if p.contains("/.codex/") { "codex" } else if p.contains("/.claude/") { "claude" } else { "external" }
}

/// Built-in skills then user skills (user overrides), sorted by name.
pub fn skills(paths: &Paths) -> Vec<Skill> { skills_with(paths, &[]) }

/// Skills incl. the settings' external folders (lowest priority: external < builtin < user).
pub fn skills_for(paths: &Paths, s: &crate::types::Settings) -> Vec<Skill> { skills_with(paths, &external_dirs(s)) }

pub fn skills_with(paths: &Paths, external: &[PathBuf]) -> Vec<Skill> {
    let mut m = std::collections::BTreeMap::new();
    let mut sources: Vec<(PathBuf, &str)> = external.iter().map(|d| (d.clone(), external_label(d))).collect();
    sources.push((paths.resources.join("skills"), "builtin"));
    sources.push((paths.app_support.join("skills"), "user"));
    for (d, label) in sources {
        let mut subs: Vec<PathBuf> = std::fs::read_dir(&d).into_iter().flatten().flatten().map(|e| e.path()).collect();
        subs.sort();
        for sub in subs {
            if let Ok(t) = std::fs::read_to_string(sub.join("SKILL.md")) {
                if let Some(s) = parse_skill(&t, &sub, label) { m.insert(s.name.clone(), s); }
            }
        }
    }
    m.into_values().collect()
}

pub fn skills_catalog(skills: &[Skill], budget: usize) -> Option<String> {
    if skills.is_empty() { return None; }
    let mut lines = vec!["# Skills".to_string(), "Available skills (call the `skill` tool with the name to load full instructions before doing that kind of task):".to_string()];
    let mut used: usize = lines.iter().map(|l| l.chars().count()).sum();
    for s in skills {
        let d = if s.description.chars().count() > 250 { crate::text::prefix(&s.description, 250) + "…" } else { s.description.clone() };
        let line = format!("- {}: {d}", s.name);
        if used + line.chars().count() > budget { lines.push(format!("- …({} skills total; truncated)", skills.len())); break; }
        used += line.chars().count();
        lines.push(line);
    }
    Some(lines.join("\n"))
}

const INSTR_BUDGET: usize = 6000;

fn expand_imports(text: &str, base: &Path, root: &Path, depth: usize) -> String {
    if !text.contains('@') { return text.to_string(); }
    let rootp = crate::platform::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    text.split('\n').map(|line| {
        let t = line.trim();
        if !t.starts_with('@') || t.contains(' ') || t.len() < 2 { return line.to_string(); }
        let rel = &t[1..];
        if !(rel.ends_with(".md") || rel.ends_with(".txt")) { return line.to_string(); }
        if depth >= 3 { return format!("（@{rel}：超过 3 层引入，已跳过）"); }
        let u = if Path::new(rel).is_absolute() { PathBuf::from(rel) } else { base.join(rel) };
        let Ok(u) = crate::platform::canonicalize(&u) else { return format!("（@{rel}：文件不存在）") };
        if !u.starts_with(&rootp) { return format!("（@{rel}：路径不在允许范围内）"); }
        match std::fs::read_to_string(&u) {
            Ok(s) => expand_imports(&s, u.parent().unwrap_or(base), root, depth + 1),
            Err(_) => format!("（@{rel}：文件不存在）"),
        }
    }).collect::<Vec<_>>().join("\n")
}

pub fn instruction_block(app_support: &Path, project_root: Option<&Path>) -> Option<String> {
    let mut parts: Vec<(String, String)> = vec![];
    let mut add = |label: String, u: PathBuf, root: &Path| {
        if let Ok(t) = std::fs::read_to_string(&u) {
            let s = expand_imports(&t, u.parent().unwrap_or(root), root, 0).trim().to_string();
            if !s.is_empty() { parts.push((label, s)); }
        }
    };
    add("用户级 DATAPILOT.md（所有项目通用）".into(), app_support.join("DATAPILOT.md"), app_support);
    if let Some(p) = project_root {
        add("项目 DATAPILOT.md".into(), p.join("DATAPILOT.md"), p);
        add("DATAPILOT.local.md（本机私有）".into(), p.join("DATAPILOT.local.md"), p);
        let mut rules: Vec<PathBuf> = std::fs::read_dir(p.join(".datapilot/rules")).into_iter().flatten().flatten()
            .map(|e| e.path()).filter(|x| x.extension().map(|e| e == "md").unwrap_or(false)).collect();
        rules.sort();
        for r in rules { let n = r.file_name().unwrap().to_string_lossy().to_string(); add(format!("规则 rules/{n}"), r, p); }
    }
    if parts.is_empty() { return None; }
    let total: usize = parts.iter().map(|p| p.1.chars().count()).sum();
    let body = parts.iter().map(|(l, t)| {
        let n = t.chars().count();
        let cap = if total <= INSTR_BUDGET { n } else { (INSTR_BUDGET * n / total.max(1)).max(400) };
        format!("## {l}\n{}", crate::text::truncate_middle(t, cap))
    }).collect::<Vec<_>>().join("\n\n");
    Some(format!("# 项目说明与规则 / Instructions (DATAPILOT)\n以下是用户为你写的长期指令，优先于默认做法（用户本次的明确要求除外）。\n\n{body}"))
}

/// Always-loaded user layer ("about you"): the user-memory index (incl. the core block kept by consolidation), capped
/// at USER_INDEX_MAX chars. None when there is no user memory yet.
pub const USER_INDEX_MAX: usize = 2000;
pub fn user_memory_block(paths: &Paths) -> Option<String> {
    let idx = crate::memory::load_index(&crate::memory::user_dir(&paths.app_support))?;
    let idx = if idx.chars().count() > USER_INDEX_MAX {
        idx.chars().take(USER_INDEX_MAX).collect::<String>() + "\n…（用户记忆索引过长，其余条目按需召回或用 memory_search）"
    } else { idx };
    Some(prompt(paths, "user_memory").replace("{{INDEX}}", &idx))
}

pub fn project_memory_block(paths: &Paths, root: &Path) -> String {
    let index = crate::memory::load_index(&crate::memory::project_dir(root)).unwrap_or_else(|| "（还没有项目记忆）".into());
    prompt(paths, "project_memory").replace("{{INDEX}}", &index)
}

const DATA_EXT: &[&str] = &["csv", "tsv", "txt", "xlsx", "xls", "xlsm", "ods", "parquet", "json", "jsonl", "ndjson"];
const SKIP_DIRS: &[&str] = &[".datapilot", ".git", "node_modules", ".venv", "venv", "__pycache__", ".build", "build", "DerivedData"];

/// Relative paths of files under `root` (max depth 4, hidden / heavy folders skipped), sorted.
pub fn files(root: &Path, limit: usize) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, depth: usize, out: &mut Vec<String>, limit: usize) {
        if depth > 4 || out.len() >= limit { return; }
        let mut ents: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        ents.sort();
        for p in ents {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) { continue; }
            if p.is_dir() { walk(&p, root, depth + 1, out, limit) } else if let Ok(r) = p.strip_prefix(root) {
                out.push(r.to_string_lossy().to_string());
                if out.len() >= limit { return; }
            }
        }
    }
    let mut out = vec![];
    walk(root, root, 1, &mut out, limit);
    out
}

pub fn data_files(root: &Path, limit: usize) -> Vec<String> {
    files(root, limit * 4).into_iter().filter(|f| {
        Path::new(f).extension().map(|e| DATA_EXT.contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)
    }).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_memory_always_loaded() {
        let sup = std::env::temp_dir().join(format!("dpusermem-{}", crate::text::short_id(6)));
        let paths = Paths::dev(sup.clone());
        assert!(user_memory_block(&paths).is_none());
        crate::memory::write(&crate::memory::user_dir(&sup), "金额单位", "金额默认用万元展示", "user", "金额一律用万元", None).unwrap();
        let b = user_memory_block(&paths).unwrap();
        assert!(b.contains("关于用户") && b.contains("金额单位") && b.contains("](金额单位.md)"), "{b}");
        // long index is cut to USER_INDEX_MAX
        for i in 0..80 { crate::memory::write(&crate::memory::user_dir(&sup), &format!("偏好条目{i}"), &"很长的描述".repeat(10), "user", "x", None).unwrap(); }
        assert!(user_memory_block(&paths).unwrap().contains("用户记忆索引过长"));
    }
}
