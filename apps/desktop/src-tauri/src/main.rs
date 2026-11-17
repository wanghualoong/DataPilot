//! DataPilot desktop shell. The UI talks to the agent core through exactly one command (`core_request`) and one
//! event (`core-event`); secrets, files and processes stay behind the core's own checks.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
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

#[tauri::command]
fn newapi_login_cancel(app: tauri::AppHandle) { newapi_login::cancel(&app); }

/// "我已登录，继续": read the keys now even if the page did not look logged in.
#[tauri::command]
fn newapi_login_force() { newapi_login::FORCE.store(true, std::sync::atomic::Ordering::SeqCst); }

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let exe_dir = std::env::current_exe()?.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let layout = core::Layout::locate(&exe_dir, app.path().resource_dir().ok());
            let handle = app.handle().clone();
            let sink: core::EventSink = Arc::new(move |v: Value| { let _ = handle.emit("core-event", v); });
            app.manage(CoreState(core::CoreProcess::new(layout, sink)));
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
        .invoke_handler(tauri::generate_handler![core_request, core_info, newapi_login, newapi_login_cancel, newapi_login_force])
        .build(tauri::generate_context!())
        .expect("error while building DataPilot")
        .run(|app, ev| {
            if let tauri::RunEvent::Exit = ev { app.state::<CoreState>().0.shutdown(); }
        });
}
