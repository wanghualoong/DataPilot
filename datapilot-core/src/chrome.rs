//! Headless Chrome over the DevTools Protocol (port of Swift `ChromeLocator` / `ChromeCDP`).
//! Minimal WebSocket client on std `TcpStream` (DevTools listens on 127.0.0.1 only: no TLS, no proxy).
//! Headless Chrome (web search / rendering): profile `AppSupport/chrome-profile`, started lazily, stopped after 10 idle
//! minutes. Visible Chrome (browser tool): profile `AppSupport/browser-profile`, runs until the app quits.
use base64::Engine as _;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(not(windows))]
pub const CANDIDATES: &[&str] = &["/Applications/Google Chrome.app", "~/Applications/Google Chrome.app", "/Applications/Chromium.app",
                                  "/Applications/Microsoft Edge.app", "/Applications/Google Chrome Beta.app"];
/// Windows: install folders (`%VAR%` expanded) holding chrome.exe / msedge.exe.
#[cfg(windows)]
pub const CANDIDATES: &[&str] = &[r"%ProgramFiles%\Google\Chrome\Application", r"%ProgramFiles(x86)%\Google\Chrome\Application",
                                  r"%LocalAppData%\Google\Chrome\Application", r"%ProgramFiles(x86)%\Microsoft\Edge\Application",
                                  r"%ProgramFiles%\Microsoft\Edge\Application"];
const IDLE: Duration = Duration::from_secs(600);

#[cfg(not(windows))]
fn plist_get(app: &Path, key: &str) -> Option<String> {
    let out = Command::new("/usr/bin/plutil").args(["-extract", key, "raw"]).arg(app.join("Contents/Info.plist")).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(unix)]
pub fn executable(app: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let u = app.join("Contents/MacOS").join(plist_get(app, "CFBundleExecutable")?);
    let m = std::fs::metadata(&u).ok()?;
    if m.is_file() && m.permissions().mode() & 0o111 != 0 { Some(u) } else { None }
}
/// Windows: `app` is the install folder (…\Application or a CfT unzip dir) holding chrome.exe / msedge.exe.
#[cfg(not(unix))]
pub fn executable(app: &Path) -> Option<PathBuf> {
    if app.is_file() { return Some(app.to_path_buf()); }
    ["chrome.exe", "msedge.exe"].iter().map(|n| app.join(n)).find(|p| p.is_file())
}

fn expand(c: &str) -> PathBuf {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let mut out = c.replace('~', &home);
    while let Some(a) = out.find('%') {
        let Some(len) = out[a + 1..].find('%') else { break };
        let var = &out[a + 1..a + 1 + len];
        let val = std::env::var(var).unwrap_or_default();
        out = format!("{}{}{}", &out[..a], val, &out[a + 2 + len..]);
    }
    PathBuf::from(out)
}

/// System browsers first, then the app-managed Chrome for Testing (`AppSupport/chrome`).
pub fn find(app_support: &Path) -> Option<PathBuf> {
    let mut apps: Vec<PathBuf> = CANDIDATES.iter().map(|c| expand(c)).filter(|p| p.is_absolute()).collect();
    fn walk(d: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 4 { return; }
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            let bundle = if cfg!(windows) { p.join("chrome.exe").is_file() } else { p.extension().map(|x| x == "app").unwrap_or(false) };
            if bundle { out.push(p) } else if p.is_dir() { walk(&p, depth + 1, out) }
        }
    }
    walk(&app_support.join("chrome"), 0, &mut apps);
    apps.iter().find_map(|a| executable(a))
}

pub fn install_dir(app_support: &Path) -> PathBuf { app_support.join("chrome") }

pub const CFT_INDEX: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";

fn platform() -> &'static str {
    if cfg!(windows) { "win64" } else if cfg!(target_arch = "aarch64") { "mac-arm64" } else { "mac-x64" }
}

/// Only Google's official Chrome for Testing bucket over HTTPS is accepted.
pub fn official_url(u: &str) -> bool { u.starts_with("https://storage.googleapis.com/chrome-for-testing-public/") && !u.contains("..") }

/// Stable download URL + version from the CfT index JSON.
pub fn pick_download(index: &Value) -> Option<(String, String)> {
    let st = &index["channels"]["Stable"];
    let ver = st["version"].as_str()?.to_string();
    let url = st["downloads"]["chrome"].as_array()?.iter().find(|d| d["platform"] == platform())?["url"].as_str()?.to_string();
    Some((url, ver))
}

#[cfg(not(windows))]
/// First *.app bundle (depth ≤ 3) inside an unpacked archive.
fn find_app(d: &Path, depth: usize) -> Option<PathBuf> {
    if depth > 3 { return None; }
    let mut es: Vec<PathBuf> = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| !n.starts_with('.') && n != "__MACOSX").unwrap_or(false)).collect();
    es.sort();
    if let Some(a) = es.iter().find(|p| p.extension().map(|x| x == "app").unwrap_or(false)) { return Some(a.clone()); }
    es.iter().filter(|p| p.is_dir()).find_map(|p| find_app(p, depth + 1))
}

#[cfg(not(windows))]
fn unpack_and_check(zip: &Path, unpack: &Path) -> Result<PathBuf, String> {
    let ok = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(zip).arg(unpack).status().map(|s| s.success()).unwrap_or(false);
    if !ok { return Err("解压失败".into()); }
    let _ = Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(unpack).status();
    let app = find_app(unpack, 0).ok_or("压缩包中没有找到 Chrome 应用")?;
    // CfT is ad-hoc signed (codesign --verify cannot pass); integrity rests on the official HTTPS source + bundle id.
    if plist_get(&app, "CFBundleIdentifier").as_deref() != Some("com.google.chrome.for.testing") {
        return Err("压缩包内容不是 Chrome for Testing（bundle id 不符），已放弃安装。".into());
    }
    Ok(app)
}

/// Windows: unzip with the zip crate (entries escaping the target are rejected), expect `chrome-win64\chrome.exe`.
#[cfg(windows)]
fn unpack_and_check(zip: &Path, unpack: &Path) -> Result<PathBuf, String> {
    let f = std::fs::File::open(zip).map_err(|e| e.to_string())?;
    let mut a = ::zip::ZipArchive::new(f).map_err(|e| format!("解压失败：{e}"))?;
    a.extract(unpack).map_err(|e| format!("解压失败：{e}"))?;
    let app = unpack.join("chrome-win64");
    if !app.join("chrome.exe").is_file() { return Err("压缩包内容不是 Chrome for Testing（缺少 chrome-win64\\chrome.exe），已放弃安装。".into()); }
    Ok(app)
}

/// Download the stable Chrome for Testing build (official index → official bucket only), unpack into AppSupport/chrome,
/// check the bundle id. Same layout as the Swift installer. `agent` carries the user's proxy settings.
pub fn install(app_support: &Path, agent: &ureq::Agent, progress: &dyn Fn(&str)) -> Result<PathBuf, String> {
    progress("读取 Chrome for Testing 版本索引…");
    let body = agent.get(CFT_INDEX).call().map_err(|e| format!("读取版本索引失败：{e}"))?
        .into_string().map_err(|e| format!("读取版本索引失败：{e}"))?;
    let idx: Value = serde_json::from_str(&body).map_err(|e| format!("版本索引格式错误：{e}"))?;
    let (url, ver) = pick_download(&idx).ok_or("无法解析 Chrome for Testing 下载索引")?;
    if !official_url(&url) { return Err(format!("下载地址不是 Google 官方地址，已放弃安装：{url}")); }
    let tmp = app_support.join(format!("chrome-download-{}", crate::text::short_id(6)));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let _c = Cleanup(tmp.clone());
    let zip = tmp.join("chrome.zip");
    progress(&format!("下载 Chrome for Testing {ver}…"));
    let resp = agent.get(&url).call().map_err(|e| format!("下载失败：{e}"))?;
    let total: f64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let mut r = resp.into_reader();
    let mut f = std::fs::File::create(&zip).map_err(|e| e.to_string())?;
    let (mut done, mut last, mut buf) = (0f64, Instant::now(), vec![0u8; 1 << 16]);
    loop {
        let n = r.read(&mut buf).map_err(|e| format!("下载中断：{e}"))?;
        if n == 0 { break; }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as f64;
        if last.elapsed() > Duration::from_secs(1) {
            last = Instant::now();
            let mb = done / 1_048_576.0;
            progress(&if total > 0.0 { format!("下载 Chrome {mb:.0} / {:.0} MB", total / 1_048_576.0) } else { format!("下载 Chrome {mb:.0} MB") });
        }
    }
    drop(f);
    if total > 0.0 && (done - total).abs() > 0.5 { return Err(format!("下载不完整（{done:.0} / {total:.0} 字节）")); }
    progress("解压并检查…");
    let unpack = tmp.join("x");
    let app = unpack_and_check(&zip, &unpack)?;
    let dir = install_dir(app_support);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(app.file_name().unwrap());
    std::fs::rename(&app, &dest).map_err(|e| format!("安装失败：{e}"))?;
    let exe = executable(&dest).ok_or("安装后找不到 Chrome 可执行文件")?;
    progress(&format!("Chrome for Testing {ver} 已安装"));
    Ok(exe)
}

#[cfg(not(windows))]
pub fn major_version(exe: &Path) -> String {
    let app = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).unwrap_or(exe);
    plist_get(app, "CFBundleShortVersionString").and_then(|v| v.split('.').next().map(String::from)).unwrap_or_else(|| "142".into())
}
/// Windows: the install folder holds a `142.0.7444.60\` directory (Chrome / Edge) or `142.0.7444.60.manifest` (CfT).
#[cfg(windows)]
pub fn major_version(exe: &Path) -> String {
    let dir = exe.parent().unwrap_or(exe);
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|n| n.matches('.').count() >= 3 && n.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
        .filter_map(|n| n.split('.').next().and_then(|m| m.parse::<u32>().ok())).max()
        .map(|m| m.to_string()).unwrap_or_else(|| "142".into())
}

pub fn user_agent(exe: &Path) -> String {
    let os = if cfg!(windows) { "Windows NT 10.0; Win64; x64" } else { "Macintosh; Intel Mac OS X 10_15_7" };
    format!("Mozilla/5.0 ({os}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36", major_version(exe))
}

/// Chrome proxy flags from the app settings: none → direct, custom → host:port, system → follow the OS settings (incl. PAC).
pub fn proxy_args(s: &crate::types::Settings) -> Vec<String> {
    match s.proxy_mode.as_str() {
        "none" => vec!["--no-proxy-server".into()],
        "custom" if !s.proxy_host.is_empty() => {
            let h = s.proxy_host.trim_start_matches("http://").trim_start_matches("https://");
            vec![format!("--proxy-server={}", if h.contains(':') || s.proxy_port == 0 { h.to_string() } else { format!("{h}:{}", s.proxy_port) })]
        }
        _ => vec![],
    }
}

fn random_bytes(n: usize) -> Vec<u8> {
    let hex = crate::text::short_id(n * 2);
    (0..n).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0)).collect()
}

// ---------- WebSocket (RFC 6455, client side, text frames) ----------
pub struct Ws { s: TcpStream }

impl Ws {
    pub fn connect(url: &str, timeout: Duration) -> Result<Ws, String> {
        let rest = url.strip_prefix("ws://").ok_or_else(|| format!("not a ws:// url: {url}"))?;
        let (hostport, path) = match rest.split_once('/') { Some((h, p)) => (h, format!("/{p}")), None => (rest, "/".into()) };
        let mut s = TcpStream::connect(hostport).map_err(|e| format!("CDP connect: {e}"))?;
        s.set_read_timeout(Some(timeout)).ok();
        s.set_nodelay(true).ok();
        let key = base64::engine::general_purpose::STANDARD.encode(random_bytes(16));
        let req = format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n");
        s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        // read the response head byte by byte so no frame bytes are consumed
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut b).map_err(|e| format!("CDP handshake: {e}"))?;
            head.push(b[0]);
            if head.len() > 16_384 { return Err("CDP handshake: header too long".into()); }
        }
        let h = String::from_utf8_lossy(&head);
        if !h.starts_with("HTTP/1.1 101") { return Err(format!("CDP handshake refused: {}", h.lines().next().unwrap_or(""))); }
        Ok(Ws { s })
    }

    fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        let mut f = vec![0x80 | opcode];
        let n = payload.len();
        if n < 126 { f.push(0x80 | n as u8) } else if n < 65_536 { f.push(0x80 | 126); f.extend((n as u16).to_be_bytes()) }
        else { f.push(0x80 | 127); f.extend((n as u64).to_be_bytes()) }
        let mask = random_bytes(4);
        f.extend(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.s.write_all(&f).map_err(|e| format!("CDP write: {e}"))
    }

    pub fn send_text(&mut self, t: &str) -> Result<(), String> { self.write_frame(0x1, t.as_bytes()) }

    fn read_frame(&mut self) -> std::io::Result<(bool, u8, Vec<u8>)> {
        let mut h = [0u8; 2];
        self.s.read_exact(&mut h)?;
        let (fin, op, masked) = (h[0] & 0x80 != 0, h[0] & 0x0f, h[1] & 0x80 != 0);
        let mut n = (h[1] & 0x7f) as u64;
        if n == 126 { let mut e = [0u8; 2]; self.s.read_exact(&mut e)?; n = u16::from_be_bytes(e) as u64; }
        else if n == 127 { let mut e = [0u8; 8]; self.s.read_exact(&mut e)?; n = u64::from_be_bytes(e); }
        if n > 256 << 20 { return Err(std::io::Error::other("frame too large")); }
        let mut mask = [0u8; 4];
        if masked { self.s.read_exact(&mut mask)?; }
        let mut p = vec![0u8; n as usize];
        self.s.read_exact(&mut p)?;
        if masked { for (i, b) in p.iter_mut().enumerate() { *b ^= mask[i % 4]; } }
        Ok((fin, op, p))
    }

    /// Next complete text message (pings answered, fragments joined).
    pub fn recv_text(&mut self, timeout: Duration) -> Result<String, String> {
        self.s.set_read_timeout(Some(timeout.max(Duration::from_millis(10)))).ok();
        let mut buf = Vec::new();
        loop {
            let (fin, op, p) = self.read_frame().map_err(|e| format!("CDP read: {e}"))?;
            match op {
                0x9 => { self.write_frame(0xA, &p)?; continue; }
                0xA => continue,
                0x8 => return Err("CDP connection closed".into()),
                _ => buf.extend(p),
            }
            if fin { return Ok(String::from_utf8_lossy(&buf).into_owned()); }
        }
    }
}

// ---------- one tab ----------
/// A JavaScript dialog (alert / confirm / prompt / beforeunload) opened while a command ran: the command is answered
/// with this error and the dialog kept in `Page::dialog` until handled (Page.handleJavaScriptDialog).
pub const DIALOG_OPEN: &str = "DIALOG_OPEN";

pub struct Page { ws: Ws, next: u64, port: u16, target: String, close_on_drop: bool, pub dialog: Option<Value> }

impl Page {
    pub fn port(&self) -> u16 { self.port }
    /// Page events on (needed to notice JavaScript dialogs).
    pub fn enable_events(&mut self) -> Result<(), String> { self.send("Page.enable", json!({}), Duration::from_secs(10)).map(|_| ()) }
    pub fn target(&self) -> &str { &self.target }
    pub fn send(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.ws.send_text(&json!({"id": id, "method": method, "params": params}).to_string())?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.checked_duration_since(Instant::now()).ok_or_else(|| format!("CDP {method} timed out"))?;
            let t = self.ws.recv_text(left).map_err(|e| if e.contains("timed out") || e.contains("Resource temporarily") { format!("CDP {method} timed out") } else { e })?;
            let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
            if v["id"].as_u64() != Some(id) {   // events (dialogs only reported after `enable_events`)
                match v["method"].as_str() {
                    Some("Page.javascriptDialogOpening") => { self.dialog = Some(v["params"].clone()); return Err(DIALOG_OPEN.into()); }
                    Some("Page.javascriptDialogClosed") => self.dialog = None,
                    _ => {}
                }
                continue;
            }
            if let Some(e) = v.get("error") { return Err(e["message"].as_str().map(String::from).unwrap_or_else(|| e.to_string())); }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Runtime.evaluate → value (Promises awaited).
    pub fn eval(&mut self, js: &str, timeout: Duration) -> Result<Value, String> {
        let r = self.send("Runtime.evaluate", json!({"expression": js, "returnByValue": true, "awaitPromise": true}), timeout)?;
        if let Some(ex) = r.get("exceptionDetails") {
            let msg = ex["exception"]["description"].as_str().or(ex["text"].as_str()).unwrap_or("").to_string();
            return Err(format!("JS error: {msg}"));
        }
        Ok(r["result"]["value"].clone())
    }

    pub fn navigate_and_wait(&mut self, url: &str, ready_js: &str, timeout: Duration, poll: Duration) -> Result<bool, String> {
        self.send("Page.navigate", json!({"url": url}), timeout)?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            std::thread::sleep(poll);
            if let Ok(v) = self.eval(&format!("!!({ready_js})"), Duration::from_secs(5)) { if v == Value::Bool(true) { return Ok(true); } }
        }
        Ok(false)
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        if self.close_on_drop { let _ = direct().get(&format!("http://127.0.0.1:{}/json/close/{}", self.port, self.target)).call(); }
    }
}

fn direct() -> ureq::Agent { ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build() }

// ---------- the process ----------
struct Proc { child: Child, port: u16, exe: PathBuf, last: Instant }

static STATE: Mutex<Option<Proc>> = Mutex::new(None);
static WATCHER: OnceLock<()> = OnceLock::new();
/// Visible Chrome window (same profile) opened so the user can pass a Google captcha; headless can't start meanwhile.
static VERIFY: Mutex<Option<Child>> = Mutex::new(None);

/// The captcha window is still open.
pub fn verifying() -> bool {
    let mut g = VERIFY.lock().unwrap();
    match g.as_mut().map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => { *g = None; false }
        None => false,
    }
}

/// Close headless Chrome and open a normal window on the same profile at `url`, so the user can pass the captcha once
/// (cookies persist in the profile; later headless searches reuse them).
pub fn open_visible(url: &str, exe: &Path, app_support: &Path, proxy: &[String]) -> Result<(), String> {
    if verifying() { return Ok(()); }
    shutdown();
    let profile = profile_dir(app_support);
    std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
    let child = Command::new(exe)
        .args(["--no-first-run", "--no-default-browser-check", "--lang=zh-CN"])
        .arg(format!("--user-data-dir={}", profile.display())).args(proxy).arg(url)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("Chrome 启动失败：{e}"))?;
    *VERIFY.lock().unwrap() = Some(child);
    Ok(())
}

pub fn profile_dir(app_support: &Path) -> PathBuf { app_support.join("chrome-profile") }

fn read_port(f: &Path) -> Option<u16> { std::fs::read_to_string(f).ok().and_then(|s| s.lines().next().and_then(|l| l.trim().parse::<u16>().ok())) }

/// Start Chrome with a DevTools port on `profile` and wait for its DevToolsActivePort file.
fn spawn_debuggable(exe: &Path, profile: &Path, args: &[String]) -> Result<(Child, u16), String> {
    std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let port_file = profile.join("DevToolsActivePort");
    let _ = std::fs::remove_file(&port_file);
    let mut child = Command::new(exe).arg("--remote-debugging-port=0").arg(format!("--user-data-dir={}", profile.display())).args(args)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|e| format!("Chrome 启动失败：{e}"))?;
    for _ in 0..150 {
        if let Some(n) = read_port(&port_file) { return Ok((child, n)); }
        if let Ok(Some(_)) = child.try_wait() { break; }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill(); let _ = child.wait();
    Err("Chrome 未能启动调试端口 / Chrome did not expose a DevTools port（可能有另一个 Chrome 正在使用同一个配置目录）".into())
}

fn ensure_started(exe: &Path, app_support: &Path, proxy: &[String]) -> Result<u16, String> {
    let mut g = STATE.lock().unwrap();
    if let Some(p) = g.as_mut() {
        if p.exe == exe && matches!(p.child.try_wait(), Ok(None)) { p.last = Instant::now(); return Ok(p.port); }
    }
    if let Some(mut old) = g.take() { let _ = old.child.kill(); let _ = old.child.wait(); }
    if verifying() { return Err("Google 人机验证窗口仍打开，请先完成验证并关闭该 Chrome 窗口 / captcha window still open".into()); }
    let mut args: Vec<String> = ["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-blink-features=AutomationControlled",
                                 "--disable-background-networking", "--lang=zh-CN", "--window-size=1280,900"].map(String::from).to_vec();
    args.push(format!("--user-agent={}", user_agent(exe)));
    args.extend(proxy.iter().cloned());
    args.push("about:blank".into());
    let (child, n) = spawn_debuggable(exe, &profile_dir(app_support), &args)?;
    *g = Some(Proc { child, port: n, exe: exe.to_path_buf(), last: Instant::now() });
    WATCHER.get_or_init(|| { std::thread::spawn(idle_watch); });
    Ok(n)
}

fn idle_watch() {
    loop {
        std::thread::sleep(Duration::from_secs(30));
        let mut g = STATE.lock().unwrap();
        if g.as_ref().map(|p| p.last.elapsed() >= IDLE).unwrap_or(false) {
            if let Some(mut p) = g.take() { let _ = p.child.kill(); let _ = p.child.wait(); }
        }
    }
}

/// Live Chrome tests share the one process: run them one at a time.
#[cfg(test)]
pub static LIVE_TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn shutdown() {
    if let Some(mut p) = STATE.lock().unwrap().take() { let _ = p.child.kill(); let _ = p.child.wait(); }
}

/// App quit: also close a captcha window we opened.
pub fn shutdown_all() {
    shutdown();
    shutdown_visible();
    if let Some(mut c) = VERIFY.lock().unwrap().take() { let _ = c.kill(); let _ = c.wait(); }
}

pub fn touch() { if let Some(p) = STATE.lock().unwrap().as_mut() { p.last = Instant::now(); } }

pub fn open_tab(exe: &Path, app_support: &Path, proxy: &[String]) -> Result<Page, String> {
    let port = ensure_started(exe, app_support, proxy)?;
    new_page(port, true)
}

fn new_page(port: u16, close_on_drop: bool) -> Result<Page, String> {
    let t: Value = direct().put(&format!("http://127.0.0.1:{port}/json/new?about:blank")).call()
        .map_err(|e| format!("CDP: cannot open a tab: {e}"))?.into_string().ok()
        .and_then(|t| serde_json::from_str(&t).ok()).ok_or("CDP: bad /json/new response")?;
    let ws = t["webSocketDebuggerUrl"].as_str().ok_or("CDP: no webSocketDebuggerUrl")?;
    let target = t["id"].as_str().unwrap_or("").to_string();
    Ok(Page { ws: Ws::connect(ws, Duration::from_secs(20))?, next: 0, port, target, close_on_drop, dialog: None })
}

// ---------- visible Chrome (browser tool, Codex-style) ----------
/// The browser tool's own Chrome: a normal window the user watches and logs in to, on a dedicated profile
/// (`AppSupport/browser-profile`, separate from the everyday Chrome — Chrome 136+ refuses DevTools on the default profile
/// anyway). Runs until the app quits or the user closes it; never stopped for idleness (a login's session cookies would
/// be lost). `child` is None for a window adopted from an earlier core process.
struct Visible { child: Option<Child>, port: u16 }
static VISIBLE: Mutex<Option<Visible>> = Mutex::new(None);

/// Close the visible Chrome (tests; the app closes it via `shutdown_all`).
pub fn shutdown_visible() {
    *BROWSER_WS.lock().unwrap() = None;
    if let Some(mut v) = VISIBLE.lock().unwrap().take() {
        match v.child.as_mut() { Some(c) => { let _ = c.kill(); let _ = c.wait(); } None => { let _ = browser_call(v.port, "Browser.close", json!({})); } }
    }
}

pub fn browser_profile_dir(app_support: &Path) -> PathBuf { app_support.join("browser-profile") }

fn responds(port: u16) -> bool { direct().get(&format!("http://127.0.0.1:{port}/json/version")).call().is_ok() }

/// DevTools port of the visible Chrome, starting it when needed.
pub fn ensure_visible(exe: &Path, app_support: &Path, proxy: &[String]) -> Result<u16, String> {
    let mut g = VISIBLE.lock().unwrap();
    if let Some(v) = g.as_mut() {
        let running = v.child.as_mut().map(|c| matches!(c.try_wait(), Ok(None))).unwrap_or(true);
        if running && responds(v.port) { return Ok(v.port); }
    }
    if let Some(mut old) = g.take() { if let Some(c) = old.child.as_mut() { let _ = c.kill(); let _ = c.wait(); } }
    let profile = browser_profile_dir(app_support);
    // a window left by an earlier core process (restart / crash) still owns the profile: adopt it, logins included
    if let Some(port) = read_port(&profile.join("DevToolsActivePort")).filter(|p| responds(*p)) {
        *g = Some(Visible { child: None, port });
        return Ok(port);
    }
    // no --disable-blink-features=AutomationControlled here: a visible Chrome shows an "unsupported command-line flag"
    // warning bar for it (only the headless one uses it)
    let mut args: Vec<String> = ["--no-first-run", "--no-default-browser-check", "--lang=zh-CN",
                                 "--window-size=1280,900", "--hide-crash-restore-bubble",
                                 // keep polling pages running while the window is behind others
                                 "--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding"]
        .map(String::from).to_vec();
    args.extend(proxy.iter().cloned());
    args.push("about:blank".into());
    if cfg!(test) { args.insert(0, "--headless=new".into()); }   // live tests: no window on the developer's screen
    let (child, port) = spawn_debuggable(exe, &profile, &args)?;
    *g = Some(Visible { child: Some(child), port });
    Ok(port)
}

fn list_raw(port: u16) -> Result<Vec<Value>, String> {
    let t: Value = direct().get(&format!("http://127.0.0.1:{port}/json/list")).call().map_err(|e| format!("CDP: {e}"))?
        .into_string().ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("CDP: bad /json/list response")?;
    Ok(t.as_array().cloned().unwrap_or_default().into_iter()
        .filter(|t| t["type"] == "page" && !t["url"].as_str().unwrap_or("").starts_with("devtools://")).collect())
}

/// Page tabs of a Chrome: (id, title, url).
pub fn tabs(port: u16) -> Result<Vec<(String, String, String)>, String> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    Ok(list_raw(port)?.iter().map(|t| (s(&t["id"]), s(&t["title"]), s(&t["url"]))).collect())
}

/// A tab of the visible Chrome: a new one (None) or an existing target (tabs the page opened). Never closed on drop.
pub fn visible_tab(port: u16, target: Option<&str>) -> Result<Page, String> {
    let Some(id) = target else { return new_page(port, false) };
    let ws = list_raw(port)?.into_iter().find(|t| t["id"] == id).and_then(|t| t["webSocketDebuggerUrl"].as_str().map(String::from))
        .ok_or("该标签页已关闭 / tab is gone")?;
    Ok(Page { ws: Ws::connect(&ws, Duration::from_secs(20))?, next: 0, port, target: id.to_string(), close_on_drop: false, dialog: None })
}

fn browser_conn(port: u16) -> Result<Page, String> {
    let v: Value = direct().get(&format!("http://127.0.0.1:{port}/json/version")).call().map_err(|e| format!("CDP: {e}"))?
        .into_string().ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("CDP: bad /json/version response")?;
    let ws = v["webSocketDebuggerUrl"].as_str().ok_or("CDP: no browser webSocketDebuggerUrl")?;
    Ok(Page { ws: Ws::connect(ws, Duration::from_secs(10))?, next: 0, port, target: String::new(), close_on_drop: false, dialog: None })
}

/// Browser-wide CDP command on a short-lived connection (Browser.close…).
pub fn browser_call(port: u16, method: &str, params: Value) -> Result<Value, String> {
    browser_conn(port)?.send(method, params, Duration::from_secs(10))
}

/// Browser-level connection to the visible Chrome, kept open: Browser.setDownloadBehavior lasts only as long as the
/// connection that set it, and it covers every tab (also tabs the page opens, where report downloads often start).
static BROWSER_WS: Mutex<Option<Page>> = Mutex::new(None);

/// Downloads of the visible Chrome go to `dir` (re-sent before each action: the folder is per chat).
pub fn set_download_dir(port: u16, dir: &Path) -> Result<(), String> {
    let mut g = BROWSER_WS.lock().unwrap();
    let params = json!({"behavior": "allow", "downloadPath": dir.to_string_lossy(), "eventsEnabled": false});
    let mut last = String::new();
    for _ in 0..2 {
        if g.as_ref().map(|p| p.port != port).unwrap_or(true) { *g = Some(browser_conn(port)?); }
        match g.as_mut().unwrap().send("Browser.setDownloadBehavior", params.clone(), Duration::from_secs(10)) {
            Ok(_) => return Ok(()),
            Err(e) => { last = e; *g = None; }   // connection lost (Chrome restarted): reconnect once
        }
    }
    Err(last)
}

/// Opens `url`, waits until `ready_js` is truthy (or the timeout), returns (outerHTML, final URL).
pub fn render(url: &str, exe: &Path, app_support: &Path, proxy: &[String], ready_js: &str, timeout: Duration) -> Result<(String, String), String> {
    let mut p = open_tab(exe, app_support, proxy)?;
    p.navigate_and_wait(url, ready_js, timeout, Duration::from_millis(400))?;
    let html = p.eval("document.documentElement.outerHTML", Duration::from_secs(20))?.as_str().unwrap_or("").to_string();
    let fin = p.eval("location.href", Duration::from_secs(5))?.as_str().map(String::from).unwrap_or_else(|| url.to_string());
    touch();
    Ok((html, fin))
}

/// Opens `url`, waits for `ready_js` (error when it never becomes true), evaluates `script` (may return a Promise).
pub fn evaluate(url: &str, exe: &Path, app_support: &Path, proxy: &[String], ready_js: &str, script: &str, timeout: Duration) -> Result<Value, String> {
    let mut p = open_tab(exe, app_support, proxy)?;
    if !p.navigate_and_wait(url, ready_js, timeout, Duration::from_millis(200))? {
        return Err(format!("page not ready: {}", url.rsplit('/').next().unwrap_or(url)));
    }
    let v = p.eval(script, timeout)?;
    touch();
    Ok(v)
}

/// file:// URL with spaces and non-ASCII percent-encoded.
pub fn file_url(p: &Path) -> String {
    let s = p.display().to_string();
    let mut out = String::from("file://");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { out.push(b as char) } else { out += &format!("%{b:02X}") }
    }
    out
}

/// Query-string component encoding (RFC 3986 unreserved kept).
pub fn qenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { out.push(b as char) } else { out += &format!("%{b:02X}") }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cft_index_and_url_guard() {
        let plat = platform();
        let idx = json!({"channels": {"Stable": {"version": "142.0.1", "downloads": {"chrome": [
            {"platform": "linux64", "url": "https://storage.googleapis.com/chrome-for-testing-public/142.0.1/linux64/x.zip"},
            {"platform": plat, "url": format!("https://storage.googleapis.com/chrome-for-testing-public/142.0.1/{plat}/chrome-{plat}.zip")}]}}}});
        let (u, v) = pick_download(&idx).unwrap();
        assert_eq!(v, "142.0.1");
        assert!(u.contains(plat) && official_url(&u));
        assert!(!official_url("http://storage.googleapis.com/chrome-for-testing-public/x.zip"));
        assert!(!official_url("https://evil.example.com/chrome-for-testing-public/x.zip"));
        assert!(!official_url("https://storage.googleapis.com/other-bucket/x.zip"));
        assert!(!official_url("https://storage.googleapis.com/chrome-for-testing-public/../x.zip"));
    }
    #[test]
    fn encoders() {
        assert_eq!(qenc("销售 a&b"), "%E9%94%80%E5%94%AE%20a%26b");
        assert_eq!(file_url(Path::new("/a b/c.html")), "file:///a%20b/c.html");
    }

    /// Real headless Chrome (skipped when no Chrome is installed): render a data: URL and evaluate JS.
    #[test]
    fn live_render_and_eval() {
        let _g = LIVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let support = std::env::temp_dir().join(format!("dp-chrome-{}", crate::text::short_id(6)));
        let Some(exe) = find(&support) else { return };
        let (html, _) = render("data:text/html,<h1 id=t>hello</h1>", &exe, &support, &[], "document.getElementById('t')", Duration::from_secs(15)).unwrap();
        assert!(html.contains("hello"));
        let v = evaluate("data:text/html,<p>x</p>", &exe, &support, &[], "document.readyState === 'complete'",
                         "(async () => { await new Promise(r => setTimeout(r, 50)); return 6 * 7; })()", Duration::from_secs(15)).unwrap();
        assert_eq!(v, json!(42));
        shutdown();
        let _ = std::fs::remove_dir_all(&support);
    }
}
