//! The `datapilot-core` child process (port of the Swift `CoreProcess`): one stdio JSON-RPC connection, requests
//! matched by id, notifications forwarded to the UI as `core-event`. If the core dies, the next request relaunches it
//! and reopens the sessions that were open (with their last mode), then tells each of them with a notice.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type EventSink = Arc<dyn Fn(Value) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct Layout { pub core: PathBuf, pub resources: PathBuf, pub python_root: PathBuf, pub app_support: PathBuf }

impl Layout {
    /// Bundled layout (next to the executable / in the resource dir), else the repo's development layout.
    pub fn locate(exe_dir: &Path, resource_dir: Option<PathBuf>) -> Layout {
        let core_name = if cfg!(windows) { "datapilot-core.exe" } else { "datapilot-core" };
        let repo = std::env::var("DP_REPO").map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."));
        let bundled_core = exe_dir.join(core_name);
        let res = resource_dir.filter(|r| r.join("Resources/kernel").is_dir());
        let core = std::env::var("DP_CORE").map(PathBuf::from).ok()
            .or_else(|| bundled_core.exists().then_some(bundled_core))
            .unwrap_or_else(|| repo.join("datapilot-core/target/release").join(core_name));
        let (resources, python_root) = match res {
            Some(r) => (r.join("Resources"), r.join("python")),
            None => (repo.join("Resources"), repo.join("build/vendor/python")),
        };
        let app_support = std::env::var("DP_APP_SUPPORT").map(PathBuf::from).unwrap_or_else(|_| default_app_support());
        Layout { core, resources, python_root, app_support }
    }
}

/// Same folder as the SwiftUI app, so both UIs share sessions, config and memory during the transition.
pub fn default_app_support() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default());
    if cfg!(windows) {
        std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| home.join("AppData/Roaming")).join("DataPilot")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support/DataPilot")
    } else {
        home.join(".local/share/DataPilot")
    }
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;

struct Running { child: Child, stdin: ChildStdin, generation: u64 }

pub struct CoreProcess {
    layout: Layout,
    sink: EventSink,
    running: Mutex<Option<Running>>,
    pending: Pending,
    next_id: Mutex<u64>,
    generation: Mutex<u64>,
    /// session id → session/open params (mode kept up to date) for crash recovery.
    open: Mutex<HashMap<String, Value>>,
    pub restarts: Mutex<u32>,
}

impl CoreProcess {
    pub fn new(layout: Layout, sink: EventSink) -> Arc<CoreProcess> {
        Arc::new(CoreProcess { layout, sink, running: Mutex::new(None), pending: Arc::default(), next_id: Mutex::new(0),
                               generation: Mutex::new(0), open: Mutex::new(HashMap::new()), restarts: Mutex::new(0) })
    }

    pub fn layout(&self) -> &Layout { &self.layout }

    fn launch(&self) -> Result<Running, String> {
        let l = &self.layout;
        std::fs::create_dir_all(&l.app_support).map_err(|e| format!("{}: {e}", l.app_support.display()))?;
        // core.log = the core's stderr; keep one previous generation at 5 MB (the core expires core.log.1 after 14 days)
        let lp = l.app_support.join("core.log");
        if std::fs::metadata(&lp).map(|m| m.len() > 5 * 1024 * 1024).unwrap_or(false) {
            let _ = std::fs::rename(&lp, l.app_support.join("core.log.1"));
        }
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&lp).map_err(|e| e.to_string())?;
        let mut c = Command::new(&l.core);
        c.env("DP_RESOURCES", &l.resources).env("DP_PYTHON_ROOT", &l.python_root).env("DP_APP_SUPPORT", &l.app_support)
            .env_remove("DP_API_KEY")   // keys come from the keychain / credential store, never the environment
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(log));
        #[cfg(windows)]
        { use std::os::windows::process::CommandExt; c.creation_flags(0x0800_0000); }   // CREATE_NO_WINDOW
        let mut child = c.spawn().map_err(|e| format!("无法启动核心 {}: {e}", l.core.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let gen = { let mut g = self.generation.lock().unwrap(); *g += 1; *g };
        let (pending, sink) = (self.pending.clone(), self.sink.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                match v.get("id").and_then(Value::as_u64) {
                    Some(id) if v.get("method").is_none() => {
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let r = match v.get("error") {
                                Some(e) if !e.is_null() => Err(e["message"].as_str().unwrap_or("core error").to_string()),
                                _ => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(r);
                        }
                    }
                    _ => sink(v),
                }
            }
            // EOF: the core exited — fail whatever was waiting (the next request relaunches it)
            for (_, tx) in pending.lock().unwrap().drain() { let _ = tx.send(Err("核心进程已退出 / the core exited".into())); }
        });
        Ok(Running { child, stdin, generation: gen })
    }

    /// Start the core if needed. Returns true when this was a restart after a crash.
    fn ensure(&self) -> Result<bool, String> {
        let mut g = self.running.lock().unwrap();
        if let Some(r) = g.as_mut() {
            if matches!(r.child.try_wait(), Ok(None)) { return Ok(false); }
        }
        let crashed = g.is_some();
        *g = Some(self.launch()?);
        if crashed { *self.restarts.lock().unwrap() += 1; }
        Ok(crashed)
    }

    fn send(&self, method: &str, params: &Value) -> Result<std::sync::mpsc::Receiver<Result<Value, String>>, String> {
        let id = { let mut n = self.next_id.lock().unwrap(); *n += 1; *n };
        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string() + "\n";
        let mut g = self.running.lock().unwrap();
        let r = g.as_mut().ok_or("core not running")?;
        if let Err(e) = r.stdin.write_all(line.as_bytes()).and_then(|_| r.stdin.flush()) {
            self.pending.lock().unwrap().remove(&id);
            return Err(format!("写入核心失败 / write to core failed: {e}"));
        }
        let _ = r.generation;
        Ok(rx)
    }

    fn call(&self, method: &str, params: &Value) -> Result<Value, String> {
        let rx = self.send(method, params)?;
        rx.recv().map_err(|_| "核心进程已退出 / the core exited".to_string())?
    }

    /// One JSON-RPC request (blocking; call from a worker thread). Relaunches a crashed core and reopens sessions first.
    pub fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        if self.ensure()? { self.reopen(); }
        let r = self.call(method, &params);
        if let Ok(v) = &r { self.remember(method, &params, v); }
        r
    }

    fn remember(&self, method: &str, params: &Value, result: &Value) {
        let mut open = self.open.lock().unwrap();
        match method {
            "session/open" => if let Some(id) = result["session"].as_str() {
                let mut p = params.clone();
                p["id"] = json!(id);
                open.insert(id.to_string(), p);
            },
            "session/close" | "session/delete" => if let Some(id) = params["session"].as_str() { open.remove(id); },
            "mode/set" => if let (Some(id), Some(m)) = (params["session"].as_str(), params["mode"].as_str()) {
                if let Some(p) = open.get_mut(id) { p["mode"] = json!(m); }
            },
            _ => {}
        }
    }

    fn reopen(&self) {
        let sessions: Vec<(String, Value)> = self.open.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (id, p) in sessions {
            let ok = self.call("session/open", &p).is_ok();
            let text = if ok { "⚠️ 核心进程意外退出，已自动重启 / The core restarted after a crash" } else { "⚠️ 核心重启后无法重新打开这个会话 / Could not reopen this chat after a core restart" };
            (self.sink)(json!({"jsonrpc": "2.0", "method": "agent/event", "params": {"session": id, "event": {"type": "notice", "text": text}}}));
        }
    }

    /// Polite `shutdown` (kernels, Chrome, ssh tunnels, MCP servers stop with it), then kill after a grace period.
    pub fn shutdown(&self) {
        let alive = self.running.lock().unwrap().as_mut().map(|r| matches!(r.child.try_wait(), Ok(None))).unwrap_or(false);
        if alive {
            if let Ok(rx) = self.send("shutdown", &json!({})) { let _ = rx.recv_timeout(Duration::from_secs(8)); }
        }
        if let Some(mut r) = self.running.lock().unwrap().take() {
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(3) {
                if !matches!(r.child.try_wait(), Ok(None)) { return; }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }
}
