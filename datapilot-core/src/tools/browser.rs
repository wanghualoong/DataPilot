//! `browser` tool (port of Swift `BrowserTool` / `BrowserController`): headless Chrome over CDP instead of the app's
//! WKWebView, so it works without the GUI shell. One tab per session, kept between calls; page scripts are the same
//! as Swift `BrowserJS` (snapshot with refs → click / type / select by ref). Page content is untrusted data.
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{sv, with_risk, imp, Impl, ToolCtx, ToolResult};
use crate::chrome::Page;
use crate::permissions::Risk;
use crate::text;

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("browser", with_risk(imp(browser, summary), risk));
}

fn risk(a: &Value) -> Risk {
    match a["action"].as_str().unwrap_or("") {
        "snapshot" | "scroll" | "back" | "extract_tables" | "screenshot" => Risk::ReadOnly,
        "type" | "select" => Risk::External, // may submit forms
        _ => Risk::Network,
    }
}

fn summary(a: &Value) -> String {
    match a["action"].as_str().unwrap_or("") {
        "navigate" => format!("浏览器打开 {}", sv(a, "url")),
        "type" => format!("浏览器输入 [{}] {}{}", sv(a, "ref"), sv(a, "text"), if a["submit"] == true { " ⏎" } else { "" }),
        "click" => format!("浏览器点击 [{}]", sv(a, "ref")),
        act => format!("浏览器 {act}"),
    }
}

pub const SNAPSHOT_JS: &str = r#"(() => {
  const vis = el => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const sel = 'a,button,input,select,textarea,[role=button],[role=link],[role=tab],[role=checkbox],[role=menuitem],[onclick],summary,h1,h2,h3,th,label';
  let n = 0; const lines = [];
  document.querySelectorAll('[data-dp-ref]').forEach(e => e.removeAttribute('data-dp-ref'));
  for (const el of document.querySelectorAll(sel)) {
    if (!vis(el)) continue;
    const tag = el.tagName.toLowerCase();
    const role = el.getAttribute('role') || ({a:'link',button:'button',input:(el.type||'text'),select:'combobox',textarea:'textbox',summary:'button'}[tag] || tag);
    let name = (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.title || el.alt || '').trim().replace(/\s+/g,' ').slice(0,80);
    if (!name && !['input','select','textarea'].includes(tag)) continue;
    const ref = 'e' + (++n); el.setAttribute('data-dp-ref', ref);
    let extra = '';
    if (tag === 'a' && el.href) extra = ' -> ' + el.href.slice(0,100);
    if (tag === 'select') extra = ' options: [' + [...el.options].slice(0,15).map(o => o.text.trim()).join(' | ') + ']';
    if (tag === 'input' && el.value) extra = ' value="' + String(el.value).slice(0,40) + '"';
    lines.push(`[${ref}] ${role} "${name}"${extra}`);
    if (n >= 300) { lines.push('…(truncated)'); break; }
  }
  const text = (document.body ? document.body.innerText : '').replace(/\n{3,}/g,'\n\n').slice(0, 4000);
  return `Page: ${document.title}\nURL: ${location.href}\nTables: ${document.querySelectorAll('table').length}\n\n## Interactive elements\n${lines.join('\n')}\n\n## Visible text (truncated)\n${text}`;
})()"#;

const TABLES_JS: &str = "[...document.querySelectorAll('table')].slice(0,10).map(t => [...t.rows].slice(0,5000).map(r => [...r.cells].map(c => c.innerText.trim().replace(/\\s+/g,' '))))";

pub fn act_js(r: &str, body: &str) -> String {
    let r = r.replace(['"', '\'', '\\', ']'], "");
    format!("(() => {{ const el = document.querySelector('[data-dp-ref=\"{r}\"]'); if (!el) return 'ERROR: ref not found, take a new snapshot'; el.scrollIntoView({{block:'center'}}); {body} }})()")
}

fn type_js(r: &str, t: &str, submit: bool) -> String {
    let t = serde_json::to_string(t).unwrap_or_else(|_| "\"\"".into());
    let sub = if submit { "el.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', code:'Enter', keyCode:13, bubbles:true})); if (el.form) { el.form.requestSubmit ? el.form.requestSubmit() : el.form.submit(); }" } else { "" };
    act_js(r, &format!("el.focus(); const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; \
        const setter = Object.getOwnPropertyDescriptor(proto, 'value'); if (setter && setter.set) setter.set.call(el, {t}); else el.value = {t}; \
        el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); {sub} return 'typed';"))
}

fn select_js(r: &str, v: &str) -> String {
    let v = serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
    act_js(r, &format!("const o=[...el.options].find(o=>o.value==={v}||o.text.trim()==={v}); if(!o) return 'option not found'; el.value=o.value; el.dispatchEvent(new Event('change',{{bubbles:true}})); return 'selected '+o.text;"))
}

pub fn domain_allowed(host: &str, allowed: &[String]) -> bool {
    allowed.is_empty() || allowed.iter().any(|d| { let d = d.trim().trim_start_matches("*.").to_lowercase(); !d.is_empty() && (host == d || host.ends_with(&format!(".{d}"))) })
}

fn csv(rows: &[Vec<String>]) -> String {
    rows.iter().map(|r| r.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n")
}

// ---------- per-session tabs ----------
struct Tab { page: Page, url: String }
static TABS: Mutex<Option<HashMap<String, Tab>>> = Mutex::new(None);

fn take(id: &str) -> Option<Tab> { TABS.lock().unwrap().as_mut().and_then(|m| m.remove(id)) }
fn put(id: &str, t: Tab) { TABS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.to_string(), t); }

/// Session closed: close its tab.
pub fn close(id: &str) { drop(take(id)); }

fn str_of(v: Value) -> String { match v { Value::String(s) => s, Value::Null => String::new(), o => o.to_string() } }

/// After an action: give the page time to load / render (same pacing as the Swift controller).
fn settle(p: &mut Page, timeout: Duration) {
    std::thread::sleep(Duration::from_millis(300));
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if p.eval("document.readyState", Duration::from_secs(3)).ok().and_then(|v| v.as_str().map(|s| s == "complete")).unwrap_or(false) { break; }
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(400));
}

/// The session's tab; a tab whose Chrome went away (idle shutdown) is reopened at its last URL.
fn tab(c: &ToolCtx) -> Result<Tab, String> {
    let old = take(&c.shared.id);
    let last_url = match old {
        Some(mut t) => {
            if t.page.eval("1", Duration::from_secs(3)).is_ok() { return Ok(t); }
            std::mem::take(&mut t.url)
        }
        None => String::new(),
    };
    let mut notes = vec![];
    let (exe, proxy) = super::web::chrome_or_install(c, &mut notes).ok_or_else(|| format!("浏览器不可用：{}", if notes.is_empty() { "未找到 Chrome".to_string() } else { notes.join("；") }))?;
    let mut page = crate::chrome::open_tab(&exe, &c.shared.paths.app_support, &proxy)?;
    if !last_url.is_empty() {
        page.send("Page.navigate", json!({"url": last_url}), Duration::from_secs(20))?;
        settle(&mut page, Duration::from_secs(20));
    }
    Ok(Tab { page, url: last_url })
}

fn browser(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let action = sv(a, "action");
    // validate before touching Chrome
    let nav_url = if action == "navigate" {
        let u = sv(a, "url");
        let u = if u.starts_with("http://") || u.starts_with("https://") { u } else { format!("https://{u}") };
        let Some(host) = crate::permissions::host(&json!({"url": u})) else { return Ok(ToolResult::err("invalid url")) };
        if !domain_allowed(&host.to_lowercase(), &c.shared.settings.browser_allowed_domains) {
            return Ok(ToolResult::err(format!("域名 {host} 不在允许列表中（设置 → 通用 → 浏览器允许域名）")));
        }
        Some(u)
    } else { None };
    if !["navigate", "snapshot", "click", "type", "select", "scroll", "back", "extract_tables", "screenshot"].contains(&action.as_str()) {
        return Ok(ToolResult::err(format!("unknown action '{action}'")));
    }
    let mut t = tab(c)?;
    let mut r = run(&action, nav_url.as_deref(), a, c, &mut t);
    if let Ok(v) = t.page.eval("location.href", Duration::from_secs(3)) { if let Some(u) = v.as_str() { t.url = u.to_string(); } }
    // the app has no live view of headless Chrome: after every action save a small screenshot for its browser panel
    if let Ok(res) = r.as_mut() {
        if !res.is_error {
            if let Some(path) = panel_shot(&mut t.page, c.workspace()) {
                let mut d = res.display.take().unwrap_or_else(|| json!({"kind": "browser"}));
                if d["kind"] == "browser" {
                    d["url"] = json!(t.url);
                    d["screenshot"] = json!(path);
                    d["ts"] = json!(crate::text::now());
                }
                res.display = Some(d);
            }
        }
    }
    put(&c.shared.id, t);
    crate::chrome::touch();
    r
}

/// `<workspace>/browser/latest.jpg` (overwritten each step; the panel shows only the latest page).
fn panel_shot(p: &mut Page, ws: &std::path::Path) -> Option<String> {
    let r = p.send("Page.captureScreenshot", json!({"format": "jpeg", "quality": 55}), Duration::from_secs(10)).ok()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(r["data"].as_str()?).ok()?;
    let dir = ws.join("browser");
    std::fs::create_dir_all(&dir).ok()?;
    let f = dir.join("latest.jpg");
    std::fs::write(&f, bytes).ok()?;
    Some(f.to_string_lossy().to_string())
}

fn run(action: &str, nav_url: Option<&str>, a: &Value, c: &ToolCtx, t: &mut Tab) -> Result<ToolResult, String> {
    let p = &mut t.page;
    let ev = |p: &mut Page, js: &str| -> Result<String, String> { p.eval(js, Duration::from_secs(20)).map(str_of) };
    let wrap = |s: String| format!("<untrusted_web_content>\n{}\n</untrusted_web_content>", text::truncate_middle(&s, 12000));
    let need_ref = || -> Option<ToolResult> { if sv(a, "ref").is_empty() { Some(ToolResult::err("ref is required (take a snapshot first)")) } else { None } };
    match action {
        "navigate" => {
            let u = nav_url.unwrap_or_default();
            (c.emit)(crate::events::tool_progress(&c.call_id, &format!("打开 {u}")));
            p.send("Page.navigate", json!({"url": u}), Duration::from_secs(30))?;
            settle(p, Duration::from_secs(20));
            let snap = ev(p, SNAPSHOT_JS)?;
            Ok(ToolResult::with(wrap(snap), json!({"kind": "browser", "url": u})))
        }
        "snapshot" => Ok(ToolResult::ok(wrap(ev(p, SNAPSHOT_JS)?))),
        "click" => {
            if let Some(e) = need_ref() { return Ok(e); }
            let r = ev(p, &act_js(&sv(a, "ref"), &format!("el.click(); return 'clicked {}';", sv(a, "ref").replace(['\'', '\\'], ""))))?;
            settle(p, Duration::from_secs(10));
            Ok(ToolResult::ok(format!("{r}\n{}", wrap(ev(p, SNAPSHOT_JS)?))))
        }
        "type" => {
            if let Some(e) = need_ref() { return Ok(e); }
            let submit = a["submit"] == true;
            let r = ev(p, &type_js(&sv(a, "ref"), &sv(a, "text"), submit))?;
            if submit { settle(p, Duration::from_secs(15)); }
            Ok(ToolResult::ok(format!("{r}\n{}", wrap(ev(p, SNAPSHOT_JS)?))))
        }
        "select" => {
            if let Some(e) = need_ref() { return Ok(e); }
            Ok(ToolResult::ok(ev(p, &select_js(&sv(a, "ref"), &sv(a, "value")))?))
        }
        "scroll" => {
            let dir = if sv(a, "direction") == "up" { -1 } else { 1 };
            let r = ev(p, &format!("(window.scrollBy(0, {dir} * window.innerHeight * 0.8), 'scrolled')"))?;
            Ok(ToolResult::ok(format!("{r}\n{}", wrap(ev(p, SNAPSHOT_JS)?))))
        }
        "back" => {
            let _ = ev(p, "(history.back(), 'back')");
            settle(p, Duration::from_secs(10));
            Ok(ToolResult::ok(wrap(ev(p, SNAPSHOT_JS)?)))
        }
        "extract_tables" => {
            let v = p.eval(TABLES_JS, Duration::from_secs(30))?;
            let tables: Vec<Vec<Vec<String>>> = serde_json::from_value(v).unwrap_or_default();
            let dir = c.workspace().join("uploads");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut saved = vec![];
            for (i, tb) in tables.iter().enumerate().filter(|(_, tb)| tb.len() >= 2) {
                let rel = format!("uploads/browser_table_{}_{}.csv", text::short_id(4), i + 1);
                std::fs::write(c.workspace().join(&rel), csv(tb)).map_err(|e| e.to_string())?;
                saved.push(format!("{rel}（{} 行）", tb.len()));
            }
            Ok(ToolResult::ok(if saved.is_empty() { "页面没有可提取的表格".to_string() } else { format!("已保存表格：\n{}\n用 data_import 导入分析。", saved.join("\n")) }))
        }
        "screenshot" => {
            let r = p.send("Page.captureScreenshot", json!({"format": "jpeg", "quality": 75}), Duration::from_secs(20))?;
            let data = r["data"].as_str().ok_or("screenshot failed")?;
            if base64::engine::general_purpose::STANDARD.decode(data).is_err() { return Err("screenshot failed".into()); }
            Ok(ToolResult { content: "截图已附上。".into(), images: Some(vec![crate::types::ImagePart { data_url: format!("data:image/jpeg;base64,{data}") }]), ..Default::default() })
        }
        _ => Ok(ToolResult::err("unknown action")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers() {
        assert!(domain_allowed("a.example.com", &["example.com".into()]));
        assert!(domain_allowed("example.com", &["*.example.com".into()]));
        assert!(!domain_allowed("badexample.com", &["example.com".into()]));
        assert!(domain_allowed("x.org", &[]));
        assert_eq!(risk(&json!({"action": "snapshot"})), Risk::ReadOnly);
        assert_eq!(risk(&json!({"action": "type"})), Risk::External);
        assert_eq!(risk(&json!({"action": "navigate"})), Risk::Network);
        assert!(act_js("e1\"]');alert(1)//", "").contains("e1);alert(1)//"));
        assert_eq!(csv(&[vec!["a\"b".into(), "c".into()]]), "\"a\"\"b\",\"c\"");
    }

    /// Real headless Chrome (skipped when none is installed): snapshot → type → click on a data: page.
    #[test]
    fn live_snapshot_and_act() {
        let _g = crate::chrome::LIVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let support = std::env::temp_dir().join(format!("dp-browser-{}", crate::text::short_id(6)));
        let Some(exe) = crate::chrome::find(&support) else { return };
        let mut p = crate::chrome::open_tab(&exe, &support, &[]).unwrap();
        let html = "data:text/html,<input id=q placeholder=搜索><button onclick=\"document.title='ok:'+q.value\">Go</button><table><tr><td>a</td></tr><tr><td>1</td></tr></table>";
        p.send("Page.navigate", json!({"url": html}), Duration::from_secs(15)).unwrap();
        settle(&mut p, Duration::from_secs(10));
        let snap = str_of(p.eval(SNAPSHOT_JS, Duration::from_secs(10)).unwrap());
        assert!(snap.contains("[e1]") && snap.contains("Tables: 1"), "{snap}");
        assert_eq!(str_of(p.eval(&type_js("e1", "销售", false), Duration::from_secs(10)).unwrap()), "typed");
        p.eval(&act_js("e2", "el.click(); return 'clicked';"), Duration::from_secs(10)).unwrap();
        assert_eq!(p.eval("document.title", Duration::from_secs(5)).unwrap(), json!("ok:销售"));
        let tables: Vec<Vec<Vec<String>>> = serde_json::from_value(p.eval(TABLES_JS, Duration::from_secs(10)).unwrap()).unwrap();
        assert_eq!(tables[0], vec![vec!["a".to_string()], vec!["1".to_string()]]);
        drop(p);
        crate::chrome::shutdown();
        let _ = std::fs::remove_dir_all(&support);
    }
}
