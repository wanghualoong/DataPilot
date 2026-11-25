//! Sandboxed Python data kernel (JSON lines over stdio) — port of the Swift `KernelProcess` / `KernelClient`.
//! One process per project (DuckDB shared, Python namespaces per session via `ns`), lazily started, restarted on exit.
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::paths::Paths;
use crate::sandbox;

fn real(p: &Path) -> PathBuf { crate::platform::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()) }

/// TCP ports of the workspace's database connections (connections.json; file databases need none).
pub fn db_ports(workspace: &Path) -> Vec<u16> {
    let v: Value = std::fs::read_to_string(workspace.join("connections.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let mut out: Vec<u16> = v.get("connections").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|c| {
        let def = match c.get("type")?.as_str()? { "postgres" => 5432, "mysql" => 3306, "sqlserver" => 1433, "oracle" => 1521, "clickhouse" => 8123, _ => return None };
        // SSH tunnel: the kernel connects to the tunnel's local end, not to the database port
        if crate::ssh_tunnel::enabled(c) { return c.get("id")?.as_str().map(crate::ssh_tunnel::local_port); }
        Some(c.get("port").and_then(|p| p.as_u64()).map(|p| p as u16).unwrap_or(def))
    }).collect()).unwrap_or_default();
    out.sort(); out.dedup();
    out
}

#[derive(Debug, Clone)]
pub struct KernelResponse { pub result: Value, pub stdout: String, pub ms: i64 }

type Pending = Arc<Mutex<HashMap<String, Sender<Value>>>>;

struct Running {
    child: sandbox::Proc,
    stdin: Box<dyn Write + Send>,
    pending: Pending,
    alive: Arc<AtomicBool>,
    stderr: Arc<Mutex<String>>,
    ports: Vec<u16>,
    network: bool,
    /// set by `shutdown` before it stops the process, so the reader can tell a planned stop from a crash (diagnostic log)
    quitting: Arc<AtomicBool>,
}

pub struct KernelProcess {
    pub workspace: PathBuf,
    paths: Paths,
    pub project_root: Option<PathBuf>,
    pub output_dir: Option<PathBuf>,
    network: AtomicBool,
    small_rows: AtomicU64,
    counter: AtomicU64,
    run: Mutex<Option<Running>>,
    last_used: Mutex<Instant>,
    pub ready_timeout: Duration,
    pub cancel_grace: Duration,
}

impl KernelProcess {
    pub fn new(workspace: PathBuf, paths: Paths, project_root: Option<PathBuf>) -> Self {
        let output_dir = project_root.as_ref().map(|r| r.join("outputs"));
        KernelProcess { workspace, paths, project_root, output_dir, network: AtomicBool::new(false), small_rows: AtomicU64::new(5000),
                        counter: AtomicU64::new(0), run: Mutex::new(None), last_used: Mutex::new(Instant::now()), ready_timeout: Duration::from_secs(90), cancel_grace: Duration::from_secs(10) }
    }

    pub fn network(&self) -> bool { self.network.load(Ordering::SeqCst) }
    pub fn set_small_rows(&self, n: u64) { self.small_rows.store(n, Ordering::SeqCst) }

    /// Network switch changes need a new sandbox: stop now, the next call relaunches.
    pub fn set_network(&self, on: bool) {
        if self.network.swap(on, Ordering::SeqCst) != on { self.shutdown(); }
    }

    pub fn is_running(&self) -> bool {
        self.run.lock().unwrap().as_ref().map(|r| r.alive.load(Ordering::SeqCst)).unwrap_or(false)
    }

    /// Restart (lazily) when connections.json gained a port the running sandbox does not allow.
    pub fn sync_db_ports(&self) -> bool {
        let now = db_ports(&self.workspace);
        let restart = match self.run.lock().unwrap().as_ref() {
            Some(r) if r.alive.load(Ordering::SeqCst) && !r.network => !now.iter().all(|p| r.ports.contains(p)),
            _ => false,
        };
        if restart { self.shutdown(); }
        restart
    }

    pub fn launched_ports(&self) -> Vec<u16> { self.run.lock().unwrap().as_ref().map(|r| r.ports.clone()).unwrap_or_default() }

    /// Start the kernel. A start that fails because the sandbox denied the kernel its own folders (Windows: folders moved
    /// in by a restore / copy with foreign or protected ACLs, OneDrive, another account's folder…) is retried after
    /// `sandbox::recover` repaired the grants or, failing that, turned isolation off for this workspace.
    fn launch(&self) -> Result<Running, String> {
        let mut attempt = 0;
        loop {
            match self.launch_once() {
                Ok(r) => return Ok(r),
                Err((msg, Some((policy, err)))) if attempt < 2 && sandbox::denied(&err) && sandbox::recover(&policy, &err, attempt) => {
                    crate::log::warn("kernel_retry", "", &[("attempt", (attempt + 1).to_string()), ("why", crate::log::err_text(&msg))]);
                    attempt += 1;
                }
                Err((msg, _)) => return Err(msg),
            }
        }
    }

    /// One start. Err carries the policy and stderr when the kernel itself failed (for `launch` to judge a retry).
    fn launch_once(&self) -> Result<Running, (String, Option<(sandbox::Policy, String)>)> {
        let plain = |e: String| (e, None);
        std::fs::create_dir_all(&self.workspace).map_err(|e| plain(e.to_string()))?;
        let tmp = if self.project_root.is_some() { self.workspace.join("tmp") } else { self.workspace.parent().unwrap_or(&self.workspace).join("tmp") };
        std::fs::create_dir_all(&tmp).map_err(|e| plain(e.to_string()))?;
        if let Some(o) = &self.output_dir { let _ = std::fs::create_dir_all(o); }
        let network = self.network();
        let ports = db_ports(&self.workspace);
        let policy = sandbox::Policy { workspace: self.workspace.clone(), tmp, paths: self.paths.clone(), network,
                                       read_roots: self.project_root.iter().cloned().collect(), write_roots: self.output_dir.iter().cloned().collect(),
                                       db_ports: ports.clone() };
        // `-X utf8`: -I ignores PYTHONIOENCODING / PYTHONUTF8, and on Chinese Windows stdio would otherwise be GBK (the reader
        // expects UTF-8; one GBK reply used to end the reader and the kernel looked "exited")
        let cmd = vec![self.paths.python().display().to_string(), "-I".into(), "-X".into(), "utf8".into(), "-u".into(), self.paths.kernel_script().display().to_string()];
        let mut env = policy.env();
        env.push(("DP_SMALL_ROWS".into(), self.small_rows.load(Ordering::SeqCst).to_string()));
        if let Some(r) = &self.project_root {
            let rp = real(r).display().to_string();
            env.push(("DP_PROJECT_ROOT".into(), rp.clone()));
            env.push(("DP_CWD".into(), rp));
            if let Some(o) = &self.output_dir { env.push(("DP_OUTPUT_DIR".into(), real(o).display().to_string())); }
        }
        let sp = sandbox::spawn(&policy, &cmd, &env, self.project_root.as_ref().unwrap_or(&self.workspace), true)
            .map_err(|e| plain(format!("cannot start kernel: {e}")))?;
        let mut child = sp.proc;
        let stdin = sp.stdin.ok_or_else(|| plain("no stdin".into()))?;
        let stdout = sp.stdout;
        let mut stderr_pipe = sp.stderr;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_done = {
            let stderr = stderr.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr_pipe.read(&mut buf) {
                    if n == 0 { break; }
                    let mut s = stderr.lock().unwrap();
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if s.len() > 20000 { let cut = s.len() - 10000; let keep = s[s.ceil_char_boundary(cut)..].to_string(); *s = keep; }
                }
            })
        };
        let (ready_tx, ready_rx) = channel::<Value>();
        let quitting = Arc::new(AtomicBool::new(false));
        let launched = Instant::now();
        let label = self.workspace.parent().filter(|_| self.project_root.is_none()).unwrap_or(&self.workspace)
            .file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        {
            let pending = pending.clone();
            let alive = alive.clone();
            let stderr = stderr.clone();
            let quitting = quitting.clone();
            let label = label.clone();
            std::thread::spawn(move || {
                let mut ready = Some(ready_tx);
                let mut rd = BufReader::new(stdout);
                let mut raw = Vec::new();
                loop {
                    raw.clear();
                    match rd.read_until(b'\n', &mut raw) { Ok(0) | Err(_) => break, Ok(_) => {} }
                    // never stop on a bad byte sequence: decode lossily (a reply with invalid UTF-8 still finds its caller)
                    let line = String::from_utf8_lossy(&raw).trim_end_matches(['\r', '\n']).to_string();
                    if line.is_empty() { continue; }
                    let v = match serde_json::from_str::<Value>(&line) {
                        Ok(v) => v,
                        Err(e) => match line.strip_prefix("{\"id\": \"").and_then(|r| r.split('"').next()) {
                            Some(id) => json!({"id": id, "ok": false, "error": format!("kernel reply was not valid JSON: {e}")}),
                            None => continue,
                        },
                    };
                    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if id == "_ready" { if let Some(t) = ready.take() { let _ = t.send(v); } continue; }
                    if let Some(tx) = pending.lock().unwrap().remove(&id) { let _ = tx.send(v); }
                }
                alive.store(false, Ordering::SeqCst);
                let err = stderr.lock().unwrap().clone();
                let tail: String = err.chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect();
                let msg = if tail.is_empty() { "kernel exited".to_string() } else { format!("kernel exited: {tail}") };
                if !quitting.load(Ordering::SeqCst) {
                    let n = pending.lock().unwrap().len();
                    crate::log::warn("kernel_exit", "", &[("ws", label), ("uptime_s", launched.elapsed().as_secs().to_string()),
                                                          ("pending", n.to_string()), ("stderr", crate::log::err_text(&tail))]);
                }
                for (_, tx) in pending.lock().unwrap().drain() { let _ = tx.send(json!({"ok": false, "error": msg, "exited": true})); }
            });
        }
        match ready_rx.recv_timeout(self.ready_timeout) {
            Ok(_) => {
                crate::log::info("kernel_start", "", &[("ws", label), ("ms", launched.elapsed().as_millis().to_string()), ("network", (network as u8).to_string())]);
                Ok(Running { child, stdin, pending, alive, stderr, ports, network, quitting })
            }
            Err(e) => {
                quitting.store(true, Ordering::SeqCst);
                child.kill(); child.wait();
                // the process is gone: wait for the rest of its stderr (the traceback) before judging it
                let t = Instant::now();
                while !stderr_done.is_finished() && t.elapsed() < Duration::from_secs(3) { std::thread::sleep(Duration::from_millis(20)); }
                let err = stderr.lock().unwrap().clone();
                let exited = matches!(e, RecvTimeoutError::Disconnected);
                crate::log::error("kernel_ready_timeout", "", &[("ws", label), ("secs", launched.elapsed().as_secs().to_string()), ("exited", (exited as u8).to_string()),
                                                                 ("stderr", crate::log::err_text(&err))]);
                let head = if exited { "Kernel not running: kernel exited during startup".to_string() }
                           else { format!("Kernel not running: kernel did not become ready within {}s", self.ready_timeout.as_secs()) };
                let msg = format!("{head}{}", if err.is_empty() { String::new() } else { format!(": {}", crate::text::prefix(&err, 1500)) });
                Err((msg, Some((policy, err))))
            }
        }
    }

    pub fn ensure_started(&self) -> Result<(), String> {
        let mut g = self.run.lock().unwrap();
        if let Some(r) = g.as_ref() { if r.alive.load(Ordering::SeqCst) { return Ok(()); } }
        if let Some(mut old) = g.take() { old.quitting.store(true, Ordering::SeqCst); old.child.kill(); old.child.wait(); }
        *g = Some(self.launch()?);
        Ok(())
    }

    fn write_line(&self, line: &str) -> Result<(Pending, Arc<Mutex<String>>), String> {
        let mut g = self.run.lock().unwrap();
        let r = g.as_mut().ok_or("Kernel not running: no process")?;
        r.stdin.write_all(line.as_bytes()).and_then(|_| r.stdin.flush()).map_err(|e| format!("Kernel not running: {e}"))?;
        Ok((r.pending.clone(), r.stderr.clone()))
    }

    /// Send an op; Err on kernel-side exceptions (error + stdout + traceback, like the Swift KernelError text).
    pub fn call(&self, op: &str, params: Map<String, Value>, timeout: Duration) -> Result<KernelResponse, String> {
        self.call_with_cancel(op, params, timeout, None)
    }

    /// Like `call`, but returns as soon as `cancel` is set (Stop): the request is cancelled in the kernel and the
    /// caller does not wait for its timeout.
    pub fn call_with_cancel(&self, op: &str, params: Map<String, Value>, timeout: Duration, cancel: Option<&AtomicBool>) -> Result<KernelResponse, String> {
        self.ensure_started()?;
        *self.last_used.lock().unwrap() = Instant::now();
        let id = (self.counter.fetch_add(1, Ordering::SeqCst) + 1).to_string();
        let mut req = params;
        req.insert("id".into(), json!(id));
        req.insert("op".into(), json!(op));
        let (tx, rx) = channel::<Value>();
        {
            let g = self.run.lock().unwrap();
            g.as_ref().ok_or("Kernel not running")?.pending.lock().unwrap().insert(id.clone(), tx);
        }
        let (pending, _) = self.write_line(&(Value::Object(req).to_string() + "\n"))?;
        let t0 = Instant::now();
        let waited = match cancel {
            None => rx.recv_timeout(timeout),
            Some(c) => loop {
                if c.load(Ordering::SeqCst) {
                    let _ = self.write_line(&(json!({"id": format!("_cancel_{id}"), "op": "cancel", "target": id}).to_string() + "\n"));
                    let _ = rx.recv_timeout(Duration::from_secs(2));
                    pending.lock().unwrap().remove(&id);
                    return Err("cancelled by user".into());
                }
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Err(RecvTimeoutError::Timeout) if t0.elapsed() < timeout => continue,
                    r => break r,
                }
            },
        };
        let resp = match waited {
            Ok(v) => v,
            Err(RecvTimeoutError::Disconnected) => return Err("Kernel not running: kernel exited".into()),
            Err(RecvTimeoutError::Timeout) => {
                let _ = self.write_line(&(json!({"id": format!("_cancel_{id}"), "op": "cancel", "target": id}).to_string() + "\n"));
                match rx.recv_timeout(self.cancel_grace) {
                    Ok(v) => {
                        if v.get("state").and_then(|s| s.as_str()) == Some("queued") {
                            return Err(format!("Kernel busy: waited {}s behind an earlier step of this chat (e.g. a large import); nothing was restarted", timeout.as_secs()));
                        }
                        return Err(format!("Kernel timeout after {}s: this step was cancelled on its own; the kernel keeps running", timeout.as_secs()));
                    }
                    Err(_) => {
                        pending.lock().unwrap().remove(&id);
                        crate::log::warn("kernel_restart", "", &[("op", op.to_string()), ("reason", "timeout_uninterruptible".into()), ("secs", timeout.as_secs().to_string())]);
                        self.shutdown();
                        return Err(format!("Kernel timeout after {}s; the step could not be interrupted, so the kernel was restarted", timeout.as_secs()));
                    }
                }
            }
        };
        let ms = resp.get("ms").and_then(|m| m.as_i64()).unwrap_or(t0.elapsed().as_millis() as i64);
        let stdout = resp.get("stdout").and_then(|s| s.as_str()).unwrap_or("").to_string();
        if resp.get("ok").and_then(|b| b.as_bool()) == Some(true) {
            return Ok(KernelResponse { result: resp.get("result").cloned().unwrap_or(Value::Null), stdout, ms });
        }
        let mut msg = resp.get("error").and_then(|s| s.as_str()).unwrap_or("kernel error").to_string();
        if resp.get("exited").is_some() { msg = format!("Kernel not running: {msg}"); }
        if !stdout.is_empty() { msg += &format!("\nstdout:\n{stdout}"); }
        if let Some(tb) = resp.get("traceback").and_then(|s| s.as_str()) { msg += &format!("\ntraceback:\n{tb}"); }
        Err(msg)
    }

    /// What the kernel is doing right now (all chats). Never starts the kernel and does not count as use.
    pub fn running_tasks(&self) -> Vec<Value> {
        if !self.is_running() { return vec![]; }
        let id = format!("run{}", self.counter.fetch_add(1, Ordering::SeqCst) + 1);
        let (tx, rx) = channel::<Value>();
        let pending = {
            let g = self.run.lock().unwrap();
            let Some(r) = g.as_ref() else { return vec![] };
            r.pending.lock().unwrap().insert(id.clone(), tx);
            r.pending.clone()
        };
        if self.write_line(&(json!({"id": id, "op": "running"}).to_string() + "\n")).is_err() { pending.lock().unwrap().remove(&id); return vec![]; }
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(v) => v["result"]["running"].as_array().cloned().unwrap_or_default(),
            Err(_) => { pending.lock().unwrap().remove(&id); vec![] }
        }
    }

    /// Cancel any request by kernel id (e.g. another chat's long step, from the UI task list).
    pub fn cancel_task(&self, target: &str) {
        if !self.is_running() { return; }
        let _ = self.write_line(&(json!({"id": format!("_cancel_{target}"), "op": "cancel", "target": target}).to_string() + "\n"));
    }

    /// Stop the process when it has been idle (no request in flight, unused for `secs`). Next call relaunches.
    pub fn reap_if_idle(&self, secs: f64) -> bool {
        if !self.is_running() || self.last_used.lock().unwrap().elapsed().as_secs_f64() < secs { return false; }
        let busy = self.run.lock().unwrap().as_ref().map(|r| !r.pending.lock().unwrap().is_empty()).unwrap_or(false);
        if busy { return false; }
        self.shutdown();
        true
    }

    /// Cancel one chat's requests (the shared kernel keeps running for other chats).
    pub fn cancel_ns(&self, ns: Option<&str>) {
        if !self.is_running() { return; }
        let _ = self.write_line(&(json!({"id": "_cancel_ns", "op": "cancel", "ns": ns.unwrap_or("_default")}).to_string() + "\n"));
    }

    pub fn shutdown(&self) {
        let Some(mut r) = self.run.lock().unwrap().take() else { return };
        r.quitting.store(true, Ordering::SeqCst);
        if r.alive.load(Ordering::SeqCst) {
            let _ = r.stdin.write_all(b"{\"id\":\"_bye\",\"op\":\"shutdown\"}\n");
            let _ = r.stdin.flush();
            for _ in 0..75 {
                if r.child.try_wait().is_some() { break; }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        r.child.kill();
        r.child.wait();
    }
}

impl Drop for KernelProcess {
    fn drop(&mut self) { self.shutdown(); }
}

/// Per-session handle: in a project, `ns` isolates the session's Python variables while DuckDB is shared.
#[derive(Clone)]
pub struct KernelClient { pub process: Arc<KernelProcess>, pub ns: Option<String> }

impl KernelClient {
    pub fn call(&self, op: &str, params: Value, timeout_secs: u64) -> Result<KernelResponse, String> {
        let mut p = match params { Value::Object(m) => m, Value::Null => Map::new(), other => { let mut m = Map::new(); m.insert("value".into(), other); m } };
        if let Some(ns) = &self.ns { p.insert("ns".into(), json!(ns)); }
        self.process.call(op, p, Duration::from_secs(timeout_secs))
    }
    pub fn call_cancellable(&self, op: &str, params: Value, timeout_secs: u64, cancel: &AtomicBool) -> Result<KernelResponse, String> {
        let mut p = match params { Value::Object(m) => m, Value::Null => Map::new(), other => { let mut m = Map::new(); m.insert("value".into(), other); m } };
        if let Some(ns) = &self.ns { p.insert("ns".into(), json!(ns)); }
        self.process.call_with_cancel(op, p, Duration::from_secs(timeout_secs), Some(cancel))
    }
    pub fn workspace(&self) -> &Path { &self.process.workspace }
    pub fn interrupt(&self) { self.process.cancel_ns(self.ns.as_deref()) }
}
