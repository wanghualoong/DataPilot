//! DataPilot desktop shell. The UI talks to the agent core through exactly one command (`core_request`) and one
//! event (`core-event`); secrets, files and processes stay behind the core's own checks.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
#[cfg(windows)]
mod folder_picker;
mod newapi_login;

use serde_json::Value;
use std::sync::Arc;
use tauri::{Emitter, Manager};

struct CoreState(Arc<core::CoreProcess>);

/// Forward one JSON-RPC request to the core. Runs on Tauri's worker pool (requests may take minutes).
#[tauri::command(async)]
fn core_request(state: tauri::State<'_, CoreState>, method: String, params: Option<Value>) -> Result<Value, String> {
    if method == "shutdown" { return Err("not allowed from the UI".into()); }
    state.0.request(&method, params.unwrap_or_else(|| serde_json::json!({})))
}

/// Paths the UI needs to show (log, data folder) — no secrets.
#[tauri::command]
fn core_info(state: tauri::State<'_, CoreState>) -> Value {
    let l = state.0.layout();
    serde_json::json!({"app_support": l.app_support, "resources": l.resources, "core": l.core,
                       "restarts": *state.0.restarts.lock().unwrap(), "platform": std::env::consts::OS})
}

/// New API gateway: show its own login page in a separate window; returns the dashboard auth (memory only).
#[tauri::command]
async fn newapi_login(app: tauri::AppHandle, origin: String, fresh: Option<bool>) -> Result<Value, String> {
    let h = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        newapi_login::login(&app, &origin, fresh.unwrap_or(false), move |t| {
            let _ = h.emit("core-event", serde_json::json!({"method": "newapi/login_status", "params": {"text": t}}));
        })
    }).await.map_err(|e| e.to_string())?
}

/// System notification (turn finished / error / needs approval), shown only when the main window is not focused
/// (another app in front, or minimized). Returns whether it was shown.
/// Windows: the taskbar button also flashes (a toast can be hidden by Focus Assist / notification settings).
#[tauri::command]
fn notify(app: tauri::AppHandle, title: String, body: String) -> bool {
    use tauri_plugin_notification::NotificationExt;
    let Some(w) = app.get_webview_window("main") else { return false };
    if in_front(&w) { return false; }
    #[cfg(windows)]
    let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
    match app.notification().builder().title(title).body(body).show() {
        Ok(()) => true,
        Err(e) => { eprintln!("notify: {e}"); false }
    }
}

/// Is the main window the one the user is looking at? Windows: ask the OS for the foreground window (focus sits in
/// the WebView2 child of another process, so the shell's own focus flag can be stale); elsewhere the window state.
fn in_front(w: &tauri::WebviewWindow) -> bool {
    if w.is_minimized().unwrap_or(false) || !w.is_visible().unwrap_or(true) { return false; }
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, GA_ROOT};
        let Ok(me) = w.hwnd() else { return false };
        let fg = unsafe { GetForegroundWindow() };
        if fg.0.is_null() { return false; }
        let root = unsafe { GetAncestor(fg, GA_ROOT) };
        return root.0 as isize == me.0 as isize || fg.0 as isize == me.0 as isize;
    }
    #[allow(unreachable_code)]
    w.is_focused().unwrap_or(false)
}

/// Folder picker like the macOS panel: files are visible but only folders can be chosen. Windows uses its own
/// dialog (the standard folder picker hides files). Ok(None) = cancelled; Err elsewhere = use the dialog plugin
/// (an `Option<Option<_>>` would serialize both "cancelled" and "not supported" as null).
#[tauri::command(async)]
fn pick_folder_native(app: tauri::AppHandle, title: String, ok: String, field: String, start: Option<String>) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let owner = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).map(|h| h.0 as isize).unwrap_or(0);
        Ok(folder_picker::pick(owner, title, ok, field, start))
    }
    #[cfg(not(windows))]
    { let _ = (app, title, ok, field, start); Err("unsupported".into()) }
}

#[tauri::command]
fn newapi_login_cancel(app: tauri::AppHandle) { newapi_login::cancel(&app); }

/// "我已登录，继续": read the keys now even if the page did not look logged in.
#[tauri::command]
fn newapi_login_force() { newapi_login::FORCE.store(true, std::sync::atomic::Ordering::SeqCst); }

/// Start so that the whole window is visible: Windows maximized (like most Windows apps); macOS the configured size
/// shrunk to the screen's work area (menu bar and Dock excluded) and centred — 820 px is taller than small laptop screens.
fn fit_to_screen(w: &tauri::WebviewWindow) {
    #[cfg(windows)]
    { let _ = w.maximize(); }
    #[cfg(not(windows))]
    {
        let Ok(Some(m)) = w.current_monitor() else { return };
        let wa = m.work_area();
        let Ok(size) = w.outer_size() else { return };
        let (ww, wh) = (size.width.min(wa.size.width), size.height.min(wa.size.height));
        if (ww, wh) != (size.width, size.height) { let _ = w.set_size(tauri::PhysicalSize::new(ww, wh)); }
        let x = wa.position.x + ((wa.size.width - ww) / 2) as i32;
        let y = wa.position.y + ((wa.size.height - wh) / 2) as i32;
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let exe_dir = std::env::current_exe()?.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let layout = core::Layout::locate(&exe_dir, app.path().resource_dir().ok());
            let handle = app.handle().clone();
            let sink: core::EventSink = Arc::new(move |v: Value| { let _ = handle.emit("core-event", v); });
            app.manage(CoreState(core::CoreProcess::new(layout, sink)));
            if let Some(w) = app.get_webview_window("main") { fit_to_screen(&w); }
            if let Ok(origin) = std::env::var("DP_SELFTEST_NEWAPI") {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    let r = newapi_login::login(&h, &origin, false, |t| eprintln!("newapi status: {t}"));
                    println!("NEWAPI_SELFTEST {}", serde_json::to_string(&r).unwrap_or_default());
                    h.exit(0);
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![core_request, core_info, notify, pick_folder_native, newapi_login, newapi_login_cancel, newapi_login_force])
        .build(tauri::generate_context!())
        .expect("error while building DataPilot")
        .run(|app, ev| {
            if let tauri::RunEvent::Exit = ev { app.state::<CoreState>().0.shutdown(); }
        });
}
