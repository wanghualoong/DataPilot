//! UI data layer (M4 phase A): everything a UI needs that is not the agent loop — session / project index and
//! full-text search, transcript → display items, chat export, dashboards, memory topics, skills, file lists.
//! Shares `AppSupport/app.sqlite` with the SwiftUI app (same tables and migrations as Swift `AppDatabase`), so both
//! UIs can run against the same data during the transition.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::types::{Role, ToolCall};

pub struct UiDb { db: Mutex<Connection>, pub app_support: PathBuf }

/// Same steps as Swift `AppDatabase.migrate` (user_version 1 → 3) so either app can open a database the other created.
fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=3000;")?;
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if v < 1 {
        c.execute_batch("
        CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, title TEXT, created REAL, updated REAL, model TEXT,
            mode TEXT, archived INTEGER DEFAULT 0, pinned INTEGER DEFAULT 0, summary TEXT, dataset_fps TEXT);
        CREATE TABLE IF NOT EXISTS messages_fts_src(id TEXT PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, ts REAL);
        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(content, session_id UNINDEXED, msg_id UNINDEXED, tokenize='trigram');
        CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY AUTOINCREMENT, ts REAL, session_id TEXT, dataset_fp TEXT,
            kind TEXT, tool TEXT, args_digest TEXT, result_digest TEXT, ok INTEGER, ms INTEGER, feedback TEXT);
        CREATE INDEX IF NOT EXISTS events_session ON events(session_id);
        CREATE INDEX IF NOT EXISTS events_fp ON events(dataset_fp);
        CREATE TABLE IF NOT EXISTS episodes(id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT UNIQUE, ts REAL, title TEXT,
            summary TEXT, datasets TEXT, artifacts TEXT);
        CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts USING fts5(title, summary, episode_id UNINDEXED, tokenize='trigram');
        CREATE TABLE IF NOT EXISTS memories(id INTEGER PRIMARY KEY AUTOINCREMENT, scope TEXT, scope_key TEXT, kind TEXT,
            content TEXT, evidence_count INTEGER DEFAULT 1, confidence REAL DEFAULT 0.5, status TEXT DEFAULT 'pending',
            source TEXT, source_session TEXT, created REAL, updated REAL, last_used REAL);
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(content, memory_id UNINDEXED, tokenize='trigram');
        CREATE TABLE IF NOT EXISTS memory_history(id INTEGER PRIMARY KEY AUTOINCREMENT, memory_id INTEGER, ts REAL,
            old_content TEXT, new_content TEXT, reason TEXT);
        CREATE TABLE IF NOT EXISTS qsql_pairs(id INTEGER PRIMARY KEY AUTOINCREMENT, dataset_fp TEXT, question TEXT, sql TEXT,
            verified INTEGER DEFAULT 0, uses INTEGER DEFAULT 0, created REAL, last_used REAL);
        CREATE VIRTUAL TABLE IF NOT EXISTS qsql_fts USING fts5(question, sql, pair_id UNINDEXED, tokenize='trigram');
        CREATE TABLE IF NOT EXISTS usage_stats(key TEXT, value TEXT, count INTEGER DEFAULT 0, last_seen REAL, PRIMARY KEY(key, value));
        CREATE TABLE IF NOT EXISTS datasets(fp TEXT PRIMARY KEY, name TEXT, columns_json TEXT, catalog_json TEXT,
            relations_json TEXT, first_seen REAL, last_seen REAL, session_ids TEXT);
        CREATE TABLE IF NOT EXISTS vectors(kind TEXT, ref_id TEXT, model TEXT, dim INTEGER, vec BLOB, text TEXT, PRIMARY KEY(kind, ref_id, model));
        CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT);
        PRAGMA user_version = 1;")?;
    }
    if v < 2 {
        c.execute_batch("CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, name TEXT, path TEXT, created REAL, updated REAL);
                         ALTER TABLE sessions ADD COLUMN project_id TEXT; PRAGMA user_version = 2;")?;
    }
    if v < 3 {
        let has: bool = c.prepare("PRAGMA table_info(memories)")?.query_map([], |r| r.get::<_, String>(1))?.flatten().any(|n| n == "usage_count");
        if !has { c.execute_batch("ALTER TABLE memories ADD COLUMN usage_count INTEGER DEFAULT 0;")?; }
        c.execute_batch("DROP TABLE IF EXISTS vectors; PRAGMA user_version = 3;")?;
    }
    Ok(())
}

fn now() -> f64 { crate::text::now() }
fn opt_s(v: Option<String>) -> Value { v.map(Value::from).unwrap_or(Value::Null) }

impl UiDb {
    pub fn open(app_support: &Path) -> Result<UiDb, String> {
        std::fs::create_dir_all(app_support).map_err(|e| e.to_string())?;
        let c = Connection::open(app_support.join("app.sqlite")).map_err(|e| format!("app.sqlite: {e}"))?;
        migrate(&c).map_err(|e| format!("app.sqlite migrate: {e}"))?;
        Ok(UiDb { db: Mutex::new(c), app_support: app_support.to_path_buf() })
    }

    // ---------- projects ----------
    pub fn list_projects(&self) -> Vec<Value> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT id, name, path FROM projects ORDER BY updated DESC").unwrap();
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))).unwrap().flatten()
            .map(|(id, name, path)| {
                let p = path.unwrap_or_default();
                json!({"id": id, "name": name.unwrap_or_default(), "path": p, "exists": Path::new(&p).join(".datapilot").exists()})
            }).collect()
    }
    fn project_path(&self, pid: &str) -> Option<PathBuf> {
        self.db.lock().unwrap().query_row("SELECT path FROM projects WHERE id=?1", [pid], |r| r.get::<_, String>(0)).optional().ok().flatten().map(PathBuf::from)
    }
    fn project_id_for_path(&self, root: &Path) -> Option<String> {
        let want = root.to_string_lossy().trim_end_matches('/').to_string();
        self.db.lock().unwrap().query_row("SELECT id FROM projects WHERE rtrim(path,'/')=?1", [want], |r| r.get(0)).optional().ok().flatten()
    }

    /// Open (or create) a project in `folder` — same layout as Swift `SessionStore.openProject`; adopts sessions on disk.
    pub fn open_project(&self, folder: &Path) -> Result<Value, String> {
        if !folder.is_dir() { return Err(format!("文件夹不存在：{}", folder.display())); }
        let state = folder.join(".datapilot");
        let pj = state.join("project.json");
        let mut id = format!("p_{}", crate::text::short_id(8));
        if let Some(old) = std::fs::read_to_string(&pj).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["id"].as_str().map(String::from)) { id = old; }
        if let Some(existing) = self.project_id_for_path(folder) { id = existing; }
        let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for d in [state.clone(), state.join("data"), state.join("sessions"), folder.join("outputs")] { std::fs::create_dir_all(&d).map_err(|e| e.to_string())?; }
        std::fs::write(&pj, serde_json::to_string_pretty(&json!({"id": id, "name": name, "app": "DataPilot", "version": 1})).unwrap()).map_err(|e| e.to_string())?;
        let gi = state.join(".gitignore");
        if !gi.exists() { let _ = std::fs::write(&gi, "# DataPilot local state (database, transcripts)\n*\n"); }
        let t = now();
        let db = self.db.lock().unwrap();
        db.execute("INSERT INTO projects(id, name, path, created, updated) VALUES(?1,?2,?3,?4,?4)
                    ON CONFLICT(id) DO UPDATE SET path=excluded.path, name=excluded.name, updated=excluded.updated",
                   params![id, name, folder.to_string_lossy(), t]).map_err(|e| e.to_string())?;
        for e in std::fs::read_dir(state.join("sessions")).into_iter().flatten().flatten() {
            let sub = e.path();
            if !sub.join("transcript.jsonl").exists() { continue; }
            let sid = e.file_name().to_string_lossy().to_string();
            let meta: Value = std::fs::read_to_string(sub.join("session.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
            let created = meta["created"].as_f64().unwrap_or(t);
            let _ = db.execute("INSERT INTO sessions(id, title, created, updated, mode, project_id) VALUES(?1,?2,?3,?3,'ask',?4)
                                ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id",
                               params![sid, meta["title"].as_str().unwrap_or(&sid), created, id]);
        }
        Ok(json!({"id": id, "name": name, "path": folder, "exists": true}))
    }

    pub fn relink_project(&self, pid: &str, folder: &Path) -> Result<Value, String> {
        let name = folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let _ = self.db.lock().unwrap().execute("UPDATE projects SET path=?1, name=?2 WHERE id=?3", params![folder.to_string_lossy(), name, pid]);
        self.open_project(folder)
    }

    /// Forget a project in the index (its folder is kept). Returns the project root it had.
    pub fn remove_project(&self, pid: &str) -> Option<PathBuf> {
        let root = self.project_path(pid);
        let db = self.db.lock().unwrap();
        let _ = db.execute("DELETE FROM projects WHERE id=?1", [pid]);
        let _ = db.execute("DELETE FROM messages_fts WHERE session_id IN (SELECT id FROM sessions WHERE project_id=?1)", [pid]);
        let _ = db.execute("DELETE FROM sessions WHERE project_id=?1", [pid]);
        root
    }

    /// What "delete project data" moves to the trash: only what DataPilot generated (never the user's own files).
    pub fn generated_items(root: &Path) -> Vec<PathBuf> {
        [root.join(".datapilot"), root.join("outputs"), root.join("DATAPILOT.md")].into_iter().filter(|p| p.exists()).collect()
    }

    // ---------- sessions ----------
    pub fn list_sessions(&self, include_archived: bool) -> Vec<Value> {
        let db = self.db.lock().unwrap();
        let sql = format!("SELECT id, title, created, updated, pinned, archived, project_id FROM sessions {} ORDER BY pinned DESC, updated DESC",
                          if include_archived { "" } else { "WHERE archived=0" });
        let mut st = db.prepare(&sql).unwrap();
        st.query_map([], |r| Ok(json!({"id": r.get::<_, String>(0)?, "title": r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            "created": r.get::<_, Option<f64>>(2)?.unwrap_or(0.0), "updated": r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
            "pinned": r.get::<_, Option<i64>>(4)?.unwrap_or(0) == 1, "archived": r.get::<_, Option<i64>>(5)?.unwrap_or(0) == 1,
            "project_id": opt_s(r.get(6)?)}))).unwrap().flatten().collect()
    }
    pub fn session_project(&self, id: &str) -> Option<(String, PathBuf)> {
        let pid: Option<String> = self.db.lock().unwrap().query_row("SELECT project_id FROM sessions WHERE id=?1", [id], |r| r.get(0)).optional().ok().flatten().flatten();
        let pid = pid?;
        self.project_path(&pid).map(|p| (pid, p))
    }
    pub fn session_dir(&self, id: &str) -> PathBuf {
        match self.session_project(id) {
            Some((_, root)) => root.join(".datapilot/sessions").join(id),
            None => self.app_support.join("sessions").join(id),
        }
    }
    pub fn transcript(&self, id: &str) -> PathBuf { self.session_dir(id).join("transcript.jsonl") }
    pub fn workspace(&self, id: &str) -> PathBuf {
        match self.session_project(id) { Some((_, root)) => root.join(".datapilot/data"), None => self.session_dir(id).join("workspace") }
    }

    /// New session (same files as Swift `SessionStore.create`). Open it in the core afterwards with `session/open {id}`.
    pub fn create_session(&self, title: &str, project_id: Option<&str>) -> Result<Value, String> {
        let id = crate::text::session_id();
        let t = now();
        self.db.lock().unwrap().execute("INSERT INTO sessions(id, title, created, updated, mode, project_id) VALUES(?1,?2,?3,?3,'ask',?4)",
                                        params![id, title, t, project_id]).map_err(|e| e.to_string())?;
        let dir = self.session_dir(&id);
        std::fs::create_dir_all(self.workspace(&id).join("uploads")).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let rec = json!({"type": "session_meta", "ts": t, "payload": {"id": id, "created": t, "app": "DataPilot", "project": project_id}});
        std::fs::write(dir.join("transcript.jsonl"), rec.to_string() + "\n").map_err(|e| e.to_string())?;
        self.write_session_file(&id, title, t);
        let root = project_id.and_then(|p| self.project_path(p));
        Ok(json!({"id": id, "title": title, "created": t, "updated": t, "pinned": false, "archived": false, "project_id": project_id, "project_root": root}))
    }
    fn write_session_file(&self, id: &str, title: &str, created: f64) {
        if self.session_project(id).is_none() { return; }
        let _ = std::fs::write(self.session_dir(id).join("session.json"), serde_json::to_string_pretty(&json!({"id": id, "title": title, "created": created})).unwrap());
    }
    /// The core opened a session (maybe created by `Core::open` without the UI): make sure the index has it.
    pub fn ensure_session(&self, id: &str, project_root: Option<&str>) {
        let pid = project_root.and_then(|r| self.project_id_for_path(Path::new(r)).or_else(|| self.open_project(Path::new(r)).ok().and_then(|v| v["id"].as_str().map(String::from))));
        let t = now();
        let _ = self.db.lock().unwrap().execute("INSERT OR IGNORE INTO sessions(id, title, created, updated, mode, project_id) VALUES(?1,'新会话',?2,?2,'ask',?3)", params![id, t, pid]);
    }
    pub fn rename(&self, id: &str, title: &str) {
        let created: f64 = {
            let db = self.db.lock().unwrap();
            let _ = db.execute("UPDATE sessions SET title=?1 WHERE id=?2", params![title, id]);
            db.query_row("SELECT created FROM sessions WHERE id=?1", [id], |r| r.get::<_, Option<f64>>(0)).optional().ok().flatten().flatten().unwrap_or_else(now)
        };
        self.write_session_file(id, title, created);
    }
    pub fn title(&self, id: &str) -> Option<String> {
        self.db.lock().unwrap().query_row("SELECT title FROM sessions WHERE id=?1", [id], |r| r.get::<_, Option<String>>(0)).optional().ok().flatten().flatten()
    }
    pub fn exists(&self, id: &str) -> bool {
        self.db.lock().unwrap().query_row("SELECT 1 FROM sessions WHERE id=?1", [id], |_| Ok(())).optional().ok().flatten().is_some()
    }
    pub fn touch(&self, id: &str) { let _ = self.db.lock().unwrap().execute("UPDATE sessions SET updated=?1 WHERE id=?2", params![now(), id]); }
    pub fn set_flag(&self, id: &str, col: &str, on: bool) {
        if !["pinned", "archived"].contains(&col) { return; }
        let _ = self.db.lock().unwrap().execute(&format!("UPDATE sessions SET {col}=?1 WHERE id=?2"), params![on as i64, id]);
    }
    /// Removes the index rows and the session folder (project sessions: only that session's folder; shared data stays).
    pub fn delete_session(&self, id: &str) {
        let dir = self.session_dir(id);
        { let db = self.db.lock().unwrap();
          let _ = db.execute("DELETE FROM sessions WHERE id=?1", [id]);
          let _ = db.execute("DELETE FROM messages_fts WHERE session_id=?1", [id]); }
        if dir.file_name().map(|n| n == id).unwrap_or(false) { let _ = std::fs::remove_dir_all(dir); }
    }

    // ---------- search ----------
    pub fn index_message(&self, session: &str, msg_id: &str, content: &str) {
        if content.trim().is_empty() { return; }
        let db = self.db.lock().unwrap();
        let seen: bool = db.query_row("SELECT 1 FROM messages_fts WHERE msg_id=?1 LIMIT 1", [msg_id], |_| Ok(())).optional().ok().flatten().is_some();
        if !seen { let _ = db.execute("INSERT INTO messages_fts(content, session_id, msg_id) VALUES(?1,?2,?3)", params![crate::text::prefix(content, 4000), session, msg_id]); }
    }
    /// Sessions whose messages (FTS5 trigram) or title match `q`: [{session_id, snippet}].
    pub fn search(&self, q: &str, limit: usize) -> Vec<Value> {
        let q = q.trim();
        if q.is_empty() { return vec![]; }
        let db = self.db.lock().unwrap();
        let mut out: Vec<Value> = vec![];
        let mut seen = std::collections::HashSet::new();
        if let Some(fq) = crate::index::fts_query(q) {
            if let Ok(mut st) = db.prepare("SELECT session_id, snippet(messages_fts, 0, '[', ']', '…', 12) FROM messages_fts WHERE messages_fts MATCH ?1 ORDER BY rank LIMIT ?2") {
                for (s, snip) in st.query_map(params![fq, limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).into_iter().flatten().flatten() {
                    if seen.insert(s.clone()) { out.push(json!({"session_id": s, "snippet": snip})); }
                }
            }
        }
        let like = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
        if let Ok(mut st) = db.prepare("SELECT id, title FROM sessions WHERE title LIKE ?1 ESCAPE '\\'") {
            for (s, t) in st.query_map([like], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))).into_iter().flatten().flatten() {
                if seen.insert(s.clone()) { out.push(json!({"session_id": s, "snippet": t.unwrap_or_default()})); }
            }
        }
        out
    }
}

/// `/fork`: copy a chat's transcript (rewriting the session id), its notes and — for chats outside a project — its
/// workspace into the freshly created session `new_id`. Snapshots are not copied (the fork starts its own history).
pub fn fork_files(src_transcript: &Path, dst_transcript: &Path, new_id: &str, workspace: Option<(PathBuf, PathBuf)>) -> Result<(), String> {
    let text = std::fs::read_to_string(src_transcript).map_err(|e| format!("transcript: {e}"))?;
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() { continue; }
        if i == 0 {
            if let Ok(mut v) = serde_json::from_str::<Value>(line) {
                if v["type"] == "session_meta" { v["payload"]["id"] = json!(new_id); v["payload"]["forkedFrom"] = json!(src_transcript.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string())); out += &(v.to_string() + "\n"); continue; }
            }
        }
        out += line; out.push('\n');
    }
    std::fs::write(dst_transcript, out).map_err(|e| e.to_string())?;
    if let (Some(a), Some(b)) = (src_transcript.parent(), dst_transcript.parent()) {
        if a.join("notes.md").is_file() { let _ = std::fs::copy(a.join("notes.md"), b.join("notes.md")); }
    }
    if let Some((src, dst)) = workspace {
        if src.is_dir() {
            let _ = std::fs::remove_dir_all(&dst);
            if !crate::platform::copy_tree(&src, &dst) { return Err("复制工作区失败 / could not copy the workspace".into()); }
        }
    }
    Ok(())
}

pub fn move_to_trash(p: &Path) -> Result<(), String> { crate::platform::move_to_trash(p) }

// ---------- transcript → display items (port of ChatViewModel.rebuild) ----------
pub fn tool_summary(c: &ToolCall) -> String {
    let Some(a) = c.parsed() else { return c.name.clone() };
    let one = |s: &str, n: usize| crate::text::prefix(&s.replace('\n', " "), n);
    if let Some(s) = a["sql"].as_str() { return format!("SQL: {}", one(s, 100)); }
    if let Some(s) = a["path"].as_str() { return format!("{} {s}", c.name); }
    if let Some(s) = a["code"].as_str() { return format!("python: {}", crate::text::prefix(s.lines().next().unwrap_or(""), 90)); }
    if let Some(s) = a["title"].as_str().or_else(|| a["spec"]["title"].as_str()) { return format!("{}: {s}", c.name); }
    c.name.clone()
}

pub fn history(transcript: &Path) -> Value {
    let l = crate::session::load_transcript(transcript);
    let mut out: Vec<Value> = vec![];
    let mut tool_at: HashMap<String, usize> = HashMap::new();
    for m in &l.ui {
        match m.role {
            Role::User => {
                if m.is_meta {
                    if let Some(s) = m.content.strip_prefix(crate::compact::SUMMARY_PREFIX) { out.push(json!({"id": m.id, "kind": "compacted", "text": s})); }
                    else if m.content.starts_with("<context>") { out.push(json!({"id": m.id, "kind": "context", "text": m.content})); }
                    continue;
                }
                out.push(json!({"id": m.id, "kind": "user", "text": m.content, "turn_id": m.turn_id}));
            }
            Role::Assistant => {
                let r = m.reasoning.clone().unwrap_or_default();
                if !m.content.is_empty() || !r.is_empty() { out.push(json!({"id": m.id, "kind": "assistant", "text": m.content, "reasoning": r})); }
                for c in m.tool_calls.iter().flatten() {
                    tool_at.insert(c.id.clone(), out.len());
                    out.push(json!({"id": c.id, "kind": "tool", "tool_name": c.name, "summary": tool_summary(c), "arguments": c.arguments, "output": "", "is_error": false}));
                }
            }
            Role::Tool => if let Some(i) = m.tool_call_id.as_ref().and_then(|id| tool_at.get(id)) {
                let o = &mut out[*i];
                o["output"] = json!(m.content);
                o["is_error"] = json!(m.content.starts_with("Error") || m.content.starts_with("Permission denied"));
                if let Some(d) = m.tool_call_id.as_ref().and_then(|id| l.displays.get(id)) { o["display"] = d.clone(); }
            },
            Role::System => {}
        }
    }
    json!({"items": out, "todos": l.todos, "compactions": l.compactions})
}

// ---------- chat export (port of ChatExporter) ----------
fn md_table(cols: &[String], rows: &[Value], max: usize) -> String {
    let cell = |v: &Value| match v {
        Value::Null => String::new(),
        Value::String(s) => s.replace('|', "\\|"),
        Value::Number(n) => crate::text::fmt_num(n.as_f64().unwrap_or(0.0)),
        o => o.to_string(),
    };
    let mut l = vec![format!("| {} |", cols.join(" | ")), format!("|{}|", cols.iter().map(|_| "---").collect::<Vec<_>>().join("|"))];
    for r in rows.iter().take(max) { l.push(format!("| {} |", r.as_array().map(|a| a.iter().map(cell).collect::<Vec<_>>().join(" | ")).unwrap_or_default())); }
    if rows.len() > max { l.push(format!("\n_… {} rows_", rows.len())); }
    l.join("\n")
}

pub fn export_markdown(title: &str, items: &[Value]) -> String {
    let (y, mo, d, h, mi, _, _) = crate::text::local_time(now());
    let mut out = vec![format!("# {title}"), String::new(), format!("_导出自 DataPilot · {y}-{mo:02}-{d:02} {h:02}:{mi:02}_"), String::new()];
    for it in items {
        let text = it["text"].as_str().unwrap_or("");
        match it["kind"].as_str().unwrap_or("") {
            "user" => out.extend(["## 🧑 用户".into(), String::new(), text.into(), String::new()]),
            "assistant" if !text.is_empty() => out.extend(["## 🤖 DataPilot".into(), String::new(), text.into(), String::new()]),
            "tool" => {
                let dsp = &it["display"];
                let s = it["summary"].as_str().filter(|s| !s.is_empty()).or(it["tool_name"].as_str()).unwrap_or("");
                out.push(format!("<details><summary>🔧 {s}{}</summary>\n", if it["is_error"] == true { " ❌" } else { "" }));
                if let Some(sql) = dsp["sql"].as_str() { out.extend(["```sql".into(), sql.into(), "```".into()]); }
                else if let Some(code) = dsp["code"].as_str() { out.extend(["```python".into(), code.into(), "```".into()]); }
                let cols = crate::tools::strs(&dsp["columns"]);
                let dcols = crate::tools::strs(&dsp["data"]["columns"]);
                if !cols.is_empty() && dsp["rows"].is_array() { out.push(md_table(&cols, dsp["rows"].as_array().unwrap(), 30)); }
                else if !dcols.is_empty() && dsp["data"]["rows"].is_array() { out.push(md_table(&dcols, dsp["data"]["rows"].as_array().unwrap(), 30)); }
                else if let Some(o) = it["output"].as_str().filter(|o| !o.is_empty()) { out.extend(["```".into(), crate::text::prefix(o, 3000), "```".into()]); }
                out.extend(["</details>".into(), String::new()]);
            }
            "notice" => out.extend([format!("> {}", text.replace('\n', "\n> ")), String::new()]),
            "error" => out.extend([format!("> ⚠️ {text}"), String::new()]),
            "compacted" => out.extend(["> （上下文已压缩）".into(), String::new()]),
            _ => {}
        }
    }
    out.join("\n")
}

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }
fn inline(s: &str) -> String {
    let t = esc(s);
    let b = regex::Regex::new(r"\*\*(.+?)\*\*").unwrap().replace_all(&t, "<b>$1</b>").to_string();
    regex::Regex::new(r"`([^`]+)`").unwrap().replace_all(&b, "<code>$1</code>").to_string()
}

/// Minimal Markdown → self-contained HTML (headings, tables, code, bullets, bold), like the Swift exporter.
pub fn export_html(title: &str, items: &[Value]) -> String {
    let md = export_markdown(title, items);
    let (mut html, mut in_code, mut in_table) = (vec![], false, false);
    for line in md.split('\n') {
        if line.starts_with("```") { html.push(if in_code { "</code></pre>".into() } else { "<pre><code>".into() }); in_code = !in_code; continue; }
        if in_code { html.push(esc(line)); continue; }
        if line.starts_with("<details") || line.starts_with("</details") { html.push(line.to_string()); continue; }
        if line.starts_with('|') {
            if line.starts_with("|---") { continue; }
            let cells: Vec<&str> = line.trim_matches(|c| c == '|' || c == ' ').split(" | ").collect();
            if !in_table { html.push("<table>".into()); in_table = true; html.push(format!("<tr>{}</tr>", cells.iter().map(|c| format!("<th>{}</th>", inline(c))).collect::<String>())); continue; }
            html.push(format!("<tr>{}</tr>", cells.iter().map(|c| format!("<td>{}</td>", inline(c))).collect::<String>())); continue;
        } else if in_table { html.push("</table>".into()); in_table = false; }
        html.push(if let Some(r) = line.strip_prefix("## ") { format!("<h2>{}</h2>", inline(r)) }
            else if let Some(r) = line.strip_prefix("# ") { format!("<h1>{}</h1>", inline(r)) }
            else if let Some(r) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) { format!("<li>{}</li>", inline(r)) }
            else if let Some(r) = line.strip_prefix("> ") { format!("<blockquote>{}</blockquote>", inline(r)) }
            else if line.is_empty() { String::new() } else { format!("<p>{}</p>", inline(line)) });
    }
    if in_table { html.push("</table>".into()); }
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>
body{{font:15px/1.6 -apple-system,system-ui,\"PingFang SC\",\"Microsoft YaHei\",sans-serif;max-width:900px;margin:32px auto;padding:0 20px;color:#1d1d1f}}
h2{{font-size:16px;margin-top:28px;border-top:1px solid #eee;padding-top:16px}}
pre{{background:#f5f5f7;padding:10px;border-radius:8px;overflow:auto;font-size:12.5px}}
table{{border-collapse:collapse;margin:8px 0;font-size:13px}}td,th{{border:1px solid #ddd;padding:4px 8px}}th{{background:#f5f5f7}}
blockquote{{color:#666;border-left:3px solid #ddd;margin:6px 0;padding-left:10px}}details{{margin:6px 0;color:#444}}
@media(prefers-color-scheme:dark){{body{{background:#1c1c1e;color:#eee}}pre,th{{background:#2c2c2e}}td,th{{border-color:#444}}}}
</style></head><body>
{}
</body></html>", esc(title), html.join("\n"))
}

// ---------- dashboards (port of Swift DashboardStore) ----------
pub const LEGACY_SESSION_DASHBOARD: &str = "db_session";
pub fn session_dashboard_id(session: &str, in_project: bool) -> String {
    if in_project { format!("db_s_{session}") } else { LEGACY_SESSION_DASHBOARD.into() }
}

fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn dashboard_entries(ws: &Path) -> Vec<Value> {
    let mut v: Vec<(f64, Value)> = std::fs::read_dir(ws.join("dashboards")).into_iter().flatten().flatten().filter_map(|e| {
        let p = e.path();
        if p.extension()? != "json" { return None; }
        let id = p.file_stem()?.to_string_lossy().to_string();
        let s = crate::tools::bi::load(&id, ws).unwrap_or(Value::Null);
        Some((mtime(&p), json!({"id": id, "title": s["title"].as_str().map(String::from).unwrap_or_else(|| id.clone()), "owner": s["owner"]})))
    }).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    v.into_iter().map(|x| x.1).collect()
}

/// Inline chart display (`kind: chart`) → dashboard widget + dataset key + dataset.
pub fn widget_from_chart(d: &Value, index: usize) -> Option<(Value, String, Value)> {
    let sql = d["sql"].as_str()?;
    let spec = d.get("spec").filter(|s| s.is_object())?;
    let key = format!("c{index}");
    let ty = spec["type"].as_str().unwrap_or("bar");
    let cols = crate::tools::strs(&d["data"]["columns"]);
    let mut ys = crate::tools::strs(&spec["y"]);
    if ys.is_empty() { if let Some(c) = cols.last() { ys = vec![c.clone()]; } }
    let mut w = Map::new();
    w.insert("id".into(), json!(format!("w{index}")));
    w.insert("type".into(), json!(ty));
    w.insert("title".into(), spec.get("title").cloned().unwrap_or(json!("")));
    w.insert("dataset".into(), json!(key));
    if let Some(x) = spec["x"].as_str() { w.insert("x".into(), json!(x)); }
    if let Some(s) = spec["series"].as_str() { w.insert("series".into(), json!(s)); }
    if ty == "kpi" { w.insert("measure".into(), json!({"field": ys.first().cloned().unwrap_or_default(), "agg": "sum"})); }
    else if ty != "table" { w.insert("y".into(), Value::Array(ys.iter().map(|y| json!({"field": y, "agg": "sum"})).collect())); }
    if let Some(f) = spec["options"].get("format") { w.insert("format".into(), f.clone()); }
    if let Some(o) = spec["options"].as_object().filter(|o| !o.is_empty()) { w.insert("options".into(), Value::Object(o.clone())); }
    if ty != "kpi" && ty != "table" {
        w.insert("typeOptions".into(), json!(["bar", "hbar", "line", "pie", "donut", "radar", "table"]));
        if ys.len() > 1 { w.insert("measureOptions".into(), json!(ys)); }
    }
    Some((Value::Object(w), key, json!({"sql": sql})))
}

/// Append a chart to the session dashboard (created on demand, de-duplicated by SQL + type). Returns its id.
pub fn pin_chart(ws: &Path, chart: &Value, session: &str, in_project: bool, title: &str) -> Result<String, String> {
    let id = session_dashboard_id(session, in_project);
    let mut spec = crate::tools::bi::load(&id, ws).and_then(|v| v.as_object().cloned())
        .unwrap_or_else(|| json!({"id": id, "version": 2, "title": title, "datasets": {}, "widgets": []}).as_object().unwrap().clone());
    if in_project && spec.get("owner").map(|o| o.is_null()).unwrap_or(true) { spec.insert("owner".into(), json!(session)); }
    let mut datasets = spec.get("datasets").and_then(|d| d.as_object().cloned()).unwrap_or_default();
    let mut widgets = spec.get("widgets").and_then(|w| w.as_array().cloned()).unwrap_or_default();
    let (sql, ty) = (chart["sql"].as_str().unwrap_or(""), chart["spec"]["type"].as_str().unwrap_or(""));
    if widgets.iter().any(|w| datasets.get(w["dataset"].as_str().unwrap_or("")).and_then(|d| d["sql"].as_str()) == Some(sql) && w["type"].as_str() == Some(ty)) {
        return Ok(id);
    }
    let mut i = widgets.len() + 1;
    while datasets.contains_key(&format!("c{i}")) { i += 1; }
    let (w, key, ds) = widget_from_chart(chart, i).ok_or("图表没有 SQL，不能加入看板 / chart has no SQL")?;
    datasets.insert(key, ds);
    widgets.push(w);
    spec.insert("layout".into(), crate::tools::bi::auto_layout(&widgets));
    spec.insert("datasets".into(), Value::Object(datasets));
    spec.insert("widgets".into(), Value::Array(widgets));
    crate::tools::bi::save(&Value::Object(spec), &id, ws)?;
    Ok(id)
}

/// Card edit from the dashboard renderer: merge `patch` into one widget (null removes a key).
pub fn patch_widget(ws: &Path, id: &str, widget: &str, patch: &Map<String, Value>) -> Result<bool, String> {
    let Some(mut spec) = crate::tools::bi::load(id, ws) else { return Ok(false) };
    let Some(ws_arr) = spec["widgets"].as_array_mut() else { return Ok(false) };
    let Some(w) = ws_arr.iter_mut().find(|w| w["id"].as_str() == Some(widget)).and_then(|w| w.as_object_mut()) else { return Ok(false) };
    for (k, v) in patch { if v.is_null() { w.remove(k); } else { w.insert(k.clone(), v.clone()); } }
    spec["version"] = json!(2);
    crate::tools::bi::save(&spec, id, ws)?;
    Ok(true)
}

// ---------- memory topics ----------
pub fn topics_json(dir: &Path) -> Value {
    let usage: Value = std::fs::read_to_string(dir.join(".usage.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    Value::Array(crate::memory::topics(dir, 500).into_iter().map(|t| json!({"file": t.file, "name": t.name, "description": t.description,
        "type": t.kind, "body": t.body, "modified": t.modified, "usage": usage.get(&t.file).cloned().unwrap_or(Value::Null)})).collect())
}

// ---------- files ----------
/// Files panel: project data files (relative) + the workspace's uploads / exports / artifacts / dashboards (absolute).
pub fn files(ws: &Path, project_root: Option<&Path>) -> Value {
    let mut wsf: Vec<String> = vec![];
    for sub in ["uploads", "exports", "artifacts", "dashboards"] {
        let mut v: Vec<String> = std::fs::read_dir(ws.join(sub)).into_iter().flatten().flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.')).map(|e| e.path().to_string_lossy().to_string()).collect();
        v.sort();
        wsf.extend(v);
    }
    for f in ["notes.md", "catalog.json"] { if ws.join(f).exists() { wsf.push(ws.join(f).to_string_lossy().to_string()); } }
    let project = project_root.map(|r| crate::prompt::files(r, 2000)).unwrap_or_default();
    let data: Vec<String> = project_root.map(|r| crate::prompt::data_files(r, 500)).unwrap_or_default();
    json!({"workspace_dir": ws, "workspace": wsf, "project_root": project_root, "project": project, "data_files": data})
}

// ---------- file import / export (port of ChatViewModel.importFiles / saveFile) ----------
/// Returns (paths as the model should see them, failures). Only regular files; symlinks resolved first.
pub fn import_files(ws: &Path, project_root: Option<&Path>, paths: &[PathBuf]) -> (Vec<String>, Vec<String>) {
    let up = ws.join("uploads");
    let _ = std::fs::create_dir_all(&up);
    let root = project_root.and_then(|r| crate::platform::canonicalize(r).ok());
    let (mut out, mut failed) = (vec![], vec![]);
    for p in paths {
        let Ok(real) = crate::platform::canonicalize(p) else { failed.push(p.display().to_string()); continue };
        if !real.is_file() { failed.push(p.display().to_string()); continue; }
        if let Some(rel) = root.as_ref().and_then(|r| real.strip_prefix(r).ok()) { out.push(rel.to_string_lossy().replace('\\', "/")); continue; }
        let name = real.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let mut dest = up.join(&name);
        if dest.exists() {
            let stem = real.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let ext = real.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            dest = up.join(format!("{stem}_{}{ext}", crate::text::short_id(4)));
        }
        match std::fs::copy(&real, &dest) {
            Ok(_) => out.push(format!("uploads/{}", dest.file_name().unwrap().to_string_lossy())),
            Err(_) => failed.push(p.display().to_string()),
        }
    }
    (out, failed)
}

const EXPORT_EXTS: &[&str] = &["csv", "html", "png", "md", "json", "txt", "svg"];

pub fn save_export(ws: &Path, project_root: Option<&Path>, name: &str, ext: &str, data: &[u8], bom: bool) -> Result<PathBuf, String> {
    let ext = ext.trim_start_matches('.').to_lowercase();
    if !EXPORT_EXTS.contains(&ext.as_str()) { return Err(format!("不支持的导出格式 .{ext}")); }
    let dir = project_root.map(|r| r.join("outputs")).unwrap_or_else(|| ws.join("exports"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let base: String = name.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let base = base.trim().trim_start_matches('.');
    let base = if base.is_empty() { "export" } else { base };
    let base = crate::text::prefix(base, 80);
    let mut u = dir.join(format!("{base}.{ext}"));
    let mut i = 2;
    while u.exists() { u = dir.join(format!("{base}_{i}.{ext}")); i += 1; }
    let mut bytes = if bom { vec![0xEF, 0xBB, 0xBF] } else { vec![] };
    bytes.extend_from_slice(data);
    std::fs::write(&u, bytes).map_err(|e| e.to_string())?;
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dp-ui-{n}-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sessions_projects_search() {
        let sup = tmp("sup");
        let ui = UiDb::open(&sup).unwrap();
        let s = ui.create_session("新会话", None).unwrap();
        let id = s["id"].as_str().unwrap().to_string();
        assert!(ui.transcript(&id).exists() && ui.workspace(&id).join("uploads").is_dir());
        ui.rename(&id, "销售分析");
        ui.index_message(&id, "m1", "华东大区的销售额最高");
        ui.index_message(&id, "m1", "华东大区的销售额最高");   // de-duplicated
        assert_eq!(ui.search("华东大区", 10).len(), 1);
        assert_eq!(ui.search("销售分析", 10)[0]["session_id"], id.as_str());   // title match
        ui.set_flag(&id, "pinned", true);
        assert_eq!(ui.list_sessions(false)[0]["pinned"], true);
        // project: layout + adopted session + session.json
        let proj = tmp("proj");
        let p = ui.open_project(&proj).unwrap();
        let pid = p["id"].as_str().unwrap().to_string();
        assert!(proj.join(".datapilot/project.json").exists() && proj.join("outputs").is_dir());
        let ps = ui.create_session("项目会话", Some(&pid)).unwrap();
        let psid = ps["id"].as_str().unwrap().to_string();
        assert_eq!(ui.workspace(&psid), proj.join(".datapilot/data"));
        assert!(proj.join(".datapilot/sessions").join(&psid).join("session.json").exists());
        // reopening the same folder keeps the id; removing forgets sessions but keeps files; reopening adopts them
        assert_eq!(ui.open_project(&proj).unwrap()["id"], pid.as_str());
        ui.remove_project(&pid);
        assert!(!ui.list_sessions(true).iter().any(|s| s["id"] == psid.as_str()));
        ui.open_project(&proj).unwrap();
        assert!(ui.list_sessions(true).iter().any(|s| s["id"] == psid.as_str() && s["title"] == "项目会话"));
        ui.delete_session(&id);
        assert!(!ui.session_dir(&id).exists());
        // a second open of the same database (other UI) sees the same rows
        let ui2 = UiDb::open(&sup).unwrap();
        assert_eq!(ui2.list_projects().len(), 1);
        let _ = std::fs::remove_dir_all(&sup); let _ = std::fs::remove_dir_all(&proj);
    }

    #[test]
    fn import_and_save() {
        let d = tmp("imp");
        let ws = d.join("ws"); let proj = d.join("proj"); let outside = d.join("out");
        for x in [&ws, &proj, &outside] { std::fs::create_dir_all(x).unwrap(); }
        std::fs::write(proj.join("a.csv"), "x\n1\n").unwrap();
        std::fs::write(outside.join("b.csv"), "y\n2\n").unwrap();
        let (f, bad) = import_files(&ws, Some(&proj), &[proj.join("a.csv"), outside.join("b.csv"), outside.join("b.csv"), outside.clone(), d.join("nope")]);
        assert_eq!(f[0], "a.csv");
        assert_eq!(f[1], "uploads/b.csv");
        assert!(f[2].starts_with("uploads/b_") && ws.join(&f[2]).exists());
        assert_eq!(bad.len(), 2);
        let p1 = save_export(&ws, None, "../销售:汇总", "csv", b"a,b", true).unwrap();
        let p2 = save_export(&ws, None, "../销售:汇总", "csv", b"a,b", true).unwrap();
        assert!(p1.starts_with(ws.join("exports")) && p1 != p2);
        assert_eq!(std::fs::read(&p1).unwrap()[..3], [0xEF, 0xBB, 0xBF]);
        assert!(save_export(&ws, Some(&proj), "x", "sh", b"", false).is_err());
        assert!(save_export(&ws, Some(&proj), "x", "png", b"1", false).unwrap().starts_with(proj.join("outputs")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn history_export_dashboard() {
        let d = tmp("hist");
        let t = d.join("transcript.jsonl");
        let lines = [
            json!({"type": "message", "message": {"id": "u1", "role": "user", "content": "按大区汇总", "turnId": "t1"}}),
            json!({"type": "message", "message": {"id": "a1", "role": "assistant", "content": "", "toolCalls": [{"id": "c1", "name": "sql_query", "arguments": "{\"sql\":\"SELECT region, sum(amount) FROM t GROUP BY 1\"}"}]}}),
            json!({"type": "message", "message": {"id": "r1", "role": "tool", "content": "| region |", "toolCallId": "c1", "name": "sql_query"}}),
            json!({"type": "display", "callId": "c1", "payload": {"kind": "sql", "sql": "SELECT 1", "columns": ["region", "v"], "rows": [["华东", 150]]}}),
            json!({"type": "message", "message": {"id": "a2", "role": "assistant", "content": "华东最高"}}),
        ];
        std::fs::write(&t, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
        let h = history(&t);
        let items = h["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["turn_id"], "t1");
        assert!(items[1]["summary"].as_str().unwrap().starts_with("SQL: SELECT region"));
        assert_eq!(items[1]["display"]["kind"], "sql");
        let md = export_markdown("测试", items);
        assert!(md.contains("| 华东 | 150 |") && md.contains("华东最高"));
        assert!(export_html("测试", items).contains("<td>华东</td>"));
        // pin twice → one widget
        let chart = json!({"kind": "chart", "sql": "SELECT region, v FROM t", "spec": {"type": "bar", "title": "x", "x": "region", "y": ["v"]}, "data": {"columns": ["region", "v"]}});
        let id = pin_chart(&d, &chart, "s1", true, "本会话看板").unwrap();
        assert_eq!(id, "db_s_s1");
        pin_chart(&d, &chart, "s1", true, "本会话看板").unwrap();
        let spec = crate::tools::bi::load(&id, &d).unwrap();
        assert_eq!(spec["widgets"].as_array().unwrap().len(), 1);
        assert_eq!(spec["owner"], "s1");
        let mut p = Map::new(); p.insert("type".into(), json!("line")); p.insert("x".into(), Value::Null);
        assert!(patch_widget(&d, &id, "w1", &p).unwrap());
        let spec = crate::tools::bi::load(&id, &d).unwrap();
        assert_eq!(spec["widgets"][0]["type"], "line");
        assert!(spec["widgets"][0].get("x").is_none());
        assert_eq!(dashboard_entries(&d)[0]["id"], id.as_str());
        let _ = std::fs::remove_dir_all(&d);
    }
}
