//! Built-in defaults that differ per distribution: provider presets of the first-run wizard, the document parsing
//! (OCR) server and the gateways of the weekly local-model sync. `Resources/defaults.json` (in the repository) is
//! neutral: no gateway, no OCR server, no sync host. An optional `Resources/defaults.local.json` (not in the
//! repository, bundled by local builds) adds an organisation's own values: its presets come first, its other keys
//! replace the neutral ones. `DP_NO_LOCAL_DEFAULTS=1` ignores the local file (tests, public builds).
use serde_json::{json, Value};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct Defaults {
    /// first-run / quick-login presets: {id, zh, en, providerName, baseURL, proxyMode, login?, loginHint?: {zh, en}}
    pub provider_presets: Vec<Value>,
    /// MinerU server used when settings.docParse.server is empty ("" = none: scanned pages ask for a server)
    pub doc_parse_server: String,
    /// settings.localModelSyncHosts when unset ([] = no sync)
    pub local_model_sync_hosts: Vec<String>,
}

static DEFAULTS: OnceLock<Defaults> = OnceLock::new();

fn read(p: &Path) -> Option<Value> {
    let t = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&t).map_err(|e| eprintln!("{}: {e}", p.display())).ok()
}

fn merge(base: Option<Value>, local: Option<Value>) -> Defaults {
    let (b, l) = (base.unwrap_or(Value::Null), local.unwrap_or(Value::Null));
    let pick = |k: &str| if l.get(k).is_some() { &l[k] } else { &b[k] };
    let mut presets: Vec<Value> = l["providerPresets"].as_array().cloned().unwrap_or_default();
    for p in b["providerPresets"].as_array().into_iter().flatten() {
        if !presets.iter().any(|x| x["id"] == p["id"]) { presets.push(p.clone()); }
    }
    presets.retain(|p| p["id"].is_string());
    Defaults {
        provider_presets: presets,
        doc_parse_server: pick("docParseServer").as_str().unwrap_or("").trim().to_string(),
        local_model_sync_hosts: pick("localModelSyncHosts").as_array().into_iter().flatten()
            .filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
    }
}

/// defaults.json (+ defaults.local.json unless `no_local`) of a Resources folder.
pub fn load(resources: &Path, no_local: bool) -> Defaults {
    let local = if no_local { None } else { read(&resources.join("defaults.local.json")) };
    merge(read(&resources.join("defaults.json")), local)
}

/// Called once at startup (main.rs). Without it (unit tests) every default is empty.
pub fn init(resources: &Path) {
    let no_local = std::env::var("DP_NO_LOCAL_DEFAULTS").map(|v| v == "1").unwrap_or(false);
    let _ = DEFAULTS.set(load(resources, no_local));
}

pub fn get() -> &'static Defaults {
    static EMPTY: OnceLock<Defaults> = OnceLock::new();
    DEFAULTS.get().unwrap_or_else(|| EMPTY.get_or_init(Defaults::default))
}

/// `app/defaults` for the UI.
pub fn to_json(d: &Defaults) -> Value {
    json!({"providerPresets": d.provider_presets, "docParseServer": d.doc_parse_server, "localModelSyncHosts": d.local_model_sync_hosts})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_file_adds_presets_and_replaces_values() {
        let dir = std::env::temp_dir().join(format!("dp-defaults-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("defaults.json"), r#"{"providerPresets":[{"id":"custom","baseURL":""},{"id":"ollama","baseURL":"http://localhost:11434/v1"}],"docParseServer":"","localModelSyncHosts":[]}"#).unwrap();
        let d = load(&dir, false);
        assert_eq!(d.provider_presets.len(), 2);
        assert_eq!((d.doc_parse_server.as_str(), d.local_model_sync_hosts.len()), ("", 0));
        std::fs::write(dir.join("defaults.local.json"), r#"{"providerPresets":[{"id":"org","baseURL":"https://gw.example.org/v1"},{"id":"ollama","baseURL":"http://10.0.0.2:11434/v1"}],"docParseServer":" http://10.0.0.5:8010 ","localModelSyncHosts":["gw.example.org"," "]}"#).unwrap();
        let d = load(&dir, false);
        let ids: Vec<&str> = d.provider_presets.iter().map(|p| p["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["org", "ollama", "custom"]);
        assert_eq!(d.provider_presets[1]["baseURL"], "http://10.0.0.2:11434/v1");
        assert_eq!(d.doc_parse_server, "http://10.0.0.5:8010");
        assert_eq!(d.local_model_sync_hosts, ["gw.example.org"]);
        // ignored when asked (tests / public builds)
        let n = load(&dir, true);
        assert_eq!((n.provider_presets.len(), n.doc_parse_server.as_str()), (2, ""));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repository_defaults_are_neutral() {
        let res = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("Resources");
        let d = load(&res, true);
        let text = to_json(&d).to_string().to_lowercase();
        assert!(d.doc_parse_server.is_empty() && d.local_model_sync_hosts.is_empty(), "{text}");
        assert!(d.provider_presets.iter().any(|p| p["id"] == "custom"));
        // no organisation gateway: only presets without a URL or on this machine
        for p in &d.provider_presets {
            let u = p["baseURL"].as_str().unwrap_or("");
            assert!(u.is_empty() || u.starts_with("http://localhost"), "preset with a gateway URL in defaults.json: {u}");
        }
    }
}
