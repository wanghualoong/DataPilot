//! `computer` tool (port of Swift `ComputerController` / `ComputerTool`): macOS desktop control.
//! Two modes: AX (accessibility tree + refs; text-only models) and vision (screenshot + coordinates, Qwen-VL action names).
//! Runs outside the kernel sandbox → state-changing actions are `external` risk (approval in ask / auto mode, denied in
//! plan mode, never available to sub-agents). Off unless settings.computerUseEnabled; per-turn cap on actions;
//! sensitive apps (keychain, password managers, System Settings) are refused.
use base64::Engine as _;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use super::{imp, with_risk, Impl, ToolCtx, ToolResult};
use crate::permissions::Risk;
use crate::text;

pub fn register(m: &mut HashMap<&'static str, Impl>) {
    m.insert("computer", with_risk(imp(computer, summary), risk));
}

const READ_ONLY: &[&str] = &["ax_snapshot", "screenshot", "wait", "terminate"];
pub const BLOCKED_BUNDLES: &[&str] = &["com.apple.keychainaccess", "com.apple.systempreferences", "com.apple.Passwords",
                                       "com.1password.1password", "com.agilebits.onepassword7", "com.bitwarden.desktop"];

/// Web browsers: the computer tool refuses to drive them — web pages go through the browser tool (DataPilot's own Chrome
/// with real DOM access: iframes, dropdowns, downloads, waits), never the user's everyday browser.
pub const BROWSER_BUNDLES: &[&str] = &["com.google.Chrome", "com.google.Chrome.beta", "com.google.Chrome.dev", "com.google.Chrome.canary",
    "com.google.chrome.for.testing", "org.chromium.Chromium", "com.apple.Safari", "com.apple.SafariTechnologyPreview", "com.microsoft.edgemac",
    "com.microsoft.edgemac.Beta", "org.mozilla.firefox", "org.mozilla.firefoxdeveloperedition", "company.thebrowser.Browser", "com.brave.Browser",
    "com.operasoftware.Opera", "com.vivaldi.Vivaldi"];
pub const USE_BROWSER: &str = "computer 工具不能操作浏览器（Chrome / Safari / Edge 等）。网页任务请改用 browser 工具：它在 DataPilot 专用的 Chrome 窗口里按元素操作（支持登录后继续、iframe、下拉框、弹窗、下载、等待），不要再尝试用 computer 打开或操作浏览器。/ The computer tool does not operate web browsers: use the browser tool for web pages.";

/// Is this app (name or bundle id) a web browser?
pub fn is_browser(name_or_bundle: &str) -> bool {
    let l = name_or_bundle.trim().trim_end_matches(".app").to_lowercase();
    if l.is_empty() { return false; }
    BROWSER_BUNDLES.iter().any(|b| b.to_lowercase() == l)
        || ["chrome", "chromium", "safari", "firefox", "浏览器", "browser"].iter().any(|k| l.contains(k))
        || ["arc", "edge", "microsoft edge", "brave", "opera", "vivaldi"].contains(&l.as_str())
}

fn browser_in_front() -> bool { front_app().map(|a| is_browser(&a.bundle) || is_browser(&a.name)).unwrap_or(false) }

fn risk(a: &Value) -> Risk { if READ_ONLY.contains(&a["action"].as_str().unwrap_or("")) { Risk::ReadOnly } else { Risk::External } }

fn summary(a: &Value) -> String {
    let mut s = format!("电脑操作：{}", a["action"].as_str().unwrap_or(""));
    if let Some(r) = a["ref"].as_str() { s += &format!(" [{r}]"); }
    if let Some(t) = a["text"].as_str() { s += &format!(" “{}”", text::prefix(t, 40)); }
    if let Some(c) = a["coordinate"].as_array() { s += &format!(" @{:?}", c.iter().filter_map(|v| v.as_f64()).map(|v| v as i64).collect::<Vec<_>>()); }
    if let Some(k) = a["keys"].as_array() { s += &format!(" {}", k.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("+")); }
    s
}

// ---------- FFI (ApplicationServices / CoreGraphics) ----------
#[repr(C)] #[derive(Clone, Copy, Debug, PartialEq)] pub struct CGPoint { pub x: f64, pub y: f64 }
#[repr(C)] #[derive(Clone, Copy)] struct CGSize { width: f64, height: f64 }
#[repr(C)] #[derive(Clone, Copy)] struct CGRect { origin: CGPoint, size: CGSize }
type Ref = *const std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> Ref;
    fn AXUIElementCopyAttributeValue(e: Ref, attr: CFStringRef, out: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(e: Ref, attr: CFStringRef, v: CFTypeRef) -> i32;
    fn AXUIElementPerformAction(e: Ref, action: CFStringRef) -> i32;
    fn AXUIElementSetMessagingTimeout(e: Ref, secs: f32) -> i32;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(d: u32) -> CGRect;
    fn CGEventCreateMouseEvent(src: Ref, ty: u32, p: CGPoint, button: u32) -> Ref;
    fn CGEventCreateKeyboardEvent(src: Ref, key: u16, down: bool) -> Ref;
    fn CGEventCreateScrollWheelEvent2(src: Ref, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> Ref;
    fn CGEventKeyboardSetUnicodeString(e: Ref, len: usize, s: *const u16);
    fn CGEventSetIntegerValueField(e: Ref, field: u32, v: i64);
    fn CGEventSetFlags(e: Ref, flags: u64);
    fn CGEventPost(tap: u32, e: Ref);
}

pub fn has_accessibility() -> bool { unsafe { AXIsProcessTrusted() != 0 } }
pub fn has_screen_recording() -> bool { unsafe { CGPreflightScreenCaptureAccess() } }

/// Retained AX element kept across calls (refs from the last ax_snapshot).
struct Elem(Ref);
unsafe impl Send for Elem {}
impl Elem { fn retain(p: Ref) -> Elem { unsafe { CFRetain(p) }; Elem(p) } }
impl Drop for Elem { fn drop(&mut self) { unsafe { CFRelease(self.0) } } }

static REFS: Mutex<Option<HashMap<String, Elem>>> = Mutex::new(None);
static SHOT: Mutex<(u32, u32)> = Mutex::new((1280, 800));

fn attr(e: Ref, name: &str) -> Option<CFType> {
    let n = CFString::new(name);
    let mut v: CFTypeRef = std::ptr::null();
    let r = unsafe { AXUIElementCopyAttributeValue(e, n.as_concrete_TypeRef(), &mut v) };
    if r != 0 || v.is_null() { return None; }
    Some(unsafe { CFType::wrap_under_create_rule(v) })
}

fn attr_text(e: Ref, name: &str) -> Option<String> {
    let v = attr(e, name)?;
    if let Some(s) = v.downcast::<CFString>() { return Some(s.to_string()); }
    if let Some(n) = v.downcast::<CFNumber>() { return n.to_f64().map(|f| if f.fract() == 0.0 { format!("{}", f as i64) } else { f.to_string() }); }
    if let Some(b) = v.downcast::<CFBoolean>() { return Some(bool::from(b).to_string()); }
    None
}

// ---------- frontmost app (lsappinfo: no AppKit needed) ----------
#[derive(Debug, PartialEq)]
pub struct App { pub name: String, pub bundle: String, pub pid: i32 }

pub fn parse_lsappinfo(s: &str) -> Option<App> {
    let name = s.lines().next()?.split('"').nth(1).unwrap_or("").to_string();
    let bundle = s.split("bundleID=\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("").to_string();
    let pid = s.split("pid = ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    Some(App { name, bundle, pid })
}

pub fn front_app() -> Option<App> {
    let asn = Command::new("/usr/bin/lsappinfo").arg("front").output().ok()?;
    let asn = String::from_utf8_lossy(&asn.stdout).trim().to_string();
    if asn.is_empty() { return None; }
    let out = Command::new("/usr/bin/lsappinfo").args(["info", "-only", "name", "-only", "bundleid", "-only", "pid"]).arg(&asn).output().ok()?;
    let full = Command::new("/usr/bin/lsappinfo").args(["info"]).arg(&asn).output().ok();
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    parse_lsappinfo(&s).filter(|a| a.pid > 0)
        .or_else(|| full.and_then(|f| parse_lsappinfo(&String::from_utf8_lossy(&f.stdout))))
}

fn guard_front(c: &ToolCtx) -> Result<(), String> {
    if c.shared.cancelled() { return Err("已被用户停止 / stopped by user".into()); }
    if let Some(a) = front_app() { if BLOCKED_BUNDLES.contains(&a.bundle.as_str()) { return Err(format!("前台应用 {} 在黑名单中，拒绝操作", a.bundle)); } }
    Ok(())
}

// ---------- AX ----------
const INTERESTING: &[&str] = &["AXButton", "AXTextField", "AXTextArea", "AXCheckBox", "AXRadioButton", "AXPopUpButton", "AXMenuItem",
                               "AXMenuBarItem", "AXLink", "AXComboBox", "AXTab", "AXCell", "AXStaticText", "AXSearchField", "AXSlider", "AXRow"];

pub fn ax_snapshot(max_nodes: usize) -> Result<String, String> {
    if !has_accessibility() { return Err("需要“辅助功能”权限：系统设置 → 隐私与安全性 → 辅助功能 → 勾选 DataPilot".into()); }
    let app = front_app().ok_or("no frontmost app")?;
    let root_ref = unsafe { AXUIElementCreateApplication(app.pid) };
    if root_ref.is_null() { return Err("AX: cannot open the frontmost app".into()); }
    let root = unsafe { CFType::wrap_under_create_rule(root_ref) };
    unsafe { AXUIElementSetMessagingTimeout(root_ref, 3.0) };
    let mut refs: HashMap<String, Elem> = HashMap::new();
    let mut lines = vec![format!("App: {} ({})", app.name, app.bundle)];
    fn walk(e: Ref, depth: usize, max: usize, refs: &mut HashMap<String, Elem>, lines: &mut Vec<String>) {
        if refs.len() >= max || depth >= 25 { return; }
        let role = attr_text(e, "AXRole").unwrap_or_default();
        if INTERESTING.contains(&role.as_str()) {
            let title = attr_text(e, "AXTitle").filter(|t| !t.is_empty()).or_else(|| attr_text(e, "AXDescription")).unwrap_or_default();
            let value = attr_text(e, "AXValue").unwrap_or_default();
            if !(role == "AXStaticText" && value.is_empty() && title.is_empty()) {
                let r = format!("a{}", refs.len() + 1);
                refs.insert(r.clone(), Elem::retain(e));
                let v = if value.is_empty() { String::new() } else { format!(" value=\"{}\"", text::prefix(&value, 60)) };
                lines.push(format!("{}[{r}] {} \"{}\"{v}", "  ".repeat(depth.min(8)), &role[2..], text::prefix(&title, 60)));
            }
        }
        let Some(kids) = attr(e, "AXChildren") else { return };
        let k = kids.as_CFTypeRef();
        if unsafe { CFGetTypeID(k) } != unsafe { CFArrayGetTypeID() } { return; }
        let n = unsafe { CFArrayGetCount(k as CFArrayRef) };
        for i in 0..n {
            let child = unsafe { CFArrayGetValueAtIndex(k as CFArrayRef, i) };
            if !child.is_null() { walk(child, depth + 1, max, refs, lines); }
        }
    }
    if let Some(w) = attr(root.as_CFTypeRef(), "AXFocusedWindow") { walk(w.as_CFTypeRef(), 0, max_nodes, &mut refs, &mut lines); }
    if refs.len() < max_nodes { if let Some(m) = attr(root.as_CFTypeRef(), "AXMenuBar") { walk(m.as_CFTypeRef(), 0, max_nodes, &mut refs, &mut lines); } }
    if refs.len() >= max_nodes { lines.push("…(truncated)".into()); }
    *REFS.lock().unwrap() = Some(refs);
    Ok(lines.join("\n"))
}

fn with_elem<T>(r: &str, f: impl FnOnce(Ref) -> T) -> Result<T, String> {
    let g = REFS.lock().unwrap();
    let e = g.as_ref().and_then(|m| m.get(r)).ok_or_else(|| format!("ref {r} 不存在，请先 ax_snapshot"))?;
    Ok(f(e.0))
}

fn ax_press(r: &str) -> Result<String, String> {
    let code = with_elem(r, |e| unsafe { AXUIElementPerformAction(e, CFString::new("AXPress").as_concrete_TypeRef()) })?;
    Ok(if code == 0 { format!("pressed {r}") } else { format!("press failed ({code})") })
}

fn ax_set_value(r: &str, t: &str) -> Result<String, String> {
    let code = with_elem(r, |e| unsafe {
        AXUIElementSetAttributeValue(e, CFString::new("AXFocused").as_concrete_TypeRef(), CFBoolean::true_value().as_CFTypeRef());
        AXUIElementSetAttributeValue(e, CFString::new("AXValue").as_concrete_TypeRef(), CFString::new(t).as_CFTypeRef())
    })?;
    Ok(if code == 0 { format!("set {r}") } else { format!("set value failed ({code})") })
}

// ---------- screenshot (screencapture + sips; main display, scaled to ≤ 1280 px wide) ----------
fn sips_dims(p: &std::path::Path) -> Option<(u32, u32)> {
    let o = Command::new("/usr/bin/sips").args(["-g", "pixelWidth", "-g", "pixelHeight"]).arg(p).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout);
    let get = |k: &str| s.lines().find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().parse::<u32>().ok()))?;
    Some((get("pixelWidth:")?, get("pixelHeight:")?))
}

pub fn screenshot(max_width: u32) -> Result<(Vec<u8>, u32, u32), String> {
    if !has_screen_recording() { return Err("需要“屏幕录制”权限：系统设置 → 隐私与安全性 → 屏幕与系统录音 → 勾选 DataPilot".into()); }
    let f = std::env::temp_dir().join(format!("dp-shot-{}.png", text::short_id(8)));
    let ok = Command::new("/usr/sbin/screencapture").args(["-x", "-m", "-t", "png"]).arg(&f).status().map(|s| s.success()).unwrap_or(false);
    if !ok || !f.exists() { return Err("截图失败".into()); }
    if let Some((w, _)) = sips_dims(&f) {
        if w > max_width { let _ = Command::new("/usr/bin/sips").args(["--resampleWidth", &max_width.to_string()]).arg(&f).output(); }
    }
    let dims = sips_dims(&f).unwrap_or((max_width, max_width * 5 / 8));
    let png = std::fs::read(&f).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&f);
    Ok((png?, dims.0, dims.1))
}

// ---------- input (CGEvent) ----------
fn screen() -> (f64, f64) {
    let b = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    if b.size.width > 0.0 { (b.size.width, b.size.height) } else { (1440.0, 900.0) }
}

/// Model coordinates → global points on the main display.
pub fn to_point(x: f64, y: f64, shot: (u32, u32), mode: &str, screen: (f64, f64)) -> CGPoint {
    if mode == "absolutePixels" {
        CGPoint { x: x / shot.0.max(1) as f64 * screen.0, y: y / shot.1.max(1) as f64 * screen.1 }
    } else {
        CGPoint { x: x / 1000.0 * screen.0, y: y / 1000.0 * screen.1 }
    }
}

fn post(e: Ref) { if !e.is_null() { unsafe { CGEventPost(0, e); CFRelease(e) } } }

fn click(p: CGPoint, right: bool, count: i64) {
    let (down, up, btn) = if right { (3, 4, 1) } else { (1, 2, 0) };
    unsafe {
        post(CGEventCreateMouseEvent(std::ptr::null(), 5, p, btn));
        for i in 1..=count {
            let d = CGEventCreateMouseEvent(std::ptr::null(), down, p, btn);
            if !d.is_null() { CGEventSetIntegerValueField(d, 1, i); }
            post(d);
            let u = CGEventCreateMouseEvent(std::ptr::null(), up, p, btn);
            if !u.is_null() { CGEventSetIntegerValueField(u, 1, i); }
            post(u);
        }
    }
}

fn move_to(p: CGPoint) { unsafe { post(CGEventCreateMouseEvent(std::ptr::null(), 5, p, 0)) } }

fn drag(a: CGPoint, b: CGPoint) {
    unsafe {
        post(CGEventCreateMouseEvent(std::ptr::null(), 1, a, 0));
        post(CGEventCreateMouseEvent(std::ptr::null(), 6, b, 0));
        post(CGEventCreateMouseEvent(std::ptr::null(), 2, b, 0));
    }
}

fn scroll(amount: i32) { unsafe { post(CGEventCreateScrollWheelEvent2(std::ptr::null(), 1, 1, amount, 0, 0)) } }

fn type_text(t: &str) {
    for ch in t.chars() {
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        for down in [true, false] {
            unsafe {
                let e = CGEventCreateKeyboardEvent(std::ptr::null(), 0, down);
                if !e.is_null() { CGEventKeyboardSetUnicodeString(e, units.len(), units.as_ptr()); }
                post(e);
            }
        }
    }
}

const KEY_CODES: &[(&str, u16)] = &[("return", 36), ("enter", 36), ("tab", 48), ("space", 49), ("delete", 51), ("backspace", 51), ("escape", 53), ("esc", 53),
    ("left", 123), ("right", 124), ("down", 125), ("up", 126), ("home", 115), ("end", 119), ("pageup", 116), ("pagedown", 121),
    ("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7), ("c", 8), ("v", 9), ("b", 11), ("q", 12), ("w", 13), ("e", 14), ("r", 15), ("y", 16), ("t", 17),
    ("o", 31), ("u", 32), ("i", 34), ("p", 35), ("l", 37), ("j", 38), ("k", 40), ("n", 45), ("m", 46),
    ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("5", 23), ("6", 22), ("7", 26), ("8", 28), ("9", 25), ("0", 29)];

/// ["cmd","s"] → (key code, CGEventFlags).
pub fn key_spec(keys: &[String]) -> Result<(u16, u64), String> {
    let (mut flags, mut main) = (0u64, None);
    for k in keys.iter().map(|k| k.trim().to_lowercase()) {
        match k.as_str() {
            "cmd" | "command" | "meta" | "super" => flags |= 0x100000,
            "ctrl" | "control" => flags |= 0x40000,
            "alt" | "option" => flags |= 0x80000,
            "shift" => flags |= 0x20000,
            other => main = KEY_CODES.iter().find(|(n, _)| *n == other).map(|x| x.1),
        }
    }
    main.map(|m| (m, flags)).ok_or_else(|| format!("unsupported key combo {keys:?}"))
}

fn key(keys: &[String]) -> Result<(), String> {
    let (code, flags) = key_spec(keys)?;
    for down in [true, false] {
        unsafe {
            let e = CGEventCreateKeyboardEvent(std::ptr::null(), code, down);
            if !e.is_null() { CGEventSetFlags(e, flags); }
            post(e);
        }
    }
    Ok(())
}

// ---------- the tool ----------
/// Settings / per-turn cap check before an action. None = go.
pub fn gate(enabled: bool, action: &str, steps_taken: usize, cap: usize) -> Option<String> {
    if !enabled { return Some("Computer Use 未开启（设置 → 通用 → 允许控制电脑）".into()); }
    if !READ_ONLY.contains(&action) && steps_taken > cap.max(1) {
        return Some(format!("本轮电脑操作已达上限（{cap} 步，设置中可调）。请停止操作并向用户汇报进度。/ Per-turn desktop action limit reached."));
    }
    None
}

fn shot_result(msg: &str, png: &[u8], w: u32, h: u32) -> ToolResult {
    ToolResult { content: format!("{msg}（截图 {w}x{h}）"),
                 images: Some(vec![crate::types::ImagePart { data_url: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)) }]),
                 ..Default::default() }
}

fn computer(a: &Value, c: &ToolCtx) -> Result<ToolResult, String> {
    let s = &c.shared.settings;
    let act = a["action"].as_str().unwrap_or("").to_string();
    let steps = if READ_ONLY.contains(&act.as_str()) { 0 } else { c.shared.computer_steps.fetch_add(1, Ordering::SeqCst) + 1 };
    if let Some(e) = gate(s.computer_use_enabled, &act, steps, s.computer_max_steps_per_turn) { return Ok(ToolResult::err(e)); }
    let caps = c.shared.model_caps.lock().unwrap().clone().unwrap_or_default();
    let vision = caps.supports_vision;
    let pt = |k: &str| -> Option<CGPoint> {
        let v: Vec<f64> = a[k].as_array()?.iter().filter_map(|x| x.as_f64()).collect();
        (v.len() == 2).then(|| to_point(v[0], v[1], *SHOT.lock().unwrap(), &caps.coordinate_mode, screen()))
    };
    // after a state change: a fresh screenshot for vision models, otherwise the AX tree
    let after = |msg: String| -> ToolResult {
        std::thread::sleep(Duration::from_millis(600));
        if vision { if let Ok((png, w, h)) = screenshot(1280) { *SHOT.lock().unwrap() = (w, h); return shot_result(&msg, &png, w, h); } }
        let tree = ax_snapshot(250).unwrap_or_default();
        ToolResult::ok(format!("{msg}\n{}", text::truncate_middle(&tree, 6000)))
    };
    let need = |p: Option<CGPoint>| p.ok_or_else(|| "coordinate [x, y] required".to_string());
    let r = match act.as_str() {
        "ax_snapshot" if browser_in_front() => ToolResult::err(USE_BROWSER),
        "ax_snapshot" => ToolResult::ok(ax_snapshot(250)?),
        "screenshot" => {
            let (png, w, h) = screenshot(1280)?;
            *SHOT.lock().unwrap() = (w, h);
            if !vision { return Ok(ToolResult::ok("当前模型不支持图片输入，请使用 ax_snapshot。")); }
            shot_result("已截图", &png, w, h)
        }
        "wait" => {
            let t = a["time"].as_f64().unwrap_or(1.0).clamp(0.0, 30.0);
            std::thread::sleep(Duration::from_secs_f64(t));
            after("waited".into())
        }
        "terminate" => ToolResult::ok(format!("任务结束：{}", a["status"].as_str().unwrap_or("success"))),
        _ => {
            guard_front(c)?;
            if act == "open_app" && is_browser(a["text"].as_str().unwrap_or("")) { return Ok(ToolResult::err(USE_BROWSER)); }
            if act != "open_app" && browser_in_front() { return Ok(ToolResult::err(USE_BROWSER)); }
            match act.as_str() {
                "ax_press" => after(ax_press(a["ref"].as_str().unwrap_or(""))?),
                "ax_set_value" => after(ax_set_value(a["ref"].as_str().unwrap_or(""), a["text"].as_str().unwrap_or(""))?),
                "open_app" => {
                    let name = a["text"].as_str().unwrap_or("");
                    let ok = Command::new("/usr/bin/open").args(["-a", name]).status().map(|s| s.success()).unwrap_or(false);
                    std::thread::sleep(Duration::from_secs(1));
                    after(if ok { format!("opened {name}") } else { format!("cannot open {name}") })
                }
                "left_click" | "double_click" | "right_click" => {
                    let p = need(pt("coordinate"))?;
                    click(p, act == "right_click", if act == "double_click" { 2 } else { 1 });
                    after(format!("{act} at {},{}", p.x as i64, p.y as i64))
                }
                "mouse_move" => { move_to(need(pt("coordinate"))?); ToolResult::ok("moved") }
                "left_click_drag" => {
                    let p1 = need(pt("coordinate"))?;
                    let p2 = pt("coordinate2").unwrap_or(p1);
                    drag(p1, p2);
                    after("dragged".into())
                }
                "type" => { type_text(a["text"].as_str().unwrap_or("")); after("typed".into()) }
                "key" => {
                    let keys: Vec<String> = match a["keys"].as_array() {
                        Some(k) => k.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                        None => a["text"].as_str().map(|t| t.split('+').map(String::from).collect()).unwrap_or_default(),
                    };
                    key(&keys)?;
                    after("key sent".into())
                }
                "scroll" => { scroll(a["pixels"].as_i64().unwrap_or(-5).clamp(-100, 100) as i32); after("scrolled".into()) }
                other => ToolResult::err(format!("unknown action '{other}'")),
            }
        }
    };
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pure_helpers() {
        for b in ["Google Chrome", "com.google.Chrome", "Safari", "Microsoft Edge", "Arc", "Firefox", "Chromium"] { assert!(is_browser(b), "{b}"); }
        for b in ["Microsoft Excel", "Archive Utility", "Finder", "Numbers", "DataPilot", ""] { assert!(!is_browser(b), "{b}"); }
        assert_eq!(risk(&json!({"action": "ax_snapshot"})), Risk::ReadOnly);
        assert_eq!(risk(&json!({"action": "left_click"})), Risk::External);
        assert_eq!(key_spec(&["cmd".into(), "S".into()]).unwrap(), (1, 0x100000));
        assert_eq!(key_spec(&["ctrl".into(), "shift".into(), "tab".into()]).unwrap(), (48, 0x60000));
        assert!(key_spec(&["cmd".into()]).is_err());
        assert_eq!(to_point(500.0, 250.0, (1280, 800), "relative1000", (1440.0, 900.0)), CGPoint { x: 720.0, y: 225.0 });
        assert_eq!(to_point(640.0, 400.0, (1280, 800), "absolutePixels", (1440.0, 900.0)), CGPoint { x: 720.0, y: 450.0 });
        assert!(gate(false, "wait", 0, 40).unwrap().contains("未开启"));
        assert!(gate(true, "key", 2, 2).is_none());
        assert!(gate(true, "key", 3, 2).unwrap().contains("上限"));
        assert!(gate(true, "screenshot", 0, 2).is_none());
        let sample = "\"iTerm2\" ASN:0x0-0x29029: (in front) \n    bundleID=\"com.googlecode.iterm2\"\n    pid = 1327 !cgsConnection\n";
        assert_eq!(parse_lsappinfo(sample), Some(App { name: "iTerm2".into(), bundle: "com.googlecode.iterm2".into(), pid: 1327 }));
        assert!(summary(&json!({"action": "key", "keys": ["cmd", "s"]})).ends_with("cmd+s"));
    }

    /// Read-only system queries only (never posts events in tests).
    #[test]
    fn live_queries_do_not_crash() {
        let _ = has_accessibility();
        let _ = has_screen_recording();
        if let Some(a) = front_app() { assert!(a.pid > 0, "{a:?}"); }
        let (w, h) = screen();
        assert!(w > 0.0 && h > 0.0);
        if !has_accessibility() { assert!(ax_snapshot(10).unwrap_err().contains("辅助功能")); }
        assert!(with_elem("a999999", |_| ()).is_err());
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" { fn AXIsProcessTrustedWithOptions(opts: core_foundation::dictionary::CFDictionaryRef) -> u8; }
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" { fn CGRequestScreenCaptureAccess() -> bool; }

/// Shows the system Accessibility prompt (attributed to the app that launched the core). Returns the current state.
pub fn request_accessibility() -> bool {
    let k = CFString::new("AXTrustedCheckOptionPrompt");
    let d = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(k.as_CFType(), CFBoolean::true_value().as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(d.as_concrete_TypeRef()) != 0 }
}
pub fn request_screen_recording() -> bool { unsafe { CGRequestScreenCaptureAccess() } }
