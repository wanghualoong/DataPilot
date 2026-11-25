//! `browser` tool (Codex-style): DataPilot's own *visible* Chrome window (profile `AppSupport/browser-profile`,
//! separate from the everyday Chrome), driven over CDP. The user watches every step, logs in themselves (the tool never
//! types into password fields) and can take over at any time; logins persist in the profile.
//! - One current tab per session; tabs the page opens (window.open, target=_blank) are followed.
//! - Snapshot refs cover same-origin iframes / frames (ERP pages); clicks are real mouse events at the element centre
//!   (script click when something covers it); typing uses Input.insertText (works with React / Vue inputs).
//! - Long waits (`wait_for` with `reload_every` / `refresh_click`) run here, without model calls, until a text appears.
//! - JavaScript dialogs are reported and handled with `dialog`; downloads land in `<workspace>/downloads/`.
//! Page content is untrusted data.
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{sv, with_risk, imp, Impl, ToolCtx, ToolResult};
use crate::chrome::{Page, DIALOG_OPEN};
use crate::permissions::Risk;
use crate::text;

const ACTIONS: &[&str] = &["navigate", "snapshot", "click", "type", "select", "press", "hover", "scroll", "back", "wait_for", "tabs",
                           "switch_tab", "dialog", "downloads", "extract_tables", "screenshot"];
/// longest `wait_for` (seconds)
const MAX_WAIT: f64 = 6.0 * 3600.0;

fn secs(n: u64) -> Duration { Duration::from_secs(n) }

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("browser", with_risk(imp(browser, summary), risk));
}

fn risk(a: &Value) -> Risk {
    match a["action"].as_str().unwrap_or("") {
        "wait_for" if !sv(a, "refresh_click").is_empty() => Risk::Network,
        "snapshot" | "scroll" | "back" | "hover" | "wait_for" | "tabs" | "switch_tab" | "downloads" | "extract_tables" | "screenshot" => Risk::ReadOnly,
        "type" | "select" | "press" | "dialog" => Risk::External, // may submit forms
        _ => Risk::Network,
    }
}

fn summary(a: &Value) -> String {
    match a["action"].as_str().unwrap_or("") {
        "navigate" => format!("浏览器打开 {}", sv(a, "url")),
        "type" => format!("浏览器输入 [{}] {}{}", sv(a, "ref"), sv(a, "text"), if a["submit"] == true { " ⏎" } else { "" }),
        "click" if sv(a, "ref").is_empty() => format!("浏览器点击「{}」", sv(a, "text")),
        "click" => format!("浏览器点击 [{}]", sv(a, "ref")),
        "select" => format!("浏览器选择 [{}] {}", sv(a, "ref"), sv(a, "value")),
        "press" => format!("浏览器按键 {}", sv(a, "key")),
        "dialog" => format!("浏览器对话框：{}", if a["accept"] == false { "取消" } else { "确定" }),
        "wait_for" => format!("浏览器等待「{}」{}", sv(a, "text"), if a["gone"] == true { "消失" } else { "出现" }),
        "switch_tab" => format!("浏览器切换到标签页 {}", a["index"]),
        act => format!("浏览器 {act}"),
    }
}

/// `__vis(el)`: rendered and not hidden.
/// `__box(el)`: the element's box, or its first rendered descendant's when it has none itself (Oracle's "Search ..."
/// link is 0x0: its text sits in floated spans).
const BOX_JS: &str = "const __box = el => { const r = el.getBoundingClientRect(); if (r.width > 0 && r.height > 0) return r; for (const c of el.querySelectorAll('*')) { const q = c.getBoundingClientRect(); if (q.width > 0 && q.height > 0) return q; } return r; };";
const VIS_JS: &str = "const __box = el => { const r = el.getBoundingClientRect(); if (r.width > 0 && r.height > 0) return r; for (const c of el.querySelectorAll('*')) { const q = c.getBoundingClientRect(); if (q.width > 0 && q.height > 0) return q; } return r; }; const __vis = el => { const r = __box(el); const s = el.ownerDocument.defaultView.getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };";
const CLICKABLE: &str = "a,button,input[type=button],input[type=submit],[role=button],[role=link],[role=tab],[role=menuitem],[role=option],[onclick],[tabindex]:not([tabindex=\"-1\"]),label,summary";

/// Mark the visible clickable element whose text best matches `label` (exact, then prefix, then contained; shortest
/// text wins) as ref `dp-text` → "found" / "not found".
fn find_text_js(label: &str) -> String {
    let want = serde_json::to_string(label).unwrap_or_else(|_| "\"\"".into());
    format!("(() => {{ {DOCS_JS} {VIS_JS} const want = {want}.trim().replace(/\\s+/g,' '); let best = null, rank = 9, len = 1e9; \
             for (const d of __docs) {{ d.querySelectorAll('[data-dp-ref=\"dp-text\"]').forEach(e => e.removeAttribute('data-dp-ref')); \
               for (const el of d.querySelectorAll('{CLICKABLE}')) {{ if (!__vis(el)) continue; \
                 const n = (el.innerText || el.value || el.getAttribute('aria-label') || el.title || '').toString().trim().replace(/\\s+/g,' '); if (!n) continue; \
                 const r = n === want ? 0 : n.startsWith(want) ? 1 : n.includes(want) ? 2 : 9; \
                 if (r < rank || (r === rank && r < 9 && n.length < len)) {{ best = el; rank = r; len = n.length; }} }} }} \
             if (!best || rank === 9) return 'not found'; best.setAttribute('data-dp-ref', 'dp-text'); return 'found'; }})()")
}

/// Every same-origin document of the page (top + nested iframes / frames) as `__docs`.
const DOCS_JS: &str = "const __docs = []; (function w(d) { __docs.push(d); for (const f of d.querySelectorAll('iframe,frame')) { let x = null; try { x = f.contentDocument; } catch (e) {} if (x && x.documentElement) w(x); } })(document);";

pub const SNAPSHOT_JS: &str = r#"(() => {
  const base = 'a,button,input,select,textarea,[role=button],[role=link],[role=tab],[role=checkbox],[role=radio],[role=menuitem],[role=option],[role=treeitem],[role=combobox],[role=switch],[aria-haspopup],[contenteditable=true],[onclick],summary,h1,h2,h3,th,label';
  const sel = base + ',[tabindex]:not([tabindex="-1"])';
  // own box, else the first rendered descendant's (a link whose text sits in floated spans is 0x0 itself)
  const box = el => { const r = el.getBoundingClientRect(); if (r.width > 0 && r.height > 0) return r;
    for (const c of el.querySelectorAll('*')) { const q = c.getBoundingClientRect(); if (q.width > 0 && q.height > 0) return q; } return r; };
  const vis = el => { const r = box(el); const s = el.ownerDocument.defaultView.getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const LIST_MAX = 15;
  const last = id => (id || '').split(/[_:.\-\/]/).filter(Boolean).pop() || '';
  // icon-only buttons (Move / Remove arrows): image alt / title, else the last word of the id or image file name
  const nameOf = el => {
    let s = (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.title || el.alt || el.getAttribute('name') || '').toString().trim().replace(/\s+/g, ' ');
    if (!s) {
      const img = el.querySelector('img,svg,i');
      if (img) s = (img.getAttribute('alt') || img.getAttribute('title') || img.getAttribute('aria-label') || '').trim();
      if (!s && (img || el.id)) s = ('icon ' + (last(el.id) || (img && last(img.id)) || (img && (img.getAttribute('src') || '').split('/').pop().replace(/\.\w+$/, '')) || '')).trim();
      if (s === 'icon') s = '';
    }
    return s.slice(0, 80);
  };
  let n = 0; const lines = []; const texts = []; const lists = new Map();
  const walk = (doc, label) => {
    doc.querySelectorAll('[data-dp-ref]').forEach(e => e.removeAttribute('data-dp-ref'));
    if (label) lines.push('--- iframe ' + label + ' ---');
    for (const el of doc.querySelectorAll(sel)) {
      if (n >= 400) { lines.push('…(truncated)'); break; }
      if (!vis(el)) continue;
      // focusable containers (whole panels with tabindex) are not targets; small focusable items (search results) are
      if (!el.matches(base) && (el.querySelector(base) || (el.innerText || '').length > 200)) continue;
      // long lists (a dropdown with 1000 projects): the first LIST_MAX items, then one line, so entries after the list stay listed
      const unit = el.closest('li,tr,option,[role=option],[role=menuitem],[role=treeitem],[role=row]');
      if (unit && unit.parentElement) {
        let L = lists.get(unit.parentElement);
        if (!L) { L = { shown: new Set(), hidden: new Set(), at: -1 }; lists.set(unit.parentElement, L); }
        if (!L.shown.has(unit)) {
          if (L.shown.size >= LIST_MAX) { L.hidden.add(unit); if (L.at < 0) { L.at = lines.length; lines.push(''); } continue; }
          L.shown.add(unit);
        }
      }
      const tag = el.tagName.toLowerCase();
      const type = (el.getAttribute('type') || '').toLowerCase();
      const role = el.getAttribute('role') || ({a:'link',button:'button',input:(type||'text'),select:'combobox',textarea:'textbox',summary:'button'}[tag] || (el.matches(base) ? tag : 'item'));
      let name = nameOf(el);
      if (type === 'password') name = el.placeholder || el.getAttribute('aria-label') || 'password';
      if (!name && !['input','select','textarea'].includes(tag)) continue;
      const ref = 'e' + (++n); el.setAttribute('data-dp-ref', ref);
      let extra = '';
      if (tag === 'a' && el.href && !el.href.startsWith('javascript')) extra += ' -> ' + el.href.slice(0,100);
      if (tag === 'select') extra += ' options: [' + [...el.options].slice(0,20).map(o => (o.selected ? '*' : '') + o.text.trim()).join(' | ') + ']';
      if ((tag === 'input' || tag === 'textarea') && el.value && type !== 'password' && type !== 'checkbox' && type !== 'radio') extra += ' value="' + String(el.value).slice(0,40) + '"';
      if (type === 'checkbox' || type === 'radio') extra += el.checked ? ' [checked]' : ' [unchecked]';
      if (el.disabled || el.getAttribute('aria-disabled') === 'true') extra += ' [disabled]';
      if (el.getAttribute('aria-expanded')) extra += ' [expanded=' + el.getAttribute('aria-expanded') + ']';
      // short, generic names (Search / OK / Name …) repeat in every field's dialog: show the id saying which field it is
      if (name.length <= 14 && /^(a|button|input|select|textarea)$/.test(tag)) {
        const anc = el.id || ((el.parentElement && el.parentElement.closest('[id]')) || {}).id || '';
        if (anc && !/^\d+$/.test(anc)) extra += (el.id ? ' #' : ' in #') + anc.slice(-70);
      }
      lines.push(`[${ref}] ${role} "${name}"${extra}`);
    }
    if (doc.body) texts.push((label ? '[iframe ' + label + ']\n' : '') + doc.body.innerText);
    for (const f of doc.querySelectorAll('iframe,frame')) {
      let d = null; try { d = f.contentDocument; } catch (e) {}
      const lab = (f.name || f.id || f.getAttribute('src') || 'frame').slice(0, 80);
      if (d && d.documentElement) { if (vis(f)) walk(d, lab); }
      else if (f.src && vis(f)) lines.push('[iframe from another site — navigate to its URL to work in it] ' + f.src.slice(0, 160));
    }
  };
  walk(document, '');
  for (const L of lists.values()) if (L.at >= 0) lines[L.at] = `  …(${L.hidden.size} more items of this list not shown — type in its search / filter box or use its Search… entry, not scrolling)`;
  const text = texts.join('\n\n').replace(/\n{3,}/g,'\n\n').slice(0, 5000);
  return `Page: ${document.title}\nURL: ${location.href}\nTables: ${document.querySelectorAll('table').length}\n\n## Interactive elements\n${lines.join('\n')}\n\n## Visible text (truncated)\n${text}`;
})()"#;

fn tables_js() -> String {
    format!("(() => {{ {DOCS_JS} return __docs.flatMap(d => [...d.querySelectorAll('table')]).slice(0,10).map(t => [...t.rows].slice(0,5000).map(r => [...r.cells].map(c => c.innerText.trim().replace(/\\s+/g,' ')))); }})()")
}

/// Run `body` with `el` = the element of snapshot ref `r` (searched in every same-origin document), scrolled into view.
pub fn act_js(r: &str, body: &str) -> String {
    let r = r.replace(['"', '\'', '\\', ']'], "");
    format!("(() => {{ {DOCS_JS} {BOX_JS} let el = null; for (const d of __docs) {{ el = d.querySelector('[data-dp-ref=\"{r}\"]'); if (el) break; }} \
             if (!el) return 'ERROR: ref not found, take a new snapshot'; el.scrollIntoView({{block:'center', inline:'center'}}); {body} }})()")
}

/// Centre of `el` in top-level viewport coordinates (frame offsets added) and whether it is the topmost element there.
const POINT_BODY: &str = "const r = __box(el); const lx = r.left + r.width / 2, ly = r.top + r.height / 2; \
    const hit = el.ownerDocument.elementFromPoint(lx, ly); let x = lx, y = ly, w = el.ownerDocument.defaultView; \
    while (w.frameElement) { const f = w.frameElement, fr = f.getBoundingClientRect(); x += fr.left + f.clientLeft; y += fr.top + f.clientTop; w = w.parent; } \
    return {x, y, ok: !!hit && r.width > 0 && (hit === el || el.contains(hit))};";

const FOCUS_BODY: &str = "if (el.disabled || el.readOnly === true) return 'ERROR: field is disabled / read-only'; \
    if ((el.getAttribute('type') || '').toLowerCase() === 'password') return 'PASSWORD'; el.focus(); \
    if (el.select && /^(INPUT|TEXTAREA)$/.test(el.tagName)) el.select(); else if (el.isContentEditable) el.ownerDocument.execCommand('selectAll'); return 'focused';";

const VALUE_BODY: &str = "return el.isContentEditable ? el.innerText : String(el.value ?? '');";

/// Fallback typing: native value setter + input / change events (when Input.insertText did not reach the field).
fn type_js(r: &str, t: &str) -> String {
    let t = serde_json::to_string(t).unwrap_or_else(|_| "\"\"".into());
    act_js(r, &format!("el.focus(); const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; \
        const setter = Object.getOwnPropertyDescriptor(proto, 'value'); if (setter && setter.set) setter.set.call(el, {t}); else el.value = {t}; \
        el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); return 'typed';"))
}

fn select_js(r: &str, v: &str) -> String {
    let v = serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
    act_js(r, &format!("const o=[...el.options].find(o=>o.value==={v}||o.text.trim()==={v}); if(!o) return 'ERROR: option not found'; el.value=o.value; el.dispatchEvent(new Event('input',{{bubbles:true}})); el.dispatchEvent(new Event('change',{{bubbles:true}})); return 'selected '+o.text;"))
}

/// Does any same-origin document's text contain one of `terms`?
fn has_text_js(terms: &[String]) -> String {
    let want = serde_json::to_string(terms).unwrap_or_else(|_| "[]".into());
    format!("(() => {{ {DOCS_JS} const t = __docs.map(d => d.body ? d.body.innerText : '').join('\\n'); return {want}.some(w => t.includes(w)); }})()")
}

/// Click the button / link whose text is `label` (exact match preferred) — `wait_for(refresh_click=…)`.
fn click_text_js(label: &str) -> String {
    let want = serde_json::to_string(label).unwrap_or_else(|_| "\"\"".into());
    format!("(() => {{ {DOCS_JS} {VIS_JS} const want = {want}; const sel = 'a,button,input[type=button],input[type=submit],[role=button],[role=link],[role=tab],[role=menuitem]'; \
             const name = el => (el.getAttribute('aria-label') || el.innerText || el.value || el.title || '').trim().replace(/\\s+/g,' '); let best = null; \
             for (const d of __docs) for (const el of d.querySelectorAll(sel)) {{ if (!__vis(el)) continue; const n = name(el); if (n === want) best = el; else if (!best && n.includes(want)) best = el; }} \
             if (!best) return 'not found'; best.click(); return 'clicked'; }})()")
}

pub fn domain_allowed(host: &str, allowed: &[String]) -> bool {
    allowed.is_empty() || allowed.iter().any(|d| { let d = d.trim().trim_start_matches("*.").to_lowercase(); !d.is_empty() && (host == d || host.ends_with(&format!(".{d}"))) })
}

fn csv(rows: &[Vec<String>]) -> String {
    rows.iter().map(|r| r.iter().map(|c| format!("\"{}\"", c.replace('"', "\"\""))).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n")
}

fn str_of(v: Value) -> String { match v { Value::String(s) => s, Value::Null => String::new(), o => o.to_string() } }

fn dur(s: f64) -> String {
    if s < 90.0 { format!("{s:.0} 秒") } else if s < 5400.0 { format!("{:.0} 分钟", s / 60.0) } else { format!("{:.1} 小时", s / 3600.0) }
}

fn size(n: u64) -> String { if n >= 1 << 20 { format!("{:.1} MB", n as f64 / 1048576.0) } else { format!("{:.1} KB", n as f64 / 1024.0) } }

// ---------- input events ----------
fn mouse(p: &mut Page, kind: &str, x: f64, y: f64, clicks: u32) -> Result<(), String> {
    p.send("Input.dispatchMouseEvent", json!({"type": kind, "x": x, "y": y, "button": if clicks > 0 { "left" } else { "none" }, "clickCount": clicks}), secs(15)).map(|_| ())
}

/// Real mouse click at the element centre; script click when the element is covered or has no box.
fn click_ref(p: &mut Page, r: &str) -> Result<String, String> {
    let v = p.eval(&act_js(r, POINT_BODY), secs(20))?;
    if let Some(s) = v.as_str() { return Ok(s.to_string()); }   // ERROR: ref not found
    if v["ok"] == true {
        let (x, y) = (v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0));
        mouse(p, "mouseMoved", x, y, 0)?;
        mouse(p, "mousePressed", x, y, 1)?;
        mouse(p, "mouseReleased", x, y, 1)?;
        Ok(format!("clicked {}", r.replace(['\'', '\\'], "")))
    } else {
        Ok(str_of(p.eval(&act_js(r, "el.click(); return 'clicked (covered element: script click)';"), secs(20))?))
    }
}

/// (key, code, virtual key code, text) for a key name or one character.
fn key_def(k: &str) -> Option<(String, String, i64, String)> {
    let named: &[(&[&str], &str, &str, i64, &str)] = &[
        (&["enter", "return"], "Enter", "Enter", 13, "\r"), (&["tab"], "Tab", "Tab", 9, ""), (&["escape", "esc"], "Escape", "Escape", 27, ""),
        (&["backspace"], "Backspace", "Backspace", 8, ""), (&["delete", "del"], "Delete", "Delete", 46, ""), (&["space"], " ", "Space", 32, " "),
        (&["arrowup", "up"], "ArrowUp", "ArrowUp", 38, ""), (&["arrowdown", "down"], "ArrowDown", "ArrowDown", 40, ""),
        (&["arrowleft", "left"], "ArrowLeft", "ArrowLeft", 37, ""), (&["arrowright", "right"], "ArrowRight", "ArrowRight", 39, ""),
        (&["pageup"], "PageUp", "PageUp", 33, ""), (&["pagedown"], "PageDown", "PageDown", 34, ""), (&["home"], "Home", "Home", 36, ""), (&["end"], "End", "End", 35, ""),
    ];
    let l = k.trim().to_lowercase();
    if let Some((_, key, code, vk, t)) = named.iter().find(|(names, ..)| names.contains(&l.as_str())) {
        return Some((key.to_string(), code.to_string(), *vk, t.to_string()));
    }
    if let Some(n) = l.strip_prefix('f').and_then(|x| x.parse::<i64>().ok()).filter(|n| (1..=12).contains(n)) {
        return Some((format!("F{n}"), format!("F{n}"), 111 + n, String::new()));
    }
    let mut cs = k.chars();
    let (Some(ch), None) = (cs.next(), cs.next()) else { return None };
    let up = ch.to_ascii_uppercase();
    let code = if ch.is_ascii_alphabetic() { format!("Key{up}") } else if ch.is_ascii_digit() { format!("Digit{ch}") } else { String::new() };
    Some((ch.to_string(), code, if ch.is_ascii_alphanumeric() { up as i64 } else { 0 }, ch.to_string()))
}

const KEY_HELP: &str = "Enter, Tab, Escape, ArrowDown, F12, Control+A, Shift+Tab … or one character";

/// A key with optional modifiers ("Control+A", "Shift+Tab", "Meta+F"): (CDP modifier bits, key). Alt 1, Ctrl 2, Meta 4, Shift 8.
fn parse_key(k: &str) -> Option<(i64, (String, String, i64, String))> {
    let k = k.trim();
    if k.chars().count() <= 1 || !k.contains('+') { return key_def(k).map(|d| (0, d)); }
    let parts: Vec<&str> = k.split('+').map(str::trim).collect();
    let (main, mods) = parts.split_last()?;
    let mut m = 0;
    for x in mods {
        m |= match x.to_lowercase().as_str() { "alt" | "option" | "opt" => 1, "ctrl" | "control" => 2, "meta" | "cmd" | "command" | "super" | "win" => 4, "shift" => 8, _ => return None };
    }
    key_def(main).map(|d| (m, d))
}

fn press_key(p: &mut Page, k: &str) -> Result<(), String> {
    let (m, (mut key, code, vk, mut txt)) = parse_key(k).ok_or_else(|| format!("unknown key '{k}' ({KEY_HELP})"))?;
    if m & 7 != 0 { txt.clear(); } else if m == 8 && txt.chars().count() == 1 { txt = txt.to_uppercase(); key = txt.clone(); }
    let mut down = json!({"type": if txt.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": m});
    if !txt.is_empty() { down["text"] = json!(txt); down["unmodifiedText"] = json!(txt); }
    p.send("Input.dispatchKeyEvent", down, secs(15))?;
    p.send("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": m}), secs(15))?;
    Ok(())
}

/// Focus the field, replace its content with `text` (real text input; script fallback). Err("PASSWORD") for password fields.
fn type_ref(p: &mut Page, r: &str, text: &str) -> Result<String, String> {
    let f = str_of(p.eval(&act_js(r, FOCUS_BODY), secs(20))?);
    if f != "focused" { return Ok(f); }
    if text.is_empty() { press_key(p, "Backspace")?; return Ok("cleared".into()); }
    p.send("Input.insertText", json!({"text": text}), secs(20))?;
    if str_of(p.eval(&act_js(r, VALUE_BODY), secs(10))?).is_empty() { p.eval(&type_js(r, text), secs(10))?; }
    Ok("typed".into())
}

// ---------- per-session tabs ----------
struct Tab { page: Page, url: String, known: HashSet<String>, dl_seen: HashSet<String> }
static TABS: Mutex<Option<HashMap<String, Tab>>> = Mutex::new(None);

fn take(id: &str) -> Option<Tab> { TABS.lock().unwrap().as_mut().and_then(|m| m.remove(id)) }
fn put(id: &str, t: Tab) { TABS.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.to_string(), t); }

/// Session closed: forget its tab (the tab itself stays open in the user's window).
pub fn close(id: &str) { drop(take(id)); }

/// After an action: give the page time to load / render.
fn settle(p: &mut Page, timeout: Duration) {
    std::thread::sleep(Duration::from_millis(300));
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        match p.eval("document.readyState", secs(3)) {
            Ok(v) if v.as_str() == Some("complete") => break,
            Err(e) if e == DIALOG_OPEN => return,
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(400));
}

fn tab_ids(port: u16) -> HashSet<String> { crate::chrome::tabs(port).map(|v| v.into_iter().map(|t| t.0).collect()).unwrap_or_default() }

fn downloads_dir(c: &ToolCtx) -> PathBuf { c.workspace().join("downloads") }

/// Downloads of the whole visible Chrome (every tab, also ones the page opens) go to `dir`; per-tab fallback.
fn set_downloads(p: &mut Page, dir: &Path) {
    if crate::chrome::set_download_dir(p.port(), dir).is_err() {
        let _ = p.send("Page.setDownloadBehavior", json!({"behavior": "allow", "downloadPath": dir.to_string_lossy()}), secs(10));
    }
}

/// (finished file names, number of downloads in progress)
fn listing(d: &Path) -> (Vec<String>, usize) {
    let (mut done, mut partial) = (vec![], 0);
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.ends_with(".crdownload") || n.ends_with(".tmp") { partial += 1; } else if !n.starts_with('.') && e.path().is_file() { done.push(n); }
    }
    done.sort();
    (done, partial)
}

/// The session's tab in the visible Chrome: the same one when still there (reconnected after a lost connection),
/// else a new tab at the last URL. Chrome is started (or restarted after the user closed it) as needed.
fn tab(c: &ToolCtx) -> Result<Tab, String> {
    let mut notes = vec![];
    let (exe, proxy) = super::web::chrome_or_install(c, &mut notes)
        .ok_or_else(|| format!("浏览器不可用：{}", if notes.is_empty() { "未找到 Chrome".to_string() } else { notes.join("；") }))?;
    let port = crate::chrome::ensure_visible(&exe, &c.shared.paths.app_support, &proxy)?;
    let (mut last_url, mut dl_seen) = (String::new(), None);
    if let Some(mut t) = take(&c.shared.id) {
        if t.page.port() == port {
            match t.page.eval("1", secs(3)) {
                Ok(_) => return Ok(t),
                Err(e) if e == DIALOG_OPEN => return Ok(t),
                Err(_) => {}
            }
            let target = t.page.target().to_string();
            if let Ok(mut pg) = crate::chrome::visible_tab(port, Some(&target)) {
                if pg.enable_events().is_ok() || pg.dialog.is_some() { t.page = pg; return Ok(t); }
            }
        }
        last_url = std::mem::take(&mut t.url);
        dl_seen = Some(t.dl_seen);
    }
    // reuse a blank tab no other chat owns (the one Chrome opened at start) instead of leaving it next to a new one
    let owned: HashSet<String> = TABS.lock().unwrap().as_ref().map(|m| m.values().map(|t| t.page.target().to_string()).collect()).unwrap_or_default();
    let blank = crate::chrome::tabs(port).unwrap_or_default().into_iter()
        .find(|(id, _, url)| (url == "about:blank" || url.is_empty()) && !owned.contains(id)).map(|t| t.0);
    let mut page = match blank.and_then(|id| crate::chrome::visible_tab(port, Some(&id)).ok()) {
        Some(p) => p,
        None => crate::chrome::visible_tab(port, None)?,
    };
    page.enable_events()?;
    let _ = page.send("Page.bringToFront", json!({}), secs(5));
    if !last_url.is_empty() {
        page.send("Page.navigate", json!({"url": last_url}), secs(20))?;
        settle(&mut page, secs(20));
    }
    let dl_seen = dl_seen.unwrap_or_else(|| listing(&downloads_dir(c)).0.into_iter().collect());
    Ok(Tab { page, url: last_url, known: tab_ids(port), dl_seen })
}

/// A tab the page opened since the last look: switch to it.
fn follow_new_tab(t: &mut Tab) -> Option<String> {
    let port = t.page.port();
    let list = crate::chrome::tabs(port).ok()?;
    let fresh: Vec<(String, String, String)> = list.iter().filter(|x| !t.known.contains(&x.0) && x.0 != t.page.target()).cloned().collect();
    t.known = list.iter().map(|x| x.0.clone()).collect();
    let (id, title, url) = fresh.first()?.clone();
    let mut pg = crate::chrome::visible_tab(port, Some(&id)).ok()?;
    let _ = pg.enable_events();
    settle(&mut pg, secs(15));
    t.page = pg;
    Some(format!("\n（页面打开了新标签页，已切换过去：{title} {url}）"))
}

/// A copy in the user's own Downloads folder (where people look for downloads); " (1)", " (2)"… when the name is taken.
fn copy_to_user_downloads(src: &Path) -> Option<PathBuf> {
    if cfg!(test) { return None; }
    let dir = crate::platform::home().join("Downloads");
    if !dir.is_dir() { return None; }
    let name = src.file_name()?.to_string_lossy().to_string();
    let (stem, ext) = match name.rfind('.') { Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()), _ => (name.clone(), String::new()) };
    let mut dst = dir.join(&name);
    let mut i = 1;
    while dst.exists() { dst = dir.join(format!("{stem} ({i}){ext}")); i += 1; }
    std::fs::copy(src, &dst).ok().map(|_| dst)
}

/// Files that finished downloading since the last report (waits up to `appear` for one to show up and for every
/// download in progress to finish). Each is also copied to the user's Downloads folder.
fn new_downloads(c: &ToolCtx, t: &mut Tab, appear: Duration) -> Vec<(String, u64, Option<PathBuf>)> {
    let dir = downloads_dir(c);
    let (t0, mut note) = (Instant::now(), Instant::now());
    loop {
        let (done, partial) = listing(&dir);
        let fresh: Vec<String> = done.into_iter().filter(|n| !t.dl_seen.contains(n)).collect();
        if partial == 0 && (!fresh.is_empty() || t0.elapsed() >= appear) {
            t.dl_seen.extend(fresh.iter().cloned());
            return fresh.into_iter().map(|n| {
                let p = dir.join(&n);
                let s = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                (n, s, copy_to_user_downloads(&p))
            }).collect();
        }
        if c.shared.cancelled() || t0.elapsed() > secs(1800) { return vec![]; }
        if partial > 0 && note.elapsed() > secs(5) { (c.emit)(crate::events::tool_progress(&c.call_id, "正在下载…")); note = Instant::now(); }
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn downloads_note(files: &[(String, u64, Option<PathBuf>)]) -> String {
    if files.is_empty() { return String::new(); }
    let list: Vec<String> = files.iter().map(|(n, s, _)| format!("downloads/{n}（{}）", size(*s))).collect();
    let copies: Vec<String> = files.iter().filter_map(|(_, _, c)| c.as_ref().map(|p| p.display().to_string())).collect();
    let mut s = format!("\n已下载到工作区：{}。可用 data_import 导入（路径如 downloads/文件名）。", list.join("、"));
    if !copies.is_empty() { s += &format!("同时已复制到用户的「下载」文件夹：{}（告诉用户在这里能找到）。", copies.join("、")); }
    s
}

/// After an action that may open a tab or start a download.
fn after_action(c: &ToolCtx, t: &mut Tab) -> String {
    let mut s = follow_new_tab(t).unwrap_or_default();
    s += &downloads_note(&new_downloads(c, t, Duration::ZERO));
    s
}

fn dialog_msg(d: &Value) -> String {
    let kind = d["type"].as_str().unwrap_or("");
    let label = match kind { "alert" => "提示", "confirm" => "确认", "prompt" => "输入", "beforeunload" => "离开页面确认", o => o };
    format!("页面弹出了{label}对话框：<untrusted_web_content>{}</untrusted_web_content>\n用 dialog(accept=true 确定 / accept=false 取消{}) 处理后再继续。",
            text::prefix(d["message"].as_str().unwrap_or(""), 500), if kind == "prompt" { "，text=要填的内容" } else { "" })
}

fn browser(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let action = sv(a, "action");
    if !ACTIONS.contains(&action.as_str()) { return Ok(ToolResult::err(format!("unknown action '{action}'"))); }
    // validate before touching Chrome
    let nav_url = if action == "navigate" {
        let u = sv(a, "url");
        let u = if u.starts_with("http://") || u.starts_with("https://") { u } else { format!("https://{u}") };
        let Some(host) = crate::permissions::host(&json!({"url": u})) else { return Ok(ToolResult::err("invalid url")) };
        if !domain_allowed(&host.to_lowercase(), &c.shared.settings.browser_allowed_domains) {
            return Ok(ToolResult::err(format!("域名 {host} 不在允许列表中（设置 → 扩展 → 浏览器工具）")));
        }
        Some(u)
    } else { None };
    let mut t = tab(c)?;
    if action != "dialog" {
        if let Some(d) = t.page.dialog.clone() { put(&c.shared.id, t); return Ok(ToolResult::ok(dialog_msg(&d))); }
    }
    // downloads land in this chat's workspace
    let dl = downloads_dir(c);
    let _ = std::fs::create_dir_all(&dl);
    set_downloads(&mut t.page, &dl);
    let mut r = run(&action, nav_url.as_deref(), a, c, &mut t);
    if matches!(&r, Err(e) if e == DIALOG_OPEN) {
        r = Ok(ToolResult::ok(t.page.dialog.as_ref().map(dialog_msg).unwrap_or_else(|| "页面弹出了对话框，用 dialog 处理。".into())));
    }
    if t.page.dialog.is_none() {
        if let Ok(v) = t.page.eval("location.href", secs(3)) { if let Some(u) = v.as_str() { t.url = u.to_string(); } }
        // the browser panel shows the page after every step
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
    }
    put(&c.shared.id, t);
    r
}

/// `<workspace>/browser/latest.jpg` (overwritten each step; the panel shows only the latest page).
fn panel_shot(p: &mut Page, ws: &Path) -> Option<String> {
    let r = p.send("Page.captureScreenshot", json!({"format": "jpeg", "quality": 55}), secs(10)).ok()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(r["data"].as_str()?).ok()?;
    let dir = ws.join("browser");
    std::fs::create_dir_all(&dir).ok()?;
    let f = dir.join("latest.jpg");
    std::fs::write(&f, bytes).ok()?;
    Some(f.to_string_lossy().to_string())
}

fn run(action: &str, nav_url: Option<&str>, a: &Value, c: &ToolCtx, t: &mut Tab) -> Result<ToolResult, String> {
    let wrap = |s: String| format!("<untrusted_web_content>\n{}\n</untrusted_web_content>", text::truncate_middle(&s, 12000));
    let snap = |p: &mut Page| -> Result<String, String> { Ok(wrap(str_of(p.eval(SNAPSHOT_JS, secs(20))?))) };
    let r = sv(a, "ref");
    let need_ref = || -> Option<ToolResult> { if r.is_empty() { Some(ToolResult::err("ref is required (take a snapshot first)")) } else { None } };
    let progress = |m: &str| (c.emit)(crate::events::tool_progress(&c.call_id, m));
    match action {
        "navigate" => {
            let u = nav_url.unwrap_or_default();
            progress(&format!("打开 {u}"));
            t.page.send("Page.navigate", json!({"url": u}), secs(30))?;
            let _ = t.page.send("Page.bringToFront", json!({}), secs(5));
            settle(&mut t.page, secs(20));
            let extra = after_action(c, t);
            let s = snap(&mut t.page)?;
            Ok(ToolResult::with(format!("{s}{extra}"), json!({"kind": "browser", "url": u})))
        }
        "snapshot" => Ok(ToolResult::ok(snap(&mut t.page)?)),
        "click" => {
            let label = sv(a, "text");
            let target = if r.is_empty() && !label.trim().is_empty() {
                if str_of(t.page.eval(&find_text_js(&label), secs(20))?) != "found" {
                    return Ok(ToolResult::err(format!("页面上没有可见的「{label}」可点击（先 snapshot 看看，或先展开它所在的下拉框）")));
                }
                "dp-text".to_string()
            } else {
                if let Some(e) = need_ref() { return Ok(e); }
                r.clone()
            };
            let m = click_ref(&mut t.page, &target)?;
            if m.starts_with("ERROR") { return Ok(ToolResult::err(m)); }
            settle(&mut t.page, secs(10));
            let extra = after_action(c, t);
            Ok(ToolResult::ok(format!("{m}{extra}\n{}", snap(&mut t.page)?)))
        }
        "type" => {
            if let Some(e) = need_ref() { return Ok(e); }
            let m = type_ref(&mut t.page, &r, &sv(a, "text"))?;
            if m == "PASSWORD" {
                return Ok(ToolResult::err("出于安全考虑，不能在密码框里输入。请用 ask_user 请用户在 DataPilot 浏览器窗口里自己输入密码并登录，完成后再 snapshot 继续。"));
            }
            if m.starts_with("ERROR") { return Ok(ToolResult::err(m)); }
            let mut extra = String::new();
            if a["submit"] == true {
                press_key(&mut t.page, "Enter")?;
                settle(&mut t.page, secs(15));
                extra = after_action(c, t);
            } else { std::thread::sleep(Duration::from_millis(400)); }   // autocomplete lists
            Ok(ToolResult::ok(format!("{m}{extra}\n{}", snap(&mut t.page)?)))
        }
        "select" => {
            if let Some(e) = need_ref() { return Ok(e); }
            let m = str_of(t.page.eval(&select_js(&r, &sv(a, "value")), secs(20))?);
            if m.starts_with("ERROR") { return Ok(ToolResult::err(m)); }
            std::thread::sleep(Duration::from_millis(400));
            Ok(ToolResult::ok(format!("{m}\n{}", snap(&mut t.page)?)))
        }
        "press" => {
            let key = sv(a, "key");
            if parse_key(&key).is_none() { return Ok(ToolResult::err(format!("unknown key '{key}' ({KEY_HELP}). Browser-window shortcuts such as find (⌘F) are not available: use snapshot."))); }
            if !r.is_empty() {
                let f = str_of(t.page.eval(&act_js(&r, "el.focus(); return 'focused';"), secs(20))?);
                if f.starts_with("ERROR") { return Ok(ToolResult::err(f)); }
            }
            press_key(&mut t.page, &key)?;
            if parse_key(&key).map(|(m, k)| m == 0 && k.0 == "Enter").unwrap_or(false) { settle(&mut t.page, secs(15)); } else { std::thread::sleep(Duration::from_millis(400)); }
            let extra = after_action(c, t);
            Ok(ToolResult::ok(format!("pressed {key}{extra}\n{}", snap(&mut t.page)?)))
        }
        "hover" => {
            if let Some(e) = need_ref() { return Ok(e); }
            let v = t.page.eval(&act_js(&r, POINT_BODY), secs(20))?;
            if let Some(s) = v.as_str() { return Ok(ToolResult::err(s.to_string())); }
            mouse(&mut t.page, "mouseMoved", v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0), 0)?;
            std::thread::sleep(Duration::from_millis(600));
            Ok(ToolResult::ok(format!("hovered {r}\n{}", snap(&mut t.page)?)))
        }
        "scroll" => {
            if !r.is_empty() {
                let m = str_of(t.page.eval(&act_js(&r, "return 'scrolled into view';"), secs(20))?);
                if m.starts_with("ERROR") { return Ok(ToolResult::err(m)); }
            } else {
                // a wheel event at the viewport centre scrolls whatever is there (inner panes and iframes too)
                let v = t.page.eval("[innerWidth, innerHeight]", secs(5))?;
                let (w, h) = (v[0].as_f64().unwrap_or(1280.0), v[1].as_f64().unwrap_or(800.0));
                let dy = if sv(a, "direction") == "up" { -0.8 * h } else { 0.8 * h };
                t.page.send("Input.dispatchMouseEvent", json!({"type": "mouseWheel", "x": w / 2.0, "y": h / 2.0, "deltaX": 0, "deltaY": dy}), secs(10))?;
            }
            std::thread::sleep(Duration::from_millis(500));
            Ok(ToolResult::ok(snap(&mut t.page)?))
        }
        "back" => {
            let _ = t.page.eval("(history.back(), 'back')", secs(10));
            settle(&mut t.page, secs(10));
            Ok(ToolResult::ok(snap(&mut t.page)?))
        }
        "wait_for" => {
            let terms: Vec<String> = sv(a, "text").split('|').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            if terms.is_empty() { return Ok(ToolResult::err("text is required (one or more alternatives separated by |)")); }
            let gone = a["gone"] == true;
            let limit = a["timeout"].as_f64().unwrap_or(30.0).clamp(1.0, MAX_WAIT);
            let every = a["reload_every"].as_f64().filter(|s| *s > 0.0).map(|s| s.max(15.0));
            let refresh_click = sv(a, "refresh_click");
            let every = every.or(if refresh_click.is_empty() { None } else { Some(60.0) });
            let js = has_text_js(&terms);
            let label = terms.join(" | ");
            let (t0, mut last_refresh, mut last_note, mut n, mut errors) = (Instant::now(), Instant::now(), Instant::now(), 0u32, 0u32);
            loop {
                match t.page.eval(&js, secs(15)) {
                    Ok(v) => {
                        errors = 0;
                        if (v == Value::Bool(true)) != gone {
                            let extra = after_action(c, t);
                            return Ok(ToolResult::ok(format!("条件已满足：页面上{}「{label}」（等了 {}{}）{extra}\n{}", if gone { "已没有" } else { "出现了" },
                                                             dur(t0.elapsed().as_secs_f64()), if n > 0 { format!("，刷新 {n} 次") } else { String::new() }, snap(&mut t.page)?)));
                        }
                    }
                    Err(e) if e == DIALOG_OPEN => return Err(e),
                    Err(e) => { errors += 1; if errors > 30 { return Err(format!("页面无响应：{e}")); } }   // mid-reload: retry
                }
                let el = t0.elapsed().as_secs_f64();
                if el >= limit {
                    return Ok(ToolResult::ok(format!("等待超时（{}）：页面上仍{}「{label}」。\n{}", dur(el), if gone { "有" } else { "没有" }, snap(&mut t.page)?)));
                }
                if c.shared.cancelled() { return Ok(ToolResult::err("已停止等待（用户中止）")); }
                if let Some(e) = every {
                    if last_refresh.elapsed().as_secs_f64() >= e {
                        if refresh_click.is_empty() { let _ = t.page.send("Page.reload", json!({}), secs(30)); }
                        else { let _ = t.page.eval(&click_text_js(&refresh_click), secs(15)); }
                        settle(&mut t.page, secs(30));
                        last_refresh = Instant::now();
                        n += 1;
                    }
                }
                if last_note.elapsed() >= secs(30) {
                    progress(&format!("等待「{label}」{}… 已等 {}{}", if gone { "消失" } else { "出现" }, dur(el), if n > 0 { format!("，已刷新 {n} 次") } else { String::new() }));
                    last_note = Instant::now();
                }
                for _ in 0..10 { if c.shared.cancelled() { break; } std::thread::sleep(Duration::from_millis(100)); }
            }
        }
        "tabs" => {
            let list = crate::chrome::tabs(t.page.port())?;
            t.known = list.iter().map(|x| x.0.clone()).collect();
            let lines: Vec<String> = list.iter().enumerate()
                .map(|(i, (id, title, url))| format!("[{}]{} {} — {}", i + 1, if id == t.page.target() { " (current)" } else { "" }, title, url)).collect();
            Ok(ToolResult::ok(wrap(lines.join("\n"))))
        }
        "switch_tab" => {
            let list = crate::chrome::tabs(t.page.port())?;
            let i = a["index"].as_u64().unwrap_or(0) as usize;
            let Some((id, _, _)) = list.get(i.wrapping_sub(1)) else { return Ok(ToolResult::err(format!("没有第 {i} 个标签页（先用 tabs 查看，从 1 开始）"))) };
            let mut pg = crate::chrome::visible_tab(t.page.port(), Some(id))?;
            pg.enable_events()?;
            let _ = pg.send("Page.bringToFront", json!({}), secs(5));
            t.page = pg;
            t.known = list.iter().map(|x| x.0.clone()).collect();
            Ok(ToolResult::ok(snap(&mut t.page)?))
        }
        "dialog" => {
            let accept = a["accept"] != false;
            let mut p = json!({"accept": accept});
            if a.get("text").and_then(|v| v.as_str()).is_some() { p["promptText"] = json!(sv(a, "text")); }
            t.page.send("Page.handleJavaScriptDialog", p, secs(10))?;
            t.page.dialog = None;
            settle(&mut t.page, secs(10));
            let extra = after_action(c, t);
            Ok(ToolResult::ok(format!("{}{extra}\n{}", if accept { "已确定" } else { "已取消" }, snap(&mut t.page)?)))
        }
        "downloads" => {
            let wait = a["timeout"].as_f64().unwrap_or(0.0).clamp(0.0, 3600.0);
            if wait > 0.0 { progress("等待下载完成…"); }
            let fresh = new_downloads(c, t, Duration::from_secs_f64(wait));
            if !fresh.is_empty() { return Ok(ToolResult::ok(downloads_note(&fresh).trim().to_string())); }
            let (all, partial) = listing(&downloads_dir(c));
            Ok(ToolResult::ok(format!("没有新下载的文件{}。downloads/ 里现有：{}", if partial > 0 { format!("（{partial} 个仍在下载）") } else { String::new() },
                                      if all.is_empty() { "（空）".to_string() } else { all.join("、") })))
        }
        "extract_tables" => {
            let v = t.page.eval(&tables_js(), secs(30))?;
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
            let r = t.page.send("Page.captureScreenshot", json!({"format": "jpeg", "quality": 75}), secs(20))?;
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
    use std::io::{Read, Write};

    #[test]
    fn helpers() {
        assert!(domain_allowed("a.example.com", &["example.com".into()]));
        assert!(domain_allowed("example.com", &["*.example.com".into()]));
        assert!(!domain_allowed("badexample.com", &["example.com".into()]));
        assert!(domain_allowed("x.org", &[]));
        assert_eq!(risk(&json!({"action": "snapshot"})), Risk::ReadOnly);
        assert_eq!(risk(&json!({"action": "wait_for", "text": "完成"})), Risk::ReadOnly);
        assert_eq!(risk(&json!({"action": "wait_for", "text": "完成", "refresh_click": "刷新"})), Risk::Network);
        assert_eq!(risk(&json!({"action": "type"})), Risk::External);
        assert_eq!(risk(&json!({"action": "press"})), Risk::External);
        assert_eq!(risk(&json!({"action": "navigate"})), Risk::Network);
        assert!(act_js("e1\"]');alert(1)//", "").contains("e1);alert(1)//"));
        assert_eq!(csv(&[vec!["a\"b".into(), "c".into()]]), "\"a\"\"b\",\"c\"");
        assert_eq!(key_def("enter").unwrap().2, 13);
        assert_eq!(key_def("a").unwrap(), ("a".into(), "KeyA".into(), 65, "a".into()));
        assert_eq!(parse_key("Ctrl+Shift+Q").unwrap().0, 10);
        assert_eq!(parse_key("Meta+f").unwrap().0, 4);
        assert_eq!(parse_key("F12").unwrap().1 .2, 123);
        assert!(parse_key("Hyper+x").is_none());
        assert!(dur(30.0).contains("秒") && dur(600.0).contains("分钟") && dur(9000.0).contains("小时"));
    }

    /// Minimal HTTP server on 127.0.0.1 serving `pages` (path → html) — same-origin iframes need a real origin.
    fn serve(pages: Vec<(&'static str, String)>) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut s = s;
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (ctype, body) = match pages.iter().find(|(p, _)| *p == path) {
                    Some((p, b)) if p.ends_with(".csv") => ("text/csv", b.clone()),
                    Some((_, b)) => ("text/html; charset=utf-8", b.clone()),
                    None => ("text/plain", String::new()),
                };
                let disp = if ctype == "text/csv" { "Content-Disposition: attachment; filename=\"report.csv\"\r\n" } else { "" };
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\n{disp}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        port
    }

    /// Real Chrome (headless under test; skipped when none is installed): frames, mouse click, typing, password guard,
    /// dialogs, new tabs and downloads on the visible-browser code path.
    #[test]
    fn live_visible_browser() {
        let _g = crate::chrome::LIVE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let support = std::env::temp_dir().join(format!("dp-browser-{}", crate::text::short_id(6)));
        let Some(exe) = crate::chrome::find(&support) else { return };
        let inner = "<input id=q placeholder=搜索><input type=password id=pw placeholder=密码><button onclick=\"parent.document.title='ok:'+q.value\">Go</button>".to_string();
        let top = "<h1>ERP</h1><iframe src=/inner style='width:600px;height:200px'></iframe><button onclick=\"alert('saved')\">Save</button>\
                   <button onclick=\"window.open('/inner')\">Popup</button><a href=/report.csv>Download</a><div id=st>running</div>\
                   <script>setTimeout(()=>{document.getElementById('st').textContent='Completed'},1500)</script>".to_string()
            + &format!("<ul id=proj>{}</ul>", (0..60).map(|i| format!("<li><label><input type=checkbox>P{i:03} | Project</label></li>")).collect::<String>())
            + "<div style=width:400px><a id=x_PROJECT_div_s class=dataLink onmousedown=\"document.title='search'\"><span><span style=float:left>Search ...</span><span style=float:right>F12</span></span></a></div><div style=clear:both></div>\
               <a id=x_searchDialog_move href='javascript:void(0)'><img src=/shuttleright_ena.png></a>\
               <div class=item tabindex=0 onclick=\"document.title='picked'\">27UL005002 | Internal Audit</div>\
               <button style=display:none onclick=\"document.title='bad'\">Go2</button><button onclick=\"document.title='good'\">Go2</button>";
        let port = serve(vec![("/", top), ("/inner", inner), ("/report.csv", "a,b\n1,2\n".into())]);
        let cport = crate::chrome::ensure_visible(&exe, &support, &[]).unwrap();
        let mut p = crate::chrome::visible_tab(cport, None).unwrap();
        p.enable_events().unwrap();
        let dl = support.join("downloads");
        std::fs::create_dir_all(&dl).unwrap();
        set_downloads(&mut p, &dl);
        p.send("Page.navigate", json!({"url": format!("http://127.0.0.1:{port}/")}), secs(15)).unwrap();
        settle(&mut p, secs(10));
        std::thread::sleep(Duration::from_millis(500));
        let snap = str_of(p.eval(SNAPSHOT_JS, secs(10)).unwrap());
        assert!(snap.contains("--- iframe /inner ---") && snap.contains("\"搜索\""), "{snap}");
        let refs = |snap: &str, name: &str| snap.lines().find(|l| l.contains(name)).and_then(|l| l.split(']').next()).map(|s| s.trim_start_matches('[').to_string()).unwrap();
        // long list collapsed, entries after it (Search ..., icon-only Move, focusable result) still listed
        let elems = snap.split("## Visible text").next().unwrap_or("");
        assert!(elems.contains("more items of this list not shown") && !elems.contains("P030"), "{snap}");
        assert!(snap.contains("\"Search ... F12\"") || snap.contains("\"Search ...F12\""), "{snap}");
        assert!(snap.contains("\"icon move\"") && snap.contains("\"27UL005002 | Internal Audit\""), "{snap}");
        // the real-page shape: the link itself has no box, only its floated text does
        assert_eq!(p.eval("document.getElementById('x_PROJECT_div_s').getBoundingClientRect().width", secs(5)).unwrap().as_f64(), Some(0.0));
        assert_eq!(p.eval(&find_text_js("Search ..."), secs(5)).unwrap(), json!("found"));
        assert!(click_ref(&mut p, "dp-text").unwrap().starts_with("clicked"));
        assert_eq!(p.eval("document.title", secs(5)).unwrap(), json!("search"));
        p.eval(&click_text_js("Go2"), secs(5)).unwrap();
        assert_eq!(p.eval("document.title", secs(5)).unwrap(), json!("good"));
        // typing + real click inside the iframe
        assert_eq!(type_ref(&mut p, &refs(&snap, "\"搜索\""), "销售").unwrap(), "typed");
        assert_eq!(type_ref(&mut p, &refs(&snap, "\"密码\""), "x").unwrap(), "PASSWORD");
        assert!(click_ref(&mut p, &refs(&snap, "\"Go\"")).unwrap().starts_with("clicked"));
        assert_eq!(p.eval("document.title", secs(5)).unwrap(), json!("ok:销售"));
        // dialog
        assert_eq!(click_ref(&mut p, &refs(&snap, "\"Save\"")).unwrap_err(), DIALOG_OPEN);
        assert_eq!(p.dialog.as_ref().unwrap()["message"], "saved");
        p.send("Page.handleJavaScriptDialog", json!({"accept": true}), secs(10)).unwrap();
        p.dialog = None;
        // text appears later
        let js = has_text_js(&["Completed".into()]);
        let t0 = Instant::now();
        while p.eval(&js, secs(5)).unwrap() != json!(true) { assert!(t0.elapsed() < secs(10)); std::thread::sleep(Duration::from_millis(200)); }
        // new tab
        let before = tab_ids(cport);
        click_ref(&mut p, &refs(&snap, "\"Popup\"")).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        assert!(tab_ids(cport).len() > before.len());
        // download started in the popup tab, not the tab the setting was made on
        let popup = tab_ids(cport).into_iter().find(|id| !before.contains(id) && id != p.target()).unwrap();
        let mut q = crate::chrome::visible_tab(cport, Some(&popup)).unwrap();
        q.send("Page.navigate", json!({"url": format!("http://127.0.0.1:{port}/report.csv")}), secs(15)).unwrap();
        drop(q);
        let t0 = Instant::now();
        while listing(&dl).0.is_empty() || listing(&dl).1 > 0 { assert!(t0.elapsed() < secs(15), "no download"); std::thread::sleep(Duration::from_millis(200)); }
        assert_eq!(std::fs::read_to_string(dl.join(&listing(&dl).0[0])).unwrap(), "a,b\n1,2\n");
        drop(p);
        crate::chrome::shutdown_visible();
        let _ = std::fs::remove_dir_all(&support);
    }
}
