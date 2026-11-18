//! UI RPCs (M4 phase A): what a UI needs besides the agent loop — session / project index, history and export,
//! config and secrets (write-only), model listing and capability probe, network test, Chrome install, New API keys,
//! memory topic files, dashboards, skills, file lists, Computer Use permissions. Child module of `server`.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{send, Server};
use crate::types::{ChatMessage, ImagePart, ModelRef, ToolSpec};

/// Methods that block (network, process shutdown, file moves): answered from a worker thread.
pub const ASYNC: &[&str] = &["config/set", "secret/set", "provider/models", "provider/probe", "net/test", "chrome/install",
    "newapi/status", "newapi/login", "newapi/tokens", "newapi/use", "newapi/create", "newapi/logout",
    "project/delete_data", "project/remove", "session/delete", "session/export", "session/fork", "app/doctor"];

pub const METHODS: &[&str] = &["session/list", "session/create", "session/rename", "session/pin", "session/archive", "session/delete",
    "session/touch", "session/search", "session/history", "session/export",
    "project/list", "project/open", "project/relink", "project/remove", "project/delete_data",
    "config/get", "config/set", "secret/set", "secret/has", "provider/models", "provider/probe", "net/test",
    "chrome/status", "chrome/install", "newapi/status", "newapi/login", "newapi/tokens", "newapi/use", "newapi/create", "newapi/logout",
    "memory/topics", "dashboard/list", "dashboard/get", "dashboard/save", "dashboard/patch", "dashboard/pin", "dashboard/delete",
    "skill/list", "skill/get", "skill/save", "skill/delete", "files/list", "files/import", "files/save", "files/image", "computer/permissions", "computer/request", "app/about",
    "session/fork", "app/doctor", "model/preset"];

/// 64×32 PNG, left half red, right half blue (vision probe).
const PROBE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAAAgCAIAAAAt/+nTAAAAN0lEQVR42u3PsQkAAAzDsPz/dHpDh2wCzwalybTxvgEAAAAAAAAAAAAAAAAAAAAAAAAAAAAA+HY6S/hqZZnSdgAAAABJRU5ErkJggg==";

static CONFIG_LOCK: Mutex<()> = Mutex::new(());

fn s<'a>(p: &'a Value, k: &str) -> Result<&'a str, String> { p[k].as_str().filter(|x| !x.trim().is_empty()).ok_or_else(|| format!("missing {k}")) }

/// Deep merge: objects recursively, `null` deletes a key, anything else replaces.
pub fn merge(dst: &mut Value, patch: &Value) {
    match (dst.as_object_mut(), patch.as_object()) {
        (Some(d), Some(p)) => for (k, v) in p {
            if v.is_null() { d.remove(k); }
            else if v.is_object() && d.get(k).map(Value::is_object).unwrap_or(false) { merge(d.get_mut(k).unwrap(), v); }
            else { d.insert(k.clone(), v.clone()); }
        },
        _ => *dst = patch.clone(),
    }
}

pub fn read_raw(path: &Path) -> Value {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// Merge `patch` into config.json (unknown keys kept), check it still parses, write atomically. Returns the new file.
pub fn patch_config(path: &Path, patch: &Value) -> Result<Value, String> {
    let _g = CONFIG_LOCK.lock().unwrap();
    let mut v = read_raw(path);
    merge(&mut v, patch);
    serde_json::from_value::<crate::types::Config>(v.clone()).map_err(|e| format!("配置无效 / invalid config: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Port of Swift `LocalModels.isLocal`: explicit mark wins, else model name / endpoint host.
pub fn is_local_model(provider_id: &str, model: &str, base_url: &str, marks: &Value) -> bool {
    if let Some(b) = marks[format!("{provider_id}|{model}")].as_bool() { return b; }
    let m = model.to_lowercase();
    if m.contains("local") || m.contains("ollama") || m.starts_with("lmstudio") { return true; }
    let h = base_url.split("://").nth(1).unwrap_or(base_url).split(['/', '?']).next().unwrap_or("");
    let h = if h.starts_with('[') { h.trim_start_matches('[').split(']').next().unwrap_or("") } else { h.rsplit_once(':').map(|x| x.0).unwrap_or(h) }.to_lowercase();
    if h == "localhost" || h == "::1" || h.ends_with(".local") || h.starts_with("127.") { return true; }
    let parts: Vec<u32> = h.split('.').filter_map(|x| x.parse().ok()).collect();
    parts.len() == 4 && (parts[0] == 10 || (parts[0] == 192 && parts[1] == 168) || (parts[0] == 172 && (16..=31).contains(&parts[1])))
}

/// Dashboard / skill ids become file names: letters, digits, `_`, `-` only.
fn safe_id(id: &str) -> Result<&str, String> {
    if !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') { Ok(id) } else { Err(format!("invalid id {id:?}")) }
}

pub struct SessionLoc { pub id: String, pub workspace: PathBuf, pub project_root: Option<PathBuf>, pub transcript: PathBuf }

impl Server {
    fn notify(&self, method: &str, params: Value) { send(&self.out, &json!({"jsonrpc": "2.0", "method": method, "params": params})); }

    /// Where a session lives: from the open session, else from the UI index.
    fn loc(&self, p: &Value) -> Result<SessionLoc, String> {
        let id = s(p, "session")?.to_string();
        if let Ok(sh) = self.shared(p) {
            return Ok(SessionLoc { id, workspace: sh.workspace.clone(), project_root: sh.project_root.clone(), transcript: sh.transcript.clone() });
        }
        let ui = &self.core.ui;
        Ok(SessionLoc { workspace: ui.workspace(&id), project_root: ui.session_project(&id).map(|x| x.1), transcript: ui.transcript(&id), id })
    }

    fn reload(self: &Arc<Self>) -> Value { self.handle("config/reload", &json!({})).unwrap_or_else(|e| json!({"error": e})) }

    fn close_session(&self, id: &str) {
        if let Some(e) = self.sessions.lock().unwrap().remove(id) { e.shared.broker.cancel_all(); }
        crate::tools::browser::close(id);
    }

    /// Close open sessions of a project and release its DuckDB file.
    fn close_project(&self, root: &Path) {
        let ids: Vec<String> = self.sessions.lock().unwrap().iter().filter(|(_, e)| e.shared.project_root.as_deref() == Some(root)).map(|(k, _)| k.clone()).collect();
        for id in ids { self.close_session(&id); }
        self.core.close_workspace(&root.join(".datapilot/data"));
    }

    fn memory_dir(&self, p: &Value) -> Result<PathBuf, String> {
        match p["scope"].as_str().unwrap_or("user") {
            "user" => Ok(crate::memory::user_dir(&self.core.paths.app_support)),
            "project" => {
                let root = match p["root"].as_str() { Some(r) => PathBuf::from(r), None => self.loc(p)?.project_root.ok_or("该会话不属于项目 / not a project session")? };
                Ok(crate::memory::project_dir(&root))
            }
            x => Err(format!("unknown scope {x}")),
        }
    }

    fn settings(&self) -> crate::types::Settings { self.core.config.lock().unwrap().settings.clone() }

    /// UI dispatch; `None` = not a UI method.
    pub(super) fn handle_ui(self: &Arc<Self>, method: &str, p: &Value) -> Option<Result<Value, String>> {
        if !METHODS.contains(&method) { return None; }
        Some(self.ui_call(method, p))
    }

    fn ui_call(self: &Arc<Self>, method: &str, p: &Value) -> Result<Value, String> {
        let ui = &self.core.ui;
        let app_support = self.core.paths.app_support.clone();
        let cfg_path = app_support.join("config.json");
        match method {
            // ---------- sessions ----------
            "session/list" => Ok(json!({"sessions": ui.list_sessions(p["include_archived"].as_bool().unwrap_or(false)), "projects": ui.list_projects()})),
            "session/create" => ui.create_session(p["title"].as_str().unwrap_or("新会话"), p["project_id"].as_str()),
            "session/rename" => {
                let (id, t) = (s(p, "session")?, s(p, "title")?.trim());
                ui.rename(id, t);
                self.core.index.set_title(id, t);
                Ok(json!({}))
            }
            "session/pin" | "session/archive" => {
                ui.set_flag(s(p, "session")?, if method == "session/pin" { "pinned" } else { "archived" }, p["on"].as_bool().unwrap_or(true));
                Ok(json!({}))
            }
            "session/touch" => { ui.touch(s(p, "session")?); Ok(json!({})) }
            "session/delete" => {
                let l = self.loc(p)?;
                self.close_session(&l.id);
                if l.project_root.is_none() { self.core.close_workspace(&l.workspace); }
                ui.delete_session(&l.id);
                self.core.index.delete_session(&l.id);
                Ok(json!({}))
            }
            "session/search" => Ok(json!({"hits": ui.search(p["q"].as_str().unwrap_or(""), p["limit"].as_u64().unwrap_or(200) as usize)})),
            "session/history" => {
                let l = self.loc(p)?;
                let mut h = crate::ui::history(&l.transcript);
                h["title"] = json!(ui.title(&l.id).unwrap_or_default());
                h["workspace"] = json!(l.workspace);
                h["project_root"] = json!(l.project_root);
                Ok(h)
            }
            "session/export" => {
                let l = self.loc(p)?;
                let title = ui.title(&l.id).unwrap_or_else(|| l.id.clone());
                let items = crate::ui::history(&l.transcript)["items"].as_array().cloned().unwrap_or_default();
                let text = match p["format"].as_str().unwrap_or("md") {
                    "md" | "markdown" => crate::ui::export_markdown(&title, &items),
                    "html" => crate::ui::export_html(&title, &items),
                    "jsonl" => std::fs::read_to_string(&l.transcript).map_err(|e| e.to_string())?,
                    f => return Err(format!("unknown format {f}")),
                };
                match p["path"].as_str() {
                    Some(path) => { std::fs::write(path, &text).map_err(|e| format!("{path}: {e}"))?; Ok(json!({"path": path})) }
                    None => Ok(json!({"text": text})),
                }
            }
            // ---------- projects ----------
            "project/list" => Ok(json!({"projects": ui.list_projects()})),
            "project/open" => ui.open_project(Path::new(s(p, "path")?)),
            "project/relink" => ui.relink_project(s(p, "id")?, Path::new(s(p, "path")?)),
            "project/remove" | "project/delete_data" => {
                let pid = s(p, "id")?;
                let root = ui.list_projects().into_iter().find(|x| x["id"] == pid).and_then(|x| x["path"].as_str().map(PathBuf::from));
                if let Some(r) = &root { self.close_project(r); }
                ui.remove_project(pid);
                let mut failed = vec![];
                if method == "project/delete_data" {
                    for it in root.as_deref().map(crate::ui::UiDb::generated_items).unwrap_or_default() {
                        if let Err(e) = crate::ui::move_to_trash(&it) { failed.push(e); }
                    }
                }
                Ok(json!({"root": root, "failed": failed}))
            }
            // ---------- config / secrets ----------
            "config/get" => Ok(read_raw(&cfg_path)),
            "config/set" => {
                let patch = p.get("patch").filter(|v| v.is_object()).ok_or("missing patch")?;
                let v = patch_config(&cfg_path, patch)?;
                Ok(json!({"config": v, "reload": self.reload()}))
            }
            // write-only: values go to the keychain / credential store, never back to a UI
            "secret/set" => {
                let (kind, id) = (s(p, "kind")?, s(p, "id")?);
                self.core.secrets.set(kind, id, p["value"].as_str())?;
                let reload = (kind == "api_key").then(|| self.reload());
                Ok(json!({"stored": p["value"].as_str().map(|v| !v.trim().is_empty()).unwrap_or(false), "reload": reload}))
            }
            "secret/has" => {
                let (kind, id) = (s(p, "kind")?, s(p, "id")?);
                let sec = &self.core.secrets;
                Ok(json!({"has": match kind { "api_key" => sec.api_key(id), "db_password" => sec.db_password(id), "ssh_secret" => sec.ssh_secret(id), k => return Err(format!("unknown kind {k}")) }.is_some()}))
            }
            // ---------- models / network ----------
            "provider/models" => {
                let raw = read_raw(&cfg_path);
                let prov: crate::types::Provider = match p.get("provider").filter(|v| v.is_object()) {
                    Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("bad provider: {e}"))?,
                    None => { let id = s(p, "provider_id")?; self.core.config.lock().unwrap().providers.iter().find(|x| x.id == id).cloned().ok_or_else(|| format!("unknown provider {id}"))? }
                };
                let key = self.core.secrets.api_key(&prov.id);
                let c = self.core.client_for_provider(prov.clone(), key)?;
                let ids = c.list_models_meta().map_err(|e| e.to_string())?;
                let marks = &raw["settings"]["localMarks"];
                Ok(json!({"models": ids.iter().map(|(m, ctx)| json!({"id": m, "local": is_local_model(&prov.id, m, &prov.base_url, marks), "context_window": ctx})).collect::<Vec<_>>()}))
            }
            "provider/probe" => self.probe(s(p, "role")?, &cfg_path),
            "net/test" => {
                let url = p["url"].as_str().unwrap_or("https://www.google.com/generate_204");
                if !(url.starts_with("https://") || url.starts_with("http://")) { return Err("invalid url".into()); }
                let mut st = self.settings();
                if let Some(o) = p.get("settings").filter(|v| v.is_object()) {   // unsaved form values
                    let mut v = serde_json::to_value(&st).unwrap(); merge(&mut v, o);
                    st = serde_json::from_value(v).map_err(|e| e.to_string())?;
                }
                let (agent, proxy) = crate::llm::http_agent(url, &st, 15)?;
                let t0 = Instant::now();
                let r = agent.get(url).call();
                let ms = t0.elapsed().as_millis() as u64;
                Ok(match r {
                    Ok(resp) => json!({"ok": true, "status": resp.status(), "ms": ms, "proxy": proxy}),
                    Err(ureq::Error::Status(c, _)) => json!({"ok": c < 500, "status": c, "ms": ms, "proxy": proxy}),
                    Err(e) => json!({"ok": false, "error": e.to_string(), "ms": ms, "proxy": proxy}),
                })
            }
            // ---------- Chrome ----------
            "chrome/status" => {
                let exe = crate::chrome::find(&app_support);
                let managed = exe.as_ref().map(|e| e.starts_with(crate::chrome::install_dir(&app_support))).unwrap_or(false);
                Ok(json!({"path": exe, "managed": managed, "version": exe.as_deref().map(crate::chrome::major_version)}))
            }
            "chrome/install" => {
                let (agent, _) = crate::llm::http_agent(crate::chrome::CFT_INDEX, &self.settings(), 600)?;
                let me = self.clone();
                let exe = crate::chrome::install(&app_support, &agent, &move |t: &str| me.notify("core/notice", json!({"topic": "chrome", "text": t})))?;
                Ok(json!({"path": exe}))
            }
            // ---------- New API gateway ----------
            "newapi/status" | "newapi/login" | "newapi/tokens" | "newapi/use" | "newapi/create" | "newapi/logout" => {
                let origin = crate::newapi::origin(s(p, "origin")?)?;
                let (agent, _) = crate::llm::http_agent(&origin, &self.settings(), 20)?;
                let a = crate::newapi::Auth::from(&p["auth"]);
                match method {
                    "newapi/status" => Ok(json!({"status": crate::newapi::status(&agent, &origin)?})),
                    "newapi/login" => {
                        let a = crate::newapi::login(&agent, &origin, s(p, "username")?, p["password"].as_str().unwrap_or(""))?;
                        Ok(json!({"auth": {"user_id": a.user_id, "cookie": a.cookie}}))
                    }
                    "newapi/tokens" => Ok(json!({"tokens": crate::newapi::tokens(&agent, &origin, &a)?})),
                    "newapi/create" => Ok(json!({"token": crate::newapi::create(&agent, &origin, &a, p["name"].as_str().unwrap_or("DataPilot"))?})),
                    "newapi/logout" => { crate::newapi::logout(&agent, &origin, &a); Ok(json!({})) }
                    _ => {   // full key → secret store for `provider_id`; the UI only sees the masked form
                        let key = crate::newapi::full_key(&agent, &origin, &a, p["token_id"].as_i64().ok_or("missing token_id")?)?;
                        self.core.secrets.set("api_key", s(p, "provider_id")?, Some(&key))?;
                        Ok(json!({"stored": true, "masked": crate::newapi::mask(&key), "reload": self.reload()}))
                    }
                }
            }
            // ---------- memory topic files ----------
            "memory/topics" => {
                let d = self.memory_dir(p)?;
                match p["action"].as_str().unwrap_or("list") {
                    "list" => Ok(json!({"dir": d, "topics": crate::ui::topics_json(&d), "index": crate::memory::load_index(&d)})),
                    "write" => {
                        let t = crate::memory::write(&d, s(p, "name")?, p["description"].as_str().unwrap_or(""), p["type"].as_str().unwrap_or("project"),
                                                     p["body"].as_str().unwrap_or(""), p["file"].as_str())?;
                        Ok(json!({"file": t.file}))
                    }
                    "delete" => { crate::memory::delete(&d, s(p, "file")?)?; Ok(json!({})) }
                    a => Err(format!("unknown action {a}")),
                }
            }
            // ---------- dashboards ----------
            "dashboard/list" => {
                let l = self.loc(p)?;
                Ok(json!({"dashboards": crate::ui::dashboard_entries(&l.workspace), "session_dashboard": crate::ui::session_dashboard_id(&l.id, l.project_root.is_some())}))
            }
            "dashboard/get" => {
                let l = self.loc(p)?;
                crate::tools::bi::load(safe_id(s(p, "id")?)?, &l.workspace).map(|spec| json!({"spec": spec})).ok_or_else(|| "看板不存在 / dashboard not found".into())
            }
            "dashboard/save" => {
                let l = self.loc(p)?;
                let id = safe_id(s(p, "id")?)?;
                let spec = p.get("spec").filter(|v| v.is_object()).ok_or("missing spec")?;
                if let Some(e) = crate::tools::bi::validate(spec) { return Err(e); }
                crate::tools::bi::save(spec, id, &l.workspace)?;
                Ok(json!({"id": id}))
            }
            "dashboard/patch" => {
                let l = self.loc(p)?;
                let patch = p["patch"].as_object().ok_or("missing patch")?;
                Ok(json!({"patched": crate::ui::patch_widget(&l.workspace, safe_id(s(p, "id")?)?, s(p, "widget")?, patch)?}))
            }
            "dashboard/pin" => {
                let l = self.loc(p)?;
                let chart = p.get("chart").filter(|v| v.is_object()).ok_or("missing chart")?;
                Ok(json!({"id": crate::ui::pin_chart(&l.workspace, chart, &l.id, l.project_root.is_some(), p["title"].as_str().unwrap_or("本会话看板"))?}))
            }
            "dashboard/delete" => {
                let l = self.loc(p)?;
                let f = l.workspace.join("dashboards").join(format!("{}.json", safe_id(s(p, "id")?)?));
                let _ = std::fs::remove_file(f);
                Ok(json!({}))
            }
            // ---------- skills ----------
            "skill/list" => {
                let st = self.settings();
                let sk = crate::prompt::skills_for(&self.core.paths, &st);
                Ok(json!({"skills": sk.iter().map(|k| json!({"name": k.name, "description": k.description, "source": k.source, "dir": k.dir})).collect::<Vec<_>>(),
                          "user_dir": app_support.join("skills"), "external_dirs": crate::prompt::external_dirs(&st),
                          "external_exists": crate::prompt::external_dirs(&st).iter().map(|d| d.is_dir()).collect::<Vec<_>>()}))
            }
            "skill/get" => {
                let n = s(p, "name")?;
                let k = crate::prompt::skills_for(&self.core.paths, &self.settings()).into_iter().find(|k| k.name == n).ok_or_else(|| format!("unknown skill {n}"))?;
                Ok(json!({"name": k.name, "description": k.description, "source": k.source, "dir": k.dir, "body": k.body}))
            }
            "skill/save" => {
                let re = regex::Regex::new("[^a-z0-9-]+").unwrap();
                let name = re.replace_all(&s(p, "name")?.to_lowercase(), "-").trim_matches('-').to_string();
                safe_id(&name)?;
                let dir = app_support.join("skills").join(&name);
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let text = format!("---\nname: {name}\ndescription: {}\n---\n\n{}\n", p["description"].as_str().unwrap_or("").replace('\n', " "), p["body"].as_str().unwrap_or(""));
                std::fs::write(dir.join("SKILL.md"), text).map_err(|e| e.to_string())?;
                Ok(json!({"name": name, "dir": dir}))
            }
            "skill/delete" => {
                let dir = app_support.join("skills").join(safe_id(s(p, "name")?)?);
                if !dir.join("SKILL.md").exists() { return Err("只能删除用户自己的 skill / only user skills can be deleted".into()); }
                std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
                Ok(json!({}))
            }
            "files/list" => { let l = self.loc(p)?; Ok(crate::ui::files(&l.workspace, l.project_root.as_deref())) }
            // image inside the session's workspace / project (browser screenshots, saved charts) as a data URL
            "files/image" => {
                let l = self.loc(p)?;
                let real = crate::platform::canonicalize(s(p, "path")?).map_err(|e| e.to_string())?;
                let roots: Vec<PathBuf> = [Some(l.workspace.clone()), l.project_root.clone()].into_iter().flatten().filter_map(|r| crate::platform::canonicalize(r).ok()).collect();
                if !roots.iter().any(|r| real.starts_with(r)) { return Err("只能读取工作区内的图片 / outside the workspace".into()); }
                let mime = match real.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
                    Some("png") => "image/png", Some("jpg") | Some("jpeg") => "image/jpeg", Some("webp") => "image/webp", Some("gif") => "image/gif",
                    _ => return Err("不是图片 / not an image".into()),
                };
                let b = std::fs::read(&real).map_err(|e| e.to_string())?;
                if b.len() > 20 << 20 { return Err("图片太大 / image too large".into()); }
                use base64::Engine as _;
                Ok(json!({"data_url": format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(b))}))
            }
            // files the user dropped / picked: project files are referenced in place, others copied into uploads/
            "files/import" => {
                let l = self.loc(p)?;
                let paths: Vec<PathBuf> = p["paths"].as_array().ok_or("missing paths")?.iter().filter_map(|v| v.as_str().map(PathBuf::from)).collect();
                let (files, failed) = crate::ui::import_files(&l.workspace, l.project_root.as_deref(), &paths);
                Ok(json!({"files": files, "failed": failed}))
            }
            // export produced in the UI (chart CSV, dashboard HTML / PNG): outputs/ (project) or exports/, never overwrites
            "files/save" => {
                let l = self.loc(p)?;
                let data = match (p["text"].as_str(), p["base64"].as_str()) {
                    (Some(t), _) => t.as_bytes().to_vec(),
                    (None, Some(b)) => { use base64::Engine as _; base64::engine::general_purpose::STANDARD.decode(b.trim()).map_err(|e| e.to_string())? }
                    _ => return Err("missing text / base64".into()),
                };
                let path = crate::ui::save_export(&l.workspace, l.project_root.as_deref(), p["name"].as_str().unwrap_or("export"), s(p, "ext")?, &data, p["bom"].as_bool().unwrap_or(false))?;
                Ok(json!({"path": path}))
            }
            // ---------- Computer Use permissions ----------
            "computer/permissions" | "computer/request" => computer_rpc(method, p),
            // About page: credits.json and the bundled license files (repo root in dev builds)
            "app/about" => {
                let r = &self.core.paths.resources;
                let credits = std::fs::read_to_string(r.join("about/credits.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
                let doc = |n: &str| [Some(r.join("about").join(n)), r.parent().map(|p| p.join(n))].into_iter().flatten().find(|p| p.is_file());
                Ok(json!({"version": crate::VERSION, "credits": credits,
                          "docs": {"license": doc("LICENSE"), "notices": doc("THIRD_PARTY_NOTICES.md"), "chat2db": doc("CHAT2DB_LICENSE")}}))
            }
            // `/fork`: copy of a chat (transcript, notes; own workspace for non-project chats) to try another direction
            "session/fork" => {
                let l = self.loc(p)?;
                let title = format!("{}（分支）", ui.title(&l.id).unwrap_or_else(|| "新会话".into()));
                let pid = ui.session_project(&l.id).map(|x| x.0);
                let v = ui.create_session(&title, pid.as_deref())?;
                let nid = v["id"].as_str().unwrap_or_default().to_string();
                crate::ui::fork_files(&l.transcript, &ui.transcript(&nid), &nid,
                                      (l.project_root.is_none()).then(|| (l.workspace.clone(), ui.workspace(&nid))))?;
                self.core.index.upsert_session(&nid, l.project_root.as_ref().map(|r| r.to_string_lossy().to_string()).as_deref());
                self.core.index.set_title(&nid, &title);
                Ok(v)
            }
            "app/doctor" => Ok(self.doctor(p)),
            // resolved capabilities of a model (saved ones, else built-in table / preset) and the plain preset
            "model/preset" => {
                let model = p["model"].as_str().ok_or("missing model")?.to_string();
                let saved = p.get("capabilities").filter(|v| v.is_object()).map(|v| serde_json::from_value(v.clone())).transpose().map_err(|e| format!("bad capabilities: {e}"))?;
                let m = ModelRef { provider_id: String::new(), model: model.clone(), capabilities: saved };
                Ok(json!({"capabilities": m.caps(), "preset": crate::types::Capabilities::preset(&model), "table_size": crate::model_table::len()}))
            }
            m => Err(format!("method not found: {m}")),
        }
    }

    /// `/doctor`: one-shot self-check. Every item: {key, ok: true|false|null(warning), detail}.
    fn doctor(self: &Arc<Self>, p: &Value) -> Value {
        let mut items: Vec<Value> = vec![];
        let mut add = |key: &str, ok: Option<bool>, detail: String| items.push(json!({"key": key, "ok": ok, "detail": detail}));
        let paths = &self.core.paths;
        // bundled Python + core packages
        let py = crate::platform::python_exe(&paths.python_root);
        if !py.exists() { add("python", Some(false), format!("找不到 {}", py.display())); }
        else {
            let mut c = std::process::Command::new(&py);
            c.args(["-I", "-c", "import sys, duckdb, pandas, pyarrow, openpyxl; print(sys.version.split()[0], duckdb.__version__, pandas.__version__)"]);
            #[cfg(windows)] { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); }
            match c.output() {
                Ok(o) if o.status.success() => add("python", Some(true), format!("Python {}", String::from_utf8_lossy(&o.stdout).trim().replacen(' ', " · duckdb ", 1).replacen(' ', " · pandas ", 2))),
                Ok(o) => add("python", Some(false), crate::text::prefix(String::from_utf8_lossy(&o.stderr).trim(), 300)),
                Err(e) => add("python", Some(false), e.to_string()),
            }
        }
        // session kernel (only when a session is given and open)
        if let Ok(sh) = self.shared(p) {
            let k = &sh.kernel.process;
            if k.is_running() {
                let t0 = Instant::now();
                match sh.kernel.call("ping", json!({}), 10) { Ok(_) => add("kernel", Some(true), format!("运行中，响应 {} ms", t0.elapsed().as_millis())), Err(e) => add("kernel", Some(false), e) }
            } else { add("kernel", None, "未启动（发消息时自动启动）".into()); }
        }
        // main model: configured, key, reachable
        let cfg = self.core.config.lock().unwrap().clone();
        match cfg.roles.main.clone() {
            None => add("model", Some(false), "还没有设置主对话模型".into()),
            Some(m) => match cfg.providers.iter().find(|x| x.id == m.provider_id).cloned() {
                None => add("model", Some(false), format!("{}：找不到模型服务", m.model)),
                Some(prov) => {
                    let key = self.core.secrets.api_key(&prov.id);
                    add("key", Some(key.is_some()), if key.is_some() { format!("{}：已保存", prov.name) } else { format!("{}：钥匙串 / 凭据管理器里没有 Key", prov.name) });
                    let t0 = Instant::now();
                    match self.core.client_for_provider(prov.clone(), key).and_then(|c| c.list_models().map_err(|e| e.to_string())) {
                        Ok(ids) => add("model", Some(ids.is_empty() || ids.iter().any(|i| i == &m.model)),
                                       format!("{} · {} ms · 服务端 {} 个模型{}", m.model, t0.elapsed().as_millis(), ids.len(),
                                               if ids.is_empty() || ids.iter().any(|i| i == &m.model) { "" } else { "（列表里没有当前模型）" })),
                        Err(e) => add("model", Some(false), format!("{}：{}", m.model, crate::text::prefix(&e, 300))),
                    }
                }
            },
        }
        // proxy / network
        let st = &cfg.settings;
        let probe = "https://www.bing.com";
        match crate::llm::http_agent(probe, st, 8) {
            Ok((agent, proxy)) => {
                let t0 = Instant::now();
                let r = agent.head(probe).call();
                let via = proxy.map(|x| format!("，经代理 {x}")).unwrap_or_else(|| "，直连".into());
                match r {
                    Ok(_) | Err(ureq::Error::Status(_, _)) => add("network", Some(true), format!("代理设置：{}{via} · {} ms", st.proxy_mode, t0.elapsed().as_millis())),
                    Err(e) => add("network", None, format!("代理设置：{}{via} · 无法访问外网：{}", st.proxy_mode, crate::text::prefix(&e.to_string(), 200))),
                }
            }
            Err(e) => add("network", Some(false), e),
        }
        // Chrome (web search / browser)
        match crate::chrome::find(&paths.app_support) {
            Some(exe) => add("chrome", Some(true), format!("{}（版本 {}）", exe.display(), crate::chrome::major_version(&exe))),
            None => add("chrome", None, "未找到 Chrome / Edge：联网搜索和浏览器工具不可用（设置 → 通用 可下载）".into()),
        }
        // data folder writable + free space
        let probe_file = paths.app_support.join(".doctor-write-test");
        let writable = std::fs::write(&probe_file, b"ok").is_ok();
        let _ = std::fs::remove_file(&probe_file);
        add("storage", Some(writable), format!("{}{}", paths.app_support.display(), if writable { "" } else { "（不可写）" }));
        if let Some(free) = crate::platform::free_space(&paths.app_support) {
            let gb = free as f64 / 1e9;
            add("disk", Some(gb >= 2.0).filter(|&b| b).or(if gb < 0.5 { Some(false) } else { None }), format!("剩余 {gb:.1} GB"));
        }
        // MCP servers
        let ms = crate::mcp::status();
        if !ms.is_empty() {
            let bad: Vec<String> = ms.iter().filter(|(_, v)| !v.starts_with("ok") && !v.contains("connected") && !v.contains("已连接")).map(|(k, v)| format!("{k}: {v}")).collect();
            add("mcp", if bad.is_empty() { Some(true) } else { None }, if bad.is_empty() { format!("{} 个服务器正常", ms.len()) } else { bad.join("；") });
        }
        add("core", Some(true), format!("datapilot-core {} · {} · {}", crate::VERSION, std::env::consts::OS, std::env::consts::ARCH));
        json!({"items": items})
    }

    /// Capability test (port of the Swift settings "Test" button): streaming + native tool call, reasoning field,
    /// image input. Writes toolCallMode / supportsVision back into the role's capabilities in config.json.
    fn probe(self: &Arc<Self>, role: &str, cfg_path: &Path) -> Result<Value, String> {
        let raw = read_raw(cfg_path);
        let r = raw["roles"][role].clone();
        if !r.is_object() { return Err("这个角色还没有选择模型 / no model for this role".into()); }
        let m: ModelRef = serde_json::from_value(r.clone()).map_err(|e| format!("bad model: {e}"))?;
        let prov = self.core.config.lock().unwrap().providers.iter().find(|x| x.id == m.provider_id).cloned().ok_or("找不到模型服务 / provider not found")?;
        if self.settings().local_only && !is_local_model(&prov.id, &m.model, &prov.base_url, &raw["settings"]["localMarks"]) {
            return Err("本地模式下不能调用非本地模型 / local-only mode: this model is not local".into());
        }
        let c = self.core.client_for_provider(prov.clone(), self.core.secrets.api_key(&prov.id))?;
        let never = AtomicBool::new(false);
        let tool = ToolSpec { name: "get_weather".into(), description: "Get weather of a city".into(),
                              parameters: json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}) };
        let mut tm = m.clone();
        let mut caps = tm.caps(); caps.tool_call_mode = "native".into(); caps.max_output_tokens = 300; tm.capabilities = Some(caps);
        let t0 = Instant::now();
        let res = c.stream(&tm, &[ChatMessage::user("What's the weather in Beijing? Use the tool.")], &[tool], &never, &mut |_| {}).map_err(|e| e.to_string())?;
        let ms = t0.elapsed().as_millis() as u64;
        let native = res.tool_calls.first().map(|t| t.name == "get_weather").unwrap_or(false);
        let mut vm = m.clone();
        let mut vc = vm.caps(); vc.supports_vision = true; vc.max_output_tokens = 50; vc.thinking_enabled = false; vm.capabilities = Some(vc);
        let mut msg = ChatMessage::user("What two colors are in this image? Answer with two English color words only.");
        msg.images = Some(vec![ImagePart { data_url: format!("data:image/png;base64,{PROBE_PNG}") }]);
        let vision = c.stream(&vm, &[msg], &[], &never, &mut |_| {}).map(|r| { let l = r.content.to_lowercase(); l.contains("red") && l.contains("blue") }).unwrap_or(false);
        let mut base = serde_json::to_value(m.caps()).unwrap();
        base["toolCallMode"] = json!(if native { "native" } else { "hermes" });
        base["supportsVision"] = json!(vision);
        // context length reported by the server's /models (vLLM, LM Studio, OpenRouter…); New API gateways report none
        let reported = c.list_models_meta().ok().and_then(|l| l.into_iter().find(|x| x.0 == m.model).and_then(|x| x.1));
        if let Some(n) = reported { base["contextWindow"] = json!(n); base["contextSource"] = json!("server"); }
        patch_config(cfg_path, &json!({"roles": {role: {"capabilities": base}}}))?;
        Ok(json!({"ms": ms, "native_tools": native, "reasoning": !res.reasoning.is_empty(), "vision": vision, "context_window": reported,
                  "usage": res.usage.map(|u| json!({"prompt": u.prompt_tokens, "completion": u.completion_tokens})), "reload": self.reload()}))
    }
}

/// Computer Use permissions (macOS AX / screen recording); other platforms report `supported: false`.
#[cfg(target_os = "macos")]
fn computer_rpc(method: &str, p: &Value) -> Result<Value, String> {
    use crate::tools::computer as cu;
    if method == "computer/permissions" {
        return Ok(json!({"supported": true, "accessibility": cu::has_accessibility(), "screen_recording": cu::has_screen_recording()}));
    }
    Ok(json!({"granted": match s(p, "kind")? {
        "accessibility" => cu::request_accessibility(),
        "screen_recording" => cu::request_screen_recording(),
        k => return Err(format!("unknown kind {k}")),
    }}))
}
#[cfg(not(target_os = "macos"))]
fn computer_rpc(method: &str, _p: &Value) -> Result<Value, String> {
    if method == "computer/permissions" { Ok(json!({"supported": false, "accessibility": false, "screen_recording": false})) }
    else { Err("Computer Use 目前只支持 macOS / macOS only".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_patch_local() {
        let mut v = json!({"providers": [{"id": "a", "baseURL": "x"}], "settings": {"proxyMode": "system", "keep": 1}, "unknown": {"x": 1}});
        merge(&mut v, &json!({"settings": {"proxyMode": "none", "keep": null}, "roles": {"main": {"providerId": "a", "model": "m"}}}));
        assert_eq!(v["settings"], json!({"proxyMode": "none"}));
        assert_eq!(v["unknown"]["x"], 1);
        assert_eq!(v["roles"]["main"]["model"], "m");
        let d = std::env::temp_dir().join(format!("dp-cfg-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("config.json");
        std::fs::write(&p, v.to_string()).unwrap();
        assert!(patch_config(&p, &json!({"settings": {"maxTurns": "bad"}})).is_err());   // still a valid Config, or refused
        assert_eq!(read_raw(&p)["settings"]["proxyMode"], "none");
        patch_config(&p, &json!({"settings": {"maxTurns": 30}})).unwrap();
        assert_eq!(read_raw(&p)["unknown"]["x"], 1);
        let _ = std::fs::remove_dir_all(&d);
        let marks = json!({"p|forced": false});
        assert!(is_local_model("p", "qwen", "http://192.168.1.5:8000/v1", &json!({})));
        assert!(is_local_model("p", "qwen", "http://localhost:11434", &json!({})));
        assert!(!is_local_model("p", "qwen", "https://api.example.com/v1", &json!({})));
        assert!(!is_local_model("p", "forced", "http://localhost", &marks));
        assert!(safe_id("db_s_1").is_ok() && safe_id("../x").is_err());
    }
}

#[cfg(test)]
mod schema_tests {
    /// protocol/rpc.schema.json lists exactly the methods the server dispatches (UIs generate their types from it).
    #[test]
    fn rpc_schema_matches_dispatch() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let schema: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("protocol/rpc.schema.json")).unwrap()).unwrap();
        let listed: std::collections::BTreeSet<String> = schema["$defs"]["Method"]["enum"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let src = std::fs::read_to_string(root.join("datapilot-core/src/server.rs")).unwrap();
        let arm = regex::Regex::new(r#"(?m)^\s{12}((?:"[a-z_/]+"\s*\|\s*)*"[a-z_/]+")\s*=>"#).unwrap();
        let name = regex::Regex::new(r#""([a-z_/]+)""#).unwrap();
        let mut found: std::collections::BTreeSet<String> = super::METHODS.iter().map(|s| s.to_string()).collect();
        for c in arm.captures_iter(&src) { for n in name.captures_iter(&c[1]) { found.insert(n[1].to_string()); } }
        found.insert("shutdown".into());
        assert_eq!(listed, found, "update protocol/rpc.schema.json");
    }
}
