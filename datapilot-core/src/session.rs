//! Per-session shared state (what tools see), the approval / question broker and the transcript writer.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::index::Index;
use crate::kernel::KernelClient;
use crate::paths::Paths;
use crate::permissions::{Choice, Engine};
use crate::secrets::SecretStore;
use crate::types::{ChatMessage, Settings};

/// Waits for UI answers (approval cards, questions, plan approval). Cancel wakes every waiter.
#[derive(Default)]
pub struct Broker { answers: Mutex<HashMap<String, (String, Option<String>)>>, cv: Condvar, pub cancel: AtomicBool }

impl Broker {
    pub fn respond(&self, id: &str, answer: &str, feedback: Option<String>) {
        self.answers.lock().unwrap().insert(id.to_string(), (answer.to_string(), feedback));
        self.cv.notify_all();
    }
    pub fn cancel_all(&self) { self.cancel.store(true, Ordering::SeqCst); self.cv.notify_all(); }

    fn wait(&self, id: &str) -> Option<(String, Option<String>)> {
        let mut g = self.answers.lock().unwrap();
        loop {
            if let Some(a) = g.remove(id) { return Some(a); }
            if self.cancel.load(Ordering::SeqCst) { return None; }
            g = self.cv.wait_timeout(g, Duration::from_millis(250)).unwrap().0;
        }
    }
    pub fn wait_answer(&self, id: &str) -> String { self.wait(id).map(|a| a.0).unwrap_or_else(|| "[cancelled]".into()) }
    pub fn wait_approval(&self, id: &str) -> (Choice, Option<String>) {
        match self.wait(id) { Some((a, fb)) => (Choice::parse(&a), fb), None => (Choice::Deny, None) }
    }
}

pub struct Shared {
    pub id: String,
    pub paths: Paths,
    pub workspace: PathBuf,
    pub project_root: Option<PathBuf>,
    pub kernel: KernelClient,
    pub perms: Engine,
    pub settings: Settings,
    pub secrets: Arc<dyn SecretStore>,
    pub broker: Broker,
    pub todos: Mutex<Value>,
    pub transcript: PathBuf,
    pub index: Arc<Index>,
    /// Schema fingerprints of tables imported in this session, and of the same dataset family seen in other sessions.
    pub datasets: Mutex<Vec<String>>,
    pub family: Mutex<Vec<String>>,
    /// question → SQL pairs saved this turn; verified when the next user turn does not correct them (or on 👍).
    pub pending_qsql: Mutex<Vec<i64>>,
    /// Messages typed while a turn runs (`turn/interject`): (id, text), delivered at the next model round.
    pub interjections: Mutex<Vec<(String, String)>>,
    /// computer tool: state-changing actions this turn; capabilities of the session's main model (vision, coordinates).
    pub computer_steps: AtomicUsize,
    pub model_caps: Mutex<Option<crate::types::Capabilities>>,
    /// The UI's session index (app.sqlite): titles and searchable messages are mirrored there.
    pub ui: Option<Arc<crate::ui::UiDb>>,
    network: AtomicBool,
    writer: Mutex<()>,
}

impl Shared {
    #[allow(clippy::too_many_arguments)]
    pub fn new(id: String, paths: Paths, workspace: PathBuf, project_root: Option<PathBuf>, kernel: KernelClient, perms: Engine,
               settings: Settings, secrets: Arc<dyn SecretStore>, transcript: PathBuf, index: Arc<Index>) -> Self {
        Shared { id, paths, workspace, project_root, kernel, perms, settings, secrets, broker: Broker::default(), todos: Mutex::new(json!([])),
                 transcript, index, datasets: Mutex::new(vec![]), family: Mutex::new(vec![]), pending_qsql: Mutex::new(vec![]), interjections: Mutex::new(vec![]),
                 computer_steps: AtomicUsize::new(0), model_caps: Mutex::new(None), ui: None,
                 network: AtomicBool::new(false), writer: Mutex::new(()) }
    }
    pub fn outputs_dir(&self) -> Option<PathBuf> { self.project_root.as_ref().map(|r| r.join("outputs")) }
    pub fn network(&self) -> bool { self.network.load(Ordering::SeqCst) }
    pub fn set_network(&self, on: bool) { self.network.store(on, Ordering::SeqCst); self.kernel.process.set_network(on); }
    pub fn cancelled(&self) -> bool { self.broker.cancel.load(Ordering::SeqCst) }

    /// Append one transcript record (same record shape as the Swift `TranscriptRecord`).
    pub fn record(&self, mut rec: Value) {
        rec["ts"] = json!(crate::text::now());
        let _g = self.writer.lock().unwrap();
        if let Some(p) = self.transcript.parent() { let _ = std::fs::create_dir_all(p); }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.transcript) {
            let _ = f.write_all((rec.to_string() + "\n").as_bytes());
        }
    }
    pub fn record_message(&self, m: &ChatMessage) {
        self.record(json!({"type": "message", "message": m}));
        if (m.role == crate::types::Role::User && !m.is_meta) || (m.role == crate::types::Role::Assistant && !m.content.is_empty()) {
            self.index.index_message(&self.id, &m.id, &m.content);
            self.index.touch(&self.id);
            if let Some(u) = &self.ui { u.index_message(&self.id, &m.id, &m.content); u.touch(&self.id); }
        }
    }
    /// Remember dataset fingerprints (own + family) and persist them for reopening.
    pub fn add_datasets(&self, own: &[String], family: &[String]) {
        let mut d = self.datasets.lock().unwrap();
        for f in own { if !d.contains(f) { d.push(f.clone()); } }
        let mut fa = self.family.lock().unwrap();
        for f in family { if !fa.contains(f) && !d.contains(f) { fa.push(f.clone()); } }
        let payload = json!({"datasets": *d, "family": *fa});
        drop(d); drop(fa);
        self.record(json!({"type": "meta", "payload": payload}));
    }
    /// Own + family fingerprints (verified examples of all of them apply here).
    pub fn all_datasets(&self) -> Vec<String> {
        let mut v = self.datasets.lock().unwrap().clone();
        for f in self.family.lock().unwrap().iter() { if !v.contains(f) { v.push(f.clone()); } }
        v
    }
}

/// Transcript replay: UI history + model context rebuilt from the latest compaction (same rules as Swift `SessionStore.load`).
#[derive(Default)]
pub struct Loaded { pub ui: Vec<ChatMessage>, pub context: Vec<ChatMessage>, pub todos: Value, pub displays: HashMap<String, Value>, pub compactions: usize,
                     pub datasets: Vec<String>, pub family: Vec<String>, pub last_tokens: usize }

pub fn load_transcript(path: &std::path::Path) -> Loaded {
    let mut out = Loaded { todos: json!([]), ..Default::default() };
    let Ok(text) = std::fs::read_to_string(path) else { return out };
    let mut all: Vec<ChatMessage> = vec![];
    let mut last: Option<(String, Vec<String>, usize)> = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(r) = serde_json::from_str::<Value>(line) else { continue };
        match r["type"].as_str().unwrap_or("") {
            "message" => if let Ok(m) = serde_json::from_value::<ChatMessage>(r["message"].clone()) { all.push(m) },
            "usage" => out.last_tokens = r["tokens"].as_u64().unwrap_or(0) as usize,
            "compacted" => {
                out.compactions += 1;
                out.last_tokens = 0;
                last = Some((r["summary"].as_str().unwrap_or("").to_string(), crate::tools::strs(&r["keptMessageIds"]), all.len()));
            }
            "rewind" => if let Some(t) = r["payload"]["turnId"].as_str() {
                if let Some(i) = all.iter().position(|m| m.turn_id.as_deref() == Some(t)) {
                    all.truncate(i);
                    out.last_tokens = 0;
                    if last.as_ref().map(|c| c.2 > all.len()).unwrap_or(false) { last = None; }
                }
            },
            "display" => if let (Some(c), Some(p)) = (r["callId"].as_str(), r.get("payload")) { out.displays.insert(c.into(), p.clone()); },
            "todos" => out.todos = r["payload"].clone(),
            "meta" => if r["payload"]["datasets"].is_array() {
                out.datasets = crate::tools::strs(&r["payload"]["datasets"]);
                out.family = crate::tools::strs(&r["payload"]["family"]);
            },
            _ => {}
        }
    }
    out.context = match &last {
        Some((summary, kept, idx)) => {
            let mut c = vec![crate::compact::summary_message(summary)];
            c.extend(all[..*idx].iter().filter(|m| kept.contains(&m.id)).cloned());
            c.extend(all[*idx..].iter().cloned());
            c
        }
        None => all.clone(),
    };
    out.ui = all;
    out
}
