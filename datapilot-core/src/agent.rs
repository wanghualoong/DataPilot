//! The agent loop (port of the Swift `AgentSession.runTurn` / `executeTools` / `executeOne` / `authorize`), plus
//! session notes, selective memory recall, verified-SQL examples and background memory work.
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use crate::compact;
use crate::events::{self, Emit};
use crate::kernel::{KernelClient, KernelProcess};
use crate::llm::{Client, Delta, LlmError};
use crate::notes::{self, SessionNotes};
use crate::paths::Paths;
use crate::permissions::{self, Choice, Decision, Engine, Mode, Risk, ToolView};
use crate::prompt;
use crate::recall;
use crate::secrets::SecretStore;
use crate::session::{load_transcript, Shared};
use crate::text::{self, estimate_tokens, truncate_middle};
use crate::tools::{validate, Registry, ToolCtx, ToolResult};
use crate::types::{ChatMessage, Config, ModelRef, Role, ToolCall};

fn spec_tokens(t: &crate::types::ToolSpec) -> usize {
    estimate_tokens(&t.name) + estimate_tokens(&t.description) + estimate_tokens(&t.parameters.to_string()) + 10
}

/// Process-wide state: paths, config, secrets, tool registry, index, one kernel per data workspace.
pub struct Core {
    pub paths: Paths,
    pub config: Mutex<Config>,
    pub secrets: Arc<dyn SecretStore>,
    pub registry: Arc<Registry>,
    pub index: Arc<crate::index::Index>,
    /// UI data layer on app.sqlite (shared with the SwiftUI app).
    pub ui: Arc<crate::ui::UiDb>,
    kernels: Mutex<HashMap<PathBuf, Arc<KernelProcess>>>,
}

impl Core {
    pub fn new(paths: Paths, config: Config, secrets: Arc<dyn SecretStore>) -> Result<Arc<Core>, String> {
        let registry = Arc::new(Registry::load(&paths.tool_contracts())?);
        let index = Arc::new(crate::index::Index::open(&paths.app_support.join("index.sqlite"))?);
        let app_db = paths.app_support.join("app.sqlite");
        let ui = Arc::new(crate::ui::UiDb::open(&paths.app_support)?);
        if app_db.exists() {
            let (q, d) = index.import_legacy(&app_db);
            if q + d > 0 { eprintln!("imported {q} Q-SQL examples and {d} datasets from app.sqlite"); }
        }
        Ok(Arc::new(Core { paths, config: Mutex::new(config), secrets, registry, index, ui, kernels: Mutex::new(HashMap::new()) }))
    }

    fn kernel(&self, workspace: &Path, project_root: Option<&Path>) -> Arc<KernelProcess> {
        let mut g = self.kernels.lock().unwrap();
        g.entry(workspace.to_path_buf()).or_insert_with(|| {
            let k = KernelProcess::new(workspace.to_path_buf(), self.paths.clone(), project_root.map(Path::to_path_buf));
            k.set_small_rows(self.config.lock().unwrap().settings.small_row_threshold as u64);
            Arc::new(k)
        }).clone()
    }

    /// Stop kernels idle for `secs` (none of `busy` workspaces). Returns how many were stopped.
    pub fn reap_idle(&self, secs: f64, busy: &HashSet<PathBuf>) -> usize {
        let ks: Vec<(PathBuf, Arc<KernelProcess>)> = self.kernels.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        ks.into_iter().filter(|(w, k)| !busy.contains(w) && k.reap_if_idle(secs)).count()
    }

    /// Project removed from the app: stop its kernel and forget it.
    pub fn close_workspace(&self, workspace: &Path) -> bool {
        let k = self.kernels.lock().unwrap().remove(workspace);
        if let Some(k) = &k { k.shutdown(); }
        k.is_some()
    }

    /// One-shot completion with the fast model (falls back to the main model), no tools.
    pub fn fast_complete(&self, system: &str, user: &str) -> Result<String, String> {
        let roles = self.config.lock().unwrap().roles.clone();
        let mut m = roles.fast.or(roles.main).ok_or("尚未配置模型 / no model configured")?;
        let mut caps = m.caps();
        caps.thinking_enabled = false;
        m.capabilities = Some(caps);
        let c = self.client_for(&m)?;
        let r = c.complete(&m, &[ChatMessage::system(system), ChatMessage::user(user)]).map_err(|e| e.to_string())?;
        Ok(r.content.trim().to_string())
    }

    pub fn shutdown(&self) {
        for (_, k) in self.kernels.lock().unwrap().drain() { k.shutdown(); }
        crate::chrome::shutdown_all();
        crate::ssh_tunnel::shutdown_all();
        crate::mcp::shutdown();
    }

    pub fn client_for(&self, m: &ModelRef) -> Result<Client, String> {
        let p = {
            let cfg = self.config.lock().unwrap();
            cfg.providers.iter().find(|p| p.id == m.provider_id).or(cfg.providers.first())
                .ok_or_else(|| format!("找不到模型服务 / Provider not found for {}", m.model))?.clone()
        };
        let key = self.secrets.api_key(&p.id);
        self.client_for_provider(p, key)
    }

    /// Global proxy setting for model clients ("system" is resolved per URL / PAC by the client).
    pub fn global_proxy(&self) -> Option<String> {
        let cfg = self.config.lock().unwrap();
        let s = &cfg.settings;
        match s.proxy_mode.as_str() {
            "custom" if !s.proxy_host.is_empty() && s.proxy_port > 0 => Some(format!("http://{}:{}", s.proxy_host, s.proxy_port)),
            "system" => Some("system".to_string()),
            _ => None,
        }
    }

    pub fn client_for_provider(&self, p: crate::types::Provider, api_key: Option<String>) -> Result<Client, String> {
        Client::new(p, api_key, self.global_proxy())
    }

    /// Create (id = None) or reopen a session. Layout matches the Swift `SessionStore`.
    pub fn open(self: &Arc<Self>, id: Option<&str>, project_root: Option<PathBuf>) -> Result<Agent, String> {
        let new = id.is_none();
        let id = id.map(String::from).unwrap_or_else(text::session_id);
        let (dir, workspace) = match &project_root {
            Some(r) => (r.join(".datapilot/sessions").join(&id), r.join(".datapilot/data")),
            None => { let d = self.paths.sessions_dir().join(&id); (d.clone(), d.join("workspace")) }
        };
        std::fs::create_dir_all(workspace.join("uploads")).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        if let Some(r) = &project_root {
            let pj = r.join(".datapilot/project.json");
            if !pj.exists() {
                let name = r.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let _ = std::fs::write(&pj, serde_json::to_string_pretty(&json!({"id": text::short_id(12), "name": name, "app": "DataPilot", "version": 1})).unwrap());
            }
        }
        let cfg = self.config.lock().unwrap().clone();
        let model = cfg.roles.main.clone().ok_or("尚未配置主对话模型 / No main model configured")?;
        let mode = Mode::parse(&cfg.settings.default_mode).unwrap_or(Mode::Ask);
        let process = self.kernel(&workspace, project_root.as_deref());
        let kernel = KernelClient { process, ns: project_root.as_ref().map(|_| id.clone()) };
        let mut sh = Shared::new(id.clone(), self.paths.clone(), workspace, project_root, kernel, Engine::new(mode, cfg.rules.clone()),
                                 cfg.settings.clone(), self.secrets.clone(), dir.join("transcript.jsonl"), self.index.clone());
        sh.ui = Some(self.ui.clone());
        let shared = Arc::new(sh);
        self.ui.ensure_session(&id, shared.project_root.as_ref().map(|p| p.to_string_lossy().to_string()).as_deref());
        self.index.upsert_session(&id, shared.project_root.as_ref().map(|p| p.to_string_lossy().to_string()).as_deref());
        let mut context = vec![];
        let mut last_tokens = 0;
        if new {
            shared.record(json!({"type": "session_meta", "payload": {"id": id, "created": text::now(), "app": "DataPilot", "core": "rust"}}));
        } else {
            let l = load_transcript(&shared.transcript);
            *shared.todos.lock().unwrap() = l.todos;
            *shared.datasets.lock().unwrap() = l.datasets;
            *shared.family.lock().unwrap() = l.family;
            context = l.context;
            last_tokens = l.last_tokens;
        }
        let notes_path = if shared.project_root.is_some() { dir.join("notes.md") } else { shared.workspace.join("notes.md") };
        let notes = Arc::new(SessionNotes::new(notes_path, &self.paths));
        let client = Arc::new(self.client_for(&model)?);
        let fast = cfg.roles.fast.clone();
        let fast_client = fast.as_ref().and_then(|f| self.client_for(f).ok()).map(Arc::new);
        let memory_cursor = context.len();
        Ok(Agent { shared, registry: self.registry.clone(), model, fast, client, fast_client, context, last_injected: None,
                   last_context_tokens: last_tokens, compact_failures: 0, notes, recalled_files: HashSet::new(), recalled_bytes: 0,
                   recall_rx: Mutex::new(None), memory_cursor, after_turn_slot: Arc::new(Mutex::new((false, None))) })
    }
}

pub struct Agent {
    pub shared: Arc<Shared>,
    pub registry: Arc<Registry>,
    pub model: ModelRef,
    pub fast: Option<ModelRef>,
    pub(crate) client: Arc<Client>,
    pub(crate) fast_client: Option<Arc<Client>>,
    pub context: Vec<ChatMessage>,
    last_injected: Option<String>,
    last_context_tokens: usize,
    compact_failures: u32,
    pub notes: Arc<SessionNotes>,
    /// Project memory topics already surfaced by recall in the live context, and their bytes (reset on compaction).
    recalled_files: HashSet<String>,
    recalled_bytes: usize,
    recall_rx: Mutex<Option<Receiver<Vec<recall::Picked>>>>,
    memory_cursor: usize,
    /// (running, pending job) of the background after-turn worker
    after_turn_slot: Arc<Mutex<(bool, Option<AfterTurnJob>)>>,
}

struct AfterTurnJob {
    update_notes: bool, extract: bool, memory_on: bool, est: usize, snapshot: Vec<ChatMessage>, new_msgs: Vec<ChatMessage>,
    project: Option<PathBuf>, sid: String, kernel: Option<KernelClient>, emit: Emit,
}

pub(crate) enum Gate { Allowed, Denied(String, Option<Value>) }

impl Agent {
    pub fn id(&self) -> &str { &self.shared.id }

    fn append(&mut self, m: ChatMessage) { self.shared.record_message(&m); self.context.push(m); }

    pub fn system_prompt(&self, mode: Mode) -> String {
        let p = &self.shared.paths;
        let s = &self.shared.settings;
        let mut parts = vec![prompt::prompt(p, "base")];
        parts.push(match s.response_language.as_str() {
            "en" => "Respond in English.".into(),
            "zh-Hans" => "使用简体中文回复。".into(),
            _ => "使用用户所用的语言回复 / Reply in the user's language.".to_string(),
        });
        parts.push(format!("小表阈值 / small-table threshold: {} 行。", s.small_row_threshold));
        if cfg!(windows) {
            parts.push("运行环境：Windows。bash 工具实际执行的是 PowerShell（用 Get-ChildItem、Select-String 等 PowerShell 命令，路径分隔符为 \\）；数据分析仍优先用 python / sql_query。".into());
        }
        let budget = 1500.max(self.model.caps().context_window * 2 / 100 * 3);
        if let Some(c) = prompt::skills_catalog(&prompt::skills_for(p, &self.shared.settings), budget) { parts.push(c); }
        if let Some(c) = crate::subagent::catalog(p, self.shared.project_root.as_deref()) { parts.push(c); }
        if let Some(i) = prompt::instruction_block(&p.app_support, self.shared.project_root.as_deref()) { parts.push(i); }
        if s.memory_enabled { if let Some(r) = &self.shared.project_root { parts.push(prompt::project_memory_block(p, r)); } }
        if mode == Mode::Plan { parts.push(prompt::prompt(p, "plan_mode")); }
        parts.join("\n\n")
    }

    /// Tools offered to the model this round (built-in, then MCP).
    fn tool_specs(&self, mode: Mode) -> Vec<crate::types::ToolSpec> {
        let mut specs: Vec<_> = self.registry.tools.iter()
            .filter(|t| t.name() != "computer" || self.shared.settings.computer_use_enabled) // off by default: not offered
            .filter(|t| mode != Mode::Plan || t.contract.plan_mode || t.name() == "exit_plan_mode").map(|t| t.spec()).collect();
        if mode != Mode::Plan { specs.extend(crate::mcp::tools().iter().map(|t| t.spec())); }
        specs
    }

    /// `/context`: estimated token use by part (system prompt pieces, tool definitions, conversation), plus the window.
    pub fn context_breakdown(&self) -> Value {
        let mode = self.shared.perms.mode();
        let (p, st) = (&self.shared.paths, &self.shared.settings);
        let window = self.model.caps().context_window;
        let tok = |s: Option<String>| s.map(|s| estimate_tokens(&s)).unwrap_or(0);
        let system = estimate_tokens(&self.system_prompt(mode));
        let skills = tok(prompt::skills_catalog(&prompt::skills_for(p, st), 1500.max(window * 2 / 100 * 3)));
        let instr = tok(prompt::instruction_block(&p.app_support, self.shared.project_root.as_deref()));
        let memory = if st.memory_enabled { tok(self.shared.project_root.as_ref().map(|r| prompt::project_memory_block(p, r))) } else { 0 };
        let mcp_names: HashSet<String> = crate::mcp::tools().iter().map(|t| t.name().to_string()).collect();
        let (mut builtin, mut mcp, mut n_tools) = (0usize, 0usize, 0usize);
        for t in self.tool_specs(mode) {
            let n = spec_tokens(&t);
            if mcp_names.contains(&t.name) { mcp += n } else { builtin += n }
            n_tools += 1;
        }
        let (mut user, mut assistant, mut tool_results, mut injected, mut summary, mut images, mut n_msgs) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        for m in &self.context {
            let t = estimate_tokens(&m.content) + m.tool_calls.iter().flatten().map(|c| estimate_tokens(&c.arguments)).sum::<usize>();
            images += m.images.as_ref().map(|i| i.len() * 1000).unwrap_or(0);
            n_msgs += 1;
            match m.role {
                Role::Tool => tool_results += t,
                Role::Assistant => assistant += t,
                _ if m.is_meta && m.content.starts_with(compact::SUMMARY_PREFIX) => summary += t,
                _ if m.is_meta => injected += t,
                _ => user += t,
            }
        }
        let other_sys = system.saturating_sub(skills + instr + memory);
        let parts = json!([
            {"key": "system", "tokens": other_sys}, {"key": "skills", "tokens": skills}, {"key": "instructions", "tokens": instr},
            {"key": "memory", "tokens": memory}, {"key": "tools", "tokens": builtin, "count": n_tools}, {"key": "mcp", "tokens": mcp},
            {"key": "injected", "tokens": injected}, {"key": "summary", "tokens": summary}, {"key": "user", "tokens": user},
            {"key": "assistant", "tokens": assistant}, {"key": "tool_results", "tokens": tool_results}, {"key": "images", "tokens": images},
        ]);
        let total = system + builtin + mcp + user + assistant + tool_results + injected + summary + images;
        json!({"window": window, "estimated": total, "last_actual": self.last_context_tokens, "messages": n_msgs,
               "compact_threshold": st.compact_threshold, "model": self.model.model, "parts": parts})
    }

    fn project_block(&self) -> Option<String> {
        let root = self.shared.project_root.as_ref()?;
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut s = format!("## 项目 / Project: {name}\n路径: {}\n", root.display());
        s += "规则：项目文件夹中的原始文件只读（data_import / read_file 可直接用相对路径，如 data/sales.xlsx）；新生成的文件写到 outputs/；同一项目的会话共享 DuckDB 数据库。找文件用 glob，搜文件内容（口径说明、SQL、文档中的关键词）用 grep。\n";
        let files = prompt::data_files(root, 40);
        if !files.is_empty() { s += &format!("数据文件: {}\n", files.join(", ")); }
        if !root.join("DATAPILOT.md").exists() { s += "（项目还没有 DATAPILOT.md，可建议用户输入 /init 生成；DATAPILOT 指令和项目记忆索引在系统提示中）"; }
        Some(s)
    }

    fn kernel_status(&self) -> Option<String> {
        if !self.shared.workspace.join("data.duckdb").exists() { return None; }
        let k = &self.shared.kernel;
        let r = k.call("tables", json!({}), 30).ok()?;
        let tables: Vec<String> = r.result["tables"].as_array().into_iter().flatten()
            .filter_map(|t| t["table"].as_str().map(|n| format!("{n}({} 行)", t["rows"].as_i64().unwrap_or(0)))).collect();
        let vars: Vec<String> = k.call("vars", json!({}), 15).ok().and_then(|v| v.result["vars"].as_array().cloned()).unwrap_or_default().iter()
            .filter_map(|v| v["name"].as_str().map(|n| format!("{n}: {}", v["type"].as_str().unwrap_or("")))).collect();
        if tables.is_empty() && vars.is_empty() { return None; }
        let mut s = format!("## 工作区状态 / Workspace state\nDuckDB 表: {}", if tables.is_empty() { "无".into() } else { tables.join(", ") });
        if !vars.is_empty() { s += &format!("\nPython 变量: {}", vars.iter().take(20).cloned().collect::<Vec<_>>().join(", ")); }
        Some(s)
    }

    /// Verified question → SQL pairs on this session's datasets (own, or the same report seen before), most similar first.
    fn examples_block(&self, question: &str) -> Option<String> {
        if !self.shared.settings.memory_enabled { return None; }
        let ex = self.shared.index.search_qsql(question, &self.shared.all_datasets(), &self.shared.id, 3);
        if ex.is_empty() { return None; }
        let body = ex.iter().map(|e| format!("问：{}\n```sql\n{}\n```", e.question, e.sql)).collect::<Vec<_>>().join("\n");
        Some(format!("## 已验证的查询示例 / Verified examples（同一数据集上以前答对的问题；口径可参考，表名列名以当前 schema 为准）\n{body}"))
    }

    /// Last turn's question → SQL pairs become verified unless this message corrects them.
    fn settle_pending_qsql(&self, user_text: &str) {
        let ids: Vec<i64> = std::mem::take(&mut *self.shared.pending_qsql.lock().unwrap());
        if ids.is_empty() { return; }
        const CUES: &[&str] = &["不对", "错了", "不是这样", "算错", "有误", "不正确", "应该是", "重新算", "wrong", "incorrect"];
        let low = user_text.to_lowercase();
        if CUES.iter().any(|c| low.contains(c)) { for id in ids { self.shared.index.delete_qsql(id); } }
        else { self.shared.index.verify_qsql(&ids, true); }
    }

    fn turn_context(&self, question: &str) -> Option<String> {
        let total = 1500.max(self.model.caps().context_window / 10);
        let mut blocks: Vec<String> = vec![];
        let mut used = 0;
        let mut add = |s: Option<String>, cap: usize| {
            let Some(s) = s.filter(|s| !s.is_empty()) else { return };
            let t = estimate_tokens(&s);
            if used + t.min(cap) > total { return; }
            blocks.push(if t > cap { truncate_middle(&s, cap * 2) } else { s });
            used += t.min(cap);
        };
        add(Some(format!("当前日期 / Today: {}", text::today_line())), 50);
        let mut ks = self.kernel_status();
        if let Some(pb) = self.project_block() { ks = Some(pb + &ks.map(|k| format!("\n\n{k}")).unwrap_or_default()); }
        add(ks, 600);
        add(self.examples_block(question), 800);
        if !self.notes.is_effectively_empty() {
            add(self.notes.text().map(|n| format!("## 会话笔记 / Session notes\n{n}")), 3000.min(total / 3));
        }
        if blocks.len() <= 1 { return None; }
        Some(format!("<context>\n{}\n</context>", blocks.join("\n\n")))
    }

    /// First message → fallback title now, fast-model title in the background.
    fn auto_title(&self, first: &str, emit: &Emit) {
        let (idx, id, ui) = (self.shared.index.clone(), self.shared.id.clone(), self.shared.ui.clone());
        if idx.title(&id).map(|t| t != "新会话").unwrap_or(false) { return; }
        let quick = text::prefix(first.trim(), 24);
        idx.set_title(&id, &quick);
        if let Some(u) = &ui { u.rename(&id, &quick); }
        emit(events::title_changed(&quick));
        let emit = emit.clone();
        // no fast model configured → the main model writes the title (same fallback as compaction / notes)
        let (fm, fc) = self.fast_pair();
        let (q, sys) = (text::prefix(first, 500), prompt::prompt(&self.shared.paths, "session_title"));
        std::thread::spawn(move || {
            let mut m = fm;
            let mut caps = m.caps();
            caps.thinking_enabled = false;
            caps.max_output_tokens = 40;
            m.capabilities = Some(caps);
            if let Ok(r) = fc.complete(&m, &[ChatMessage::system(sys), ChatMessage::user(q)]) {
                let t = r.content.trim().trim_matches(|c: char| "\"'《》“”#".contains(c)).trim().to_string();
                if !t.is_empty() && t.chars().count() <= 40 {
                    idx.set_title(&id, &t);
                    if let Some(u) = &ui { u.rename(&id, &t); }
                    emit(events::title_changed(&t));
                }
            }
        });
    }

    /// One user turn. Returns the finish reason (done | cancelled | max_turns | error).
    pub fn run_turn(&mut self, user_text: &str, skill: Option<&str>, emit: &Emit) -> String {
        let turn = text::short_id(10);
        self.shared.broker.cancel.store(false, std::sync::atomic::Ordering::SeqCst);
        self.shared.computer_steps.store(0, std::sync::atomic::Ordering::SeqCst);
        *self.shared.model_caps.lock().unwrap() = Some(self.model.caps());
        emit(events::turn_started(&turn));
        // non-project chats: workspace snapshot so `session/rewind` can restore files too (project data is shared)
        if self.shared.project_root.is_none() {
            if let Some(dir) = self.shared.transcript.parent() { crate::snapshots::take(dir, &self.shared.workspace, &turn); }
        }
        // `/dream` (consolidate project memory now) and `/dream undo` are handled without the model
        let cmd = user_text.trim();
        if cmd == "/dream" || cmd == "/dream undo" {
            let msg = self.dream_command(cmd == "/dream undo");
            emit(events::notice(&msg));
            emit(events::turn_finished(&turn, "done"));
            return "done".into();
        }
        // project: changed / added / deleted source files (modified → auto re-import; others → ask)
        let sync_note = self.sync_sources(&turn, emit);
        if self.shared.cancelled() { emit(events::turn_finished(&turn, "cancelled")); return "cancelled".into(); }
        self.settle_pending_qsql(user_text);
        if !self.context.iter().any(|m| m.role == Role::User && !m.is_meta) { self.auto_title(user_text, emit); }
        if let Some(ctx) = self.turn_context(user_text) {
            if Some(&ctx) != self.last_injected.as_ref() {
                if let Some(i) = self.context.iter().rposition(|m| m.is_meta && m.role == Role::User && m.content.starts_with("<context>")) { self.context.remove(i); }
                self.last_injected = Some(ctx.clone());
                self.append(ChatMessage::meta(ctx.clone(), &turn));
                emit(events::context_injected(&ctx));
            }
        }
        if let Some(name) = skill {
            if let Some(sk) = prompt::skills_for(&self.shared.paths, &self.shared.settings).into_iter().find(|s| s.name == name) {
                let msg = format!("<skill name=\"{}\" source=\"{}\">\nBase directory: {}\n\n{}\n</skill>\n用户通过 /{} 显式调用了这个 skill：本轮请严格按上面的说明执行（无需再调用 skill 工具加载）。若其中步骤依赖本环境没有的工具或命令，说明原因并用可用工具替代。",
                                  sk.name, sk.source, sk.dir.display(), sk.body, sk.name);
                self.append(ChatMessage::meta(msg, &turn));
                emit(events::notice(&format!("📘 已加载 skill「{}」（来源：{}，{} 字说明已放入上下文）", sk.name, sk.source, sk.body.chars().count())));
            }
        }
        if let Some(n) = sync_note { self.append(ChatMessage::meta(n, &turn)); }
        self.start_recall(user_text);
        let mut um = ChatMessage::user(user_text);
        um.turn_id = Some(turn.clone());
        self.append(um);

        let max_turns = self.shared.settings.max_turns;
        let mut rounds = 0;
        let mut sigs: HashMap<String, usize> = HashMap::new();
        let mut had_tools = false;
        let reason = loop {
            if self.shared.cancelled() { break "cancelled"; }
            rounds += 1;
            if rounds > max_turns { emit(events::notice(&format!("已达到最大轮数 {max_turns}，停止 / max turns reached"))); break "max_turns"; }
            let mode = self.shared.perms.mode();
            let specs = self.tool_specs(mode);
            let system = self.system_prompt(mode);
            let overhead = estimate_tokens(&system) + specs.iter().map(spec_tokens).sum::<usize>();
            self.maybe_compact(emit, overhead);
            self.consume_recall(&turn, emit);
            compact::prune_images(&mut self.context, 3);
            let mut msgs = vec![ChatMessage::system(system)];
            msgs.extend(self.context.iter().cloned());
            let e2 = emit.clone();
            let mut on = move |d: Delta| match d {
                Delta::Content(s) => e2(events::assistant_delta(&s)),
                Delta::Reasoning(s) => e2(events::reasoning_delta(&s)),
                Delta::ToolCallStarted(i, n) => e2(events::tool_call_streaming(i, &n)),
            };
            let result = match self.client.stream(&self.model, &msgs, &specs, &self.shared.broker.cancel, &mut on) {
                Ok(r) => r,
                Err(LlmError::Cancelled) => break "cancelled",
                Err(LlmError::ContextTooLong(b)) => {
                    self.learn_context_limit(&b, emit);
                    emit(events::notice("上下文过长，强制压缩后重试 / context too long, compacting"));
                    if self.compact(emit, true) { continue; }
                    emit(events::error("压缩失败")); break "error";
                }
                Err(e) => {
                    if self.shared.cancelled() { break "cancelled"; }
                    emit(events::error(&e.to_string())); break "error";
                }
            };
            if let Some(u) = &result.usage {
                self.last_context_tokens = u.prompt_tokens + u.completion_tokens;
                self.shared.record(json!({"type": "usage", "tokens": self.last_context_tokens}));
                emit(events::usage(u, self.last_context_tokens, self.model.caps().context_window));
            }
            let mut a = ChatMessage::new(Role::Assistant, result.content.clone());
            if !result.reasoning.is_empty() { a.reasoning = Some(result.reasoning.clone()); }
            if !result.tool_calls.is_empty() { a.tool_calls = Some(result.tool_calls.clone()); }
            a.turn_id = Some(turn.clone());
            self.append(a.clone());
            emit(events::assistant_message(&a));
            if result.tool_calls.is_empty() {
                if result.finish.as_deref() == Some("length") { emit(events::notice("回复因长度限制被截断 / output truncated (max tokens)")); }
                // the user typed something while the model was answering → continue the same turn with it
                if !self.shared.cancelled() && self.deliver_interjections(&turn, emit) { continue; }
                break "done";
            }
            had_tools = true;
            self.notes.add_tool_calls(result.tool_calls.len());
            for c in &result.tool_calls {
                let n = sigs.entry(format!("{}{}", c.name, c.arguments)).or_insert(0);
                *n += 1;
                if *n >= 3 { self.append(ChatMessage::meta(format!("[系统提示] 你已经用相同参数调用 {} 3 次。请换一种方法，或向用户说明困难。", c.name), &turn)); }
            }
            let outs = self.execute_tools(&result.tool_calls, &turn, emit);
            for m in outs { self.append(m); }
            if self.shared.cancelled() { break "cancelled"; }
            self.deliver_interjections(&turn, emit);
        };
        emit(events::turn_finished(&turn, reason));
        self.after_turn(had_tools, emit);
        reason.to_string()
    }

    /// Consecutive concurrency-safe calls run in parallel (max 10 at once), others serially; results keep call order.
    pub fn execute_tools(&self, calls: &[ToolCall], turn: &str, emit: &Emit) -> Vec<ChatMessage> {
        let mut batches: Vec<(bool, Vec<&ToolCall>)> = vec![];
        for c in calls {
            let safe = self.registry.get(&c.name).map(|t| t.contract.concurrency_safe).unwrap_or(false);
            match batches.last_mut() { Some((true, b)) if safe => b.push(c), _ => batches.push((safe, vec![c])) }
        }
        let mut out = vec![];
        for (safe, b) in batches {
            if self.shared.cancelled() {
                out.extend(b.iter().map(|c| ChatMessage::tool("[cancelled by user]", &c.id, &c.name, turn)));
                continue;
            }
            if safe && b.len() > 1 {
                for chunk in b.chunks(10) {
                    let res: Vec<ChatMessage> = std::thread::scope(|s| {
                        let hs: Vec<_> = chunk.iter().map(|c| s.spawn(move || self.execute_one(c, turn, emit))).collect();
                        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| ChatMessage::tool("Error: tool panicked", "", "", turn))).collect()
                    });
                    out.extend(res);
                }
            } else {
                for c in b { out.push(self.execute_one(c, turn, emit)); }
            }
        }
        out
    }

    pub fn execute_one(&self, call: &ToolCall, turn: &str, emit: &Emit) -> ChatMessage {
        let max = self.shared.settings.tool_output_max_chars;
        let reply = |text: &str, err: bool, display: Option<Value>, ms: i64| -> ChatMessage {
            let m = ChatMessage::tool(truncate_middle(text, max), &call.id, &call.name, turn);
            if let Some(d) = &display { self.shared.record(json!({"type": "display", "callId": call.id, "payload": d})); }
            emit(events::tool_finished(&call.id, &m, display.as_ref(), err, ms));
            m
        };
        let mcp_tool = crate::mcp::tool(&call.name);
        let Some(tool) = self.registry.get(&call.name).or(mcp_tool.as_deref()) else {
            emit(events::tool_started(&call.id, &call.name, &call.name));
            return reply(&format!("Error: unknown tool '{}'. Available: {}", call.name, self.registry.names().join(", ")), true, None, 0);
        };
        let Some(args) = call.parsed() else {
            emit(events::tool_started(&call.id, &call.name, &call.name));
            return reply(&format!("Error: arguments are not valid JSON: {}. Retry with a valid JSON object.", text::prefix(&call.arguments, 300)), true, None, 0);
        };
        let summary = tool.summary(&args);
        emit(events::tool_started(&call.id, &call.name, &summary));
        if let Some(e) = validate(&args, &tool.contract.parameters) {
            return reply(&format!("Error: {e}. Schema: {}", tool.contract.parameters), true, None, 0);
        }
        if let Gate::Denied(msg, display) = self.authorize(tool, &args, &call.id, &summary, emit, None) {
            let err = display.is_none();
            return reply(&msg, err, display, 0);
        }
        let t0 = std::time::Instant::now();
        let r: Result<ToolResult, String> = if tool.name() == "agent" {
            self.run_subagent(&args, &call.id, turn, emit)
        } else {
            let ctx = ToolCtx { shared: &self.shared, turn_id: turn.to_string(), call_id: call.id.clone(), emit: emit.clone(),
                                kernel_override: None, agent_memory_dir: None };
            tool.run(&args, &ctx)
        };
        let ms = t0.elapsed().as_millis() as i64;
        match r {
            Ok(r) => {
                let vision = self.model.caps().supports_vision;
                let text = if r.images.is_some() && !vision { format!("{}\n（当前模型不支持图片输入，截图未附上；请改用 snapshot。）", r.content) } else { r.content.clone() };
                let mut m = reply(&text, r.is_error, r.display, ms);
                if vision { m.images = r.images; }
                m
            }
            Err(e) => reply(&format!("Error: {e}"), true, None, ms),
        }
    }

    /// local-only → network switch card → mode/rules decision → approval card. `origin` labels sub-agent cards.
    pub(crate) fn authorize(&self, tool: &crate::tools::Tool, args: &Value, call_id: &str, summary: &str, emit: &Emit, origin: Option<&str>) -> Gate {
        let s = &self.shared;
        let labelled = origin.map(|o| format!("[{o}] {summary}"));
        let summary = labelled.as_deref().unwrap_or(summary);
        let risk = tool.risk_for(args);
        let view = ToolView { name: tool.name(), risk, plan_mode: tool.contract.plan_mode, summary };
        let mut decision = s.perms.decide(&view, args);
        let denied = matches!(decision, Decision::Deny(_));
        if !denied && tool.needs_network(args) && s.settings.local_only {
            return Gate::Denied("本地模式下禁止联网 / Local-only mode: network tools are disabled. 请告诉用户：如需联网，先关闭本地模式。".into(), None);
        }
        if !denied && tool.needs_network(args) && !s.network() {
            let host = permissions::host(args).unwrap_or_default();
            emit(events::approval_required(call_id, tool.name(), summary, "network", args, "network", &host));
            let (choice, _) = s.broker.wait_approval(call_id);
            if choice == Choice::Deny {
                return Gate::Denied(format!("联网已关闭，用户拒绝开启 / Network is off and the user declined to enable it. 请告诉用户：需要打开输入框下方的「联网」开关才能访问 {}。不要声称网络故障。",
                                            if host.is_empty() { "网络".to_string() } else { host }), None);
            }
            s.set_network(true);
            emit(events::network_changed(true));
            if risk == Risk::Network && decision == Decision::Ask { decision = Decision::Allow; }
        }
        match decision {
            Decision::Deny(why) => {
                if why.starts_with(permissions::PLAN_BLOCKED_PREFIX) { return Gate::Denied(why.clone(), Some(json!({"kind": "blocked", "reason": why}))); }
                Gate::Denied(format!("Permission denied: {why}"), None)
            }
            Decision::Ask => {
                emit(events::approval_required(call_id, tool.name(), summary, risk.as_str(), args, "tool", &permissions::suggest(args).unwrap_or_default()));
                let (choice, fb) = s.broker.wait_approval(call_id);
                if choice == Choice::Deny {
                    if let Some(fb) = fb.filter(|f| !f.is_empty()) {
                        return Gate::Denied(format!("用户拒绝了此操作，并给出指示 / The user denied this action and said: {fb}\n请按用户的指示调整做法。"), None);
                    }
                    return Gate::Denied("用户拒绝了此操作 / The user denied this action. Do not retry it; ask the user or choose another approach.".into(), None);
                }
                if let Some(r) = s.perms.record(choice, tool.name(), args) {
                    if let Err(e) = permissions::persist_rule(&s.paths.app_support.join("config.json"), &r) { eprintln!("persist rule: {e}"); }
                }
                Gate::Allowed
            }
            Decision::Allow => Gate::Allowed,
        }
    }

    // ---------- compaction ----------
    pub fn last_context_tokens(&self) -> usize { self.last_context_tokens }
    pub fn estimate_context(&self) -> usize { compact::estimate_context(&self.context) }

    /// `overhead`: system prompt + tool definitions of this request. The model's last reported size wins when larger
    /// (also restored from the transcript after a restart).
    fn maybe_compact(&mut self, emit: &Emit, overhead: usize) {
        let window = self.model.caps().context_window;
        let req = |a: &Self| a.estimate_context().saturating_sub(2000) + overhead.max(2000);
        let before = req(self);
        let mut est = before.max(self.last_context_tokens);
        if (est as f64) <= window as f64 * 0.6 { return; }
        // clearing old tool output: subtract what was freed from the model's real count too (not just the estimate)
        if compact::microcompact(&mut self.context, 6, 1500) > 0 {
            let after = req(self);
            est = after.max(self.last_context_tokens.saturating_sub(before.saturating_sub(after)));
            self.last_context_tokens = 0;
        }
        if est <= self.compact_limit() { return; }
        self.compact(emit, false);
    }

    /// Auto-compaction point in tokens: window × threshold, capped by `compactTokenLimit` when set (smaller wins).
    pub fn compact_limit(&self) -> usize {
        let st = &self.shared.settings;
        let by_ratio = (self.model.caps().context_window as f64 * st.compact_threshold) as usize;
        if st.compact_token_limit > 0 { by_ratio.min(st.compact_token_limit) } else { by_ratio }
    }

    /// The server said the context is too long and named its limit: shrink this session's window and remember it
    /// in config.json (roles.main) so later sessions start with the real value.
    fn learn_context_limit(&mut self, body: &str, emit: &Emit) {
        let Some(n) = crate::llm::context_limit_from_error(body) else { return };
        let mut caps = self.model.caps();
        if n >= caps.context_window { return; }
        caps.context_window = n;
        caps.max_output_tokens = caps.max_output_tokens.min(n / 4);
        self.model.capabilities = Some(caps.clone());
        let cfg = self.shared.paths.app_support.join("config.json");
        let raw = crate::server::ui_rpc::read_raw(&cfg);
        if raw["roles"]["main"]["model"].as_str() == Some(self.model.model.as_str()) {
            let _ = crate::server::ui_rpc::patch_config(&cfg, &json!({"roles": {"main": {"capabilities": serde_json::to_value(&caps).unwrap()}}}));
        }
        emit(events::notice(&format!("模型服务报告上下文上限为 {n} tokens，已按此调整并记住 / context window learned: {n}")));
    }

    pub fn compact(&mut self, emit: &Emit, force: bool) -> bool {
        if self.compact_failures >= 3 && !force { return false; }
        let before = self.estimate_context();
        let (head, tail) = compact::split(&self.context, self.model.caps().context_window);
        if head.is_empty() { return false; }
        // only the previous summary is left to fold: compacting again would just re-summarise the summary
        if head.len() == 1 && head[0].is_meta && head[0].content.starts_with(compact::SUMMARY_PREFIX) {
            if force { emit(events::notice("上下文刚压缩过，暂时没有可再压缩的内容 / Nothing new to compact")); }
            return false;
        }
        let (fm, fc) = self.fast_pair();
        // a background notes update may be running: wait for it (≤ 15 s; one older than 60 s counts as stuck)
        self.notes.wait_if_updating(std::time::Duration::from_secs(15), std::time::Duration::from_secs(60));
        // refresh notes first so definitions survive; fresh notes replace the summary call when they fit
        let notes_updated = self.notes.update(&self.shared.paths, &self.context, &fc, &fm, before);
        let mut via_notes = None;
        if notes_updated && !self.notes.is_effectively_empty() {
            if let Some(n) = self.notes.text().map(|t| notes::capped_sections(&t, notes::SECTION_CAP_CHARS)) {
                let summary = format!("{}{n}", notes::NOTES_SUMMARY_PREFIX);
                let est = 2000 + estimate_tokens(&compact::summary_message(&summary).content) + tail.iter().map(|m| estimate_tokens(&m.content)).sum::<usize>();
                if est < self.compact_limit() { via_notes = Some(summary); }
            }
        }
        let used_notes = via_notes.is_some();
        let (sm, sc) = if self.shared.settings.compact_model == "main" { (self.model.clone(), self.client.clone()) } else { (fm, fc) };
        let r = match via_notes { Some(s) => Ok(s), None => compact::summarize(&self.shared.paths, &head, self.notes.text().as_deref(), &sc, &sm) };
        match r {
            Ok(summary) if !summary.is_empty() => {
                // Codex-style: the user's own words survive verbatim (newest first within the budget), then the recent tail
                let retained = compact::retained_user_messages(&head, compact::retain_budget(self.model.caps().context_window));
                let kept: Vec<String> = retained.iter().chain(tail.iter()).map(|m| m.id.clone()).collect();
                self.context = std::iter::once(compact::summary_message(&summary)).chain(retained).chain(tail).collect();
                self.shared.record(json!({"type": "compacted", "summary": summary, "keptMessageIds": kept}));
                self.last_injected = None; self.last_context_tokens = 0; self.compact_failures = 0;
                self.recalled_files.clear(); self.recalled_bytes = 0;
                self.memory_cursor = self.memory_cursor.min(self.context.len());
                if used_notes { emit(events::notice("已用会话笔记完成压缩（未额外调用总结模型）/ compacted from session notes")); }
                emit(events::compacted(&summary, before, self.estimate_context()));
                true
            }
            r => {
                self.compact_failures += 1;
                let why = match r { Err(e) => e, _ => "empty summary".into() };
                emit(events::notice(&format!("压缩失败（{}/3）：{why}", self.compact_failures)));
                false
            }
        }
    }

    fn fast_pair(&self) -> (ModelRef, Arc<Client>) {
        match (&self.fast, &self.fast_client) { (Some(m), Some(c)) => (m.clone(), c.clone()), _ => (self.model.clone(), self.client.clone()) }
    }

    /// Queued interjections → user messages + one meta hint. True when something was delivered.
    fn deliver_interjections(&mut self, turn: &str, emit: &Emit) -> bool {
        let batch: Vec<(String, String)> = std::mem::take(&mut *self.shared.interjections.lock().unwrap());
        if batch.is_empty() { return false; }
        for (id, t) in &batch {
            let mut m = ChatMessage::user(t.as_str());
            m.turn_id = Some(turn.to_string());
            self.append(m);
            emit(events::interjection_delivered(id, t));
        }
        self.append(ChatMessage::meta("[系统提示] 上面是用户在你执行任务过程中补充的消息。请据此调整接下来的做法（必要时修改计划或回答）；已完成的工具结果仍然有效，不要重复执行。/ The user added the message(s) above while you were working — adapt accordingly.", turn));
        true
    }

    /// Drop this turn and everything after it (context + transcript `rewind` record); non-project chats also restore
    /// the workspace snapshot taken before the turn (the kernel is stopped first).
    pub fn rewind(&mut self, turn_id: &str) -> Result<bool, String> {
        let mut restored = false;
        if self.shared.project_root.is_none() {
            if let Some(dir) = self.shared.transcript.parent().map(Path::to_path_buf) {
                if crate::snapshots::exists(&dir, turn_id) {
                    self.shared.kernel.process.shutdown();
                    crate::snapshots::restore(&dir, &self.shared.workspace, turn_id)?;
                    restored = true;
                }
            }
        }
        let Some(i) = self.context.iter().position(|m| m.turn_id.as_deref() == Some(turn_id)) else { return Err("turn not found in the live context".into()) };
        self.context.truncate(i);
        self.shared.record(json!({"type": "rewind", "payload": {"turnId": turn_id}}));
        self.last_injected = None; self.last_context_tokens = 0;
        self.memory_cursor = self.memory_cursor.min(self.context.len());
        self.recalled_files.clear(); self.recalled_bytes = 0;
        Ok(restored)
    }

    /// `/btw`: side question answered from the recent conversation, no tools, not added to the context.
    pub fn btw(&self, question: &str) -> Result<String, String> {
        let msgs = btw_messages(&self.context, question, self.model.caps().context_window);
        let r = self.client.complete(&self.model, &msgs).map_err(|e| e.to_string())?;
        let t = r.content.trim().to_string();
        Ok(if t.is_empty() { "（模型没有返回内容 / empty answer）".into() } else { t })
    }

    pub fn set_model(&mut self, m: ModelRef, client: Client) {
        self.model = m;
        self.client = Arc::new(client);
    }

    /// 👍 / 👎 on an SQL answer: save the question → SQL pair as verified (or drop it).
    pub fn qsql_feedback(&self, question: &str, sql: &str, up: bool) -> bool {
        let Some(fp) = self.shared.datasets.lock().unwrap().last().cloned() else { return false };
        let Some(id) = self.shared.index.save_qsql(&fp, question, sql, &self.shared.id) else { return false };
        if up { self.shared.index.verify_qsql(&[id], true); } else { self.shared.index.delete_qsql(id); }
        true
    }

    // ---------- project source sync (port of Swift AgentSession+SourceSync) ----------
    /// Runs at the start of a turn in a project. Returns a note for the model (None when nothing changed / cancelled).
    fn sync_sources(&mut self, turn: &str, emit: &Emit) -> Option<String> {
        use crate::source_sync as ss;
        let root = self.shared.project_root.clone()?;
        let _gate = ss::begin(&root)?;
        let data = self.shared.workspace.clone();
        let c = ss::detect(&root, &data);
        if c.is_empty() { return None; }
        let mut notes: Vec<String> = vec![];
        let mut kept: Vec<String> = vec![];
        let tables = |es: &[ss::Entry]| -> Vec<String> {
            let mut t: Vec<String> = es.iter().flat_map(|e| e.tables.clone()).collect::<HashSet<_>>().into_iter().collect();
            t.sort(); t
        };

        // 1. modified → re-import automatically with the recorded options
        if !c.modified.is_empty() {
            let n = c.modified.iter().map(|e| e.path.as_str()).collect::<HashSet<_>>().len();
            emit(events::notice(&format!("🔄 检测到 {n} 个源文件已修改，正在自动重新导入… / Source files changed, re-importing…")));
            for e in &c.modified {
                let ok = self.sync_import(&e.path, &e.params, turn, emit, &format!("重新导入（源文件已修改） {}", e.path));
                let t = e.tables.join(", ");
                notes.push(if ok { format!("已自动重新导入（源文件已修改）：{} → {t}", e.path) }
                           else { format!("源文件已修改但重新导入失败：{}（表 {t} 可能是旧数据）", e.path) });
            }
            let changed = tables(&c.modified);
            if let Ok(r) = self.shared.kernel.call("rebuild_derived", json!({"changed": changed}), 900) {
                let rebuilt = crate::tools::strs(&r.result["rebuilt"]);
                let failed: Vec<String> = r.result["failed"].as_array().into_iter().flatten().filter_map(|f| f["name"].as_str().map(String::from)).collect();
                if !rebuilt.is_empty() { notes.push(format!("已按新数据重建派生表：{}", rebuilt.join(", "))); }
                if !failed.is_empty() { notes.push(format!("派生表重建失败（可能列已变化，需要重新生成）：{}", failed.join(", "))); }
            }
            if let Ok(r) = self.shared.kernel.call("relation_revalidate", json!({"changed": changed}), 600) {
                for d in r.result["dropped"].as_array().into_iter().flatten() {
                    let rel = d["relation"].as_str().unwrap_or("");
                    let before = d["before"].as_f64().map(|b| format!("{:.0}%", b * 100.0)).unwrap_or_else(|| "?".into());
                    match d["after"].as_f64() {
                        Some(a) => {
                            let samples = crate::tools::strs(&d["unmatched_samples"]).into_iter().take(4).collect::<Vec<_>>().join(", ");
                            notes.push(format!("已确认的表间关系 {rel} 在数据更新后匹配率从 {before} 降到 {:.0}%（未匹配样例：{samples}）。用到这条关系前先告诉用户，必要时用 relation_test 重新确认转换规则。", a * 100.0));
                        }
                        None => notes.push(format!("已确认的表间关系 {rel} 在数据更新后无法验证（{}），请向用户确认。", d["error"].as_str().unwrap_or(""))),
                    }
                }
            }
        }
        for p in &c.modified_append {
            notes.push(format!("源文件已修改：{p}。它是以追加（append）方式导入的，没有自动重导，请向用户确认如何处理。"));
        }

        // 2. new files → ask
        if !c.added.is_empty() {
            let mut list = c.added.iter().take(12).cloned().collect::<Vec<_>>().join("、");
            if c.added.len() > 12 { list += &format!(" 等 {} 个", c.added.len()); }
            let (yes, no) = ("导入新文件", "暂不导入");
            let a = self.sync_ask(&format!("项目文件夹里新增了 {} 个数据文件：{list}。要导入到数据库吗？", c.added.len()), &[yes, no], emit)?;
            if ss::is_yes(&a, yes) {
                let done: Vec<String> = c.added.iter().filter(|f| self.sync_import(f, &Map::new(), turn, emit, &format!("导入新增文件 {f}"))).cloned().collect();
                let failed: Vec<String> = c.added.iter().filter(|f| !done.contains(f)).cloned().collect();
                notes.push(format!("用户同意导入新增文件，已导入：{}{}", done.join(", "),
                                   if failed.is_empty() { String::new() } else { format!("；失败：{}", failed.join(", ")) }));
            } else {
                notes.push(format!("项目新增了文件（用户暂不导入）：{}", c.added.join(", ")));
            }
        }

        // 3. deleted sources → ask whether to drop their tables
        if !c.deleted.is_empty() {
            let ts = tables(&c.deleted);
            let mut paths: Vec<String> = c.deleted.iter().map(|e| e.path.clone()).collect::<HashSet<_>>().into_iter().collect();
            paths.sort();
            let (yes, no) = ("删除对应的表", "保留这些表");
            let q = format!("这些已导入的源文件已从项目文件夹中删除：{}。要从数据库中删除对应的表（{}）吗？", paths.join("、"), ts.join(", "));
            let a = self.sync_ask(&q, &[yes, no], emit)?;
            if ss::is_yes(&a, yes) {
                match self.shared.kernel.call("drop_tables", json!({"tables": ts}), 60) {
                    Ok(_) => {
                        let id = format!("sync_drop_{}", text::short_id(6));
                        let m = ChatMessage::tool(format!("已删除表：{}", ts.join(", ")), &id, "data_import", turn);
                        emit(events::tool_finished(&id, &m, None, false, 0));
                        notes.push(format!("源文件已删除，用户同意后已删除表：{}", ts.join(", ")));
                    }
                    Err(e) => notes.push(format!("删除表失败：{e}")),
                }
            } else {
                kept = paths;
                notes.push(format!("源文件已删除，但用户选择保留表：{}（这些表不会再随文件更新）", ts.join(", ")));
            }
        }
        ss::mark_handled(&root, &data, &kept);
        Some(format!("[数据源同步 / source sync]\n{}", notes.iter().map(|n| format!("- {n}")).collect::<Vec<_>>().join("\n")))
    }

    /// Question card; None when the turn was stopped while it was open (files are then asked about again next turn).
    fn sync_ask(&self, question: &str, options: &[&str], emit: &Emit) -> Option<String> {
        let id = format!("srcq_{}", text::short_id(8));
        emit(events::notice("📂 项目文件夹有变化，需要你确认一下（见下方卡片，也可以直接在输入框回答）/ Project files changed — please answer the card below"));
        emit(events::question(&id, question, &options.iter().map(|s| s.to_string()).collect::<Vec<_>>()));
        let a = self.shared.broker.wait_answer(&id);
        if self.shared.cancelled() || a == "[cancelled]" { None } else { Some(a) }
    }

    /// Re-uses the data_import tool (dataset fingerprints, import card in the chat).
    fn sync_import(&self, path: &str, params: &Map<String, Value>, turn: &str, emit: &Emit, label: &str) -> bool {
        let Some(tool) = self.registry.get("data_import") else { return false };
        let mut args = params.clone();
        args.insert("path".into(), json!(path));
        let id = format!("sync_{}", text::short_id(8));
        emit(events::tool_started(&id, "data_import", label));
        let t0 = std::time::Instant::now();
        let ctx = ToolCtx { shared: &self.shared, turn_id: turn.to_string(), call_id: id.clone(), emit: emit.clone(), kernel_override: None, agent_memory_dir: None };
        let r = tool.run(&Value::Object(args), &ctx).unwrap_or_else(|e| ToolResult::err(format!("导入失败：{e}")));
        let m = ChatMessage::tool(r.content.clone(), &id, "data_import", turn);
        emit(events::tool_finished(&id, &m, r.display.as_ref(), r.is_error, t0.elapsed().as_millis() as i64));
        !r.is_error
    }

    fn memory_layers(&self) -> recall::Layers {
        recall::Layers { project: self.shared.project_root.as_deref().map(crate::memory::project_dir),
                         user: crate::memory::user_dir(&self.shared.paths.app_support) }
    }

    // ---------- selective memory recall (project + user layer) (runs in parallel with the main model, never awaited) ----------
    fn start_recall(&mut self, question: &str) {
        *self.recall_rx.lock().unwrap() = None;
        if !self.shared.settings.memory_enabled { return; }
        let layers = self.memory_layers();
        if !layers.any_exists() { return; }
        let (tx, rx) = std::sync::mpsc::channel();
        *self.recall_rx.lock().unwrap() = Some(rx);
        let (exclude, left) = (self.recalled_files.clone(), recall::SESSION_BYTES.saturating_sub(self.recalled_bytes));
        let (fm, fc) = self.fast_pair();
        let paths = self.shared.paths.clone();
        let q = question.to_string();
        std::thread::spawn(move || { let _ = tx.send(recall::select(&paths, &q, &layers, &exclude, left, Some((&fc, &fm)))); });
    }

    fn consume_recall(&mut self, turn: &str, emit: &Emit) {
        let picked = self.recall_rx.lock().unwrap().as_ref().and_then(|rx| rx.try_recv().ok());
        let Some(picked) = picked else { return };
        *self.recall_rx.lock().unwrap() = None;
        let fresh: Vec<recall::Picked> = picked.into_iter().filter(|p| !self.recalled_files.contains(&p.file)).collect();
        if fresh.is_empty() { return; }
        for f in &fresh { self.recalled_files.insert(f.file.clone()); self.recalled_bytes += f.text.len(); }
        let block = recall::block(&fresh);
        self.append(ChatMessage::meta(block.clone(), turn));
        recall::mark_usage(&self.memory_layers(), &fresh.iter().map(|f| f.file.clone()).collect::<Vec<_>>(), "recalled");
        emit(events::context_injected(&block));
    }

    /// Background work after a turn (never blocks the next turn): citation counts, session notes, memory extraction,
    /// daily consolidation. One job at a time; a newer request replaces the pending one.
    fn after_turn(&mut self, had_tools: bool, emit: &Emit) {
        let new_msgs: Vec<ChatMessage> = self.context[self.memory_cursor.min(self.context.len())..].to_vec();
        self.memory_cursor = self.context.len();
        if self.shared.settings.memory_enabled {
            let cited: Vec<String> = new_msgs.iter().filter(|m| m.role == Role::Assistant).flat_map(|m| recall::citations(&m.content)).collect();
            recall::mark_usage(&self.memory_layers(), &cited, "cited");
        }
        let est = self.estimate_context().max(self.last_context_tokens);
        let update_notes = self.notes.should_update(est, had_tools);
        let memory_on = self.shared.settings.memory_enabled;
        // skip extraction when the main agent already wrote memory this turn (mutual exclusion)
        let wrote = new_msgs.iter().any(|m| m.role == Role::Tool && m.name.as_deref() == Some("memory_write") && m.content.starts_with("已保存"));
        let extract = memory_on && !wrote && new_msgs.iter().any(|m| m.role == Role::User && !m.is_meta);
        if !update_notes && !extract && !(memory_on && self.shared.project_root.is_some()) { return; }
        let (fm, fc) = self.fast_pair();
        let (notes, paths, snapshot) = (self.notes.clone(), self.shared.paths.clone(), self.context.clone());
        let job = AfterTurnJob { update_notes, extract, memory_on, est, snapshot, new_msgs,
                                 project: self.shared.project_root.clone(), sid: self.shared.id.clone(),
                                 kernel: Some(self.shared.kernel.clone()), emit: emit.clone() };
        let slot = self.after_turn_slot.clone();
        let mut g = slot.lock().unwrap();
        g.1 = Some(job);
        if g.0 { return; }
        g.0 = true;
        drop(g);
        std::thread::spawn(move || loop {
            let next = { let mut g = slot.lock().unwrap(); let j = g.1.take(); if j.is_none() { g.0 = false; } j };
            let Some(j) = next else { break };
            if j.update_notes { notes.update(&paths, &j.snapshot, &fc, &fm, j.est); }
            let pdir = j.project.as_ref().map(|r| crate::memory::project_dir(r));
            if j.extract { crate::extract::extract(&paths, &j.new_msgs, &fc, &fm, pdir.as_deref()); }
            if let (true, Some(r), Some(d)) = (j.memory_on, &j.project, &pdir) {
                if d.exists() {
                    let ctx = crate::dream::Ctx { paths: paths.clone(), dir: d.clone(), sessions_dir: r.join(".datapilot/sessions"), current: j.sid.clone(),
                                                  kernel: j.kernel.clone(), client: fc.clone(), model: fm.clone() };
                    let rep = crate::dream::run(&ctx, crate::dream::Trigger::Due);
                    if rep.skipped.is_none() { (j.emit)(events::notice(&rep.text())); }
                }
            }
        });
    }

    /// `/dream` → run consolidation now (any gate skipped); `/dream undo` → restore the pre-dream memory.
    pub fn dream_command(&self, undo: bool) -> String {
        if !self.shared.settings.memory_enabled { return "记忆功能已关闭，无法整理。".into(); }
        let Some(ctx) = self.dream_ctx() else { return "只有在项目中才有项目记忆可以整理。".into() };
        if undo {
            return match crate::dream::rollback(&ctx.dir) { Ok(n) => format!("↩️ 已撤销上次记忆整理，恢复为 {n} 条记忆。"), Err(e) => format!("无法撤销：{e}") };
        }
        if !self.wait_background(std::time::Duration::from_secs(120)) { return "后台记忆任务仍在运行，请稍后再试。".into(); }
        crate::dream::run(&ctx, crate::dream::Trigger::Manual).text()
    }

    /// Dream context for this session's project memory (None outside a project).
    pub fn dream_ctx(&self) -> Option<crate::dream::Ctx> {
        let r = self.shared.project_root.as_ref()?;
        let (fm, fc) = self.fast_pair();
        Some(crate::dream::Ctx { paths: self.shared.paths.clone(), dir: crate::memory::project_dir(r), sessions_dir: r.join(".datapilot/sessions"),
                                 current: self.shared.id.clone(), kernel: Some(self.shared.kernel.clone()), client: fc, model: fm })
    }

    /// Block until the background after-turn worker (notes / memory extraction / consolidation) is idle.
    pub fn wait_background(&self, timeout: std::time::Duration) -> bool {
        let t0 = std::time::Instant::now();
        loop {
            { let g = self.after_turn_slot.lock().unwrap(); if !g.0 && g.1.is_none() { return true; } }
            if t0.elapsed() > timeout { return false; }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    pub fn shutdown(&self) { self.shared.broker.cancel_all(); }
}

/// Context for /btw: recent conversation (tool calls flattened), within ~1/3 of the window (port of Swift `btwMessages`).
pub fn btw_messages(context: &[ChatMessage], question: &str, window: usize) -> Vec<ChatMessage> {
    let budget = 4000.max(window / 3);
    let (mut lines, mut used) = (vec![], 0);
    for m in context.iter().rev() {
        let t = match m.role {
            Role::User => format!("{}{}", if m.is_meta { "[背景] " } else { "用户：" }, m.content),
            Role::Assistant => {
                let mut t = format!("助手：{}", m.content);
                if let Some(c) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
                    t += &format!(" [调用工具 {}]", c.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", "));
                }
                t
            }
            Role::Tool => format!("工具结果({})：{}", m.name.clone().unwrap_or_default(), truncate_middle(&m.content, 1500)),
            Role::System => continue,
        };
        let n = estimate_tokens(&t);
        if used + n > budget { break; }
        used += n;
        lines.push(t);
    }
    lines.reverse();
    let sys = "你是 DataPilot 的旁问助手。用户在主任务进行中顺便问了一个问题（/btw）。只根据下面的对话记录和你的常识简短回答，不能调用任何工具，不要改变或继续主任务；记录里没有的信息就直接说不知道。用用户的语言回答，尽量简洁。\nYou answer a quick side question (/btw) from the conversation below; no tools, be brief.";
    vec![ChatMessage::system(sys), ChatMessage::user(format!("<conversation>\n{}\n</conversation>\n\n/btw {question}", lines.join("\n\n")))]
}
