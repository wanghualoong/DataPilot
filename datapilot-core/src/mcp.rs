//! MCP client (port of Swift `MCPClient` / `MCPManager`): stdio (JSON lines) or streamable HTTP (JSON or SSE answer).
//! initialize → notifications/initialized → tools/list (paginated) → tools/call. Servers are user-configured programs
//! that run OUTSIDE the kernel sandbox, so every MCP tool is `external` risk (approval in ask mode); HTTP servers also
//! go through the network gate. Tools are exposed as `mcp__<server>__<tool>`; output is wrapped as untrusted.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::permissions::Risk;
use crate::tools::{Contract, Tool, ToolCtx, ToolResult};
use crate::types::{ImagePart, Settings};

/// Same keys as the Swift `MCPServerConfig` in config.json `settings.mcpServers`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerConfig {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: String,
    pub enabled: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig { id: String::new(), name: String::new(), transport: "stdio".into(), command: String::new(), args: vec![],
                       env: HashMap::new(), url: String::new(), enabled: true }
    }
}

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;

enum Transport {
    Stdio { child: Mutex<Child>, stdin: Arc<Mutex<ChildStdin>>, pending: Pending, alive: Arc<AtomicBool> },
    Http { agent: ureq::Agent, session: Mutex<Option<String>> },
}

pub struct Client { pub cfg: ServerConfig, t: Transport, next: AtomicU64, pub tools: Vec<Value> }

fn rpc_err(e: &Value) -> String { format!("MCP error: {}", e["message"].as_str().map(String::from).unwrap_or_else(|| e.to_string())) }

fn write_line(stdin: &Mutex<ChildStdin>, v: &Value) -> Result<(), String> {
    let mut s = stdin.lock().unwrap();
    s.write_all((v.to_string() + "\n").as_bytes()).and_then(|_| s.flush()).map_err(|e| format!("MCP write: {e}"))
}

fn is_local(url: &str) -> bool {
    let h = crate::permissions::host(&json!({"url": url})).unwrap_or_default();
    h == "localhost" || h.starts_with("127.") || h == "[::1]"
}

pub fn http_agent(url: &str, s: &Settings) -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let mut b = ureq::AgentBuilder::new().tls_connector(Arc::new(tls)).timeout_connect(Duration::from_secs(30)).timeout_read(Duration::from_secs(300));
    let proxy = if is_local(url) { None } else {
        match s.proxy_mode.as_str() {
            "custom" if !s.proxy_host.is_empty() && s.proxy_port > 0 => Some(format!("http://{}:{}", s.proxy_host, s.proxy_port)),
            "system" => crate::llm::system_proxy_for(url),
            _ => None,
        }
    };
    if let Some(p) = proxy { if let Ok(px) = ureq::Proxy::new(&p) { b = b.proxy(px); } }
    Ok(b.build())
}

/// Streamable HTTP answer: plain JSON, or an SSE stream whose `data:` line carries the JSON-RPC response.
pub fn parse_http_body(body: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(body.trim()) { return v; }
    body.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|v| v.get("id").is_some()).unwrap_or(Value::Null)
}

/// Command for a stdio MCP server. macOS: `/usr/bin/env` resolves the name with Homebrew dirs added to PATH.
/// Windows: `cmd /C` so `npx` / `uvx` (.cmd shims) resolve via PATHEXT; no console window.
#[cfg(not(windows))]
fn launcher(cfg: &ServerConfig) -> Command {
    let path = format!("{}:/opt/homebrew/bin:/usr/local/bin", std::env::var("PATH").unwrap_or_default());
    let mut c = Command::new("/usr/bin/env");
    c.arg(&cfg.command).args(&cfg.args).env("PATH", path);
    c
}
#[cfg(windows)]
fn launcher(cfg: &ServerConfig) -> Command {
    use std::os::windows::process::CommandExt;
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let mut c = Command::new(format!("{sys}\\System32\\cmd.exe"));
    // /S + one outer pair of quotes: cmd keeps the inner, individually quoted arguments intact
    let line = std::iter::once(&cfg.command).chain(cfg.args.iter()).map(|a| crate::sandbox::quote_arg(a)).collect::<Vec<_>>().join(" ");
    c.raw_arg("/D /S /C").raw_arg(format!("\"{line}\"")).creation_flags(0x0800_0000);
    c
}

impl Client {
    pub fn connect(cfg: &ServerConfig, settings: &Settings) -> Result<Client, String> {
        let t = if cfg.transport == "http" {
            if !(cfg.url.starts_with("http://") || cfg.url.starts_with("https://")) { return Err(format!("bad url '{}'", cfg.url)); }
            Transport::Http { agent: http_agent(&cfg.url, settings)?, session: Mutex::new(None) }
        } else {
            if cfg.command.trim().is_empty() { return Err("command is empty".into()); }
            let mut child = launcher(cfg).envs(&cfg.env)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .spawn().map_err(|e| format!("启动失败：{e}"))?;
            let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
            let stdout = child.stdout.take().ok_or("no stdout")?;
            let pending: Pending = Arc::default();
            let alive = Arc::new(AtomicBool::new(true));
            let (p, a, w) = (pending.clone(), alive.clone(), stdin.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if v.get("method").is_some() {
                        // server → client request: answer ping, refuse the rest (sampling / roots are not offered)
                        if let Some(id) = v.get("id").filter(|i| !i.is_null()) {
                            let r = if v["method"] == "ping" { json!({"jsonrpc": "2.0", "id": id, "result": {}}) }
                                    else { json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not supported by DataPilot"}}) };
                            let _ = write_line(&w, &r);
                        }
                        continue;
                    }
                    let Some(id) = v["id"].as_u64() else { continue };
                    if let Some(tx) = p.lock().unwrap().remove(&id) {
                        let _ = tx.send(match v.get("error") { Some(e) => Err(rpc_err(e)), None => Ok(v.get("result").cloned().unwrap_or(Value::Null)) });
                    }
                }
                a.store(false, Ordering::SeqCst);
                for (_, tx) in p.lock().unwrap().drain() { let _ = tx.send(Err("MCP server exited".into())); }
            });
            Transport::Stdio { child: Mutex::new(child), stdin, pending, alive }
        };
        let mut c = Client { cfg: cfg.clone(), t, next: AtomicU64::new(1), tools: vec![] };
        c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {},
                                       "clientInfo": {"name": "DataPilot", "version": crate::VERSION}}), Duration::from_secs(60), None)?;
        c.notify("notifications/initialized");
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let params = match &cursor { Some(cu) => json!({"cursor": cu}), None => json!({}) };
            let r = c.request("tools/list", params, Duration::from_secs(60), None)?;
            c.tools.extend(r["tools"].as_array().cloned().unwrap_or_default());
            cursor = r["nextCursor"].as_str().filter(|s| !s.is_empty()).map(String::from);
            if cursor.is_none() { break; }
        }
        Ok(c)
    }

    fn http(&self, msg: &Value) -> Result<Value, String> {
        let Transport::Http { agent, session } = &self.t else { return Err("not an HTTP server".into()) };
        let mut req = agent.post(&self.cfg.url).set("Content-Type", "application/json").set("Accept", "application/json, text/event-stream");
        if let Some(s) = session.lock().unwrap().clone() { req = req.set("Mcp-Session-Id", &s); }
        let resp = match req.send_string(&msg.to_string()) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => return Err(format!("MCP HTTP {code}: {}", crate::text::prefix(&r.into_string().unwrap_or_default(), 300))),
            Err(e) => return Err(format!("MCP HTTP: {e}")),
        };
        if let Some(s) = resp.header("Mcp-Session-Id") { *session.lock().unwrap() = Some(s.to_string()); }
        Ok(parse_http_body(&resp.into_string().map_err(|e| format!("MCP HTTP: {e}"))?))
    }

    fn notify(&self, method: &str) {
        let msg = json!({"jsonrpc": "2.0", "method": method});
        match &self.t {
            Transport::Stdio { stdin, .. } => { let _ = write_line(stdin, &msg); }
            Transport::Http { .. } => { let _ = self.http(&msg); }
        }
    }

    /// One request; stdio requests are abandoned (and `notifications/cancelled` sent) when `cancel` turns true.
    pub fn request(&self, method: &str, params: Value, timeout: Duration, cancel: Option<&AtomicBool>) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match &self.t {
            Transport::Http { .. } => {
                let r = self.http(&msg)?;
                if let Some(e) = r.get("error") { return Err(rpc_err(e)); }
                Ok(r.get("result").cloned().unwrap_or(Value::Null))
            }
            Transport::Stdio { stdin, pending, alive, .. } => {
                if !alive.load(Ordering::SeqCst) { return Err(format!("MCP server {} 已退出", self.cfg.name)); }
                let (tx, rx) = channel();
                pending.lock().unwrap().insert(id, tx);
                if let Err(e) = write_line(stdin, &msg) { pending.lock().unwrap().remove(&id); return Err(e); }
                let deadline = Instant::now() + timeout;
                loop {
                    match rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(r) => return r,
                        Err(RecvTimeoutError::Disconnected) => return Err("MCP server exited".into()),
                        Err(RecvTimeoutError::Timeout) => {
                            if cancel.map(|c| c.load(Ordering::SeqCst)).unwrap_or(false) {
                                pending.lock().unwrap().remove(&id);
                                let _ = write_line(stdin, &json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": "cancelled by user"}}));
                                return Err("cancelled".into());
                            }
                            if Instant::now() >= deadline { pending.lock().unwrap().remove(&id); return Err("MCP request timed out".into()); }
                        }
                    }
                }
            }
        }
    }

    /// (text, images, is_error)
    pub fn call_tool(&self, name: &str, args: &Value, cancel: Option<&AtomicBool>) -> Result<(String, Vec<ImagePart>, bool), String> {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}), Duration::from_secs(300), cancel)?;
        let (mut texts, mut imgs) = (vec![], vec![]);
        for c in r["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("text") => texts.push(c["text"].as_str().unwrap_or("").to_string()),
                Some("image") => if let Some(d) = c["data"].as_str() {
                    imgs.push(ImagePart { data_url: format!("data:{};base64,{d}", c["mimeType"].as_str().unwrap_or("image/png")) });
                },
                _ => texts.push(c.to_string()),
            }
        }
        if texts.is_empty() && !r["structuredContent"].is_null() { texts.push(r["structuredContent"].to_string()); }
        Ok((texts.join("\n"), imgs, r["isError"] == true))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Transport::Stdio { child, .. } = &self.t {
            let mut c = child.lock().unwrap();
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Link from an exposed tool to its server.
pub struct Remote { pub client: Arc<Client>, pub name: String }

pub fn call(r: &Remote, a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let (text, imgs, err) = r.client.call_tool(&r.name, a, Some(&c.shared.broker.cancel))?;
    Ok(ToolResult { content: format!("<untrusted_tool_output>\n{}\n</untrusted_tool_output>", crate::text::truncate_middle(&text, 40_000)),
                    is_error: err, images: if imgs.is_empty() { None } else { Some(imgs) }, ..Default::default() })
}

fn safe(s: &str) -> String { s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect() }

/// OpenAI-compatible tool name (`^[A-Za-z0-9_-]{1,64}$`).
pub fn exposed_name(server: &str, tool: &str) -> String { format!("mcp__{}__{}", safe(server), safe(tool)).chars().take(64).collect() }

// ---------- process-wide manager ----------
struct Entry { _client: Arc<Client>, tools: Vec<Arc<Tool>> }
static SERVERS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static STATUS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// (Re)connect the enabled servers (in parallel) and replace the exposed tools. Returns per-server status.
pub fn sync(cfgs: &[ServerConfig], settings: &Settings) -> BTreeMap<String, String> {
    drop(std::mem::take(&mut *SERVERS.lock().unwrap()));
    let results: Vec<(ServerConfig, Result<Client, String>)> = std::thread::scope(|s| {
        let hs: Vec<_> = cfgs.iter().filter(|c| c.enabled).map(|c| s.spawn(move || (c.clone(), Client::connect(c, settings)))).collect();
        hs.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    let (mut status, mut entries, mut seen) = (BTreeMap::new(), vec![], HashSet::new());
    for (cfg, r) in results {
        let label = if cfg.name.is_empty() { cfg.command.clone() } else { cfg.name.clone() };
        match r {
            Ok(c) => {
                let c = Arc::new(c);
                let tools: Vec<Arc<Tool>> = c.tools.iter().filter_map(|t| {
                    let n = t["name"].as_str()?;
                    let name = exposed_name(&cfg.name, n);
                    if !seen.insert(name.clone()) { return None; }
                    let mut schema = t.get("inputSchema").cloned().filter(|s| s.is_object()).unwrap_or_else(|| json!({"type": "object"}));
                    if schema.get("type").is_none() { schema["type"] = json!("object"); }
                    let desc = crate::text::prefix(t["description"].as_str().unwrap_or(n), 1000);
                    Some(Arc::new(Tool::remote(Contract { name, description: format!("[MCP {}] {desc}", cfg.name), parameters: schema,
                                                          risk: Risk::External, plan_mode: false, concurrency_safe: false },
                                               Remote { client: c.clone(), name: n.to_string() })))
                }).collect();
                status.insert(label, format!("✅ {} tools", tools.len()));
                entries.push(Entry { _client: c, tools });
            }
            Err(e) => { status.insert(label, format!("❌ {e}")); }
        }
    }
    *SERVERS.lock().unwrap() = entries;
    *STATUS.lock().unwrap() = status.clone();
    status
}

pub fn tools() -> Vec<Arc<Tool>> { SERVERS.lock().unwrap().iter().flat_map(|e| e.tools.iter().cloned()).collect() }
pub fn tool(name: &str) -> Option<Arc<Tool>> {
    if !name.starts_with("mcp__") { return None; }
    SERVERS.lock().unwrap().iter().flat_map(|e| e.tools.iter()).find(|t| t.name() == name).cloned()
}
pub fn status() -> BTreeMap<String, String> { STATUS.lock().unwrap().clone() }
pub fn shutdown() { drop(std::mem::take(&mut *SERVERS.lock().unwrap())); }

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const SERVER_PY: &str = r#"
import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if 'id' not in m or 'method' not in m: continue
    meth, p = m['method'], m.get('params') or {}
    r = None
    if meth == 'initialize': r = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "t", "version": "1"}}
    elif meth == 'tools/list':
        if p.get('cursor') == 'p2': r = {"tools": [{"name": "add", "description": "add", "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}, "required": ["a", "b"]}}]}
        else: r = {"tools": [{"name": "echo.text", "description": "echo"}], "nextCursor": "p2"}
    elif meth == 'tools/call':
        a, n = p.get('arguments') or {}, p['name']
        if n == 'add': r = {"content": [{"type": "text", "text": str(a['a'] + a['b'])}]}
        elif n == 'echo.text':
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": "srv1", "method": "ping"}) + "\n"); sys.stdout.flush()
            r = {"content": [{"type": "text", "text": a.get('s', '')}, {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}]}
        else: r = {"content": [{"type": "text", "text": "no such tool"}], "isError": True}
    out = {"jsonrpc": "2.0", "id": m['id']}
    if r is None: out["error"] = {"code": -32601, "message": "nope"}
    else: out["result"] = r
    sys.stdout.write(json.dumps(out) + "\n"); sys.stdout.flush()
"#;

    #[test]
    fn stdio_server() {
        let py = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("build/vendor/python/bin/python3");
        if !py.exists() { return; }
        let dir = std::env::temp_dir().join(format!("dpmcp-{}", crate::text::short_id(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.py");
        std::fs::write(&script, SERVER_PY).unwrap();
        let cfg = ServerConfig { name: "t s".into(), command: py.display().to_string(), args: vec!["-I".into(), script.display().to_string()], ..Default::default() };
        let c = Client::connect(&cfg, &Settings::default()).unwrap();
        assert_eq!(c.tools.len(), 2); // paginated
        assert_eq!(c.call_tool("add", &json!({"a": 2, "b": 3}), None).unwrap().0, "5");
        let (t, imgs, err) = c.call_tool("echo.text", &json!({"s": "你好"}), None).unwrap();
        assert_eq!((t.as_str(), imgs.len(), err), ("你好", 1, false));
        assert!(imgs[0].data_url.starts_with("data:image/png;base64,"));
        assert!(c.call_tool("nope", &json!({}), None).unwrap().2);
        drop(c);
        // manager: exposed names, risk, lookup
        let st = sync(&[cfg.clone()], &Settings::default());
        assert_eq!(st["t s"], "✅ 2 tools");
        let t = tool("mcp__t_s__echo_text").unwrap();
        assert_eq!(t.contract.risk, Risk::External);
        assert!(!t.needs_network(&json!({})));
        assert_eq!(t.contract.parameters["type"], "object");
        assert!(tool("mcp__t_s__add").is_some() && tool("data_import").is_none());
        shutdown();
        assert!(tools().is_empty());
        // a server that exits immediately fails fast
        let bad = ServerConfig { name: "bad".into(), command: "/bin/sh".into(), args: vec!["-c".into(), "exit 0".into()], ..Default::default() };
        let e = Client::connect(&bad, &Settings::default()).err().unwrap();
        assert!(e.contains("exited") || e.contains("退出") || e.contains("write"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn http_server_json_and_sse() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen_session = Arc::new(Mutex::new(vec![]));
        let ss = seen_session.clone();
        std::thread::spawn(move || {
            for s in l.incoming().take(5) {
                let mut s = s.unwrap();
                let mut head = vec![];
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") { s.read_exact(&mut b).unwrap(); head.push(b[0]); }
                let h = String::from_utf8_lossy(&head).to_lowercase();
                let len: usize = h.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                ss.lock().unwrap().push(h.contains("mcp-session-id: s1"));
                let mut body = vec![0u8; len];
                s.read_exact(&mut body).unwrap();
                let m: Value = serde_json::from_slice(&body).unwrap();
                let resp = match (m.get("id"), m["method"].as_str().unwrap_or("")) {
                    (None, _) => "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                    (Some(id), "tools/call") => {
                        let b = format!("event: message\ndata: {}\n\n", json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": "sse ok"}]}}));
                        format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len())
                    }
                    (Some(id), meth) => {
                        let r = if meth == "tools/list" { json!({"tools": [{"name": "q", "inputSchema": {"type": "object"}}]}) } else { json!({"protocolVersion": "2025-06-18"}) };
                        let b = json!({"jsonrpc": "2.0", "id": id, "result": r}).to_string();
                        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: s1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len())
                    }
                };
                s.write_all(resp.as_bytes()).unwrap();
            }
        });
        let cfg = ServerConfig { name: "h".into(), transport: "http".into(), url: format!("http://127.0.0.1:{port}/mcp"), ..Default::default() };
        let c = Client::connect(&cfg, &Settings::default()).unwrap();
        assert_eq!(c.tools.len(), 1);
        assert_eq!(c.call_tool("q", &json!({}), None).unwrap().0, "sse ok");
        let seen = seen_session.lock().unwrap().clone();
        assert_eq!(seen.first(), Some(&false));
        assert!(seen[1..].iter().all(|x| *x), "{seen:?}"); // session id echoed after initialize
        assert_eq!(exposed_name("my server", "a.b/c"), "mcp__my_server__a_b_c");
        assert_eq!(parse_http_body("data: {\"id\":1,\"result\":2}\n\n")["result"], 2);
    }
}
