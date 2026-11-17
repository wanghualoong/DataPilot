//! stdio JSON-RPC 2.0 "app-server" (ADR-0012): one JSON object per line on stdin / stdout.
//! Requests: initialize, session/open, session/close, turn/start, turn/cancel, approval/respond, question/answer,
//!           turn/interject {session, text, id?}, turn/withdraw, turn/take_pending,
//!           mcp/sync {servers?}, mcp/status,
//!           kernel/call {session, op, params?, timeout?} (UI op whitelist), kernel/tasks, kernel/cancel_task {id}, kernel/restart,
//!           kernel/status, session/rewind {session, turn_id}, session/btw {session, question}, model/set {session, model?, thinking?},
//!           config/reload, llm/fast {system, user}, qsql/feedback {session, question, sql, up}, project/close {root},
//!           mode/set, network/set, compact, memory/dream {session, undo?}, shutdown.
//! UI methods (M4 phase A, `server_ui.rs`): session/*, project/*, config/get|set, secret/set|has (write-only),
//!           provider/models|probe, net/test, chrome/status|install, newapi/*, memory/topics, dashboard/*, skill/*,
//!           files/list, computer/permissions|request — full list in `ui_rpc::METHODS`.
//! Notifications to the client: `agent/event` {session, event} — event is the wire form in protocol/agent-event.schema.json;
//!           `core/notice` {topic, text} (e.g. Chrome install progress).
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::agent::{Agent, Core};
use crate::events::Emit;
use crate::permissions::Mode;
use crate::session::{load_transcript, Shared};
use crate::types::{Config, ModelRef};

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

struct Entry { agent: Arc<Mutex<Agent>>, shared: Arc<Shared> }

pub struct Server { core: Arc<Core>, out: Out, sessions: Mutex<HashMap<String, Entry>> }

#[path = "server_ui.rs"]
pub mod ui_rpc;

fn send(out: &Out, v: &Value) {
    let mut o = out.lock().unwrap();
    let _ = o.write_all((v.to_string() + "\n").as_bytes());
    let _ = o.flush();
}

/// Kernel ops the UI may call directly (data panel, ER diagram, exports, DB connections). Never python / sql / exec.
pub const UI_KERNEL_OPS: &[&str] = &["tables", "preview", "profile", "schema", "relations", "relation_test", "relation_define", "er_layout",
    "derived", "queries", "result_page", "sql_format", "explain", "export", "export_doc",
    "db_types", "db_connections", "db_save", "db_delete", "db_test", "db_schema", "db_extract", "db_refresh"];

/// Methods that may block (model calls, long kernel ops, waits): answered from a worker thread so the loop keeps reading.
const ASYNC_METHODS: &[&str] = &["session/btw", "llm/fast", "config/reload", "session/flush", "memory/dream", "compact", "mcp/sync",
    "kernel/call", "dashboard/query", "kernel/tasks", "kernel/restart", "session/rewind", "project/close"];

fn read_config(app_support: &std::path::Path) -> Result<Config, String> {
    let t = std::fs::read_to_string(app_support.join("config.json")).map_err(|e| format!("config.json: {e}"))?;
    serde_json::from_str(&t).map_err(|e| format!("config.json: {e}"))
}

impl Server {
    pub fn new(core: Arc<Core>, out: Box<dyn Write + Send>) -> Arc<Server> {
        Arc::new(Server { core, out: Arc::new(Mutex::new(out)), sessions: Mutex::new(HashMap::new()) })
    }

    fn emitter(&self, session: &str) -> Emit {
        let out = self.out.clone();
        let s = session.to_string();
        Arc::new(move |e: Value| send(&out, &json!({"jsonrpc": "2.0", "method": "agent/event", "params": {"session": s, "event": e}})))
    }

    fn reply(&self, id: Value, r: Result<Value, String>) {
        send(&self.out, &match r {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": e}}),
        });
    }

    fn agent(&self, p: &Value) -> Result<Arc<Mutex<Agent>>, String> {
        let id = p["session"].as_str().ok_or("missing session")?;
        self.sessions.lock().unwrap().get(id).map(|e| e.agent.clone()).ok_or_else(|| format!("unknown session {id}"))
    }

    fn shared(&self, p: &Value) -> Result<Arc<Shared>, String> {
        let id = p["session"].as_str().ok_or("missing session")?;
        self.sessions.lock().unwrap().get(id).map(|e| e.shared.clone()).ok_or_else(|| format!("unknown session {id}"))
    }

    fn handle(self: &Arc<Self>, method: &str, p: &Value) -> Result<Value, String> {
        if let Some(r) = self.handle_ui(method, p) { return r; }
        match method {
            "initialize" => Ok(json!({"name": "datapilot-core", "version": crate::VERSION, "protocol": 1,
                                      "tools": self.core.registry.names()})),
            "session/open" => {
                // project sessions opened by id alone: the UI index knows their project
                let root = p["project_root"].as_str().map(PathBuf::from)
                    .or_else(|| p["id"].as_str().and_then(|id| self.core.ui.session_project(id)).map(|(_, r)| r));
                let agent = self.core.open(p["id"].as_str(), root)?;
                if let Some(m) = p["mode"].as_str().and_then(Mode::parse) { agent.shared.perms.set_mode(m); }
                let id = agent.id().to_string();
                let info = json!({"session": id, "workspace": agent.shared.workspace, "dir": agent.shared.transcript.parent(), "messages": agent.context.len(),
                                  "mode": agent.shared.perms.mode(), "context_tokens": agent.last_context_tokens(), "context_window": agent.model.caps().context_window});
                let shared = agent.shared.clone();
                self.sessions.lock().unwrap().insert(id, Entry { agent: Arc::new(Mutex::new(agent)), shared });
                Ok(info)
            }
            "session/flush" => {
                let id = p["session"].as_str().ok_or("missing session")?.to_string();
                let agent = self.sessions.lock().unwrap().get(&id).map(|e| e.agent.clone()).ok_or("unknown session")?;
                let a = agent.lock().unwrap();
                let secs = p["timeout"].as_u64().unwrap_or(300);
                Ok(json!({"idle": a.wait_background(std::time::Duration::from_secs(secs))}))
            }
            "session/close" => {
                let id = p["session"].as_str().unwrap_or("");
                if let Some(e) = self.sessions.lock().unwrap().remove(id) { e.shared.broker.cancel_all(); }
                crate::tools::browser::close(id);
                Ok(json!({}))
            }
            "turn/start" => {
                let id = p["session"].as_str().ok_or("missing session")?.to_string();
                let agent = self.sessions.lock().unwrap().get(&id).map(|e| e.agent.clone()).ok_or_else(|| format!("unknown session {id}"))?;
                if agent.try_lock().is_err() { return Err("a turn is already running in this session".into()); }
                let text = p["text"].as_str().unwrap_or("").to_string();
                let skill = p["skill"].as_str().map(String::from);
                let emit = self.emitter(&id);
                std::thread::spawn(move || {
                    let mut a = agent.lock().unwrap();
                    a.run_turn(&text, skill.as_deref(), &emit);
                });
                Ok(json!({"started": true}))
            }
            "turn/cancel" => {
                let s = self.shared(p)?;
                s.broker.cancel_all();
                s.kernel.interrupt();
                Ok(json!({}))
            }
            // Type while the agent works: queued for the running turn, delivered at its next model round.
            // When the turn ends first, `turn/take_pending` returns the undelivered ones (the UI sends them as a new turn).
            "turn/interject" => {
                let s = self.shared(p)?;
                let text = p["text"].as_str().filter(|t| !t.trim().is_empty()).ok_or("missing text")?.to_string();
                let id = p["id"].as_str().map(String::from).unwrap_or_else(|| crate::text::short_id(10));
                s.interjections.lock().unwrap().push((id.clone(), text));
                Ok(json!({"id": id}))
            }
            "turn/withdraw" => {
                let s = self.shared(p)?;
                let id = p["id"].as_str().ok_or("missing id")?;
                let mut q = s.interjections.lock().unwrap();
                let n = q.len();
                q.retain(|(i, _)| i != id);
                Ok(json!({"withdrawn": q.len() < n}))
            }
            "turn/take_pending" => {
                let s = self.shared(p)?;
                let q = std::mem::take(&mut *s.interjections.lock().unwrap());
                Ok(json!({"pending": q.into_iter().map(|(id, text)| json!({"id": id, "text": text})).collect::<Vec<_>>()}))
            }
            // MCP: connect the servers in config.json settings.mcpServers (optionally replaced by `servers`), list status
            "mcp/sync" => {
                let mut settings = self.core.config.lock().unwrap().settings.clone();
                if let Some(v) = p.get("servers").filter(|v| v.is_array()) {
                    settings.mcp_servers = serde_json::from_value(v.clone()).map_err(|e| format!("bad servers: {e}"))?;
                    self.core.config.lock().unwrap().settings.mcp_servers = settings.mcp_servers.clone();
                }
                let st = crate::mcp::sync(&settings.mcp_servers, &settings);
                Ok(json!({"status": st, "tools": crate::mcp::tools().iter().map(|t| t.name().to_string()).collect::<Vec<_>>()}))
            }
            "mcp/status" => Ok(json!({"status": crate::mcp::status(), "tools": crate::mcp::tools().iter().map(|t| t.name().to_string()).collect::<Vec<_>>()})),
            "approval/respond" | "question/answer" => {
                let s = self.shared(p)?;
                let id = p["id"].as_str().ok_or("missing id")?;
                let answer = p["choice"].as_str().or(p["answer"].as_str()).unwrap_or("deny");
                s.broker.respond(id, answer, p["feedback"].as_str().map(String::from));
                Ok(json!({}))
            }
            "mode/set" => {
                let s = self.shared(p)?;
                let m = p["mode"].as_str().and_then(Mode::parse).ok_or("mode must be plan|ask|auto|full")?;
                s.perms.set_mode(m);
                Ok(json!({"mode": m}))
            }
            "network/set" => {
                let s = self.shared(p)?;
                let on = p["enabled"].as_bool().unwrap_or(false);
                s.set_network(on);
                Ok(json!({"enabled": on}))
            }
            "compact" => {
                let id = p["session"].as_str().ok_or("missing session")?.to_string();
                let agent = self.sessions.lock().unwrap().get(&id).map(|e| e.agent.clone()).ok_or("unknown session")?;
                let emit = self.emitter(&id);
                let mut a = agent.try_lock().map_err(|_| "a turn is running")?;
                Ok(json!({"compacted": a.compact(&emit, true)}))
            }
            "memory/dream" => {
                let id = p["session"].as_str().ok_or("missing session")?.to_string();
                let agent = self.sessions.lock().unwrap().get(&id).map(|e| e.agent.clone()).ok_or("unknown session")?;
                let a = agent.try_lock().map_err(|_| "a turn is running")?;
                Ok(json!({"text": a.dream_command(p["undo"].as_bool().unwrap_or(false))}))
            }
            "kernel/call" => {
                let s = self.shared(p)?;
                let op = p["op"].as_str().ok_or("missing op")?;
                if !UI_KERNEL_OPS.contains(&op) { return Err(format!("kernel op not allowed from the UI: {op}")); }
                let mut params = p["params"].as_object().cloned().unwrap_or_default();
                // DB connections: the password comes from the keychain here, never through the UI layer
                // (an unsaved form's SSH secret may come along for `db_test`; it never reaches the kernel)
                let form_ssh = params.remove("ssh_secret").and_then(|v| v.as_str().map(String::from)).filter(|x| !x.is_empty());
                if op.starts_with("db_") || op == "explain" {
                    if let Some(c) = params.get("connection").and_then(|v| v.as_str()).map(String::from) {
                        if !params.contains_key("password") {
                            if let Some(pw) = s.secrets.db_password(&c).filter(|x| !x.is_empty()) { params.insert("password".into(), json!(pw)); }
                        }
                    }
                    s.kernel.process.sync_db_ports();
                    if crate::ssh_tunnel::opens_connection(op) {
                        let conn = match params.get("conn") {
                            Some(c) if c.is_object() => Some(c.clone()),
                            _ => params.get("connection").and_then(|v| v.as_str()).and_then(|r| {
                                crate::tools::db::connections(&s.workspace).into_iter().find(|x| x["id"] == r || x["name"] == r)
                            }),
                        };
                        if let Some(conn) = conn {
                            let id = conn["id"].as_str().unwrap_or("").to_string();
                            let secret = form_ssh.or_else(|| s.secrets.ssh_secret(&id));
                            crate::ssh_tunnel::inject(&conn, &s.workspace, &mut params, secret, &s.paths.app_support)?;
                        }
                    }
                }
                // data dictionary: column descriptions / relations from the workspace's catalog.json
                if op == "export" && params.get("kind").and_then(|v| v.as_str()) == Some("dictionary") {
                    let cat = crate::server::ui_rpc::read_raw(&s.workspace.join("catalog.json"));
                    if !params.contains_key("descriptions") && cat["descriptions"].is_object() { params.insert("descriptions".into(), cat["descriptions"].clone()); }
                    if !params.contains_key("relations") && !cat["relations"].is_null() { params.insert("relations".into(), cat["relations"].clone()); }
                }
                let deleted = (op == "db_delete").then(|| params.get("connection").and_then(|v| v.as_str()).map(String::from)).flatten();
                let r = s.kernel.call(op, Value::Object(params), p["timeout"].as_u64().unwrap_or(300))?;
                if let Some(id) = deleted { crate::ssh_tunnel::close(&id); }
                Ok(json!({"result": r.result, "stdout": r.stdout, "ms": r.ms}))
            }
            // dashboard panel: one dataset of a saved dashboard (SQL comes from the spec file, not from the UI)
            "dashboard/query" => {
                let s = self.shared(p)?;
                let mut r = crate::tools::bi::query_dataset_for_ui(p["id"].as_str().ok_or("missing id")?, p["dataset"].as_str().ok_or("missing dataset")?,
                                                                   &s.kernel, p["cap"].as_u64().unwrap_or(100_000))?;
                // `inline`: rows in the reply (UIs that cannot read the kernel's temp file, e.g. the Tauri webview).
                // Full detail is kept — never aggregated or sampled here.
                if p["inline"] == true {
                    if let Some(f) = r["rows_file"].as_str().map(String::from) {
                        let rows: Value = std::fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!([]));
                        let _ = std::fs::remove_file(&f);
                        r["rows"] = rows;
                        if let Some(o) = r.as_object_mut() { o.remove("rows_file"); }
                    }
                }
                Ok(r)
            }
            // memory panel: verified question → SQL examples and dataset fingerprints (core-level index, no session)
            "memory/qsql" => {
                let ix = &self.core.index;
                match p["action"].as_str().unwrap_or("list") {
                    "list" => Ok(json!({"qsql": ix.list_qsql(p["limit"].as_u64().unwrap_or(500) as usize), "count": ix.qsql_count()})),
                    "update" => {
                        ix.update_qsql(p["id"].as_i64().ok_or("missing id")?, p["question"].as_str().ok_or("missing question")?,
                                       p["sql"].as_str().ok_or("missing sql")?, p["verified"].as_bool().unwrap_or(false));
                        Ok(json!({}))
                    }
                    "delete" => { ix.delete_qsql(p["id"].as_i64().ok_or("missing id")?); Ok(json!({})) }
                    a => Err(format!("unknown action {a}")),
                }
            }
            "memory/datasets" => {
                let ix = &self.core.index;
                match p["action"].as_str().unwrap_or("list") {
                    "list" => Ok(json!({"datasets": ix.list_datasets(p["limit"].as_u64().unwrap_or(500) as usize)})),
                    "rename" => { ix.rename_dataset(p["fp"].as_str().ok_or("missing fp")?, p["name"].as_str().ok_or("missing name")?); Ok(json!({})) }
                    "delete" => { ix.delete_dataset(p["fp"].as_str().ok_or("missing fp")?); Ok(json!({})) }
                    a => Err(format!("unknown action {a}")),
                }
            }
            "kernel/tasks" => Ok(json!({"tasks": self.shared(p)?.kernel.process.running_tasks()})),
            "kernel/cancel_task" => {
                let s = self.shared(p)?;
                s.kernel.process.cancel_task(p["id"].as_str().ok_or("missing id")?);
                Ok(json!({}))
            }
            "kernel/restart" => { self.shared(p)?.kernel.process.shutdown(); Ok(json!({})) }
            "kernel/status" => {
                let k = &self.shared(p)?.kernel.process;
                Ok(json!({"running": k.is_running(), "network": k.network(), "launched_ports": k.launched_ports()}))
            }
            "session/context" => {
                let agent = self.agent(p)?;
                let a = agent.try_lock().map_err(|_| "Agent 正在运行，请本轮结束后再查看 / a turn is running")?;
                Ok(a.context_breakdown())
            }
            "session/rewind" => {
                let agent = self.agent(p)?;
                let mut a = agent.try_lock().map_err(|_| "a turn is running")?;
                let restored = a.rewind(p["turn_id"].as_str().ok_or("missing turn_id")?)?;
                Ok(json!({"workspace_restored": restored, "messages": a.context.len()}))
            }
            // /btw works while a turn runs: context comes from the transcript on disk, not the locked agent
            "session/btw" => {
                let s = self.shared(p)?;
                let q = p["question"].as_str().filter(|q| !q.trim().is_empty()).ok_or("missing question")?;
                let m = self.core.config.lock().unwrap().roles.main.clone().ok_or("尚未配置主对话模型 / No main model")?;
                let client = self.core.client_for(&m)?;
                let msgs = crate::agent::btw_messages(&load_transcript(&s.transcript).context, q, m.caps().context_window);
                let r = client.complete(&m, &msgs).map_err(|e| e.to_string())?;
                let t = r.content.trim().to_string();
                Ok(json!({"text": if t.is_empty() { "（模型没有返回内容 / empty answer）".to_string() } else { t }}))
            }
            "model/set" => {
                let agent = self.agent(p)?;
                let mut m: ModelRef = match p.get("model").filter(|v| v.is_object()) {
                    Some(v) => serde_json::from_value(v.clone()).map_err(|e| format!("bad model: {e}"))?,
                    None => self.core.config.lock().unwrap().roles.main.clone().ok_or("no main model")?,
                };
                if let Some(t) = p["thinking"].as_bool() { let mut c = m.caps(); c.thinking_enabled = t; m.capabilities = Some(c); }
                let client = self.core.client_for(&m)?;
                let mut a = agent.try_lock().map_err(|_| "a turn is running")?;
                let name = m.model.clone();
                a.set_model(m, client);
                Ok(json!({"model": name}))
            }
            // Settings saved in the app: reload config.json, reconnect MCP, reopen idle sessions so they pick up the
            // new settings (mode / network kept). Busy sessions are listed; call again after their turn.
            "config/reload" => {
                let cfg = read_config(&self.core.paths.app_support)?;
                let settings = cfg.settings.clone();
                *self.core.config.lock().unwrap() = cfg;
                std::thread::spawn(move || { crate::mcp::sync(&settings.mcp_servers, &settings); });
                let entries: Vec<(String, Arc<Mutex<Agent>>)> = self.sessions.lock().unwrap().iter().map(|(k, e)| (k.clone(), e.agent.clone())).collect();
                let (mut reopened, mut busy, mut failed) = (vec![], vec![], vec![]);
                for (id, agent) in entries {
                    let Ok(g) = agent.try_lock() else { busy.push(id); continue };
                    let (root, mode, net) = (g.shared.project_root.clone(), g.shared.perms.mode(), g.shared.network());
                    drop(g);
                    match self.core.open(Some(&id), root) {
                        Ok(a) => {
                            a.shared.perms.set_mode(mode);
                            if net { a.shared.set_network(true); }
                            let shared = a.shared.clone();
                            self.sessions.lock().unwrap().insert(id.clone(), Entry { agent: Arc::new(Mutex::new(a)), shared });
                            reopened.push(id);
                        }
                        Err(e) => failed.push(json!({"session": id, "error": e})),
                    }
                }
                Ok(json!({"reopened": reopened, "busy": busy, "failed": failed}))
            }
            "llm/fast" => Ok(json!({"text": self.core.fast_complete(p["system"].as_str().unwrap_or(""), p["user"].as_str().ok_or("missing user")?)?})),
            "qsql/feedback" => {
                let s = self.shared(p)?;
                let (q, sql) = (p["question"].as_str().ok_or("missing question")?, p["sql"].as_str().ok_or("missing sql")?);
                let Some(fp) = s.datasets.lock().unwrap().last().cloned() else { return Ok(json!({"saved": false})) };
                let Some(id) = s.index.save_qsql(&fp, q, sql, &s.id) else { return Ok(json!({"saved": false})) };
                if p["up"].as_bool().unwrap_or(true) { s.index.verify_qsql(&[id], true); } else { s.index.delete_qsql(id); }
                Ok(json!({"saved": true}))
            }
            "project/close" => {
                let root = PathBuf::from(p["root"].as_str().ok_or("missing root")?);
                Ok(json!({"closed": self.core.close_workspace(&root.join(".datapilot/data"))}))
            }
            _ => Err(format!("method not found: {method}")),
        }
    }

    /// Idle trigger for dreaming: sessions without a running turn get `Trigger::Idle` (its own gate: ≥ 6 h since the last
    /// run and memory changed). One check per project memory folder per tick.
    fn idle_tick(&self) {
        let agents: Vec<(String, Arc<Mutex<Agent>>)> = self.sessions.lock().unwrap().iter().map(|(k, e)| (k.clone(), e.agent.clone())).collect();
        // kernels of sessions without a running turn are stopped after settings.kernelIdleSeconds
        let busy: HashSet<PathBuf> = self.busy_workspaces(&agents).into_iter().collect();
        let idle = self.core.config.lock().unwrap().settings.kernel_idle_seconds;
        self.core.reap_idle(idle, &busy);
        let mut seen = std::collections::HashSet::new();
        for (id, a) in agents {
            let Ok(g) = a.try_lock() else { continue };
            if !g.shared.settings.memory_enabled { continue; }
            let Some(ctx) = g.dream_ctx() else { continue };
            drop(g);
            if !seen.insert(ctx.dir.clone()) || !ctx.dir.exists() { continue; }
            let rep = crate::dream::run(&ctx, crate::dream::Trigger::Idle);
            if rep.skipped.is_none() { (self.emitter(&id))(crate::events::notice(&rep.text())); }
        }
    }

    fn busy_workspaces(&self, agents: &[(String, Arc<Mutex<Agent>>)]) -> Vec<PathBuf> {
        let m = self.sessions.lock().unwrap();
        agents.iter().filter(|(_, a)| a.try_lock().is_err()).filter_map(|(id, _)| m.get(id).map(|e| e.shared.workspace.clone())).collect()
    }

    /// Blocking loop over `input` until EOF or `shutdown`.
    pub fn serve(self: &Arc<Self>, input: impl BufRead) {
        // MCP servers connect in the background; their tools appear once ready (status via mcp/status)
        let settings = self.core.config.lock().unwrap().settings.clone();
        if settings.mcp_servers.iter().any(|c| c.enabled) {
            std::thread::spawn(move || { crate::mcp::sync(&settings.mcp_servers, &settings); });
        }
        let weak = Arc::downgrade(self);
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            let Some(s) = weak.upgrade() else { break };
            s.idle_tick();
        });
        for line in input.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() { continue; }
            let req: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(e) => { send(&self.out, &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})); continue; }
            };
            let method = req["method"].as_str().unwrap_or("").to_string();
            let id = req.get("id").cloned();
            if method == "shutdown" {
                for (_, e) in self.sessions.lock().unwrap().drain() { e.shared.broker.cancel_all(); }
                self.core.shutdown();
                if let Some(id) = id { send(&self.out, &json!({"jsonrpc": "2.0", "id": id, "result": {}})); }
                return;
            }
            if ASYNC_METHODS.contains(&method.as_str()) || ui_rpc::ASYNC.contains(&method.as_str()) {
                let me = self.clone();
                let params = req["params"].clone();
                std::thread::spawn(move || {
                    let r = me.handle(&method, &params);
                    if let Some(id) = id { me.reply(id, r); }
                });
                continue;
            }
            let r = self.handle(&method, &req["params"]);
            if let Some(id) = id {
                send(&self.out, &match r {
                    Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
                    Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": e}}),
                });
            }
        }
        self.core.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ui_kernel_whitelist() {
        for op in ["python", "sql", "exec", "query_data", "transform", "import", "drop_tables", "shutdown", "cancel"] {
            assert!(!UI_KERNEL_OPS.contains(&op), "{op} must not be callable from the UI");
        }
        assert!(UI_KERNEL_OPS.contains(&"tables") && UI_KERNEL_OPS.contains(&"db_extract"));
    }
}
