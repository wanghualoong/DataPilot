//! Backup / restore (0.1.9): one zip, `DataPilot-备份-<date>-v<ver>.zip`.
//! Contents: `manifest.json` (format, version, time, size + sha256 + mtime per file, projects, keys to re-enter);
//! `app/{app.sqlite, index.sqlite}` (SQLite online-backup snapshots), `app/config.json` (secret-looking headers / env
//! removed), `app/DATAPILOT.md`, `app/sessions/` (without tmp/), `app/memory/`, `app/skills/`;
//! `projects/<id>/datapilot/` (the project's .datapilot/, per project) and optionally `projects/<id>/files/` (the
//! project's own files). Never: keys / passwords (keychain only), chrome-profile, browser-profile, logs, ocr-cache, caches.
//! Import: verify every entry (safe name, listed in the manifest, size, sha256) → unpack into a temp folder next to
//! the data → merge (default: add missing chats, newer memory / skill file wins, local settings kept, existing data
//! never overwritten) or replace (after an automatic pre-restore backup in backups/). Merging twice adds nothing.
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use crate::agent::Core;

pub const FORMAT: u64 = 1;
pub const AUTO_PREFIX: &str = "DataPilot-自动备份-";
pub const PRE_PREFIX: &str = "DataPilot-恢复前-";
pub const AUTO_KEEP: usize = 4;
const AUTO_EVERY_SECS: f64 = 7.0 * 86400.0;
/// never unpack more than this, whatever the manifest says
const MAX_TOTAL: u64 = 200 << 30;

pub fn dir(app_support: &Path) -> PathBuf { app_support.join("backups") }

#[derive(Clone, Debug)]
pub struct ProjectSel { pub id: String, pub files: bool }

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}
fn e2s(e: impl std::fmt::Display) -> String { e.to_string() }
fn stamp() -> String {
    let (y, m, d, hh, mm, _, _) = crate::text::local_time(crate::text::now());
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}")
}
pub fn default_name() -> String { format!("DataPilot-备份-{}-v{}.zip", stamp(), crate::VERSION) }

fn secretish(k: &str) -> bool {
    let l = k.to_lowercase();
    ["auth", "key", "token", "secret", "password", "passwd", "pwd", "cookie"].iter().any(|w| l.contains(w))
}

/// Files under `root` as (`prefix/rel/path`, path); symlinks and .DS_Store skipped, `skip(rel)` prunes.
fn walk(root: &Path, prefix: &str, skip: &dyn Fn(&str) -> bool, out: &mut Vec<(String, PathBuf)>) {
    fn go(d: &Path, rel: &str, prefix: &str, skip: &dyn Fn(&str) -> bool, out: &mut Vec<(String, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let mut es: Vec<_> = rd.flatten().collect();
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let n = e.file_name().to_string_lossy().to_string();
            if n == ".DS_Store" { continue; }
            let r = if rel.is_empty() { n.clone() } else { format!("{rel}/{n}") };
            if skip(&r) { continue; }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() { continue; }
            if ft.is_dir() { go(&e.path(), &r, prefix, skip, out); } else if ft.is_file() { out.push((format!("{prefix}/{r}"), e.path())); }
        }
    }
    go(root, "", prefix, skip, out);
}

/// config.json for the backup: headers / MCP env that look like credentials are removed (and listed to re-enter).
fn scrubbed_config(sup: &Path) -> (String, Vec<Value>) {
    let mut needs = vec![];
    let mut v: Value = std::fs::read_to_string(sup.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    for p in v["providers"].as_array_mut().into_iter().flatten() {
        let label = p["name"].as_str().unwrap_or("").to_string();
        if let Some(h) = p["extraHeaders"].as_object_mut() {
            let ks: Vec<String> = h.keys().filter(|k| secretish(k)).cloned().collect();
            for k in ks { h.remove(&k); needs.push(json!({"kind": "header", "id": k, "label": label})); }
        }
    }
    for s in v["settings"]["mcpServers"].as_array_mut().into_iter().flatten() {
        let label = s["name"].as_str().unwrap_or("").to_string();
        if let Some(env) = s["env"].as_object_mut() {
            let ks: Vec<String> = env.keys().filter(|k| secretish(k)).cloned().collect();
            for k in ks { env.remove(&k); needs.push(json!({"kind": "mcp_env", "id": k, "label": label})); }
        }
    }
    (serde_json::to_string_pretty(&v).unwrap_or_default(), needs)
}

/// Keys / passwords that exist on this machine for what the backup contains: they stay in the keychain, so after a
/// restore on another computer they must be entered again.
fn secret_needs(core: &Core, files: &[(String, PathBuf)]) -> Vec<Value> {
    let mut out = vec![];
    let providers = core.config.lock().unwrap().providers.clone();
    for p in providers {
        if core.secrets.api_key(&p.id).is_some() { out.push(json!({"kind": "api_key", "id": p.id, "label": p.name})); }
    }
    if core.secrets.api_key("docparse").is_some() { out.push(json!({"kind": "api_key", "id": "docparse", "label": "文档识别 / OCR"})); }
    let mut seen = HashSet::new();
    for (name, p) in files {
        if !name.ends_with("/connections.json") { continue; }
        let v: Value = std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        for c in v["connections"].as_array().into_iter().flatten() {
            let (Some(id), label) = (c["id"].as_str(), c["name"].as_str().unwrap_or("")) else { continue };
            if seen.insert(("db", id.to_string())) && core.secrets.db_password(id).is_some() {
                out.push(json!({"kind": "db_password", "id": id, "label": label}));
            }
            if c["ssh"].as_bool() == Some(true) && seen.insert(("ssh", id.to_string())) && core.secrets.ssh_secret(id).is_some() {
                out.push(json!({"kind": "ssh_secret", "id": id, "label": label}));
            }
        }
    }
    out
}

/// Write a backup zip at `dst` (`kind`: manual | auto | pre-restore). The caller makes sure no kernel holds a
/// DuckDB file open (kernels stopped) and no turn is running.
pub fn export(core: &Core, dst: &Path, sel: &[ProjectSel], kind: &str) -> Result<Value, String> {
    let sup = &core.paths.app_support;
    let tmp = dir(sup).join(format!(".tmp-{}", crate::text::short_id(8)));
    std::fs::create_dir_all(&tmp).map_err(e2s)?;
    let r = export_in(core, dst, sel, kind, &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

fn export_in(core: &Core, dst: &Path, sel: &[ProjectSel], kind: &str, tmp: &Path) -> Result<Value, String> {
    let t0 = std::time::Instant::now();
    let sup = &core.paths.app_support;
    core.ui.snapshot_to(&tmp.join("app.sqlite"))?;
    core.index.snapshot_to(&tmp.join("index.sqlite"))?;
    let (cfg, mut needs) = scrubbed_config(sup);
    std::fs::write(tmp.join("config.json"), cfg).map_err(e2s)?;
    let mut files: Vec<(String, PathBuf)> = vec![("app/app.sqlite".into(), tmp.join("app.sqlite")), ("app/index.sqlite".into(), tmp.join("index.sqlite")),
                                                 ("app/config.json".into(), tmp.join("config.json"))];
    if sup.join("DATAPILOT.md").is_file() { files.push(("app/DATAPILOT.md".into(), sup.join("DATAPILOT.md"))); }
    // sessions/<id>/tmp is scratch space of the kernel
    walk(&sup.join("sessions"), "app/sessions", &|r| r.split('/').count() == 2 && r.ends_with("/tmp"), &mut files);
    walk(&sup.join("memory"), "app/memory", &|_| false, &mut files);
    walk(&sup.join("skills"), "app/skills", &|_| false, &mut files);
    let known = core.ui.list_projects();
    let mut pj = vec![];
    for s in sel {
        let Some(p) = known.iter().find(|p| p["id"].as_str() == Some(s.id.as_str())) else { continue };
        let root = PathBuf::from(p["path"].as_str().unwrap_or(""));
        if !root.join(".datapilot").is_dir() { continue; }
        walk(&root.join(".datapilot"), &format!("projects/{}/datapilot", s.id), &|r| r == "data/tmp", &mut files);
        if s.files { walk(&root, &format!("projects/{}/files", s.id), &|r| r == ".datapilot" || r == ".git", &mut files); }
        pj.push(json!({"id": s.id, "name": p["name"], "path": root, "files": s.files}));
    }
    needs.extend(secret_needs(core, &files));
    if let Some(parent) = dst.parent() { std::fs::create_dir_all(parent).map_err(e2s)?; }
    let part = dst.with_extension("zip.part");
    let write = || -> Result<(Map<String, Value>, u64), String> {
        use zip::write::SimpleFileOptions;
        let mut w = zip::ZipWriter::new(std::io::BufWriter::new(File::create(&part).map_err(e2s)?));
        let (mut man, mut total) = (Map::new(), 0u64);
        let mut buf = vec![0u8; 1 << 16];
        for (name, p) in &files {
            let Ok(mut src) = File::open(p) else { continue };   // vanished since the walk
            let len = src.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
            let method = if ["zip", "xlsx", "docx", "pptx", "png", "jpg", "jpeg", "webp", "parquet", "gz"].contains(&ext.as_str()) { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated };
            w.start_file(name.as_str(), SimpleFileOptions::default().compression_method(method).large_file(len >= 1 << 31)).map_err(e2s)?;
            let (mut h, mut n) = (Sha256::new(), 0u64);
            loop {
                let k = src.read(&mut buf).map_err(|e| format!("{}: {e}", p.display()))?;
                if k == 0 { break; }
                h.update(&buf[..k]);
                w.write_all(&buf[..k]).map_err(e2s)?;
                n += k as u64;
            }
            man.insert(name.clone(), json!({"size": n, "sha256": hex(&h.finalize()), "mtime": mtime(p)}));
            total += n;
        }
        let manifest = json!({"format": FORMAT, "app": "DataPilot", "version": crate::VERSION, "created": crate::text::now(), "kind": kind,
                              "os": std::env::consts::OS, "projects": pj, "secrets_needed": needs, "files": man});
        w.start_file("manifest.json", SimpleFileOptions::default()).map_err(e2s)?;
        w.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes()).map_err(e2s)?;
        w.finish().map_err(e2s)?.flush().map_err(e2s)?;
        Ok((manifest["files"].as_object().cloned().unwrap_or_default(), total))
    };
    match write() {
        Ok((man, total)) => {
            std::fs::rename(&part, dst).map_err(e2s)?;
            let size = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
            crate::log::info("backup_export", "", &[("kind", kind.into()), ("files", man.len().to_string()), ("bytes", total.to_string()),
                                                   ("zip", size.to_string()), ("projects", pj.len().to_string()), ("ms", t0.elapsed().as_millis().to_string())]);
            Ok(json!({"path": dst, "size": size, "files": man.len(), "bytes": total, "projects": pj.len()}))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            crate::log::warn("backup_export", "", &[("kind", kind.into()), ("ok", "0".into()), ("err", crate::log::err_text(&e))]);
            Err(format!("备份失败：{e}"))
        }
    }
}

/// Entry name → relative path, or None when it is not a path a backup may contain: only `app/…` and
/// `projects/<id>/{datapilot,files}/…`, plain components (no `..`, root, drive, `:`, `\`).
pub fn safe_rel(name: &str) -> Option<PathBuf> {
    if name.contains('\\') || name.contains('\0') || name.contains(':') { return None; }
    let mut parts = vec![];
    for c in Path::new(name).components() {
        match c { Component::Normal(x) => parts.push(x.to_str()?.to_string()), _ => return None }
    }
    let ok = match parts.first().map(String::as_str) {
        Some("app") => parts.len() >= 2,
        Some("projects") => parts.len() >= 4 && parts[1].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                            && (parts[2] == "datapilot" || parts[2] == "files"),
        _ => false,
    };
    ok.then(|| parts.iter().collect())
}

fn open_archive(path: &Path) -> Result<(zip::ZipArchive<File>, Value), String> {
    let f = File::open(path).map_err(|e| format!("打不开备份文件：{e}"))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("不是有效的备份文件（zip）：{e}"))?;
    let mut s = String::new();
    z.by_name("manifest.json").map_err(|_| "不是 DataPilot 备份：缺少 manifest.json".to_string())?
        .take(20 << 20).read_to_string(&mut s).map_err(|e| format!("manifest.json：{e}"))?;
    let m: Value = serde_json::from_str(&s).map_err(|e| format!("manifest.json 无法解析：{e}"))?;
    if m["app"].as_str() != Some("DataPilot") { return Err("不是 DataPilot 备份".into()); }
    let fmt = m["format"].as_u64().unwrap_or(0);
    if fmt == 0 || fmt > FORMAT {
        return Err(format!("这个备份由更新的 DataPilot {} 生成（格式 {fmt}），请先升级到该版本或更新版本再恢复", m["version"].as_str().unwrap_or("?")));
    }
    Ok((z, m))
}

/// Verify every entry (and unpack into `dest` when given): names safe and listed, sizes and sha256 as in the manifest,
/// nothing missing. Returns the manifest and the unpacked size.
fn unpack(path: &Path, dest: Option<&Path>, free: Option<u64>) -> Result<(Value, u64), String> {
    let (mut z, m) = open_archive(path)?;
    let files = m["files"].as_object().cloned().unwrap_or_default();
    let want: u64 = files.values().filter_map(|f| f["size"].as_u64()).sum();
    if want > MAX_TOTAL { return Err("备份内容过大，拒绝解压".into()); }
    if let (Some(_), Some(free)) = (dest, free) {
        if want + (512 << 20) > free { return Err(format!("磁盘空间不足：恢复需要约 {:.1} GB，剩余 {:.1} GB", want as f64 / 1e9, free as f64 / 1e9)); }
    }
    let mut seen = HashSet::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| format!("备份已损坏：{e}"))?;
        let name = e.name().to_string();
        if name == "manifest.json" || (e.is_dir() && safe_rel(name.trim_end_matches('/')).is_some()) { continue; }
        if e.is_symlink() { return Err(format!("备份里有符号链接，拒绝恢复：{name}")); }
        let Some(rel) = safe_rel(&name).filter(|_| e.enclosed_name().is_some()) else { return Err(format!("备份里有不安全的路径，拒绝恢复：{name}")) };
        let Some(info) = files.get(&name) else { return Err(format!("备份里有清单之外的文件，拒绝恢复：{name}")) };
        if !seen.insert(name.clone()) { return Err(format!("备份里有重复的文件：{name}")); }
        let size = info["size"].as_u64().unwrap_or(0);
        let mut out = match dest {
            Some(d) => {
                let p = d.join(&rel);
                if let Some(parent) = p.parent() { std::fs::create_dir_all(parent).map_err(e2s)?; }
                Some(File::create(&p).map_err(|e| format!("{}: {e}", p.display()))?)
            }
            None => None,
        };
        let (mut h, mut n) = (Sha256::new(), 0u64);
        let mut r = (&mut e).take(size + 1);
        loop {
            let k = r.read(&mut buf).map_err(|e| format!("备份已损坏（{name}）：{e}"))?;
            if k == 0 { break; }
            n += k as u64;
            if n > size { return Err(format!("备份已损坏：{name} 比清单记录的大")); }
            h.update(&buf[..k]);
            if let Some(o) = out.as_mut() { o.write_all(&buf[..k]).map_err(e2s)?; }
        }
        if n != size || hex(&h.finalize()) != info["sha256"].as_str().unwrap_or("") {
            return Err(format!("备份已损坏：{name} 校验不一致"));
        }
        total += n;
    }
    if let Some(miss) = files.keys().find(|k| !seen.contains(*k)) { return Err(format!("备份不完整：缺少 {miss}")); }
    Ok((m, total))
}

fn chat_ids(files: &Map<String, Value>) -> Vec<String> {
    files.keys().filter(|k| k.ends_with("/transcript.jsonl")).filter_map(|k| {
        let p: Vec<&str> = k.split('/').collect();
        match p.as_slice() {
            ["app", "sessions", id, _] => Some(id.to_string()),
            ["projects", _, "datapilot", "sessions", id, _] => Some(id.to_string()),
            _ => None,
        }
    }).collect()
}

/// Check a backup (integrity included) and say what restoring it would do.
pub fn preview(core: &Core, path: &Path) -> Result<Value, String> {
    let (m, total) = unpack(path, None, None)?;
    let files = m["files"].as_object().cloned().unwrap_or_default();
    let chats = chat_ids(&files);
    let new = chats.iter().filter(|id| !core.ui.exists(id)).count();
    let memory = files.keys().filter(|k| (k.starts_with("app/memory/") || k.contains("/datapilot/memory/")) && k.ends_with(".md") && !k.ends_with("/MEMORY.md")).count();
    let skills: HashSet<&str> = files.keys().filter_map(|k| k.strip_prefix("app/skills/")).filter_map(|r| r.split('/').next()).collect();
    let known = core.ui.list_projects();
    let projects: Vec<Value> = m["projects"].as_array().into_iter().flatten().map(|p| {
        let path = p["path"].as_str().unwrap_or("");
        let local = known.iter().any(|k| k["id"] == p["id"] || k["path"].as_str().map(|x| x.trim_end_matches(['/', '\\']) == path.trim_end_matches(['/', '\\'])).unwrap_or(false));
        json!({"id": p["id"], "name": p["name"], "path": path, "exists": Path::new(path).is_dir(), "known": local, "files": p["files"]})
    }).collect();
    Ok(json!({"version": m["version"], "created": m["created"], "kind": m["kind"], "os": m["os"], "bytes": total,
              "zip": std::fs::metadata(path).map(|x| x.len()).unwrap_or(0), "chats": chats.len(), "chats_new": new,
              "memory": memory, "skills": skills.len(), "projects": projects, "secrets_needed": m["secrets_needed"],
              "has_config": files.contains_key("app/config.json")}))
}

/// Move `src` into place as `dst` (the old `dst` is removed only after the move worked).
fn swap_dir(src: &Path, dst: &Path) -> Result<(), String> {
    let old = dst.with_file_name(format!("{}.old-{}", dst.file_name().unwrap_or_default().to_string_lossy(), crate::text::short_id(6)));
    if dst.exists() { std::fs::rename(dst, &old).map_err(|e| format!("{}: {e}", dst.display()))?; }
    let r = if src.exists() { std::fs::rename(src, dst) } else { std::fs::create_dir_all(dst) };
    if let Err(e) = r {
        if old.exists() { let _ = std::fs::rename(&old, dst); }
        return Err(format!("{}: {e}", dst.display()));
    }
    if old.exists() { let _ = std::fs::remove_dir_all(&old); }
    Ok(())
}

fn sha_file(p: &Path) -> Option<String> {
    let mut f = File::open(p).ok()?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(hex(&h.finalize()))
}

/// Copy files of `src` into `dst`: missing ones always; differing ones only with `newer` and when the backup copy is
/// newer than the local file (mtime from the manifest, key `prefix/rel`). Returns the relative paths written.
fn merge_tree(src: &Path, dst: &Path, prefix: &str, files: &Map<String, Value>, newer: bool, skip: &dyn Fn(&str) -> bool) -> Result<(Vec<String>, Vec<String>), String> {
    let mut list = vec![];
    walk(src, prefix, skip, &mut list);
    let (mut added, mut updated) = (vec![], vec![]);
    for (name, p) in list {
        let rel = name[prefix.len() + 1..].to_string();
        let to = dst.join(&rel);
        let write = |p: &Path, to: &Path| -> Result<(), String> {
            if let Some(parent) = to.parent() { std::fs::create_dir_all(parent).map_err(e2s)?; }
            std::fs::copy(p, to).map(|_| ()).map_err(|e| format!("{}: {e}", to.display()))
        };
        if !to.exists() { write(&p, &to)?; added.push(rel); continue; }
        if !newer { continue; }
        let info = &files[&name];
        if info["mtime"].as_f64().unwrap_or(0.0) > mtime(&to) + 1.0 && sha_file(&to).as_deref() != info["sha256"].as_str() {
            write(&p, &to)?;
            updated.push(rel);
        }
    }
    Ok((added, updated))
}

/// Memory folder merge: topic files (newer wins), then MEMORY.md lines for what changed.
fn merge_memory(src: &Path, dst: &Path, prefix: &str, files: &Map<String, Value>) -> Result<(usize, usize), String> {
    let (a, u) = merge_tree(src, dst, prefix, files, true, &|r| r == "MEMORY.md")?;
    for f in a.iter().chain(u.iter()).filter(|f| f.ends_with(".md") && !f.contains('/')) { let _ = crate::memory::index_topic(dst, f); }
    Ok((a.iter().filter(|f| f.ends_with(".md")).count(), u.iter().filter(|f| f.ends_with(".md")).count()))
}

fn write_atomic(p: &Path, text: &str) -> Result<(), String> {
    let tmp = p.with_extension("json.restore-tmp");
    std::fs::write(&tmp, text).map_err(e2s)?;
    std::fs::rename(&tmp, p).map_err(e2s)
}

/// Restore `path`. `mode`: merge | replace. `relink`: backup project id → folder to restore it into (instead of the
/// original path). `projects_only`: skip the app-level part (second pass after choosing new folders).
/// The caller has stopped every session and kernel. Returns a report (what was added, what needs attention).
pub fn import(core: &Core, path: &Path, mode: &str, relink: &HashMap<String, PathBuf>, projects_only: bool) -> Result<Value, String> {
    let sup = core.paths.app_support.clone();
    let tmp = dir(&sup).join(format!(".restore-{}", crate::text::short_id(8)));
    std::fs::create_dir_all(&tmp).map_err(e2s)?;
    let t0 = std::time::Instant::now();
    let r = import_in(core, path, mode == "replace", relink, projects_only, &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    match &r {
        Ok(v) => crate::log::info("backup_import", "", &[("mode", mode.into()), ("chats", v["chats_added"].to_string()), ("projects", v["projects_restored"].as_array().map(|a| a.len()).unwrap_or(0).to_string()),
                                                        ("ms", t0.elapsed().as_millis().to_string())]),
        Err(e) => crate::log::warn("backup_import", "", &[("mode", mode.into()), ("ok", "0".into()), ("err", crate::log::err_text(e))]),
    }
    r
}

fn import_in(core: &Core, path: &Path, replace: bool, relink: &HashMap<String, PathBuf>, projects_only: bool, tmp: &Path) -> Result<Value, String> {
    let sup = core.paths.app_support.clone();
    let (m, _) = unpack(path, Some(tmp), crate::platform::free_space(&sup))?;
    let files = m["files"].as_object().cloned().unwrap_or_default();
    let mut rep = Map::new();
    let known = core.ui.list_projects();
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    // projects of the backup → target folders (relinked or original); missing folders are reported, not created
    let mut targets: Vec<(String, String, PathBuf)> = vec![];
    let mut missing = vec![];
    for p in m["projects"].as_array().into_iter().flatten() {
        let id = p["id"].as_str().unwrap_or("").to_string();
        if id.is_empty() || !tmp.join("projects").join(&id).exists() { continue; }
        if projects_only && !relink.contains_key(&id) { continue; }
        let target = relink.get(&id).cloned().unwrap_or_else(|| PathBuf::from(p["path"].as_str().unwrap_or("")));
        if target.is_dir() { targets.push((id, p["name"].as_str().unwrap_or("").to_string(), target)); }
        else { missing.push(json!({"id": id, "name": p["name"], "path": p["path"]})); }
    }
    if replace {
        // automatic pre-restore backup: app data + the .datapilot/ of local projects about to be overwritten
        let sel: Vec<ProjectSel> = targets.iter().filter_map(|(_, _, t)| known.iter().find(|k| k["path"].as_str().map(|x| x.trim_end_matches(['/', '\\']) == norm(t)).unwrap_or(false)))
            .filter_map(|k| k["id"].as_str()).map(|id| ProjectSel { id: id.into(), files: false }).collect();
        let pre = dir(&sup).join(format!("{PRE_PREFIX}{}-v{}.zip", stamp(), crate::VERSION));
        export(core, &pre, &sel, "pre-restore").map_err(|e| format!("恢复前自动备份失败，未做任何改动：{e}"))?;
        prune(&dir(&sup), PRE_PREFIX, AUTO_KEEP);
        rep.insert("pre_backup".into(), json!(pre));
    }
    let a = tmp.join("app");
    let restored_ids: HashSet<String> = targets.iter().map(|t| t.0.clone()).collect();
    let mut chats_added = 0usize;
    if !projects_only && a.exists() {
        if replace {
            for d in ["sessions", "memory", "skills"] { swap_dir(&a.join(d), &sup.join(d))?; }
            if a.join("DATAPILOT.md").is_file() { std::fs::copy(a.join("DATAPILOT.md"), sup.join("DATAPILOT.md")).map_err(e2s)?; } else { let _ = std::fs::remove_file(sup.join("DATAPILOT.md")); }
            if let Ok(t) = std::fs::read_to_string(a.join("config.json")) { write_atomic(&sup.join("config.json"), &t)?; }
            core.ui.restore_from(&a.join("app.sqlite"))?;
            core.index.restore_from(&a.join("index.sqlite"))?;
            chats_added = core.ui.list_sessions(true).len();
            rep.insert("replaced".into(), json!(true));
        } else {
            // chats: folders that are not here yet, then their index rows (ids kept → a second import adds nothing)
            let sd = sup.join("sessions");
            std::fs::create_dir_all(&sd).map_err(e2s)?;
            for e in std::fs::read_dir(a.join("sessions")).into_iter().flatten().flatten() {
                let id = e.file_name().to_string_lossy().to_string();
                if sd.join(&id).exists() || core.ui.exists(&id) { continue; }
                std::fs::rename(e.path(), sd.join(&id)).map_err(e2s)?;
            }
            let (ma, mu) = merge_memory(&a.join("memory"), &crate::memory::user_dir(&sup), "app/memory", &files)?;
            let (sa, su) = merge_tree(&a.join("skills"), &sup.join("skills"), "app/skills", &files, true, &|_| false)?;
            let local_md = std::fs::read_to_string(sup.join("DATAPILOT.md")).unwrap_or_default();
            if let Ok(t) = std::fs::read_to_string(a.join("DATAPILOT.md")) {
                if local_md.trim().is_empty() { std::fs::write(sup.join("DATAPILOT.md"), t).map_err(e2s)?; }
                else if local_md != t { rep.insert("kept_local".into(), json!(["DATAPILOT.md"])); }
            }
            // settings stay local; model services that exist only in the backup are added (keys re-entered)
            let mut added_providers = vec![];
            if let Ok(b) = std::fs::read_to_string(a.join("config.json")).map_err(e2s).and_then(|t| serde_json::from_str::<Value>(&t).map_err(e2s)) {
                let mut local: Value = std::fs::read_to_string(sup.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
                if !local["providers"].is_array() { local["providers"] = json!([]); }
                let have: HashSet<String> = local["providers"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str().map(String::from)).collect();
                for p in b["providers"].as_array().into_iter().flatten() {
                    if p["id"].as_str().map(|id| !have.contains(id)).unwrap_or(false) {
                        added_providers.push(p["name"].clone());
                        local["providers"].as_array_mut().unwrap().push(p.clone());
                    }
                }
                if local["roles"]["main"].is_null() && !b["roles"]["main"].is_null() { local["roles"] = b["roles"].clone(); }
                if !added_providers.is_empty() || local["roles"] != b["roles"] && local["roles"]["main"] == b["roles"]["main"] {
                    write_atomic(&sup.join("config.json"), &serde_json::to_string_pretty(&local).unwrap())?;
                }
            }
            let new = core.ui.merge_from(&a.join("app.sqlite"), &restored_ids)?;
            core.index.merge_from(&a.join("index.sqlite"));
            chats_added = new.len();
            rep.insert("memory_added".into(), json!(ma));
            rep.insert("memory_updated".into(), json!(mu));
            rep.insert("skills_changed".into(), json!(sa.len() + su.len()));
            rep.insert("providers_added".into(), json!(added_providers));
        }
    } else if projects_only && a.exists() && !replace {
        chats_added = core.ui.merge_from(&a.join("app.sqlite"), &restored_ids)?.len();
        core.index.merge_from(&a.join("index.sqlite"));
    }
    // projects: .datapilot/ (merge: missing files, newer memory; replace: swapped) + own files (missing ones only)
    let mut done = vec![];
    for (id, name, target) in &targets {
        let src = tmp.join("projects").join(id);
        let dp = target.join(".datapilot");
        let pre = format!("projects/{id}/datapilot");
        if src.join("datapilot").exists() {
            if replace || !dp.exists() { swap_dir(&src.join("datapilot"), &dp)?; }
            else {
                merge_memory(&src.join("datapilot/memory"), &dp.join("memory"), &format!("{pre}/memory"), &files)?;
                merge_tree(&src.join("datapilot"), &dp, &pre, &files, false, &|r| r == "memory")?;
            }
        }
        let mut copied = 0;
        if src.join("files").exists() {
            copied = merge_tree(&src.join("files"), target, &format!("projects/{id}/files"), &files, false, &|_| false)?.0.len();
        }
        let p = core.ui.open_project(target)?;
        done.push(json!({"id": p["id"], "name": name, "path": target, "files_added": copied}));
    }
    if replace { chats_added = core.ui.list_sessions(true).len(); }
    rep.insert("chats_added".into(), json!(chats_added));
    rep.insert("projects_restored".into(), json!(done));
    rep.insert("needs_relink".into(), json!(missing));
    rep.insert("secrets_needed".into(), m["secrets_needed"].clone());
    rep.insert("version".into(), m["version"].clone());
    Ok(Value::Object(rep))
}

/// Backups in AppSupport/backups (newest first).
pub fn list(app_support: &Path) -> Vec<Value> {
    let mut v: Vec<(f64, Value)> = std::fs::read_dir(dir(app_support)).into_iter().flatten().flatten().filter_map(|e| {
        let n = e.file_name().to_string_lossy().to_string();
        if !n.ends_with(".zip") { return None; }
        let p = e.path();
        let kind = if n.starts_with(AUTO_PREFIX) { "auto" } else if n.starts_with(PRE_PREFIX) { "pre-restore" } else { "manual" };
        let t = mtime(&p);
        Some((t, json!({"name": n, "path": p, "size": std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0), "mtime": t, "kind": kind})))
    }).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    v.into_iter().map(|x| x.1).collect()
}

/// Keep the newest `keep` zips starting with `prefix`.
pub fn prune(d: &Path, prefix: &str, keep: usize) {
    let mut v: Vec<(f64, PathBuf)> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with(prefix) && n.ends_with(".zip")).unwrap_or(false))
        .map(|p| (mtime(&p), p)).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, p) in v.into_iter().skip(keep) { let _ = std::fs::remove_file(p); }
}

static LAST_TRY: Mutex<f64> = Mutex::new(0.0);

/// Weekly automatic backup (no project data), from the idle tick: only while no kernel runs (DuckDB files closed),
/// at most one attempt every 6 hours. Keeps AUTO_KEEP.
pub fn auto_tick(core: &Core) -> Option<PathBuf> {
    if !core.config.lock().unwrap().settings.auto_backup || core.kernels_running() { return None; }
    let sup = &core.paths.app_support;
    let d = dir(sup);
    let newest = list(sup).iter().filter(|b| b["kind"] == "auto").filter_map(|b| b["mtime"].as_f64()).fold(0.0, f64::max);
    let now = crate::text::now();
    if now - newest < AUTO_EVERY_SECS { return None; }
    { let mut t = LAST_TRY.lock().unwrap(); if now - *t < 6.0 * 3600.0 { return None; } *t = now; }
    let dst = d.join(format!("{AUTO_PREFIX}{}-v{}.zip", stamp(), crate::VERSION));
    export(core, &dst, &[], "auto").ok()?;
    prune(&d, AUTO_PREFIX, AUTO_KEEP);
    Some(dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::secrets::StaticSecrets;
    use std::sync::Arc;

    const SECRET: &str = "sk-test-SECRET-abcdefghijklmnop";

    fn core_at(sup: &Path) -> Arc<Core> {
        let cfg: crate::types::Config = std::fs::read_to_string(sup.join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut s = StaticSecrets::default();
        s.api_keys.insert("p1".into(), SECRET.into());
        Core::new(Paths::dev(sup.to_path_buf()), cfg, Arc::new(s)).unwrap()
    }

    fn entries(zip: &Path) -> Vec<(String, Vec<u8>)> {
        let mut z = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
        (0..z.len()).map(|i| { let mut e = z.by_index(i).unwrap(); let mut b = vec![]; e.read_to_end(&mut b).unwrap(); (e.name().to_string(), b) }).collect()
    }

    #[test]
    fn round_trip_merge_replace() {
        let root = std::env::temp_dir().join(format!("dp-bk-{}", crate::text::short_id(6)));
        let (sup, sup2, proj) = (root.join("a"), root.join("b"), root.join("proj"));
        for d in [&sup, &sup2, &proj] { std::fs::create_dir_all(d).unwrap(); }
        std::fs::write(sup.join("config.json"), json!({"providers": [{"id": "p1", "name": "GW", "baseURL": "http://x", "extraHeaders": {"Authorization": format!("Bearer {SECRET}"), "X-Team": "a"}}],
            "roles": {"main": {"providerId": "p1", "model": "m"}}, "settings": {"mcpServers": [{"id": "m1", "name": "gh", "env": {"GITHUB_TOKEN": SECRET}}]}}).to_string()).unwrap();
        for d in ["chrome-profile", "logs", "ocr-cache"] { std::fs::create_dir_all(sup.join(d)).unwrap(); std::fs::write(sup.join(d).join("x.txt"), "x").unwrap(); }
        let c = core_at(&sup);
        let s = c.ui.create_session("聊天一", None).unwrap();
        let sid = s["id"].as_str().unwrap().to_string();
        c.ui.index_message(&sid, "m1", "各大区的销售额");
        std::fs::create_dir_all(sup.join("sessions").join(&sid).join("tmp")).unwrap();
        std::fs::write(sup.join("sessions").join(&sid).join("tmp/scratch.bin"), "tmp").unwrap();
        crate::memory::write(&crate::memory::user_dir(&sup), "金额单位", "金额用万元", "user", "以后金额都用万元", None).unwrap();
        std::fs::create_dir_all(sup.join("skills/my-skill")).unwrap();
        std::fs::write(sup.join("skills/my-skill/SKILL.md"), "---\nname: my-skill\n---\nx").unwrap();
        std::fs::write(sup.join("DATAPILOT.md"), "结论放最前面").unwrap();
        let p = c.ui.open_project(&proj).unwrap();
        let pid = p["id"].as_str().unwrap().to_string();
        let ps = c.ui.create_session("项目聊天", Some(&pid)).unwrap();
        let psid = ps["id"].as_str().unwrap().to_string();
        std::fs::write(proj.join(".datapilot/data/x.duckdb"), "duck").unwrap();
        std::fs::write(proj.join("a.csv"), "a,b\n1,2\n").unwrap();

        let zip = root.join("out/backup.zip");
        let r = export(&c, &zip, &[ProjectSel { id: pid.clone(), files: true }], "manual").unwrap();
        assert!(r["files"].as_u64().unwrap() > 5);
        let es = entries(&zip);
        let names: Vec<&str> = es.iter().map(|e| e.0.as_str()).collect();
        for want in ["manifest.json", "app/app.sqlite", "app/index.sqlite", "app/config.json", "app/DATAPILOT.md", "app/skills/my-skill/SKILL.md",
                     &format!("projects/{pid}/datapilot/data/x.duckdb"), &format!("projects/{pid}/files/a.csv")] {
            assert!(names.contains(&want), "missing {want}: {names:?}");
        }
        assert!(!names.iter().any(|n| n.contains("chrome-profile") || n.contains("logs/") || n.contains("ocr-cache") || n.contains("/tmp/") || n.contains("files/.datapilot")), "{names:?}");
        for (n, b) in &es { assert!(!String::from_utf8_lossy(b).contains(SECRET), "secret in {n}"); }
        let man: Value = serde_json::from_slice(&es.iter().find(|e| e.0 == "manifest.json").unwrap().1).unwrap();
        let needs = man["secrets_needed"].to_string();
        assert!(needs.contains("api_key") && needs.contains("Authorization") && needs.contains("GITHUB_TOKEN"), "{needs}");

        // preview + merge into a fresh install
        let c2 = core_at(&sup2);
        let pv = preview(&c2, &zip).unwrap();
        assert_eq!(pv["chats"], 2);
        assert_eq!(pv["chats_new"], 2);
        let r = import(&c2, &zip, "merge", &HashMap::new(), false).unwrap();
        assert_eq!(r["chats_added"], 2, "{r}");
        let sessions = c2.ui.list_sessions(true);
        assert!(sessions.iter().any(|s| s["id"] == sid.as_str()) && sessions.iter().any(|s| s["id"] == psid.as_str()));
        assert!(sup2.join("sessions").join(&sid).join("transcript.jsonl").exists());
        assert_eq!(crate::memory::topics(&crate::memory::user_dir(&sup2), 50).len(), 1);
        assert!(sup2.join("skills/my-skill/SKILL.md").exists() && sup2.join("DATAPILOT.md").exists());
        assert_eq!(c2.ui.search("销售额", 10).len(), 1);
        let cfg2 = std::fs::read_to_string(sup2.join("config.json")).unwrap();
        assert!(cfg2.contains("\"p1\"") && !cfg2.contains(SECRET));
        // merging again adds nothing
        let r = import(&c2, &zip, "merge", &HashMap::new(), false).unwrap();
        assert_eq!(r["chats_added"], 0, "{r}");
        assert_eq!(c2.ui.list_sessions(true).len(), sessions.len());
        assert_eq!(c2.ui.search("销售额", 10).len(), 1);
        let idx = std::fs::read_to_string(crate::memory::user_dir(&sup2).join("MEMORY.md")).unwrap();
        assert_eq!(idx.lines().count(), 1, "{idx}");

        // the project folder is gone on the new computer → relink to an empty folder (own files restored there)
        std::fs::rename(&proj, root.join("moved")).unwrap();
        let sup3 = root.join("c");
        std::fs::create_dir_all(&sup3).unwrap();
        let c3 = core_at(&sup3);
        let r = import(&c3, &zip, "merge", &HashMap::new(), false).unwrap();
        assert_eq!(r["needs_relink"][0]["id"], pid.as_str(), "{r}");
        let fresh = root.join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let r = import(&c3, &zip, "merge", &HashMap::from([(pid.clone(), fresh.clone())]), true).unwrap();
        assert_eq!(r["projects_restored"].as_array().unwrap().len(), 1, "{r}");
        assert!(fresh.join("a.csv").exists() && fresh.join(".datapilot/data/x.duckdb").exists());
        assert!(c3.ui.list_sessions(true).iter().any(|s| s["id"] == psid.as_str()));

        // replace: a pre-restore backup is made first
        c2.ui.create_session("只在 b", None).unwrap();
        let r = import(&c2, &zip, "replace", &HashMap::new(), false).unwrap();
        assert!(Path::new(r["pre_backup"].as_str().unwrap()).exists(), "{r}");
        assert!(!c2.ui.list_sessions(true).iter().any(|s| s["title"] == "只在 b"));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn evil_zip(p: &Path, name: &str, listed: bool, sha_ok: bool) {
        use zip::write::SimpleFileOptions;
        let body = b"pwned";
        let sha = if sha_ok { hex(&Sha256::digest(body)) } else { "00".into() };
        let files = if listed { json!({name: {"size": body.len(), "sha256": sha, "mtime": 0}}) } else { json!({}) };
        let mut w = zip::ZipWriter::new(File::create(p).unwrap());
        w.start_file(name, SimpleFileOptions::default()).unwrap();
        w.write_all(body).unwrap();
        w.start_file("manifest.json", SimpleFileOptions::default()).unwrap();
        w.write_all(json!({"format": 1, "app": "DataPilot", "version": "0.1.9", "files": files, "projects": []}).to_string().as_bytes()).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn rejects_malicious_archives() {
        let root = std::env::temp_dir().join(format!("dp-bk-evil-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(root.join("sup")).unwrap();
        let c = core_at(&root.join("sup"));
        for (name, listed, sha) in [("../evil.txt", true, true), ("app/../../evil.txt", true, true), ("/tmp/evil.txt", true, true), ("C:/evil.txt", true, true),
                                    ("projects/../x/datapilot/a", true, true), ("other/x.txt", true, true), ("app/unlisted.txt", false, true), ("app/bad.txt", true, false)] {
            let z = root.join("evil.zip");
            evil_zip(&z, name, listed, sha);
            assert!(preview(&c, &z).is_err(), "accepted {name}");
            assert!(import(&c, &z, "merge", &HashMap::new(), false).is_err(), "imported {name}");
        }
        assert!(!root.join("evil.txt").exists() && !std::env::temp_dir().join("evil.txt").exists());
        assert!(safe_rel("app/sessions/x/transcript.jsonl").is_some() && safe_rel("projects/p_ab12/files/a b.csv").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
