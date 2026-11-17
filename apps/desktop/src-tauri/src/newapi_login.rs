//! New API gateway login window (port of the Swift `NewAPIAccount` web login). The gateway's own login page runs in a
//! separate window, so every login method it offers works (password, Duke NetID / OIDC, GitHub …). Once logged in, a
//! same-origin request from that page gets the dashboard access token (older versions: the session cookie + user id),
//! which is handed to the UI in memory only. The window is a remote origin: it has no IPC capability at all.
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const LABEL: &str = "newapi-login";
/// "我已登录，继续" in the UI: probe now even if the page did not look logged in.
pub static FORCE: AtomicBool = AtomicBool::new(false);

/// Persistent per-gateway cookie store, isolated from the app's own webview (the login survives restarts).
/// Reuses the SwiftUI app's store when it exists, so an existing login carries over.
#[cfg(target_os = "macos")]
fn store_id(host: &str) -> [u8; 16] {
    if let Ok(o) = std::process::Command::new("/usr/bin/defaults").args(["read", "io.github.datapilot.app", &format!("newapi.store.{host}")]).output() {
        let s: String = String::from_utf8_lossy(&o.stdout).chars().filter(|c| c.is_ascii_hexdigit()).collect();
        if o.status.success() && s.len() == 32 {
            let mut b = [0u8; 16];
            for (i, x) in b.iter_mut().enumerate() { *x = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap_or(0); }
            return b;
        }
    }
    use std::hash::{Hash, Hasher};
    let mut b = [0u8; 16];
    for (i, salt) in ["datapilot.newapi.a", "datapilot.newapi.b"].iter().enumerate() {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        salt.hash(&mut h); host.hash(&mut h);
        b[i * 8..i * 8 + 8].copy_from_slice(&h.finish().to_le_bytes());
    }
    b[6] = (b[6] & 0x0f) | 0x40; b[8] = (b[8] & 0x3f) | 0x80;
    b
}

/// Run a script in the login page and return its (JSON) value.
fn eval(w: &WebviewWindow, js: &str, timeout: Duration) -> Option<Value> {
    let (tx, rx) = mpsc::channel();
    w.eval_with_callback(js, move |s| { let _ = tx.send(s); }).ok()?;
    serde_json::from_str(&rx.recv_timeout(timeout).ok()?).ok()
}

fn alive(app: &AppHandle) -> bool { app.get_webview_window(LABEL).is_some() }

/// Logged in: move to a plain same-origin JSON page (so the console's own JS can't race us on refresh-token
/// rotation), then ask for a dashboard access token with the page's cookies.
fn probe(app: &AppHandle, w: &WebviewWindow, base: &Url) -> Result<Value, String> {
    let uid = eval(w, "(function(){try{var u=JSON.parse(localStorage.getItem('user')||'null');return u&&u.id||null}catch(e){return null}})()", Duration::from_secs(3))
        .and_then(|v| v.as_i64());
    let _ = app;
    let status = base.join("/api/status").map_err(|e| e.to_string())?;
    w.navigate(status).map_err(|e| e.to_string())?;
    let t0 = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(150));
        if !alive(app) { return Err("cancelled".into()); }
        let on = w.url().map(|u| u.path() == "/api/status").unwrap_or(false);
        if on && eval(w, "document.readyState", Duration::from_secs(2)).and_then(|v| v.as_str().map(|s| s == "complete")).unwrap_or(false) { break; }
        if t0.elapsed() > Duration::from_secs(15) { return Err("网关页面加载超时 / gateway page timed out".into()); }
    }
    // newer New API rotates the refresh cookie; a concurrent refresh answers 409 AUTH_REFRESH_RACE → retry shortly
    let mut r = Value::Null;
    for attempt in 0..4 {
        if attempt > 0 { std::thread::sleep(Duration::from_millis(400 * attempt)); }
        eval(w, "window.__dp=null;fetch('/api/user/auth/refresh',{method:'POST',credentials:'include',headers:{Accept:'application/json','Cache-Control':'no-cache'}})\
                 .then(function(r){return r.text().then(function(t){window.__dp={code:r.status,text:t}})})\
                 .catch(function(e){window.__dp={code:0,text:String(e)}});0", Duration::from_secs(3));
        let t0 = Instant::now();
        r = loop {
            std::thread::sleep(Duration::from_millis(150));
            if !alive(app) { return Err("cancelled".into()); }
            if let Some(s) = eval(w, "window.__dp?JSON.stringify(window.__dp):''", Duration::from_secs(2)).and_then(|v| v.as_str().map(String::from)) {
                if !s.is_empty() { break serde_json::from_str::<Value>(&s).unwrap_or(Value::Null); }
            }
            if t0.elapsed() > Duration::from_secs(20) { return Err("读取登录状态超时 / timed out".into()); }
        };
        if r["code"].as_i64() != Some(409) { break; }
    }
    let code = r["code"].as_i64().unwrap_or(0);
    let j: Value = serde_json::from_str(r["text"].as_str().unwrap_or("")).unwrap_or(Value::Null);
    if code == 404 || code == 405 {   // pre-rc New API: cookie session + New-Api-User header
        let cookie = w.cookies_for_url(base.clone()).unwrap_or_default().iter()
            .map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
        return Ok(json!({"user_id": uid, "cookie": cookie}));
    }
    if let Some(t) = j["data"]["access_token"].as_str().filter(|t| !t.is_empty()) {
        // send the session cookies too (incl. HttpOnly), like the console does: some gateways check both
        let cookie = w.cookies_for_url(base.clone()).unwrap_or_default().iter()
            .map(|c| format!("{}={}", c.name(), c.value())).collect::<Vec<_>>().join("; ");
        return Ok(json!({"access_token": t, "user_id": j["data"]["user"]["id"].as_i64().or(uid), "cookie": cookie}));
    }
    Err(j["message"].as_str().filter(|m| !m.is_empty()).map(String::from)
        .unwrap_or_else(|| "尚未登录或登录已过期，请重新登录。/ Not logged in or session expired — please log in again.".into()))
}

/// Opens the login window and blocks until the user is logged in (→ auth for the core's `newapi/*` calls),
/// closes the window (→ "cancelled"), or 15 minutes pass. `fresh` clears the gateway's cookies first (switch account).
pub fn login(app: &AppHandle, origin: &str, fresh: bool, notify: impl Fn(&str)) -> Result<Value, String> {
    let base = Url::parse(origin.trim()).map_err(|e| format!("无效的网关地址：{e}"))?;
    if !matches!(base.scheme(), "https" | "http") || base.host_str().is_none() { return Err("无效的网关地址".into()); }
    let host = base.host_str().unwrap_or_default().to_string();
    let login_url = base.join("/login").map_err(|e| e.to_string())?;
    if let Some(w) = app.get_webview_window(LABEL) { let _ = w.destroy(); std::thread::sleep(Duration::from_millis(200)); }
    let start = if fresh { Url::parse("about:blank").unwrap() } else { login_url.clone() };
    #[allow(unused_mut)]
    let mut b = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(start))
        .title(format!("登录 New API 网关 — {host}"))
        .inner_size(920.0, 720.0)
        .center();
    #[cfg(target_os = "macos")]
    { b = b.data_store_identifier(store_id(&host)); }
    let w = b.build().map_err(|e| e.to_string())?;
    let _ = w.set_focus();
    if fresh { let _ = w.clear_all_browsing_data(); let _ = w.navigate(login_url.clone()); }
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut cool = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(600));
        if !alive(app) { return Err("cancelled".into()); }
        if Instant::now() > deadline { let _ = w.destroy(); return Err("登录超时 / login timed out".into()); }
        let forced = FORCE.swap(false, Ordering::SeqCst);
        if Instant::now() < cool && !forced { continue; }
        let Ok(url) = w.url() else { continue };
        if url.host_str() != Some(host.as_str()) && !forced { continue; }   // e.g. the NetID / OIDC pages
        let p = url.path().to_string();
        let loginish = ["/login", "/sign-in", "/oauth", "/register", "/reset"].iter().any(|x| p.starts_with(x));
        // After a login the console goes to /console (older New API) or /dashboard · /keys (newer; it also sets the
        // JS-readable `new_api_has_session` cookie instead of localStorage.user). An existing session redirects there too.
        let marker = "(function(){try{return !!localStorage.getItem('user')||document.cookie.indexOf('new_api_has_session')>=0}catch(e){return false}})()";
        let logged = forced || ["/console", "/dashboard", "/keys"].iter().any(|x| p.starts_with(x))
            || (!loginish && eval(&w, marker, Duration::from_secs(2)).and_then(|v| v.as_bool()).unwrap_or(false));
        if !logged { continue; }
        notify("正在读取你的 Key… / Reading your keys…");
        match probe(app, &w, &base) {
            Ok(a) => { let _ = w.destroy(); return Ok(a); }
            Err(m) if m == "cancelled" => return Err(m),
            Err(m) => {
                notify(&format!("⚠️ {m}"));
                let _ = eval(&w, "try{localStorage.removeItem('user')}catch(e){};0", Duration::from_secs(2));
                let _ = w.navigate(login_url.clone());
                cool = Instant::now() + Duration::from_secs(4);
            }
        }
    }
}

pub fn cancel(app: &AppHandle) { if let Some(w) = app.get_webview_window(LABEL) { let _ = w.destroy(); } }
