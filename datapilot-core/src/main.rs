//! `datapilot-core` binary: stdio JSON-RPC app-server (ADR-0012).
//! Env: DP_RESOURCES (Resources dir), DP_PYTHON_ROOT (contains bin/python3), DP_APP_SUPPORT (config.json, sessions/),
//!      DP_API_KEY (optional; otherwise the macOS keychain / Windows Credential Manager is used).
use std::path::PathBuf;
use std::sync::Arc;

use datapilot_core::agent::Core;
use datapilot_core::paths::Paths;
use datapilot_core::secrets::{SecretStore, StaticSecrets};
use datapilot_core::server::Server;
use datapilot_core::types::Config;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") { println!("datapilot-core {}", datapilot_core::VERSION); return; }
    // Windows installer hook (elevated): let the kernel sandbox reach 127.0.0.1 (SSH tunnels)
    #[cfg(windows)]
    if let Some(a) = args.iter().find(|a| *a == "--loopback-exempt" || *a == "--loopback-remove") {
        match datapilot_core::sandbox::loopback_exempt(a == "--loopback-remove") {
            Ok(sid) => { println!("ok {sid}"); return; }
            Err(e) => { eprintln!("{e}"); std::process::exit(1); }
        }
    }
    datapilot_core::platform::contain_children();
    let support = std::env::var("DP_APP_SUPPORT").map(PathBuf::from)
        .unwrap_or_else(|_| datapilot_core::platform::app_support_dir());
    datapilot_core::log::init(&support);
    datapilot_core::log::info("core_start", "", &[("version", env!("CARGO_PKG_VERSION").into()), ("os", std::env::consts::OS.into())]);
    let paths = match (std::env::var("DP_RESOURCES"), std::env::var("DP_PYTHON_ROOT")) {
        (Ok(r), Ok(p)) => Paths::new(PathBuf::from(r), PathBuf::from(p), support.clone()),
        _ => Paths::dev(support.clone()),
    };
    datapilot_core::defaults::init(&paths.resources);
    let config: Config = std::fs::read_to_string(support.join("config.json")).ok()
        .and_then(|t| serde_json::from_str(&t).map_err(|e| eprintln!("config.json: {e}")).ok()).unwrap_or_default();
    let secrets: Arc<dyn SecretStore> = match std::env::var("DP_API_KEY") {
        Ok(k) if !k.is_empty() => Arc::new(StaticSecrets { default_key: Some(k), ..Default::default() }),
        _ => datapilot_core::secrets::system_store(),
    };
    let core = match Core::new(paths, config, secrets) {
        Ok(c) => c,
        Err(e) => { eprintln!("datapilot-core: {e}"); std::process::exit(1); }
    };
    let server = Server::new(core, Box::new(std::io::stdout()));
    server.serve(std::io::stdin().lock());
}
