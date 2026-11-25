//! `AppSupport/index.sqlite` — the Rust core's own index (not shared with the Swift app; safe to delete, rebuilt from
//! files). Holds what files can't answer fast: session list + titles, full-text search over messages, verified
//! question → SQL examples per dataset fingerprint, and dataset fingerprints (to recognise a re-imported report).
//! Memory *content* stays in Markdown topic files (user / project / agent dirs).
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

pub struct Index { db: Mutex<Connection> }

const SCHEMA: &str = "
PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=3000;
CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, title TEXT, created REAL, updated REAL, project_root TEXT, archived INTEGER DEFAULT 0);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(content, session_id UNINDEXED, msg_id UNINDEXED, tokenize='trigram');
CREATE TABLE IF NOT EXISTS qsql(id INTEGER PRIMARY KEY AUTOINCREMENT, dataset_fp TEXT, question TEXT, sql TEXT, verified INTEGER DEFAULT 0,
    uses INTEGER DEFAULT 0, session_id TEXT, created REAL, last_used REAL, UNIQUE(dataset_fp, question));
CREATE VIRTUAL TABLE IF NOT EXISTS qsql_fts USING fts5(question, pair_id UNINDEXED, tokenize='trigram');
CREATE TABLE IF NOT EXISTS datasets(fp TEXT PRIMARY KEY, name TEXT, columns_json TEXT, first_seen REAL, last_seen REAL, sessions TEXT);
";

#[derive(Debug, Clone, PartialEq)]
pub struct Example { pub id: i64, pub question: String, pub sql: String, pub verified: bool }

/// FTS5 MATCH expression: quoted terms ≥ 3 chars; CJK runs split into 3-grams (trigram tokenizer needs ≥ 3 chars).
pub fn fts_query(text: &str) -> Option<String> {
    let mut terms: Vec<String> = vec![];
    for t in text.split(|c: char| c.is_whitespace() || (c.is_ascii_punctuation() && c != '_') || "，。、；：？！“”‘’（）【】《》—…".contains(c)) {
        let chars: Vec<char> = t.chars().collect();
        if chars.len() < 3 { continue; }
        let cjk = chars.iter().any(|c| ('\u{4e00}'..='\u{9fff}').contains(c));
        if cjk && chars.len() > 6 {
            let mut i = 0;
            while i + 3 <= chars.len() { terms.push(chars[i..i + 3].iter().collect()); i += 2; }
        } else { terms.push(t.to_string()); }
    }
    let mut seen = HashSet::new();
    terms.retain(|t| seen.insert(t.clone()));
    terms.truncate(24);
    if terms.is_empty() { return None; }
    Some(terms.iter().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect::<Vec<_>>().join(" OR "))
}

impl Index {
    /// Consistent copy of the index (SQLite online backup) at `dst`.
    pub fn snapshot_to(&self, dst: &Path) -> Result<(), String> {
        self.db.lock().unwrap().backup(rusqlite::DatabaseName::Main, dst, None).map_err(|e| format!("index.sqlite 快照失败：{e}"))
    }

    /// Replace the index with a backup copy (SQLite online restore).
    pub fn restore_from(&self, src: &Path) -> Result<(), String> {
        let mut db = self.db.lock().unwrap();
        db.restore(rusqlite::DatabaseName::Main, src, None::<fn(rusqlite::backup::Progress)>).map_err(|e| format!("index.sqlite 恢复失败：{e}"))?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())
    }

    /// Merge a backup copy, best effort (the index can be rebuilt): new chats with their search text, saved
    /// question → SQL pairs, datasets. Existing rows are kept.
    pub fn merge_from(&self, src: &Path) {
        let db = self.db.lock().unwrap();
        if db.execute("ATTACH DATABASE ?1 AS b", [src.to_string_lossy().to_string()]).is_err() { return; }
        let ids: Vec<String> = match db.prepare("SELECT id FROM b.sessions WHERE id NOT IN (SELECT id FROM main.sessions)") {
            Ok(mut st) => { let v = st.query_map([], |r| r.get::<_, String>(0)).map(|m| m.flatten().collect()).unwrap_or_default(); v }
            Err(_) => vec![],
        };
        for id in &ids {
            let _ = db.execute("INSERT OR IGNORE INTO main.sessions(id, title, created, updated, project_root, archived) SELECT id, title, created, updated, project_root, archived FROM b.sessions WHERE id=?1", [id]);
            let _ = db.execute("INSERT INTO main.messages_fts(content, session_id, msg_id) SELECT content, session_id, msg_id FROM b.messages_fts WHERE session_id=?1", [id]);
        }
        let _ = db.execute("INSERT OR IGNORE INTO main.qsql(dataset_fp, question, sql, verified, uses, session_id, created, last_used)
                            SELECT dataset_fp, question, sql, verified, uses, session_id, created, last_used FROM b.qsql", []);
        let _ = db.execute("INSERT INTO main.qsql_fts(question, pair_id) SELECT question, id FROM main.qsql WHERE id NOT IN (SELECT pair_id FROM main.qsql_fts)", []);
        let _ = db.execute("INSERT OR IGNORE INTO main.datasets(fp, name, columns_json, first_seen, last_seen, sessions)
                            SELECT fp, name, columns_json, first_seen, last_seen, sessions FROM b.datasets", []);
        let _ = db.execute("DETACH DATABASE b", []);
    }

    pub fn open(path: &Path) -> Result<Index, String> {
        if let Some(p) = path.parent() { let _ = std::fs::create_dir_all(p); }
        let open = || -> rusqlite::Result<Connection> { let c = Connection::open(path)?; c.execute_batch(SCHEMA)?; Ok(c) };
        let conn = match open() {
            Ok(c) => c,
            Err(_) => { // corrupt index: it's only an index → start over
                let _ = std::fs::remove_file(path);
                open().map_err(|e| e.to_string())?
            }
        };
        Ok(Index { db: Mutex::new(conn) })
    }
    pub fn memory() -> Index {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        Index { db: Mutex::new(c) }
    }

    // ---------- sessions ----------
    pub fn upsert_session(&self, id: &str, project_root: Option<&str>) {
        let now = crate::text::now();
        let _ = self.db.lock().unwrap().execute(
            "INSERT INTO sessions(id, title, created, updated, project_root) VALUES(?1, '新会话', ?2, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET updated=excluded.updated", params![id, now, project_root]);
    }
    pub fn set_title(&self, id: &str, title: &str) {
        let _ = self.db.lock().unwrap().execute("UPDATE sessions SET title=?1 WHERE id=?2", params![title, id]);
    }
    pub fn delete_session(&self, id: &str) {
        let db = self.db.lock().unwrap();
        let _ = db.execute("DELETE FROM sessions WHERE id=?1", [id]);
        let _ = db.execute("DELETE FROM messages_fts WHERE session_id=?1", [id]);
    }
    pub fn title(&self, id: &str) -> Option<String> {
        self.db.lock().unwrap().query_row("SELECT title FROM sessions WHERE id=?1", [id], |r| r.get(0)).optional().ok().flatten()
    }
    pub fn touch(&self, id: &str) {
        let _ = self.db.lock().unwrap().execute("UPDATE sessions SET updated=?1 WHERE id=?2", params![crate::text::now(), id]);
    }
    pub fn list_sessions(&self, project_root: Option<&str>, limit: usize) -> Vec<Value> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT id, title, created, updated, project_root FROM sessions WHERE archived=0 AND
                                 (?1 IS NULL OR project_root=?1) ORDER BY updated DESC LIMIT ?2").unwrap();
        st.query_map(params![project_root, limit as i64], |r| Ok(json!({"id": r.get::<_, String>(0)?, "title": r.get::<_, Option<String>>(1)?,
            "created": r.get::<_, f64>(2)?, "updated": r.get::<_, f64>(3)?, "project_root": r.get::<_, Option<String>>(4)?})))
            .map(|it| it.flatten().collect()).unwrap_or_default()
    }

    // ---------- messages ----------
    pub fn index_message(&self, session: &str, msg_id: &str, content: &str) {
        if content.trim().is_empty() { return; }
        let _ = self.db.lock().unwrap().execute("INSERT INTO messages_fts(content, session_id, msg_id) VALUES(?1, ?2, ?3)",
                                                params![crate::text::prefix(content, 4000), session, msg_id]);
    }
    /// (session id, title, snippet)
    pub fn search_messages(&self, q: &str, exclude_session: &str, limit: usize) -> Vec<(String, String, String)> {
        let Some(fq) = fts_query(q) else { return vec![] };
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare("SELECT f.session_id, COALESCE(s.title, f.session_id), snippet(messages_fts, 0, '[', ']', '…', 16)
                                     FROM messages_fts f LEFT JOIN sessions s ON s.id=f.session_id
                                     WHERE messages_fts MATCH ?1 AND f.session_id != ?2 ORDER BY rank LIMIT ?3") else { return vec![] };
        st.query_map(params![fq, exclude_session, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map(|it| it.flatten().collect()).unwrap_or_default()
    }
    pub fn message_count(&self) -> i64 {
        self.db.lock().unwrap().query_row("SELECT count(*) FROM messages_fts", [], |r| r.get(0)).unwrap_or(0)
    }

    // ---------- verified question → SQL ----------
    pub fn save_qsql(&self, fp: &str, question: &str, sql: &str, session: &str) -> Option<i64> {
        let now = crate::text::now();
        let db = self.db.lock().unwrap();
        if let Some(id) = db.query_row("SELECT id FROM qsql WHERE dataset_fp=?1 AND question=?2", params![fp, question], |r| r.get::<_, i64>(0)).optional().ok().flatten() {
            let _ = db.execute("UPDATE qsql SET sql=?1, last_used=?2, session_id=?3 WHERE id=?4", params![sql, now, session, id]);
            return Some(id);
        }
        db.execute("INSERT INTO qsql(dataset_fp, question, sql, session_id, created, last_used) VALUES(?1,?2,?3,?4,?5,?5)",
                   params![fp, question, sql, session, now]).ok()?;
        let id = db.last_insert_rowid();
        let _ = db.execute("INSERT INTO qsql_fts(question, pair_id) VALUES(?1, ?2)", params![question, id]);
        Some(id)
    }
    pub fn verify_qsql(&self, ids: &[i64], verified: bool) {
        let db = self.db.lock().unwrap();
        for id in ids { let _ = db.execute("UPDATE qsql SET verified=?1 WHERE id=?2", params![verified as i64, id]); }
    }
    pub fn delete_qsql(&self, id: i64) {
        let db = self.db.lock().unwrap();
        let _ = db.execute("DELETE FROM qsql_fts WHERE pair_id=?1", [id]);
        let _ = db.execute("DELETE FROM qsql WHERE id=?1", [id]);
    }
    /// Verified examples for these dataset fingerprints, most similar question first (FTS, then bigram fallback).
    pub fn search_qsql(&self, question: &str, fps: &[String], exclude_session: &str, k: usize) -> Vec<Example> {
        if fps.is_empty() { return vec![]; }
        let db = self.db.lock().unwrap();
        let fp_set: HashSet<&String> = fps.iter().collect();
        let row = |r: &rusqlite::Row| -> rusqlite::Result<(Example, String)> {
            let sess: Option<String> = r.get(5)?;
            Ok((Example { id: r.get(0)?, question: r.get(1)?, sql: r.get(2)?, verified: r.get::<_, i64>(3)? > 0 },
                if sess.as_deref() == Some(exclude_session) { String::new() } else { r.get(4)? }))
        };
        let mut out: Vec<Example> = vec![];
        if let Some(fq) = fts_query(question) {
            if let Ok(mut st) = db.prepare("SELECT q.id, q.question, q.sql, q.verified, q.dataset_fp, q.session_id FROM qsql_fts f JOIN qsql q ON q.id=f.pair_id
                                            WHERE qsql_fts MATCH ?1 AND q.verified=1 ORDER BY rank LIMIT 50") {
                out = st.query_map([fq], row).map(|it| it.flatten().filter(|(_, fp)| fp_set.contains(fp)).map(|x| x.0).collect()).unwrap_or_default();
            }
        }
        if out.len() < k {
            let qg = crate::memory::bigrams(question);
            let ph = fps.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            if let Ok(mut st) = db.prepare(&format!("SELECT id, question, sql, verified, dataset_fp, session_id FROM qsql WHERE verified=1 AND dataset_fp IN ({ph}) ORDER BY last_used DESC LIMIT 300")) {
                let mut extra: Vec<(f64, Example)> = st.query_map(rusqlite::params_from_iter(fps.iter()), row)
                    .map(|it| it.flatten().filter(|(_, fp)| !fp.is_empty()).map(|(e, _)| (crate::memory::overlap(&qg, &crate::memory::bigrams(&e.question)), e)).collect()).unwrap_or_default();
                extra.retain(|(s, e)| *s >= 0.3 && !out.iter().any(|o| o.id == e.id));
                extra.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                out.extend(extra.into_iter().map(|x| x.1));
            }
        }
        out.truncate(k);
        out
    }
    pub fn list_qsql(&self, limit: usize) -> Vec<Value> {
        let db = self.db.lock().unwrap();
        let mut st = db.prepare("SELECT id, dataset_fp, question, sql, verified, last_used FROM qsql ORDER BY last_used DESC LIMIT ?1").unwrap();
        st.query_map([limit as i64], |r| Ok(json!({"id": r.get::<_, i64>(0)?, "dataset_fp": r.get::<_, String>(1)?, "question": r.get::<_, String>(2)?,
            "sql": r.get::<_, String>(3)?, "verified": r.get::<_, i64>(4)? > 0, "last_used": r.get::<_, f64>(5)?})))
            .map(|it| it.flatten().collect()).unwrap_or_default()
    }

    pub fn update_qsql(&self, id: i64, question: &str, sql: &str, verified: bool) {
        let db = self.db.lock().unwrap();
        let _ = db.execute("UPDATE qsql SET question=?1, sql=?2, verified=?3 WHERE id=?4", params![question, sql, verified as i64, id]);
        let _ = db.execute("UPDATE qsql_fts SET question=?1 WHERE pair_id=?2", params![question, id]);
    }
    pub fn qsql_count(&self) -> i64 { self.db.lock().unwrap().query_row("SELECT count(*) FROM qsql", [], |r| r.get(0)).unwrap_or(0) }

    /// Datasets for the memory panel: fingerprint, name, column count, sessions seen, Q-SQL examples.
    pub fn list_datasets(&self, limit: usize) -> Vec<Value> {
        let db = self.db.lock().unwrap();
        let Ok(mut st) = db.prepare("SELECT d.fp, d.name, d.columns_json, d.sessions, d.last_seen, (SELECT count(*) FROM qsql q WHERE q.dataset_fp=d.fp)
                                     FROM datasets d ORDER BY d.last_seen DESC LIMIT ?1") else { return vec![] };
        st.query_map([limit as i64], |r| {
            let cols: Vec<String> = serde_json::from_str(&r.get::<_, Option<String>>(2)?.unwrap_or_default()).unwrap_or_default();
            let sess = r.get::<_, Option<String>>(3)?.unwrap_or_default();
            Ok(json!({"fp": r.get::<_, String>(0)?, "name": r.get::<_, Option<String>>(1)?, "columns": cols.len(),
                      "sessions": sess.split(',').filter(|x| !x.is_empty()).count(), "last_seen": r.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                      "qsql": r.get::<_, i64>(5)?}))
        }).map(|it| it.flatten().collect()).unwrap_or_default()
    }
    pub fn rename_dataset(&self, fp: &str, name: &str) {
        let _ = self.db.lock().unwrap().execute("UPDATE datasets SET name=?1 WHERE fp=?2", params![name, fp]);
    }
    /// Forget a dataset and its Q-SQL examples (DuckDB tables are not touched).
    pub fn delete_dataset(&self, fp: &str) {
        let db = self.db.lock().unwrap();
        let _ = db.execute("DELETE FROM qsql_fts WHERE pair_id IN (SELECT id FROM qsql WHERE dataset_fp=?1)", [fp]);
        let _ = db.execute("DELETE FROM qsql WHERE dataset_fp=?1", [fp]);
        let _ = db.execute("DELETE FROM datasets WHERE fp=?1", [fp]);
    }

    /// One-time import of the Swift app's Q-SQL pairs and dataset fingerprints (app.sqlite) so switching cores keeps
    /// the learned examples. Guarded by `PRAGMA user_version`; returns (qsql, datasets) imported.
    pub fn import_legacy(&self, app_db: &Path) -> (usize, usize) {
        let db = self.db.lock().unwrap();
        let ver: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0);
        if ver >= 1 { return (0, 0); }
        let _ = db.execute_batch("PRAGMA user_version=1");
        let Ok(src) = Connection::open_with_flags(app_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return (0, 0) };
        let (mut nq, mut nd) = (0, 0);
        if let Ok(mut st) = src.prepare("SELECT dataset_fp, question, sql, verified, uses, created, last_used FROM qsql_pairs") {
            let rows: Vec<(String, String, String, i64, i64, f64, f64)> = st.query_map([], |r| Ok((
                r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0), r.get::<_, Option<i64>>(4)?.unwrap_or(0), r.get::<_, Option<f64>>(5)?.unwrap_or(0.0), r.get::<_, Option<f64>>(6)?.unwrap_or(0.0))))
                .map(|it| it.flatten().collect()).unwrap_or_default();
            for (fp, q, sql, v, uses, c, lu) in rows {
                if fp.is_empty() || q.is_empty() || sql.is_empty() { continue; }
                if db.execute("INSERT OR IGNORE INTO qsql(dataset_fp, question, sql, verified, uses, session_id, created, last_used) VALUES(?1,?2,?3,?4,?5,'',?6,?7)",
                              params![fp, q, sql, v, uses, c, lu]).unwrap_or(0) > 0 {
                    let _ = db.execute("INSERT INTO qsql_fts(question, pair_id) VALUES(?1, ?2)", params![q, db.last_insert_rowid()]);
                    nq += 1;
                }
            }
        }
        if let Ok(mut st) = src.prepare("SELECT fp, name, columns_json, first_seen, last_seen, session_ids FROM datasets") {
            let rows: Vec<(String, Option<String>, Option<String>, f64, f64, Option<String>)> = st.query_map([], |r| Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, Option<f64>>(3)?.unwrap_or(0.0), r.get::<_, Option<f64>>(4)?.unwrap_or(0.0), r.get(5)?)))
                .map(|it| it.flatten().collect()).unwrap_or_default();
            for (fp, name, cols, f, l, sess) in rows {
                nd += db.execute("INSERT OR IGNORE INTO datasets(fp, name, columns_json, first_seen, last_seen, sessions) VALUES(?1,?2,?3,?4,?5,?6)",
                                 params![fp, name, cols, f, l, sess]).unwrap_or(0);
            }
        }
        (nq, nd)
    }

    // ---------- dataset fingerprints ----------
    /// Registers a table's schema fingerprint. Returns fingerprints of the same dataset family seen in *other*
    /// sessions (exact match, or column-set Jaccard ≥ 0.7) — their verified examples apply here too.
    pub fn register_dataset(&self, fp: &str, name: &str, columns: &[String], session: &str) -> Vec<String> {
        let now = crate::text::now();
        let db = self.db.lock().unwrap();
        let mine: HashSet<&str> = columns.iter().map(|s| s.as_str()).collect();
        let mut family = vec![];
        if let Ok(mut st) = db.prepare("SELECT fp, columns_json, sessions FROM datasets") {
            let rows: Vec<(String, String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))
                .map(|it| it.flatten().collect()).unwrap_or_default();
            for (f, cj, sess) in rows {
                let other_session = sess.split(',').any(|s| !s.is_empty() && s != session);
                if f == fp { if other_session { family.push(f); } continue; }
                let theirs: HashSet<String> = serde_json::from_str::<Vec<String>>(&cj).unwrap_or_default().into_iter().collect();
                let inter = mine.iter().filter(|c| theirs.contains(**c)).count();
                let uni = mine.len() + theirs.len() - inter;
                if uni > 0 && inter as f64 / uni as f64 >= 0.7 { family.push(f); }
            }
        }
        let sessions: Option<String> = db.query_row("SELECT sessions FROM datasets WHERE fp=?1", [fp], |r| r.get(0)).optional().ok().flatten().flatten();
        match sessions {
            Some(s) => {
                let mut set: Vec<String> = s.split(',').filter(|x| !x.is_empty()).map(String::from).collect();
                if !set.iter().any(|x| x == session) { set.push(session.to_string()); }
                let _ = db.execute("UPDATE datasets SET last_seen=?1, sessions=?2, name=?3 WHERE fp=?4", params![now, set.join(","), name, fp]);
            }
            None => {
                let _ = db.execute("INSERT INTO datasets(fp, name, columns_json, first_seen, last_seen, sessions) VALUES(?1,?2,?3,?4,?4,?5)",
                                   params![fp, name, serde_json::to_string(columns).unwrap(), now, session]);
            }
        }
        family
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_roundtrip() {
        let ix = Index::memory();
        ix.upsert_session("s1", None);
        ix.upsert_session("s2", None);
        ix.set_title("s1", "销售分析");
        ix.index_message("s1", "m1", "各大区已支付订单的销售额是多少");
        let hits = ix.search_messages("已支付订单销售额", "s2", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "销售分析");
        assert!(ix.search_messages("已支付订单", "s1", 5).is_empty());

        let cols: Vec<String> = ["customer_id", "大区", "等级"].iter().map(|s| s.to_string()).collect();
        assert!(ix.register_dataset("fpA", "customers", &cols, "s1").is_empty());
        assert_eq!(ix.register_dataset("fpA", "customers", &cols, "s2"), vec!["fpA"]);
        let cols2: Vec<String> = ["customer_id", "大区", "等级", "备注"].iter().map(|s| s.to_string()).collect();
        assert_eq!(ix.register_dataset("fpB", "customers2", &cols2, "s3"), vec!["fpA"]);

        let id = ix.save_qsql("fpA", "各大区的客户数", "SELECT 大区, count(*) FROM customers GROUP BY 1", "s1").unwrap();
        assert!(ix.search_qsql("每个大区有多少客户", &["fpA".into()], "s9", 3).is_empty()); // not verified yet
        ix.verify_qsql(&[id], true);
        let ex = ix.search_qsql("每个大区有多少客户数", &["fpA".into()], "s9", 3);
        assert_eq!(ex.len(), 1);
        assert!(ix.search_qsql("每个大区有多少客户数", &["fpA".into()], "s1", 3).is_empty()); // own session excluded
        assert!(ix.search_qsql("每个大区有多少客户数", &["fpZ".into()], "s9", 3).is_empty());
        assert_eq!(ix.list_sessions(None, 10).len(), 2);

        ix.update_qsql(id, "大区客户数", "SELECT 1", false);
        assert!(ix.search_qsql("每个大区有多少客户数", &["fpA".into()], "s9", 3).is_empty());
        let ds = ix.list_datasets(10);
        assert_eq!(ds.len(), 2);
        assert_eq!(ds.iter().find(|d| d["fp"] == "fpA").unwrap()["qsql"], 1);
        ix.rename_dataset("fpA", "客户表");
        ix.delete_dataset("fpA");
        assert_eq!(ix.qsql_count(), 0);
        assert_eq!(ix.list_datasets(10).len(), 1);
    }

    #[test]
    fn legacy_import_once() {
        let d = std::env::temp_dir().join(format!("dp-legacy-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(&d).unwrap();
        let app = d.join("app.sqlite");
        let c = Connection::open(&app).unwrap();
        c.execute_batch("CREATE TABLE qsql_pairs(id INTEGER PRIMARY KEY, dataset_fp TEXT, question TEXT, sql TEXT, verified INTEGER, uses INTEGER, created REAL, last_used REAL);
                         CREATE TABLE datasets(fp TEXT PRIMARY KEY, name TEXT, columns_json TEXT, catalog_json TEXT, relations_json TEXT, first_seen REAL, last_seen REAL, session_ids TEXT);
                         INSERT INTO qsql_pairs VALUES(1,'fpA','各大区客户数','SELECT 1',1,2,1,2),(2,'fpA','空','',0,0,1,1);
                         INSERT INTO datasets VALUES('fpA','customers','[\"a\",\"b\"]',NULL,NULL,1,2,'s1,s2');").unwrap();
        drop(c);
        let ix = Index::open(&d.join("index.sqlite")).unwrap();
        assert_eq!(ix.import_legacy(&app), (1, 1));
        assert_eq!(ix.import_legacy(&app), (0, 0));
        assert_eq!(ix.search_qsql("每个大区客户数", &["fpA".into()], "x", 3).len(), 1);
        assert_eq!(ix.list_datasets(5)[0]["sessions"], 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}
